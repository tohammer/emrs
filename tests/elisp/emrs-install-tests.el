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

;;;; Local build

(defconst emrs-install-test--repo
  (file-name-as-directory
   (file-truename (expand-file-name ".." emrs-install-test--elisp-dir)))
  "The root of this repository.")

(defun emrs-install-test--register-repo ()
  "Register emrs-test to build from this repository."
  (emrs-install-register
   'emrs-test :version "0.1.0" :package "emrs-test-module"
   :source (expand-file-name "elisp/emrs.el" emrs-install-test--repo)))

(defun emrs-install-test--built-from (file)
  (with-temp-buffer
    (insert-file-contents
     (expand-file-name "built-from" (file-name-directory file)))
    (buffer-string)))

(ert-deftest emrs-install-test-build-sync ()
  (skip-unless (executable-find emrs-install-cargo-program))
  (emrs-install-test--with-env (_dir _root)
    (emrs-install-test--register-repo)
    (should (emrs-install-build-sync 'emrs-test))
    (let ((file (emrs-install--installed-file 'emrs-test "0.1.0")))
      (should (file-executable-p file))
      (should (string-match-p (concat "/emrs-test-module"
                                      (regexp-quote module-file-suffix) "\\'")
                              file))
      (should (string-match-p (regexp-quote
                               (directory-file-name emrs-install-test--repo))
                              (emrs-install-test--built-from file))))))

(ert-deftest emrs-install-test-build-async ()
  (skip-unless (executable-find emrs-install-cargo-program))
  (emrs-install-test--with-env (_dir _root)
    (emrs-install-test--register-repo)
    (let ((process (emrs-install-build 'emrs-test))
          (deadline (+ (float-time) 120)))
      (should (process-live-p process))
      (should-error (emrs-install-build 'emrs-test) :type 'user-error)
      (while (and (< (float-time) deadline)
                  (or (process-live-p process)
                      (gethash "*emrs-install emrs-test*"
                               emrs-install--builds)))
        (accept-process-output nil 0.1))
      (should (emrs-install-test--installed-p))
      (should (file-exists-p
               (expand-file-name
                "built-from"
                (file-name-directory
                 (emrs-install--installed-file 'emrs-test "0.1.0"))))))))

(ert-deftest emrs-install-test-download-removes-built-from ()
  (emrs-install-test--with-env (_dir root)
    (skip-unless (emrs-install-platform))
    (emrs-install-register 'emrs-test :version "0.1.0"
                          :url (emrs-install-test--make-release root "0.1.0"))
    (let ((file (emrs-install--installed-file 'emrs-test "0.1.0")))
      (make-directory (file-name-directory file) t)
      (write-region "/old" nil
                    (expand-file-name "built-from" (file-name-directory file)))
      (let ((emrs-install-download 'always))
        (emrs-install-ensure 'emrs-test))
      (should-not (file-exists-p
                   (expand-file-name "built-from"
                                     (file-name-directory file)))))))

(defconst emrs-install-test--metadata
  (concat "{\"packages\":["
          "{\"name\":\"other\",\"manifest_path\":\"/nowhere/Cargo.toml\","
          "\"targets\":[{\"kind\":[\"lib\"],\"crate_types\":[\"lib\"],\"name\":\"other\"}]},"
          "{\"name\":\"my-mod\",\"manifest_path\":\"%s\","
          "\"targets\":[{\"kind\":[\"lib\"],\"crate_types\":[\"rlib\"],\"name\":\"x\"},"
          "{\"kind\":[\"cdylib\"],\"crate_types\":[\"cdylib\"],\"name\":\"my-mod\"}]}]}")
  "Recorded `cargo metadata' output; %s is the manifest of the second package.")

(ert-deftest emrs-install-test-library-file ()
  (emrs-install-test--with-env (dir _root)
    (let* ((checkout (file-name-as-directory (file-truename dir)))
           (manifest (expand-file-name "Cargo.toml" checkout))
           (metadata (json-parse-string
                      (format emrs-install-test--metadata manifest)
                      :object-type 'alist :array-type 'list)))
      (write-region "" nil manifest)
      (let ((system-type 'gnu/linux))
        (should (equal (emrs-install--library-file metadata checkout nil)
                       (concat checkout "target/release/libmy_mod"
                               module-file-suffix)))
        (should (equal (emrs-install--library-file metadata checkout "my-mod")
                       (concat checkout "target/release/libmy_mod"
                               module-file-suffix))))
      (let ((system-type 'windows-nt))
        (cl-letf (((symbol-function 'file-truename) #'identity))
          (should (equal (emrs-install--library-file metadata checkout nil)
                         (concat checkout "target/release/my_mod"
                                 module-file-suffix)))))
      (let ((err (should-error (emrs-install--library-file metadata checkout "other")
                               :type 'user-error)))
        (should (string-match-p "other" (cadr err)))
        (should (string-match-p "crate-type = \\[\"cdylib\"\\]" (cadr err))))
      (should-error (emrs-install--library-file metadata checkout "missing")
                    :type 'user-error))))

(ert-deftest emrs-install-test-stale ()
  (emrs-install-test--with-env (dir _root)
    (let* ((checkout (file-name-as-directory (expand-file-name "co" dir)))
           (module (expand-file-name "mod.so" dir))
           (old (time-subtract nil 1000))
           (new (time-subtract nil 10)))
      (make-directory (expand-file-name "src" checkout) t)
      (write-region "" nil (expand-file-name "Cargo.toml" checkout))
      (write-region "" nil (expand-file-name "src/lib.rs" checkout))
      (write-region "" nil module)
      (set-file-times (expand-file-name "Cargo.toml" checkout) old)
      (set-file-times (expand-file-name "src/lib.rs" checkout) old)
      (set-file-times module new)
      (should-not (emrs-install--stale-p checkout module))
      (set-file-times module (time-subtract old 10))
      (should (emrs-install--stale-p checkout module))
      (set-file-times module new)
      (write-region "" nil (expand-file-name "src/deep.rs" checkout))
      (make-directory (expand-file-name "src/a" checkout) t)
      (write-region "" nil (expand-file-name "src/a/b.rs" checkout))
      (set-file-times (expand-file-name "src/deep.rs" checkout) old)
      (set-file-times (expand-file-name "src/a/b.rs" checkout) (time-add new 5))
      (should (emrs-install--stale-p checkout module)))))

(ert-deftest emrs-install-test-missing-cargo ()
  (emrs-install-test--with-env (_dir _root)
    (emrs-install-test--register-repo)
    (let ((emrs-install-cargo-program "no-such-cargo-program"))
      (let ((err (should-error (emrs-install-build 'emrs-test)
                               :type 'user-error)))
        (should (string-match-p "emrs-install-cargo-program" (cadr err))))
      (cl-letf (((symbol-function 'display-warning) #'ignore))
        (should-not (emrs-install-build-sync 'emrs-test))))))

(ert-deftest emrs-install-test-final-error-build-outcome ()
  (emrs-install-test--with-env (_dir _root)
    (emrs-install-test--register-repo)
    (let ((err (should-error (emrs-install-ensure 'emrs-test) :type 'user-error)))
      (should (string-match-p "build: not offered in batch mode" (cadr err))))
    (let ((noninteractive nil))
      (cl-letf (((symbol-function 'y-or-n-p) (lambda (&rest _) nil)))
        (let ((err (should-error (emrs-install-ensure 'emrs-test)
                                 :type 'user-error)))
          (should (string-match-p "build: declined" (cadr err))))))
    (emrs-install-register 'emrs-test :version "0.1.0")
    (let ((err (should-error (emrs-install-ensure 'emrs-test) :type 'user-error)))
      (should (string-match-p "build: no checkout" (cadr err))))))

;;;; Commands

(ert-deftest emrs-install-test-module-download-reinstalls ()
  (emrs-install-test--with-env (_dir root)
    (skip-unless (emrs-install-platform))
    (let ((emrs-install-download 'always))
      (emrs-install-register 'emrs-test :version "0.1.0"
                            :url (emrs-install-test--make-release root "0.1.0"))
      (let ((file (emrs-install--installed-file 'emrs-test "0.1.0")))
        (make-directory (file-name-directory file) t)
        (write-region "junk" nil file)
        (emrs-install-module 'emrs-test)
        (should (equal (emrs-install-test--sha file)
                       (emrs-install-test--sha emrs-install-test--module-file)))))))

(ert-deftest emrs-install-test-module-prefix-builds ()
  (skip-unless (executable-find emrs-install-cargo-program))
  (emrs-install-test--with-env (_dir _root)
    (emrs-install-test--register-repo)
    (emrs-install-module 'emrs-test t)
    (let ((deadline (+ (float-time) 120)))
      (while (and (< (float-time) deadline)
                  (or (get-buffer-process "*emrs-install emrs-test*")
                      (gethash "*emrs-install emrs-test*" emrs-install--builds)))
        (accept-process-output nil 0.1)))
    (should (emrs-install-test--installed-p))
    (should (file-exists-p
             (expand-file-name
              "built-from"
              (file-name-directory
               (emrs-install--installed-file 'emrs-test "0.1.0")))))))

(ert-deftest emrs-install-test-module-no-way ()
  (emrs-install-test--with-env (_dir _root)
    (emrs-install-register 'emrs-test :version "0.1.0")
    (should-error (emrs-install-module 'emrs-test) :type 'user-error)
    (should-error (emrs-install-module 'emrs-test t) :type 'user-error)))

(defun emrs-install-test--make-versions (dir module versions)
  (dolist (v versions)
    (make-directory (expand-file-name (format "%s/%s" module v) dir) t)
    (write-region "" nil (expand-file-name (format "%s/%s/f" module v) dir))))

(ert-deftest emrs-install-test-clean ()
  (emrs-install-test--with-env (dir _root)
    (emrs-install-register 'emrs-test :version "0.1.0")
    (emrs-install-test--make-versions dir "emrs-test" '("0.1.0" "0.0.9" "0.0.8"))
    (emrs-install-test--make-versions dir "other" '("0.0.1"))
    (let ((d (lambda (m v) (expand-file-name (format "%s/%s" m v) dir))))
      (cl-letf (((symbol-function 'yes-or-no-p) (lambda (&rest _) nil)))
        (emrs-install-clean))
      (should (file-directory-p (funcall d "emrs-test" "0.0.9")))
      (cl-letf (((symbol-function 'yes-or-no-p) (lambda (&rest _) t))
                ((symbol-function 'emrs-install--loaded-file) (lambda (_) nil)))
        (emrs-install-clean))
      (should (file-directory-p (funcall d "emrs-test" "0.1.0")))
      (should-not (file-directory-p (funcall d "emrs-test" "0.0.9")))
      (should-not (file-directory-p (funcall d "emrs-test" "0.0.8")))
      (should (file-directory-p (funcall d "other" "0.0.1")))
      (let (msg asked)
        (cl-letf (((symbol-function 'yes-or-no-p)
                   (lambda (&rest _) (setq asked t)))
                  ((symbol-function 'message)
                   (lambda (f &rest a) (setq msg (apply #'format f a)))))
          (emrs-install-clean))
        (should-not asked)
        (should (equal msg "Nothing to clean"))))))

(ert-deftest emrs-install-test-clean-keeps-loaded ()
  (emrs-install-test--with-env (dir _root)
    (emrs-install-register 'emrs-test :version "0.1.0")
    (emrs-install-test--make-versions dir "emrs-test" '("0.1.0" "0.0.9" "0.0.8"))
    (let ((loaded (emrs-install--installed-file 'emrs-test "0.0.9")))
      (cl-letf (((symbol-function 'yes-or-no-p) (lambda (&rest _) t))
                ((symbol-function 'emrs-install--loaded-file)
                 (lambda (_) loaded)))
        (emrs-install-clean)))
    (should (file-directory-p (expand-file-name "emrs-test/0.0.9" dir)))
    (should-not (file-directory-p (expand-file-name "emrs-test/0.0.8" dir)))))

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
