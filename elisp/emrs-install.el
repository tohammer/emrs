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
(defvar url-http-attempt-keepalives)
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
  "Seconds a download may go without receiving data before it is stopped."
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
package to build).  The name of MODULE consists of letters, digits,
`.', `_', `+' and `-'.  The :url must start with https:// or file:,
since the checksums come from the same place as the module.
Registering again replaces the entry."
  (unless (and (symbolp module)
               (string-match-p "\\`[A-Za-z0-9][A-Za-z0-9._+-]*\\'"
                               (symbol-name module))
               (not (string-search ".." (symbol-name module))))
    (error "Module %S: the name must be letters, digits, `.', `_', `+' and `-', start with a letter or digit and not contain `..'"
           module))
  (let ((version (plist-get keys :version))
        (url (plist-get keys :url)))
    (unless (and (stringp version)
                 (ignore-errors (consp (version-to-list version))))
      (error "Module %s: :version must be a version string, got %S"
             module version))
    (unless (or (null url)
                (and (stringp url)
                     (or (string-prefix-p "https://" url)
                         (string-prefix-p "file:" url))))
      (error "Module %s: :url must start with https:// or file:, got %S"
             module url)))
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

(defun emrs-install--fetch-fail (url reason)
  "Signal `emrs-install--fetch-failed' for URL with the string REASON."
  (signal 'emrs-install--fetch-failed (list url reason)))

(defun emrs-install--file-url-name (url)
  "Return the local file name of the file: URL.
On Windows, file:///C:/dir/file names C:/dir/file."
  (let ((file (url-unhex-string (url-filename (url-generic-parse-url url)))))
    (if (and (memq system-type '(windows-nt ms-dos))
             (string-match-p "\\`/[A-Za-z]:" file))
        (substring file 1)
      file)))

(defun emrs-install--kill-retrieval (buffer)
  "Delete the process of the url.el BUFFER and kill BUFFER."
  (when (buffer-live-p buffer)
    (let ((process (get-buffer-process buffer)))
      (when process (delete-process process)))
    (let (kill-buffer-query-functions)
      (kill-buffer buffer))))

(defun emrs-install--retrieve (url)
  "Retrieve URL with `url-retrieve' and return the response body.
Wait until the callback runs; stop when no data arrives for
`emrs-install-timeout' seconds.  Kill the buffers of the transfer.
Signal `emrs-install--fetch-failed' on failure.  Connections are not
kept alive, so nothing stays open after the transfer."
  (let* ((url-http-attempt-keepalives nil)
         (done nil)
         (status nil)
         (data-buffer nil)
         (buffer (condition-case err
                     (url-retrieve url
                                   (lambda (s)
                                     (setq status s
                                           data-buffer (current-buffer)
                                           done t))
                                   nil t t)
                   (error (emrs-install--fetch-fail
                           url (error-message-string err))))))
    (unless buffer
      (emrs-install--fetch-fail url "no response"))
    (unwind-protect
        (let ((size -1)
              (deadline nil))
          (while (not done)
            ;; A redirect continues in another buffer.
            (let ((next (buffer-local-value 'url-redirect-buffer buffer)))
              (when (and next (not (eq next buffer)) (buffer-live-p next))
                (emrs-install--kill-retrieval buffer)
                (setq buffer next size -1)))
            (let ((now (buffer-size buffer)))
              (unless (eql now size)
                (setq size now
                      deadline (+ (float-time) emrs-install-timeout))))
            (when (> (float-time) deadline)
              (emrs-install--fetch-fail
               url (format "no data for %s seconds" emrs-install-timeout)))
            (let ((process (get-buffer-process buffer)))
              (when (and process
                         (memq (process-status process)
                               '(closed exit signal failed))
                         (not done))
                ;; Let a sentinel run the callback, else give up.
                (accept-process-output nil 0)
                (unless done
                  (emrs-install--fetch-fail url "connection closed"))))
            (unless done (accept-process-output nil 0.05)))
          (with-current-buffer data-buffer
            (let ((err (plist-get status :error)))
              (when err
                (emrs-install--fetch-fail url (error-message-string err))))
            (let ((code (and (boundp 'url-http-response-status)
                             url-http-response-status)))
              (unless (eql code 200)
                (emrs-install--fetch-fail
                 url (if code (format "HTTP status %s" code)
                       "no HTTP response"))))
            (set-buffer-multibyte nil)
            (buffer-substring-no-properties
             (1+ url-http-end-of-headers) (point-max))))
      (emrs-install--kill-retrieval buffer)
      (when data-buffer (emrs-install--kill-retrieval data-buffer)))))

(defun emrs-install--fetch (url)
  "Return the bytes of URL as a unibyte string.
URL is a file: URL or one that `url-retrieve' handles.  Signal
`emrs-install--fetch-failed' with URL and a reason on failure."
  (if (string-prefix-p "file:" url)
      (let ((file (emrs-install--file-url-name url)))
        (unless (file-regular-p file)
          (emrs-install--fetch-fail url "no such file"))
        (with-temp-buffer
          (set-buffer-multibyte nil)
          (insert-file-contents-literally file)
          (buffer-string)))
    (emrs-install--retrieve url)))

(defun emrs-install--place (module version write built-from)
  "Install MODULE of VERSION by calling WRITE with a temporary file name.
Write a temporary file in the target directory, set its mode to 0755
and rename it.  After the rename, write BUILT-FROM (a string) to the
file `built-from' next to the module, or remove that file if
BUILT-FROM is nil.  Return the path of the module."
  (let* ((target (emrs-install--installed-file module version))
         (dir (file-name-directory target))
         (marker (expand-file-name "built-from" dir))
         temp)
    (unwind-protect
        (condition-case err
            (progn
              (make-directory dir t)
              (setq temp (make-temp-file (expand-file-name ".emrs-install-" dir)))
              (funcall write temp)
              (set-file-modes temp #o755)
              (rename-file temp target t)
              (setq temp nil)
              (if built-from
                  (let ((coding-system-for-write 'utf-8))
                    (write-region built-from nil marker nil 'silent))
                (when (file-exists-p marker) (delete-file marker)))
              target)
          (file-error
           (user-error "Cannot install %s: %s%s" target (error-message-string err)
                       (if (eq system-type 'windows-nt)
                           " (a loaded module cannot be replaced before a restart)"
                         ""))))
      (when (and temp (file-exists-p temp))
        (ignore-errors (delete-file temp))))))

(defun emrs-install--install (module version bytes)
  "Install BYTES as the file of MODULE of VERSION and return its path.
A downloaded module has no file `built-from'."
  (emrs-install--place
   module version
   (lambda (temp)
     (let ((coding-system-for-write 'no-conversion))
       (write-region bytes nil temp nil 'silent)))
   nil))

(defun emrs-install--install-library (module version build)
  "Install the built library as MODULE of VERSION and return its path.
BUILD is a plist with :library (the file cargo produced), :checkout,
:package-dir (the directory of the package's Cargo.toml) and :start
\(the time the build started).  The installed file gets the start time
as its modification time; the file `built-from' records the checkout
and the package directory."
  (let ((checkout (plist-get build :checkout)))
    (emrs-install--place
     module version
     (lambda (temp)
       (copy-file (plist-get build :library) temp t)
       (when (plist-get build :start)
         (set-file-times temp (plist-get build :start))))
     (concat checkout "\n" (or (plist-get build :package-dir) checkout) "\n"))))

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
                ((and noninteractive (not (eq emrs-install-download 'always)))
                 "not asked in batch mode (set `emrs-install-download' to always)")
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
                (emrs-install--fetch-fail
                 sums-url (format "no checksum for %s" name)))
              (setq bytes (emrs-install--fetch asset-url))
              (setq actual (secure-hash 'sha256 bytes))
              (unless (string= actual expected)
                (user-error "Checksum mismatch for %s: expected %s, got %s; nothing installed"
                            asset-url expected actual))
              (prog1 (list (emrs-install--install module version bytes))
                (message "Downloading the %s module %s...done" module version)))
          (emrs-install--fetch-failed
           (let ((text (format "download of %s failed: %s"
                               (nth 1 err) (nth 2 err))))
             (message "emrs-install: %s" text)
             (cons nil text))))))))

;;;; Local build

(defvar emrs-install--builds (make-hash-table :test 'equal)
  "Running asynchronous builds by build buffer name.
Each value is (MODULE VERSION BUILD), with the module and version
captured when the build started and BUILD the plist that
`emrs-install--install-library' takes.")

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

(defun emrs-install--package (metadata checkout package)
  "Return the package that building PACKAGE in CHECKOUT builds.
METADATA is the parsed output of `cargo metadata' (alists and lists).
PACKAGE is a package name, or nil for the package whose manifest is the
Cargo.toml of CHECKOUT.  Signal a `user-error' if there is none."
  (let ((manifest (file-truename (expand-file-name "Cargo.toml" checkout))))
    (or (seq-find
         (lambda (p)
           (if package
               (equal (alist-get 'name p) package)
             (let ((path (alist-get 'manifest_path p)))
               (and path (file-exists-p path)
                    (equal (file-truename path) manifest)))))
         (alist-get 'packages metadata))
        (user-error "Cargo knows no package %s in %s; check :package of `emrs-install-register'"
                    (or package "the package in Cargo.toml") checkout))))

(defun emrs-install--package-dir (metadata checkout package)
  "Return the directory of the manifest of PACKAGE in CHECKOUT.
METADATA and PACKAGE are as for `emrs-install--package'."
  (let ((path (alist-get 'manifest_path
                         (emrs-install--package metadata checkout package))))
    (if path
        (file-name-as-directory (file-name-directory (expand-file-name path)))
      checkout)))

(defun emrs-install--library-file (metadata checkout package)
  "Return the library file that building PACKAGE in CHECKOUT produces.
METADATA and PACKAGE are as for `emrs-install--package'.  Signal a
`user-error' if there is no such package or it has no cdylib."
  (let* ((pkg (emrs-install--package metadata checkout package))
         (target (seq-find
                  (lambda (tg)
                    (or (member "cdylib" (alist-get 'kind tg))
                        (member "cdylib" (alist-get 'crate_types tg))))
                  (alist-get 'targets pkg))))
    (unless target
      (user-error "Package %s has no cdylib target; add crate-type = [\"cdylib\"] to its [lib] section"
                  (alist-get 'name pkg)))
    (expand-file-name
     (concat "target/release/"
             (if (eq system-type 'windows-nt) "" "lib")
             (replace-regexp-in-string "-" "_" (alist-get 'name target))
             module-file-suffix)
     checkout)))

(defun emrs-install--metadata (checkout program)
  "Return the parsed output of `cargo metadata' in CHECKOUT.
PROGRAM is the cargo program.  Signal a `user-error' if that fails,
with the error output of cargo."
  (let ((default-directory checkout)
        (stderr (make-temp-file "emrs-install-stderr")))
    (unwind-protect
        (with-temp-buffer
          (let ((status (call-process program nil (list t stderr) nil
                                      "metadata" "--no-deps"
                                      "--format-version" "1")))
            (unless (eql status 0)
              (user-error "`%s metadata' failed in %s: %s" program checkout
                          (with-temp-buffer
                            (insert-file-contents stderr)
                            (string-trim (buffer-string)))))
            (goto-char (point-min))
            (condition-case nil
                (json-parse-buffer :object-type 'alist :array-type 'list)
              (error (user-error "`%s metadata' gave no JSON in %s"
                                 program checkout)))))
      (ignore-errors (delete-file stderr)))))

(defun emrs-install--prepare-build (module checkout program)
  "Return the build plist of MODULE in CHECKOUT, using cargo PROGRAM.
It has :library, :checkout and :package-dir; see
`emrs-install--install-library'.  Run `cargo metadata'; signal a
`user-error' if that fails or the package has no cdylib."
  (let ((package (plist-get (gethash module emrs-install--registry) :package))
        (metadata (emrs-install--metadata checkout program)))
    (list :library (emrs-install--library-file metadata checkout package)
          :checkout checkout
          :package-dir (emrs-install--package-dir metadata checkout package))))

(defun emrs-install--install-built (module version build)
  "Install the library of BUILD as MODULE of VERSION.
BUILD is the plist of `emrs-install--install-library'.  Return the
installed file; signal a `user-error' if the library is missing."
  (let ((library (plist-get build :library)))
    (unless (file-exists-p library)
      (user-error "Cargo produced no %s; the package needs crate-type = [\"cdylib\"]"
                  library))
    (emrs-install--install-library module version build)))

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
      (pcase-let ((`(,module ,version ,build) info))
        (if (not (equal message "finished\n"))
            (message "%s module build failed (see %s)" module name)
          (condition-case err
              (progn (emrs-install--install-built module version build)
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
  (let ((version (plist-get (emrs-install--entry module) :version))
        (name (emrs-install--buffer-name module)))
    (when (get-buffer-process name)
      (user-error "The %s module is being built (see %s)" module name))
    (let* ((checkout (emrs-install--source-checkout module))
           (program (emrs-install--program))
           (build (emrs-install--prepare-build module checkout program))
           (start (current-time))
           (default-directory checkout)
           (buffer (compilation-start
                    (mapconcat #'shell-quote-argument
                               (cons program
                                     (emrs-install--build-arguments
                                      module checkout))
                               " ")
                    'compilation-mode
                    (lambda (_mode) name))))
      (puthash name (list module version (plist-put build :start start))
               emrs-install--builds)
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
      (let* ((version (plist-get (emrs-install--entry module) :version))
             (checkout (emrs-install--source-checkout module))
             (program (emrs-install--program))
             (build (emrs-install--prepare-build module checkout program))
             (default-directory checkout)
             (name (emrs-install--buffer-name module))
             buffer status)
        (when (get-buffer-process name)
          (user-error "The %s module is being built (see %s)" module name))
        (when (get-buffer name) (kill-buffer name))
        (setq buffer (get-buffer-create name))
        (setq build (plist-put build :start (current-time)))
        (setq status (apply #'call-process program nil buffer nil
                            (emrs-install--build-arguments module checkout)))
        (unless (eql status 0)
          (if noninteractive
              (error "The build failed:\n%s" (emrs-install--output-tail buffer))
            (error "The build failed (see %s)" (buffer-name buffer))))
        (emrs-install--install-built module version build)
        (message "%s" (emrs-install--built-message module))
        t)
    (error
     (display-warning
      'emrs-install
      (format "Cannot build the %s module: %s" module
              (error-message-string err)))
     nil)))

;;;; Staleness

(defun emrs-install--stale-p (checkout file &optional package-dir)
  "Return non-nil if a source file is newer than FILE.
The sources are the .rs files below src/, build.rs and Cargo.toml of
PACKAGE-DIR (default CHECKOUT) and Cargo.lock of CHECKOUT."
  (let* ((pkg (or package-dir checkout))
         (sources (append
                   (let ((src (expand-file-name "src" pkg)))
                     (and (file-directory-p src)
                          (directory-files-recursively src "\\.rs\\'")))
                   (list (expand-file-name "build.rs" pkg)
                         (expand-file-name "Cargo.toml" pkg)
                         (expand-file-name "Cargo.lock" checkout)))))
    (seq-some (lambda (source)
                (and (file-exists-p source)
                     (file-newer-than-file-p source file)))
              sources)))

(defun emrs-install--built-from (file)
  "Return what the file `built-from' next to the module FILE records.
That is (CHECKOUT PACKAGE-DIR), or nil if there is no such file or the
checkout is gone.  A marker of one line (an older format), or one
whose package directory is gone, gives the checkout as PACKAGE-DIR."
  (let ((marker (expand-file-name "built-from" (file-name-directory file))))
    (when (file-readable-p marker)
      (let* ((lines (split-string
                     (with-temp-buffer
                       (let ((coding-system-for-read 'utf-8))
                         (insert-file-contents marker))
                       (buffer-string))
                     "[\r\n]+" t "[ \t]+"))
             (checkout (car lines))
             (package-dir (cadr lines)))
        (when (and checkout (file-directory-p checkout))
          (list checkout
                (if (and package-dir (file-directory-p package-dir))
                    package-dir
                  checkout)))))))

(defun emrs-install--start-build (module)
  "Start the build of MODULE and tell the user to run the command again."
  (emrs-install-build module)
  (user-error "Building the %s module; run the command again when it is done"
              module))

(defun emrs-install--present (module version file)
  "Return FILE, the file found for MODULE of VERSION, or rebuild it.
If FILE is a local build older than its sources, ask whether to build
it again (interactive sessions only)."
  (let ((built-from (and (equal file (emrs-install--installed-file module version))
                         (emrs-install--built-from file))))
    (when (and built-from
               (not noninteractive)
               (not (emrs-install--loaded-p module))
               (emrs-install--checkout module)
               (emrs-install--stale-p (car built-from) file (cadr built-from))
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
               (or (emrs--module-file (gethash module emrs--modules))
                   "unknown"))))
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
