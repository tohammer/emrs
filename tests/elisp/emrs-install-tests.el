;;; emrs-install-tests.el --- Tests for emrs-install  -*- lexical-binding: t; -*-

;;; Commentary:

;; Run with tests/run-ert.sh.  Downloads use a temporary `file://' release
;; directory; loading is tested in subprocesses because the suite's Emacs
;; already has the emrs-test module loaded.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'emrs-install)

(defconst emrs-install-test--module-file
  (locate-file "emrs-test-module" load-path (list module-file-suffix))
  "The built emrs-test module.")

(defconst emrs-install-test--elisp-dir
  (file-name-directory (locate-library "emrs-install")))

(defun emrs-install-test--sha (file)
  "Return the SHA-256 of FILE."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert-file-contents-literally file)
    (secure-hash 'sha256 (current-buffer))))

(defun emrs-install-test--make-release (root version &optional sum)
  "Make a release of VERSION under ROOT and return the base URL.
The asset is the emrs-test module; SUM overrides its checksum."
  (let* ((platform (or (emrs-install-platform) "linux-x86_64"))
         (name (format "emrs-test-module-%s%s" platform module-file-suffix))
         (dir (expand-file-name (concat "v" version "/") root)))
    (make-directory dir t)
    (copy-file emrs-install-test--module-file (expand-file-name name dir) t)
    (with-temp-file (expand-file-name "SHA256SUMS" dir)
      (insert (or sum (emrs-install-test--sha emrs-install-test--module-file))
              "  " name "\n"))
    (concat "file://" (directory-file-name (expand-file-name root)))))

(defmacro emrs-install-test--with-env (bindings &rest body)
  "Run BODY with a temporary install dir and release root.
BINDINGS is (DIR ROOT) naming the variables for both."
  (declare (indent 1))
  `(let* ((,(car bindings) (make-temp-file "emrs-inst" t))
          (,(cadr bindings) (make-temp-file "emrs-rel" t))
          (emrs-install-directory (file-name-as-directory ,(car bindings)))
          (emrs-install--registry (make-hash-table :test 'eq))
          (emrs-install--warned nil)
          (load-path (cl-remove-if
                      (lambda (d) (locate-file "emrs-test-module" (list d)
                                               (list module-file-suffix)))
                      load-path)))
     (unwind-protect (progn ,@body)
       (delete-directory ,(car bindings) t)
       (delete-directory ,(cadr bindings) t))))

;;;; Platform

(ert-deftest emrs-install-test-platform ()
  (cl-flet ((plat (type conf)
              (let ((system-type type) (system-configuration conf))
                (emrs-install-platform))))
    (should (equal (plat 'gnu/linux "x86_64-pc-linux-gnu") "linux-x86_64"))
    (should (equal (plat 'gnu/linux "aarch64-unknown-linux-gnu") "linux-aarch64"))
    (should-not (plat 'gnu/linux "x86_64-pc-linux-musl"))
    (should-not (plat 'gnu/linux "riscv64-pc-linux-gnu"))
    (should (equal (plat 'darwin "aarch64-apple-darwin24.0.0") "macos-aarch64"))
    (should (equal (plat 'darwin "arm-apple-darwin24.0.0") "macos-aarch64"))
    (should-not (plat 'darwin "x86_64-apple-darwin23.0.0"))
    (should (equal (plat 'windows-nt "x86_64-w64-mingw32") "windows-x86_64"))
    (should-not (plat 'windows-nt "aarch64-w64-mingw32"))
    (should-not (plat 'berkeley-unix "x86_64-unknown-freebsd"))))

;;;; Checksums

(ert-deftest emrs-install-test-checksum-parsing ()
  (let* ((a (make-string 64 ?a)) (b (make-string 64 ?B))
         (text (concat a "  one.so\n" b " *two.so\r\n"
                       "short  three.so\n" a " four.so\n")))
    (should (equal (emrs-install--checksum-for text "one.so") a))
    (should (equal (emrs-install--checksum-for text "two.so") (downcase b)))
    (should-not (emrs-install--checksum-for text "three.so"))
    (should-not (emrs-install--checksum-for text "four.so"))
    (should-not (emrs-install--checksum-for text "missing.so"))))

;;;; Registration

(ert-deftest emrs-install-test-register ()
  (let ((emrs-install--registry (make-hash-table :test 'eq)))
    (should-error (emrs-install-register 'm))
    (should-error (emrs-install-register 'm :version 1))
    (should-error (emrs-install-register 'm :version "0.2.0-dev"))
    (should-not (gethash 'm emrs-install--registry))
    (emrs-install-register 'm :version "0.2.0-snapshot")
    (should (equal (plist-get (gethash 'm emrs-install--registry) :version)
                   "0.2.0-snapshot"))
    (emrs-install-register 'm :version "0.3.0" :url "u")
    (should (equal (gethash 'm emrs-install--registry)
                   '(:version "0.3.0" :url "u")))))

;;;; Lookup

(ert-deftest emrs-install-test-lookup-order ()
  (emrs-install-test--with-env (dir _root)
    (emrs-install-register 'emrs-test :version "0.1.0")
    (should-not (emrs-install-file 'emrs-test))
    (let ((load-path (cons (file-name-directory emrs-install-test--module-file)
                           load-path)))
      (should (equal (emrs-install-file 'emrs-test)
                     emrs-install-test--module-file))
      (let ((installed (emrs-install--installed-file 'emrs-test "0.1.0")))
        (make-directory (file-name-directory installed) t)
        (copy-file emrs-install-test--module-file installed)
        (should (equal (emrs-install-file 'emrs-test) installed))
        (should (string-prefix-p (file-name-as-directory dir) installed))))))

(ert-deftest emrs-install-test-checkout ()
  (emrs-install-test--with-env (dir _root)
    (let ((src (expand-file-name "pkg/lisp/p.el" dir)))
      (make-directory (file-name-directory src) t)
      (write-region "" nil src)
      (write-region "" nil (expand-file-name "pkg/Cargo.toml" dir))
      (emrs-install-register 'm :version "1.0.0" :source (concat src "c"))
      (should (equal (emrs-install--checkout 'm)
                     (file-name-as-directory
                      (file-truename (expand-file-name "pkg" dir)))))
      (emrs-install-register 'm :version "1.0.0")
      (should-not (emrs-install--checkout 'm)))))

;;;; Download

(defun emrs-install-test--installed-p ()
  (file-exists-p (emrs-install--installed-file 'emrs-test "0.1.0")))

(ert-deftest emrs-install-test-download-always ()
  (emrs-install-test--with-env (_dir root)
    (let ((emrs-install-download 'always)
          (url (emrs-install-test--make-release root "0.1.0")))
      (skip-unless (emrs-install-platform))
      (emrs-install-register 'emrs-test :version "0.1.0" :url url)
      (let ((file (emrs-install-ensure 'emrs-test)))
        (should (equal file (emrs-install--installed-file 'emrs-test "0.1.0")))
        (should (file-executable-p file))
        (should (equal (emrs-install-test--sha file)
                       (emrs-install-test--sha emrs-install-test--module-file)))
        (should-not (file-exists-p
                     (expand-file-name "built-from" (file-name-directory file))))))))

(ert-deftest emrs-install-test-download-skipped ()
  (emrs-install-test--with-env (_dir root)
    (skip-unless (emrs-install-platform))
    (let ((url (emrs-install-test--make-release root "0.1.0")))
      (let ((emrs-install-download 'never))
        (emrs-install-register 'emrs-test :version "0.1.0" :url url)
        (should-error (emrs-install-ensure 'emrs-test) :type 'user-error))
      (let ((emrs-install-download 'ask))   ; batch: counts as never
        (should-error (emrs-install-ensure 'emrs-test) :type 'user-error))
      (let ((emrs-install-download 'always))
        (emrs-install-test--make-release root "0.1.0-snapshot")
        (emrs-install-register 'emrs-test :version "0.1.0-snapshot" :url url)
        (should-error (emrs-install-ensure 'emrs-test) :type 'user-error)
        (emrs-install-register 'emrs-test :version "0.1.0")
        (should-error (emrs-install-ensure 'emrs-test) :type 'user-error)
        ;; Missing asset: falls through to the final error.
        (emrs-install-register 'emrs-test :version "0.9.0" :url url)
        (let ((err (should-error (emrs-install-ensure 'emrs-test)
                                 :type 'user-error)))
          (should (string-match-p "download from .* failed" (cadr err)))))
      (should-not (emrs-install-test--installed-p)))))

(ert-deftest emrs-install-test-download-bad-checksum ()
  (emrs-install-test--with-env (_dir root)
    (skip-unless (emrs-install-platform))
    (let ((emrs-install-download 'always)
          (url (emrs-install-test--make-release root "0.1.0"
                                                (make-string 64 ?0))))
      (emrs-install-register 'emrs-test :version "0.1.0" :url url)
      (let ((err (should-error (emrs-install-ensure 'emrs-test)
                               :type 'user-error)))
        (should (string-match-p "Checksum mismatch" (cadr err)))
        (should (string-match-p (make-string 64 ?0) (cadr err))))
      (should-not (emrs-install-test--installed-p)))))

(ert-deftest emrs-install-test-final-error ()
  (emrs-install-test--with-env (_dir _root)
    (emrs-install-register 'emrs-test :version "0.1.0")
    (let ((err (should-error (emrs-install-ensure 'emrs-test) :type 'user-error)))
      (should (string-match-p "emrs-test module 0.1.0" (cadr err)))
      (should (string-match-p "emrs-install-module" (cadr err))))
    (let ((system-type 'berkeley-unix)
          (system-configuration "x86_64-unknown-freebsd"))
      (let ((err (should-error (emrs-install-ensure 'emrs-test)
                               :type 'user-error)))
        (should (string-match-p "no prebuilt module for x86_64-unknown-freebsd"
                                (cadr err)))))
    (should-error (emrs-install-ensure 'nonexistent) :type 'user-error)))

;;;; Loading, in a fresh Emacs

(defun emrs-install-test--run (version root)
  "Run a fresh Emacs that registers emrs-test at VERSION and loads it.
RELEASE-ROOT is the release dir.  Return (EXIT . OUTPUT)."
  (let* ((dir (make-temp-file "emrs-load" t))
         (emacs (expand-file-name invocation-name invocation-directory))
         (form `(progn
                  (setq emrs-install-directory ,(concat dir "/")
                        emrs-install-download 'always
                        load-path (cl-remove-if
                                   (lambda (d) (file-exists-p
                                                (expand-file-name
                                                 (concat "emrs-test-module"
                                                         module-file-suffix) d)))
                                   load-path))
                  (emrs-install-register
                   'emrs-test :version ,version
                   :url ,(concat "file://" (directory-file-name root)))
                  (let ((h (emrs-install-load 'emrs-test)))
                    (princ (format "OK %S %s\n"
                                   (plist-get (emrs-module-info 'emrs-test)
                                              :version)
                                   (and h t))))
                  (princ (format "FILE %s\n" (emrs-install-file 'emrs-test))))))
    (unwind-protect
        (with-temp-buffer
          (let ((code (call-process
                       emacs nil t nil "-Q" "--batch" "--module-assertions"
                       "-L" emrs-install-test--elisp-dir
                       "--eval" "(require 'cl-lib)"
                       "--eval" "(require 'emrs-install)"
                       "--eval" (prin1-to-string form))))
            (cons code (buffer-string))))
      (delete-directory dir t))))

(ert-deftest emrs-install-test-load-subprocess ()
  (skip-unless (emrs-install-platform))
  (emrs-install-test--with-env (_dir root)
    (emrs-install-test--make-release root "0.1.0")
    (let ((res (emrs-install-test--run "0.1.0" root)))
      (should (eql (car res) 0))
      (should (string-match-p "^OK \"0.1.0\" t$" (cdr res)))
      (should (string-match-p "^FILE .*/emrs-test/0.1.0/emrs-test-module"
                              (cdr res)))
      (should-not (string-match-p "Warning" (cdr res))))))

(ert-deftest emrs-install-test-load-version-warning ()
  (skip-unless (emrs-install-platform))
  (emrs-install-test--with-env (_dir root)
    (emrs-install-test--make-release root "0.1.1")
    (let ((res (emrs-install-test--run "0.1.1" root)))
      (should (eql (car res) 0))
      (should (string-match-p "^OK \"0.1.0\" t$" (cdr res)))
      (should (string-match-p "emrs-install" (cdr res)))
      (should (string-match-p "reports version 0.1.0 but 0.1.1 is registered"
                              (cdr res))))))

(provide 'emrs-install-tests)
;;; emrs-install-tests.el ends here
