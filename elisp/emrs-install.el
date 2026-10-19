;;; emrs-install.el --- Install prebuilt emrs modules  -*- lexical-binding: t; -*-

;; Copyright (C) 2026  The emrs authors

;; Version: 0.1.0
;; Package-Requires: ((emacs "30.1"))
;; Keywords: extensions
;; SPDX-License-Identifier: GPL-3.0-or-later

;; This program is free software; you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;;; Commentary:

;; Gives a package built on emrs its module file.  The package registers
;; its module once, at top level of its main file:
;;
;;   (emrs-install-register 'my-module
;;     :version "0.2.0"
;;     :url "https://github.com/me/my-module/releases/download"
;;     :source (or load-file-name buffer-file-name))
;;
;; and calls `emrs-install-load' where it would call `emrs-load-module'.
;; The module is taken from the install directory or from `load-path', or
;; else downloaded from the release of the exact version (after asking,
;; see `emrs-install-download') and checked against its SHA256SUMS file.
;;
;; This file does not require `emrs' when it is loaded; `emrs-install-load'
;; does.

;;; Code:

(require 'url)
(require 'subr-x)
(require 'compile)
(require 'seq)
(require 'json)

(defvar emrs--modules)
(defvar url-http-response-status)
(defvar url-http-end-of-headers)
(declare-function emrs-load-module "emrs" (module &optional file))
(declare-function emrs-module-info "emrs" (module))
(declare-function emrs--module-file "emrs" (module))

(defgroup emrs-install nil
  "Install prebuilt modules for packages built on emrs."
  :group 'extensions
  :prefix "emrs-install-")

(defcustom emrs-install-directory (locate-user-emacs-file "emrs/modules/")
  "Directory that holds installed modules.
A module of a version lives at DIRECTORY/MODULE/VERSION/."
  :type 'directory)

(defcustom emrs-install-download 'ask
  "Whether to download prebuilt modules.
`ask' asks before each download, `always' downloads, `never' does not.
In batch mode `ask' counts as `never'."
  :type '(choice (const :tag "Ask first" ask)
                 (const :tag "Always" always)
                 (const :tag "Never" never)))

(defcustom emrs-install-timeout 60
  "Seconds to wait for each file of a download."
  :type 'natnum)

(defcustom emrs-install-cargo-program "cargo"
  "Program that builds modules from a checkout."
  :type 'string)

;;;; Registration

(defvar emrs-install--registry (make-hash-table :test 'eq)
  "Registered modules: a plist of :version :url :source :package by symbol.")

(defvar emrs-install--warned nil
  "Modules for which a version warning was already shown.")

(defun emrs-install-register (module &rest keys)
  "Register the emrs module MODULE (a symbol) for installation.
KEYS are :version (required, the module version as a string, equal
to the crate version), :url (base URL of the releases), :source (a
file of the package, to find its checkout) and :package (the Cargo
package to build).  Registering again replaces the entry."
  (let ((version (plist-get keys :version)))
    (unless (and (stringp version)
                 (ignore-errors (consp (version-to-list version))))
      (error "Module %s: :version must be a version string, got %S"
             module version)))
  (puthash module (copy-sequence keys) emrs-install--registry)
  module)

(defun emrs-install--entry (module)
  "Return the registration plist of MODULE or signal a `user-error'."
  (or (gethash module emrs-install--registry)
      (user-error "Module %s is not registered; see `emrs-install-register'"
                  module)))

;;;; Platform

(defun emrs-install-platform ()
  "Return the release platform name of this Emacs, or nil."
  (let ((conf system-configuration))
    (pcase system-type
      ('gnu/linux
       (when (string-suffix-p "-linux-gnu" conf)
         (cond ((string-prefix-p "x86_64-" conf) "linux-x86_64")
               ((string-prefix-p "aarch64-" conf) "linux-aarch64"))))
      ('darwin
       (when (or (string-prefix-p "aarch64-" conf)
                 (string-prefix-p "arm-" conf))
         "macos-aarch64"))
      ('windows-nt
       (when (string-prefix-p "x86_64-" conf) "windows-x86_64")))))

;;;; Files

(defun emrs-install--installed-file (module version)
  "Return the file where MODULE of VERSION is installed."
  (expand-file-name (format "%s-module%s" module module-file-suffix)
                    (expand-file-name
                     (concat (symbol-name module) "/" version "/")
                     emrs-install-directory)))

(defun emrs-install--checkout (module)
  "Return the checkout directory of MODULE, or nil.
It is the nearest directory above the true name of the registered
:source (of the .el next to a .elc) that contains Cargo.toml."
  (let ((source (plist-get (gethash module emrs-install--registry) :source)))
    (when (and source (file-exists-p
                       (if (string-suffix-p ".elc" source)
                           (concat (file-name-sans-extension source) ".el")
                         source)))
      (let* ((file (if (string-suffix-p ".elc" source)
                       (concat (file-name-sans-extension source) ".el")
                     source))
             (dir (locate-dominating-file (file-truename file) "Cargo.toml")))
        (and dir (file-name-as-directory (expand-file-name dir)))))))

(defun emrs-install-file (module)
  "Return the file to load for MODULE, or nil.  Has no side effects.
That is the installed file of the registered version, else the file
MODULE-module on `load-path'."
  (let* ((version (plist-get (gethash module emrs-install--registry) :version))
         (installed (and version (emrs-install--installed-file module version))))
    (if (and installed (file-exists-p installed))
        installed
      (locate-file (format "%s-module" module) load-path
                   (list module-file-suffix)))))

;;;; Download

(defun emrs-install--checksum-for (text name)
  "Return the SHA-256 for file NAME from SHA256SUMS TEXT, or nil."
  (let (found)
    (dolist (line (split-string text "[\r\n]+" t))
      (when (and (not found)
                 (string-match
                  "\\`\\([0-9a-fA-F]\\{64\\}\\)\\(?:  \\| \\*\\)\\(.+\\)\\'"
                  line)
                 (string= (match-string 2 line) name))
        (setq found (downcase (match-string 1 line)))))
    found))

(define-error 'emrs-install--fetch-failed "Download failed")

(defun emrs-install--fetch (url)
  "Return the bytes of URL as a unibyte string.
Signal `emrs-install--fetch-failed' with a reason on failure."
  (let ((parsed (url-generic-parse-url url)))
    (if (equal (url-type parsed) "file")
        (let ((file (url-unhex-string (url-filename parsed))))
          (unless (file-regular-p file)
            (signal 'emrs-install--fetch-failed (list "no such file")))
          (with-temp-buffer
            (set-buffer-multibyte nil)
            (insert-file-contents-literally file)
            (buffer-string)))
      (let ((buffer (condition-case err
                        (url-retrieve-synchronously
                         url t t emrs-install-timeout)
                      (error (signal 'emrs-install--fetch-failed
                                     (list (error-message-string err)))))))
        (unless buffer
          (signal 'emrs-install--fetch-failed (list "timeout or no response")))
        (unwind-protect
            (with-current-buffer buffer
              (unless (eql url-http-response-status 200)
                (signal 'emrs-install--fetch-failed
                        (list (format "HTTP status %s" url-http-response-status))))
              (set-buffer-multibyte nil)
              (encode-coding-string
               (buffer-substring-no-properties
                (1+ url-http-end-of-headers) (point-max))
               'no-conversion))
          (kill-buffer buffer))))))

(defun emrs-install--place (module version write built-from)
  "Install MODULE of VERSION by calling WRITE with a temporary file name.
Write a temporary file in the target directory, set its mode to 0755
and rename it.  BUILT-FROM is the checkout to record in the file
`built-from' next to the module, or nil to remove that file.  Return
the path of the module."
  (let* ((target (emrs-install--installed-file module version))
         (dir (file-name-directory target))
         (marker (expand-file-name "built-from" dir))
         temp)
    (condition-case err
        (progn
          (make-directory dir t)
          (setq temp (make-temp-file (expand-file-name ".emrs-install-" dir)))
          (funcall write temp)
          (set-file-modes temp #o755)
          (if built-from
              (let ((coding-system-for-write 'utf-8))
                (write-region built-from nil marker nil 'silent))
            (when (file-exists-p marker) (delete-file marker)))
          (rename-file temp target t)
          target)
      (file-error
       (when (and temp (file-exists-p temp)) (ignore-errors (delete-file temp)))
       (user-error "Cannot install %s: %s%s" target (error-message-string err)
                   (if (eq system-type 'windows-nt)
                       " (a loaded module cannot be replaced before a restart)"
                     ""))))))

(defun emrs-install--install (module version bytes)
  "Install BYTES as the file of MODULE of VERSION and return its path.
A downloaded module has no file `built-from'."
  (emrs-install--place
   module version
   (lambda (temp)
     (let ((coding-system-for-write 'no-conversion))
       (write-region bytes nil temp nil 'silent)))
   nil))

(defun emrs-install--install-library (module version library checkout)
  "Install the built LIBRARY as MODULE of VERSION and return its path.
CHECKOUT is recorded in the file `built-from'."
  (emrs-install--place module version
                       (lambda (temp) (copy-file library temp t))
                       checkout))

(defun emrs-install--consent (module version platform url)
  "Return non-nil if the user allows downloading MODULE of VERSION from URL.
PLATFORM is the platform name shown in the question."
  (pcase emrs-install-download
    ('always t)
    ('never nil)
    (_ (and (not noninteractive)
            (y-or-n-p (format "Download the %s module %s for %s from %s? "
                              module version platform url))))))

(defun emrs-install--download (module)
  "Download MODULE per its registration.
Return (FILE) with the installed file, or (nil . REASON) with a
string that says why there is none.  Signal a `user-error' on a
checksum mismatch."
  (let* ((entry (gethash module emrs-install--registry))
         (version (plist-get entry :version))
         (url (plist-get entry :url))
         (platform (emrs-install-platform))
         (reason
          (cond ((null url) "no :url registered")
                ((string-match-p "-snapshot" version)
                 (format "version %s is a snapshot" version))
                ((null platform)
                 (format "no prebuilt module for %s" system-configuration))
                ((eq emrs-install-download 'never)
                 "`emrs-install-download' is never")
                ((not (emrs-install--consent module version platform url))
                 "download declined"))))
    (if reason
        (cons nil (format "download: %s" reason))
      (let* ((base (format "%s/v%s/" (string-remove-suffix "/" url) version))
             (name (format "%s-module-%s%s" module platform module-file-suffix))
             (sums-url (concat base "SHA256SUMS"))
             (asset-url (concat base name)))
        (condition-case err
            (let (sums bytes expected actual)
              (message "Downloading the %s module %s..." module version)
              (setq sums (emrs-install--fetch sums-url))
              (setq expected (emrs-install--checksum-for
                              (decode-coding-string sums 'utf-8) name))
              (unless expected
                (signal 'emrs-install--fetch-failed
                        (list (format "no checksum for %s in %s" name sums-url))))
              (setq bytes (emrs-install--fetch asset-url))
              (setq actual (secure-hash 'sha256 bytes))
              (unless (string= actual expected)
                (user-error "Checksum mismatch for %s: expected %s, got %s; nothing installed"
                            asset-url expected actual))
              (prog1 (list (emrs-install--install module version bytes))
                (message "Downloading the %s module %s...done" module version)))
          (emrs-install--fetch-failed
           (let ((text (format "download from %s failed: %s"
                               base (mapconcat #'identity (cdr err) " "))))
             (message "emrs-install: %s" text)
             (cons nil text))))))))

;;;; Local build

(defvar emrs--modules)

(defvar emrs-install--builds (make-hash-table :test 'equal)
  "Running asynchronous builds: (MODULE VERSION CHECKOUT LIBRARY) by buffer name.")

(defun emrs-install--loaded-p (module)
  "Return non-nil if MODULE is loaded in this Emacs."
  (and (boundp 'emrs--modules) (hash-table-p emrs--modules)
       (gethash module emrs--modules)
       t))

(defun emrs-install--buffer-name (module)
  "Return the name of the build buffer of MODULE."
  (format "*emrs-install %s*" module))

(defun emrs-install--program ()
  "Return the full name of the cargo program, or signal a `user-error'."
  (or (executable-find emrs-install-cargo-program)
      (user-error "Program `%s' not found; install Rust or set `emrs-install-cargo-program'"
                  emrs-install-cargo-program)))

(defun emrs-install--build-arguments (module checkout)
  "Return the arguments of the cargo command that builds MODULE in CHECKOUT.
The target directory is explicit, so neither CARGO_TARGET_DIR nor the
cargo configuration can move the library."
  (let ((package (plist-get (gethash module emrs-install--registry) :package)))
    (append (list "build" "--release" "--target-dir"
                  (expand-file-name "target" checkout))
            (and package (list "-p" package)))))

(defun emrs-install--library-file (metadata checkout package)
  "Return the library file that building PACKAGE in CHECKOUT produces.
METADATA is the parsed output of `cargo metadata' (alists and lists).
PACKAGE is a package name, or nil for the package whose manifest is the
Cargo.toml of CHECKOUT.  Signal a `user-error' if there is no cdylib."
  (let* ((manifest (file-truename (expand-file-name "Cargo.toml" checkout)))
         (pkg (seq-find
               (lambda (p)
                 (if package
                     (equal (alist-get 'name p) package)
                   (let ((path (alist-get 'manifest_path p)))
                     (and path (file-exists-p path)
                          (equal (file-truename path) manifest)))))
               (alist-get 'packages metadata)))
         (name (or package "the package in Cargo.toml"))
         (target (and pkg
                      (seq-find
                       (lambda (tg)
                         (or (member "cdylib" (alist-get 'kind tg))
                             (member "cdylib" (alist-get 'crate_types tg))))
                       (alist-get 'targets pkg)))))
    (unless pkg
      (user-error "Cargo knows no package %s in %s; check :package of `emrs-install-register'"
                  name checkout))
    (unless target
      (user-error "Package %s has no cdylib target; add crate-type = [\"cdylib\"] to its [lib] section"
                  (alist-get 'name pkg)))
    (expand-file-name
     (concat "target/release/"
             (if (eq system-type 'windows-nt) "" "lib")
             (replace-regexp-in-string "-" "_" (alist-get 'name target))
             module-file-suffix)
     checkout)))

(defun emrs-install--library (module checkout program)
  "Return the library file of MODULE built in CHECKOUT, using cargo PROGRAM.
Run `cargo metadata'; signal a `user-error' if that fails."
  (let ((package (plist-get (gethash module emrs-install--registry) :package))
        (default-directory checkout)
        metadata)
    (with-temp-buffer
      (let ((status (call-process program nil (list t nil) nil
                                  "metadata" "--no-deps" "--format-version" "1")))
        (unless (eql status 0)
          (user-error "`%s metadata' failed in %s" program checkout))
        (goto-char (point-min))
        (setq metadata (condition-case nil
                           (json-parse-buffer :object-type 'alist
                                              :array-type 'list)
                         (error (user-error "`%s metadata' gave no JSON in %s"
                                            program checkout))))))
    (emrs-install--library-file metadata checkout package)))

(defun emrs-install--install-built (module checkout library)
  "Install LIBRARY built in CHECKOUT as the registered version of MODULE.
Return the installed file; signal a `user-error' if LIBRARY is missing."
  (let ((version (plist-get (emrs-install--entry module) :version)))
    (unless (file-exists-p library)
      (user-error "Cargo produced no %s; the package needs crate-type = [\"cdylib\"]"
                  library))
    (emrs-install--install-library module version library checkout)))

(defun emrs-install--source-checkout (module)
  "Return the checkout of MODULE or signal a `user-error'."
  (or (emrs-install--checkout module)
      (user-error "No checkout with Cargo.toml found for the %s module; see :source of `emrs-install-register'"
                  module)))

(defun emrs-install--read-module ()
  "Read the name of a registered module."
  (let (names)
    (maphash (lambda (k _) (push (symbol-name k) names)) emrs-install--registry)
    (unless names (user-error "No module is registered"))
    (intern (completing-read "Module: " (sort names #'string<) nil t))))

(defun emrs-install--built-message (module)
  "Return the message that says MODULE was built."
  (format "%s module built%s" module
          (if (emrs-install--loaded-p module)
              "; restart Emacs to use it"
            "")))

(defun emrs-install--build-finished (buffer message)
  "Install the module when the build in BUFFER ended with MESSAGE.
Meant for `compilation-finish-functions'; it ignores other buffers."
  (let* ((name (buffer-name buffer))
         (info (gethash name emrs-install--builds)))
    (when info
      (remhash name emrs-install--builds)
      (pcase-let ((`(,module ,checkout ,library) info))
        (if (not (equal message "finished\n"))
            (message "%s module build failed (see %s)" module name)
          (condition-case err
              (progn (emrs-install--install-built module checkout library)
                     (message "%s" (emrs-install--built-message module)))
            (error (message "%s module build failed: %s" module
                            (error-message-string err)))))))))

(add-hook 'compilation-finish-functions #'emrs-install--build-finished)

;;;###autoload
(defun emrs-install-build (module)
  "Build the registered MODULE with cargo, asynchronously.
The output goes to the buffer `*emrs-install MODULE*'.  On success the
module is installed as its registered version; a running Emacs needs a
restart to use a rebuilt module.  Return the build process."
  (interactive (list (emrs-install--read-module)))
  (emrs-install--entry module)
  (let ((name (emrs-install--buffer-name module)))
    (when (get-buffer-process name)
      (user-error "The %s module is being built (see %s)" module name))
    (let* ((checkout (emrs-install--source-checkout module))
           (program (emrs-install--program))
           (library (emrs-install--library module checkout program))
           (default-directory checkout)
           (buffer (compilation-start
                    (mapconcat #'shell-quote-argument
                               (cons program
                                     (emrs-install--build-arguments
                                      module checkout))
                               " ")
                    'compilation-mode
                    (lambda (_mode) name))))
      (puthash name (list module checkout library) emrs-install--builds)
      (get-buffer-process buffer))))

(defun emrs-install--output-tail (buffer &optional lines)
  "Return the last LINES (default 30) lines of BUFFER."
  (with-current-buffer buffer
    (save-excursion
      (goto-char (point-max))
      (forward-line (- (or lines 30)))
      (string-trim (buffer-substring-no-properties (point) (point-max))))))

;;;###autoload
(defun emrs-install-build-sync (module)
  "Build the registered MODULE with cargo, waiting for it to finish.
Never signal: a failure becomes a warning.  In batch mode the warning
includes the end of the build output.  Return non-nil on success."
  (condition-case err
      (let* ((checkout (emrs-install--source-checkout module))
             (program (emrs-install--program))
             (library (emrs-install--library module checkout program))
             (default-directory checkout)
             (name (emrs-install--buffer-name module))
             buffer status)
        (when (get-buffer-process name)
          (user-error "The %s module is being built (see %s)" module name))
        (when (get-buffer name) (kill-buffer name))
        (setq buffer (get-buffer-create name))
        (setq status (apply #'call-process program nil buffer nil
                            (emrs-install--build-arguments module checkout)))
        (unless (eql status 0)
          (if noninteractive
              (error "The build failed:\n%s" (emrs-install--output-tail buffer))
            (error "The build failed (see %s)" (buffer-name buffer))))
        (emrs-install--install-built module checkout library)
        (message "%s" (emrs-install--built-message module))
        t)
    (error
     (display-warning
      'emrs-install
      (format "Cannot build the %s module: %s" module
              (error-message-string err)))
     nil)))

;;;; Staleness

(defun emrs-install--stale-p (checkout file)
  "Return non-nil if a source file in CHECKOUT is newer than FILE.
The sources are the .rs files below src/, build.rs, Cargo.toml and
Cargo.lock."
  (let ((sources (append
                  (let ((src (expand-file-name "src" checkout)))
                    (and (file-directory-p src)
                         (directory-files-recursively src "\\.rs\\'")))
                  (mapcar (lambda (f) (expand-file-name f checkout))
                          '("build.rs" "Cargo.toml" "Cargo.lock")))))
    (seq-some (lambda (source)
                (and (file-exists-p source)
                     (file-newer-than-file-p source file)))
              sources)))

(defun emrs-install--built-from (file)
  "Return the checkout recorded next to the module FILE, or nil."
  (let ((marker (expand-file-name "built-from" (file-name-directory file))))
    (when (file-readable-p marker)
      (let ((dir (string-trim
                  (with-temp-buffer
                    (let ((coding-system-for-read 'utf-8))
                      (insert-file-contents marker))
                    (buffer-string)))))
        (and (file-directory-p dir) dir)))))

(defun emrs-install--start-build (module)
  "Start the build of MODULE and tell the user to run the command again."
  (emrs-install-build module)
  (user-error "Building the %s module; run the command again when it is done"
              module))

(defun emrs-install--present (module version file)
  "Return FILE, the file found for MODULE of VERSION, or rebuild it.
If FILE is a local build older than its sources, ask whether to build
it again (interactive sessions only)."
  (let ((checkout (and (equal file (emrs-install--installed-file module version))
                       (emrs-install--built-from file))))
    (when (and checkout
               (not noninteractive)
               (not (emrs-install--loaded-p module))
               (emrs-install--checkout module)
               (emrs-install--stale-p checkout file)
               (y-or-n-p (format "The %s module is older than its sources.  Rebuild it now with cargo? "
                                 module)))
      (emrs-install--start-build module))
    file))

(defun emrs-install--offer-build (module version)
  "Offer to build MODULE of VERSION.  Return a note about the outcome.
When the user agrees, start the build and signal a `user-error'."
  (cond ((null (emrs-install--checkout module))
         "build: no checkout with Cargo.toml found")
        (noninteractive "build: not offered in batch mode")
        ((y-or-n-p (format "Build the %s module %s with cargo? " module version))
         (emrs-install--start-build module))
        (t "build: declined")))

;;;; Ensure and load

(defun emrs-install-ensure (module)
  "Return a file to load for MODULE, getting it if necessary.
Signal a `user-error' if nothing works."
  (let* ((entry (emrs-install--entry module))
         (version (plist-get entry :version))
         (platform (emrs-install-platform))
         (download nil)
         (build nil)
         (found (emrs-install-file module)))
    (or (and found (emrs-install--present module version found))
        (car (setq download (emrs-install--download module)))
        (progn (setq build (emrs-install--offer-build module version)) nil)
        (user-error
         "No %s module %s available for %s.\nTried:\n  %s\nWays out: M-x emrs-install-module, see the README of the package, or install %s-module%s on `load-path'"
         module version
         (or platform (format "no prebuilt module for %s" system-configuration))
         (mapconcat
          #'identity
          (append
           (list (format "installed: none in %s"
                         (abbreviate-file-name
                          (file-name-directory
                           (emrs-install--installed-file module version)))))
           (list (cdr download))
           (list build))
          "\n  ")
         module module-file-suffix))))

(defun emrs-install--load-failure (module file err)
  "Signal a `user-error' about ERR when loading FILE of MODULE."
  (user-error
   "Cannot load the %s module from %s: %s\nPlatform: %s; system-configuration: %s; Emacs %s\nThe release files need Linux with glibc 2.28 or later, or macOS on Apple silicon.\nTo install the module again: M-x emrs-install-module"
   module file (error-message-string err)
   (or (emrs-install-platform) "unknown")
   system-configuration emacs-version))

(defun emrs-install-load (module)
  "Load the emrs module MODULE and return its handle.
Gets the module with `emrs-install-ensure' unless it is loaded, and
warns once if its version differs from the registered one."
  (require 'emrs)
  (let* ((loaded (gethash module emrs--modules))
         (handle
          (if loaded
              (emrs-load-module module)
            (let ((file (emrs-install-ensure module)))
              (when (file-in-directory-p file emrs-install-directory)
                (add-to-list 'load-path (file-name-directory file)))
              (condition-case err
                  (emrs-load-module module file)
                (error (emrs-install--load-failure module file err))))))
         (registered (plist-get (gethash module emrs-install--registry) :version))
         (actual (plist-get (emrs-module-info module) :version)))
    (when (and registered (not (equal registered actual))
               (not (memq module emrs-install--warned)))
      (push module emrs-install--warned)
      (display-warning
       'emrs-install
       (format "The %s module reports version %s but %s is registered (file %s).  Restart Emacs after `M-x emrs-install-module', since a module cannot be unloaded."
               module actual registered
               (or (emrs-install-file module) "unknown"))))
    handle))

;;;; Commands

;;;###autoload
(defun emrs-install-module (module &optional build)
  "Install the registered version of MODULE again, ignoring what is installed.
Download it if possible, else build it from its checkout.  With a
prefix argument BUILD, build it with cargo (asynchronously)."
  (interactive (list (emrs-install--read-module) current-prefix-arg))
  (let ((entry (emrs-install--entry module)))
    (if build
        (progn (emrs-install--source-checkout module)
               (emrs-install-build module)
               (message "Building the %s module; it is installed when the build ends"
                        module))
      (let ((download (emrs-install--download module)))
        (cond
         ((car download)
          (message "%s module %s installed in %s%s" module
                   (plist-get entry :version)
                   (abbreviate-file-name (file-name-directory (car download)))
                   (if (emrs-install--loaded-p module)
                       "; restart Emacs to use it"
                     "")))
         ((emrs-install--checkout module)
          (message "emrs-install: %s; building instead" (cdr download))
          (emrs-install-build module)
          (message "Building the %s module; it is installed when the build ends"
                   module))
         (t (user-error "Cannot install the %s module %s\nTried:\n  %s\n  build: no checkout with Cargo.toml found"
                        module (plist-get entry :version) (cdr download))))))))

(defun emrs-install--loaded-file (module)
  "Return the file MODULE was loaded from in this Emacs, or nil."
  (and (emrs-install--loaded-p module)
       (fboundp 'emrs--module-file)
       (emrs--module-file (gethash module emrs--modules))))

(defun emrs-install--clean-candidates ()
  "Return the version directories that `emrs-install-clean' would delete."
  (let (dirs)
    (maphash
     (lambda (module entry)
       (let* ((root (expand-file-name (symbol-name module)
                                      emrs-install-directory))
              (loaded (emrs-install--loaded-file module))
              (keep (list (plist-get entry :version)
                          (and loaded (file-in-directory-p loaded root)
                               (file-name-nondirectory
                                (directory-file-name
                                 (file-name-directory
                                  (file-relative-name loaded root))))))))
         (when (file-directory-p root)
           (dolist (dir (directory-files root t directory-files-no-dot-files-regexp))
             (when (and (file-directory-p dir)
                        (not (member (file-name-nondirectory dir) keep)))
               (push dir dirs))))))
     emrs-install--registry)
    (sort dirs #'string<)))

;;;###autoload
(defun emrs-install-clean ()
  "Delete installed versions of registered modules that are not in use.
Keep the registered version and the version a loaded module came from.
Ask once, listing the directories."
  (interactive)
  (let ((dirs (emrs-install--clean-candidates)))
    (if (null dirs)
        (message "Nothing to clean")
      (when (yes-or-no-p (format "Delete these directories?\n%s\nDelete them? "
                                 (mapconcat #'abbreviate-file-name dirs "\n")))
        (dolist (dir dirs) (delete-directory dir t))
        (message "Deleted %d director%s" (length dirs)
                 (if (= (length dirs) 1) "y" "ies"))))))

(provide 'emrs-install)
;;; emrs-install.el ends here
