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

(defvar emrs--modules)
(defvar url-http-response-status)
(defvar url-http-end-of-headers)
(declare-function emrs-load-module "emrs" (module &optional file))
(declare-function emrs-module-info "emrs" (module))

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

(defun emrs-install--install (module version bytes)
  "Install BYTES as the file of MODULE of VERSION and return its path.
Write a temporary file in the target directory and rename it."
  (let* ((target (emrs-install--installed-file module version))
         (dir (file-name-directory target))
         temp)
    (condition-case err
        (progn
          (make-directory dir t)
          (setq temp (make-temp-file (expand-file-name ".emrs-install-" dir)))
          (let ((coding-system-for-write 'no-conversion))
            (write-region bytes nil temp nil 'silent))
          (set-file-modes temp #o755)
          (rename-file temp target t)
          target)
      (file-error
       (when (and temp (file-exists-p temp)) (ignore-errors (delete-file temp)))
       (user-error "Cannot install %s: %s%s" target (error-message-string err)
                   (if (eq system-type 'windows-nt)
                       " (a loaded module cannot be replaced before a restart)"
                     ""))))))

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

;;;; Ensure and load

(defun emrs-install-ensure (module)
  "Return a file to load for MODULE, getting it if necessary.
Signal a `user-error' if nothing works."
  (let* ((entry (emrs-install--entry module))
         (version (plist-get entry :version))
         (platform (emrs-install-platform))
         (download nil))
    (or (emrs-install-file module)
        (car (setq download (emrs-install--download module)))
        ;; Step 3, the local build, goes here.
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
           (unless (emrs-install--checkout module)
             (list "build: no checkout with Cargo.toml found")))
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

(provide 'emrs-install)
;;; emrs-install.el ends here
