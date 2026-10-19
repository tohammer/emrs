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

(defun emrs-install-test--file-url (file)
  "Return the file: URL of the absolute FILE.
A name like C:/dir (Windows) gives file:///C:/dir."
  (let ((file (expand-file-name file)))
    (concat (if (string-prefix-p "/" file) "file://" "file:///") file)))

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
    (emrs-install-test--file-url (directory-file-name root))))

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
    (emrs-install-register 'm :version "0.3.0" :url "https://example.org/r")
    (should (equal (gethash 'm emrs-install--registry)
                   '(:version "0.3.0" :url "https://example.org/r")))))

(ert-deftest emrs-install-test-register-validates ()
  (let ((emrs-install--registry (make-hash-table :test 'eq)))
    (dolist (name '("my-mod" "a" "x1.2_b+c" "9lives"))
      (should (emrs-install-register (intern name) :version "1.0.0")))
    (dolist (name '("" "-x" ".x" "a/b" "a..b" "a b" "../x" "ä"))
      (should-error (emrs-install-register (intern name) :version "1.0.0")))
    (should-error (emrs-install-register "m" :version "1.0.0"))
    (dolist (url '("https://example.org/r" "file:///tmp/r" "file:/tmp/r"))
      (should (emrs-install-register 'm :version "1.0.0" :url url)))
    (dolist (url '("http://example.org/r" "ftp://example.org/r" "u" 42))
      (should-error (emrs-install-register 'm :version "1.0.0" :url url)))
    (should (equal (plist-get (gethash 'm emrs-install--registry) :url)
                   "file:/tmp/r"))))

(ert-deftest emrs-install-test-file-url-name ()
  (let ((system-type 'gnu/linux))
    (should (equal (emrs-install--file-url-name "file:///tmp/a%20b/c")
                   "/tmp/a b/c")))
  (let ((system-type 'windows-nt))
    (should (equal (emrs-install--file-url-name "file:///C:/a%20b/c")
                   "C:/a b/c"))))

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
        ;; Emacs on Windows counts only some extensions as executable, not .dll.
        (unless (eq system-type 'windows-nt) (should (file-executable-p file)))
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
        (let ((err (should-error (emrs-install-ensure 'emrs-test)
                                 :type 'user-error)))
          (should (string-match-p
                   (regexp-quote "download: not asked in batch mode (set `emrs-install-download' to always)")
                   (cadr err)))))
      (let ((emrs-install-download 'always))
        (emrs-install-test--make-release root "0.1.0-snapshot")
        (emrs-install-register 'emrs-test :version "0.1.0-snapshot" :url url)
        (should-error (emrs-install-ensure 'emrs-test) :type 'user-error)
        (emrs-install-register 'emrs-test :version "0.1.0")
        (should-error (emrs-install-ensure 'emrs-test) :type 'user-error)
        ;; No release of that version, so no SHA256SUMS: falls through
        ;; to the final error, which names the file that failed.
        (emrs-install-register 'emrs-test :version "0.9.0" :url url)
        (let ((err (should-error (emrs-install-ensure 'emrs-test)
                                 :type 'user-error)))
          (should (string-match-p
                   "download of file:.*/v0\\.9\\.0/SHA256SUMS failed: no such file"
                   (cadr err)))))
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

(ert-deftest emrs-install-test-download-no-checksum-line ()
  (emrs-install-test--with-env (_dir root)
    (skip-unless (emrs-install-platform))
    (let ((emrs-install-download 'always)
          (url (emrs-install-test--make-release root "0.1.0")))
      (with-temp-file (expand-file-name "v0.1.0/SHA256SUMS" root)
        (insert (make-string 64 ?a) "  some-other-file.so\n"))
      (emrs-install-register 'emrs-test :version "0.1.0" :url url)
      (let ((err (should-error (emrs-install-ensure 'emrs-test)
                               :type 'user-error)))
        (should (string-match-p
                 "download of file:.*/v0\\.1\\.0/SHA256SUMS failed: no checksum for emrs-test-module-"
                 (cadr err))))
      (should-not (emrs-install-test--installed-p)))))

(ert-deftest emrs-install-test-download-missing-asset ()
  (emrs-install-test--with-env (_dir root)
    (skip-unless (emrs-install-platform))
    (let* ((emrs-install-download 'always)
           (url (emrs-install-test--make-release root "0.1.0"))
           (name (format "emrs-test-module-%s%s" (emrs-install-platform)
                         module-file-suffix)))
      (delete-file (expand-file-name (concat "v0.1.0/" name) root))
      (emrs-install-register 'emrs-test :version "0.1.0" :url url)
      (let ((err (should-error (emrs-install-ensure 'emrs-test)
                               :type 'user-error)))
        (should (string-match-p
                 (concat "download of file:.*/v0\\.1\\.0/" (regexp-quote name)
                         " failed: no such file")
                 (cadr err))))
      (should-not (emrs-install-test--installed-p)))))

;;;; HTTP downloads against a local server

(defvar emrs-install-test--server-name "emrs-test-http"
  "Name prefix of the processes of the test HTTP server.")

(defun emrs-install-test--serve (handler)
  "Start a local HTTP server and return its process.
HANDLER is called with the connection process once a request arrived."
  (make-network-process
   :name emrs-install-test--server-name :server t :host 'local
   :service t :family 'ipv4 :coding 'binary :noquery t
   :filter (lambda (proc string)
             (process-put proc :request
                          (concat (process-get proc :request) string))
             (when (and (string-search "\r\n\r\n" (process-get proc :request))
                        (not (process-get proc :answered)))
               (process-put proc :answered t)
               (funcall handler proc)))))

(defun emrs-install-test--send (proc string)
  "Send STRING on PROC if it is still open."
  (when (process-live-p proc)
    (process-send-string proc string)))

(defun emrs-install-test--http-leftovers (processes buffers)
  "Return processes and buffers of url.el not in PROCESSES and BUFFERS."
  (append
   (seq-filter (lambda (p)
                 (not (or (memq p processes)
                          (string-prefix-p emrs-install-test--server-name
                                           (process-name p)))))
               (process-list))
   (seq-filter (lambda (b)
                 (and (not (memq b buffers))
                      (string-match-p "\\*http " (buffer-name b))))
               (buffer-list))))

(defmacro emrs-install-test--with-server (bindings handler &rest body)
  "Run BODY with a local HTTP server that answers with HANDLER.
BINDINGS is (URL) naming the variable for the base URL of the server."
  (declare (indent 2))
  `(progn
     (url-do-setup)
     (let* ((server (emrs-install-test--serve ,handler))
            (,(car bindings)
             (format "http://127.0.0.1:%d/"
                     (process-contact server :service)))
            (url-proxy-services nil))
       (unwind-protect (progn ,@body)
         (dolist (p (process-list))
           (when (string-prefix-p emrs-install-test--server-name
                                  (process-name p))
             (delete-process p)))))))

(ert-deftest emrs-install-test-http-ok ()
  (emrs-install-test--with-server (url)
      (lambda (proc)
        (emrs-install-test--send
         proc "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n\377\0ab"))
    (let ((processes (process-list)) (buffers (buffer-list)))
      (let ((bytes (emrs-install--fetch (concat url "f"))))
        (should (equal bytes "\377\0ab"))
        (should-not (multibyte-string-p bytes)))
      (should-not (emrs-install-test--http-leftovers processes buffers)))))

(ert-deftest emrs-install-test-http-redirect ()
  (emrs-install-test--with-server (url)
      (lambda (proc)
        (emrs-install-test--send
         proc
         (if (string-prefix-p "GET /from " (process-get proc :request))
             "HTTP/1.1 302 Found\r\nLocation: /to\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
           "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")))
    (let ((processes (process-list)) (buffers (buffer-list)))
      (should (equal (emrs-install--fetch (concat url "from")) "ok"))
      (should-not (emrs-install-test--http-leftovers processes buffers)))))

(ert-deftest emrs-install-test-http-timeout-is-idle ()
  ;; Five bytes 0.4 s apart: longer in total than the timeout, but never
  ;; idle for that long.
  (emrs-install-test--with-server (url)
      (lambda (proc)
        (emrs-install-test--send
         proc "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n")
        (dotimes (i 5)
          (run-at-time (* 0.4 (1+ i)) nil #'emrs-install-test--send
                       proc (string (+ ?a i)))))
    (let ((emrs-install-timeout 1)
          (start (float-time)))
      (should (equal (emrs-install--fetch (concat url "f")) "abcde"))
      (should (> (float-time) (+ start 1.5))))))

(ert-deftest emrs-install-test-http-timeout-closes-connection ()
  (emrs-install-test--with-server (url)
      (lambda (proc)
        (emrs-install-test--send
         proc "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nabc"))
    (let ((emrs-install-timeout 1)
          (processes (process-list))
          (buffers (buffer-list)))
      (let ((err (should-error (emrs-install--fetch (concat url "f"))
                               :type 'emrs-install--fetch-failed)))
        (should (equal (cdr err)
                       (list (concat url "f") "no data for 1 seconds"))))
      (should-not (emrs-install-test--http-leftovers processes buffers)))))

(ert-deftest emrs-install-test-http-status ()
  (emrs-install-test--with-server (url)
      (lambda (proc)
        (emrs-install-test--send
         proc "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"))
    (let ((processes (process-list))
          (buffers (buffer-list)))
      (let ((err (should-error (emrs-install--fetch (concat url "f"))
                               :type 'emrs-install--fetch-failed)))
        (should (equal (nth 1 err) (concat url "f")))
        (should (string-match-p "404" (nth 2 err))))
      (should-not (emrs-install-test--http-leftovers processes buffers)))))

(ert-deftest emrs-install-test-other-scheme-fails ()
  ;; A scheme that url.el handles but that has no HTTP status.
  (let ((err (should-error (emrs-install--fetch "data:text/plain,hello")
                           :type 'emrs-install--fetch-failed)))
    (should (equal (nth 2 err) "no HTTP response"))))

;;;; Installing

(ert-deftest emrs-install-test-marker-after-rename ()
  (emrs-install-test--with-env (dir _root)
    (let* ((target (emrs-install--installed-file 'm "1.0.0"))
           (vdir (file-name-directory target))
           (marker (expand-file-name "built-from" vdir))
           (lib (expand-file-name "lib.so" dir))
           (build (list :library lib :checkout "/new/" :package-dir "/new/")))
      (write-region "lib" nil lib)
      ;; A non-empty directory in place of the module: the rename fails.
      (make-directory target t)
      (write-region "" nil (expand-file-name "x" target))
      (write-region "/old/\n" nil marker)
      (should-error (emrs-install--install-library 'm "1.0.0" build)
                    :type 'user-error)
      (should (equal (with-temp-buffer (insert-file-contents marker)
                                       (buffer-string))
                     "/old/\n"))
      (should-error (emrs-install--install 'm "1.0.0" "bytes")
                    :type 'user-error)
      (should (file-exists-p marker))
      (delete-file marker)
      (should-error (emrs-install--install-library 'm "1.0.0" build)
                    :type 'user-error)
      (should-not (file-exists-p marker))
      (should-not (directory-files vdir nil "\\`\\.emrs-install-")))))

(ert-deftest emrs-install-test-temp-file-removed-on-quit ()
  (emrs-install-test--with-env (_dir _root)
    (let ((vdir (file-name-directory (emrs-install--installed-file 'm "1.0.0")))
          (quitted nil))
      (condition-case nil
          (emrs-install--place 'm "1.0.0"
                               (lambda (temp)
                                 (write-region "partial" nil temp)
                                 (signal 'quit nil))
                               nil)
        (quit (setq quitted t)))
      (should quitted)
      (should-not (directory-files vdir nil "\\`\\.emrs-install-")))))

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
    (let ((before (time-subtract nil 1)))
      (should (emrs-install-build-sync 'emrs-test))
      (let* ((file (emrs-install--installed-file 'emrs-test "0.1.0"))
             (marker (expand-file-name "built-from" (file-name-directory file)))
             (mtime (file-attribute-modification-time (file-attributes file))))
        ;; The start of the build: not before this test, not after the
        ;; marker was written.
        (should (time-less-p before mtime))
        (should-not (time-less-p
                     (file-attribute-modification-time (file-attributes marker))
                     mtime))))
    (let ((file (emrs-install--installed-file 'emrs-test "0.1.0")))
      ;; Emacs on Windows counts only some extensions as executable, not .dll.
      (unless (eq system-type 'windows-nt) (should (file-executable-p file)))
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
          (deadline (+ (float-time) 600)))
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

(ert-deftest emrs-install-test-stale-workspace ()
  (emrs-install-test--with-env (dir _root)
    (let* ((checkout (file-name-as-directory (expand-file-name "co" dir)))
           (member (file-name-as-directory (expand-file-name "crates/m" checkout)))
           (module (expand-file-name "mod.so" dir))
           (marker (expand-file-name "built-from" dir))
           (old (time-subtract nil 1000))
           (new (time-subtract nil 10)))
      (dolist (d (list (expand-file-name "src" checkout)
                       (expand-file-name "src" member)))
        (make-directory d t))
      (dolist (f '("Cargo.toml" "Cargo.lock" "src/lib.rs"
                   "crates/m/Cargo.toml" "crates/m/src/lib.rs"))
        (write-region "" nil (expand-file-name f checkout))
        (set-file-times (expand-file-name f checkout) old))
      (write-region "" nil module)
      (set-file-times module new)
      ;; Two lines: the checkout and the package directory.
      (write-region (concat checkout "\n" member "\n") nil marker)
      (should (equal (emrs-install--built-from module) (list checkout member)))
      ;; A newer file in the root's src/ does not belong to the package.
      (set-file-times (expand-file-name "src/lib.rs" checkout) (time-add new 5))
      (should-not (emrs-install--stale-p checkout module member))
      (set-file-times (expand-file-name "crates/m/src/lib.rs" checkout)
                      (time-add new 5))
      (should (emrs-install--stale-p checkout module member))
      (set-file-times (expand-file-name "crates/m/src/lib.rs" checkout) old)
      (set-file-times (expand-file-name "Cargo.lock" checkout) (time-add new 5))
      (should (emrs-install--stale-p checkout module member))
      (set-file-times (expand-file-name "Cargo.lock" checkout) old)
      (should-not (emrs-install--stale-p checkout module member))
      ;; One line (the older format): the checkout is the package.
      (write-region (concat checkout "\n") nil marker)
      (should (equal (emrs-install--built-from module) (list checkout checkout)))
      (should (emrs-install--stale-p checkout module))
      ;; A gone checkout: no marker.
      (write-region (concat (expand-file-name "gone" dir) "\n") nil marker)
      (should-not (emrs-install--built-from module)))))

(defmacro emrs-install-test--with-built (bindings &rest body)
  "Run BODY with a locally built, stale module of a registered checkout.
BINDINGS is (FILE) naming the variable for the installed file."
  (declare (indent 1))
  `(emrs-install-test--with-env (dir _root)
     (let* ((checkout (file-name-as-directory
                       (file-truename (expand-file-name "co" dir))))
            (,(car bindings) (emrs-install--installed-file 'm "1.0.0")))
       (make-directory (expand-file-name "src" checkout) t)
       (write-region "" nil (expand-file-name "Cargo.toml" checkout))
       (write-region "" nil (expand-file-name "src/lib.rs" checkout))
       (write-region "" nil (expand-file-name "m.el" checkout))
       (make-directory (file-name-directory ,(car bindings)) t)
       (write-region "" nil ,(car bindings))
       (set-file-times ,(car bindings) (time-subtract nil 1000))
       (write-region (concat checkout "\n" checkout "\n") nil
                     (expand-file-name "built-from"
                                       (file-name-directory ,(car bindings))))
       (emrs-install-register 'm :version "1.0.0"
                              :source (expand-file-name "m.el" checkout))
       ,@body)))

(ert-deftest emrs-install-test-present-rebuild-prompt ()
  (emrs-install-test--with-built (file)
    (let ((asked 0) (built nil) (answer t) (loaded nil) (noninteractive nil))
      (cl-letf (((symbol-function 'y-or-n-p)
                 (lambda (&rest _) (cl-incf asked) answer))
                ((symbol-function 'emrs-install-build)
                 (lambda (module) (push module built)))
                ((symbol-function 'emrs-install--loaded-p)
                 (lambda (_) loaded)))
        ;; Marker, interactive, not loaded, stale: ask and build.
        (should-error (emrs-install--present 'm "1.0.0" file)
                      :type 'user-error)
        (should (equal (list asked built) '(1 (m))))
        ;; Declined: use the old file.
        (setq answer nil)
        (should (equal (emrs-install--present 'm "1.0.0" file) file))
        (should (equal (list asked built) '(2 (m))))
        (setq answer t)
        ;; Batch mode, a loaded module, a fresh module, no marker: no prompt.
        (let ((noninteractive t))
          (should (equal (emrs-install--present 'm "1.0.0" file) file)))
        (setq loaded t)
        (should (equal (emrs-install--present 'm "1.0.0" file) file))
        (setq loaded nil)
        (set-file-times file (time-add nil 100))
        (should (equal (emrs-install--present 'm "1.0.0" file) file))
        (set-file-times file (time-subtract nil 1000))
        (delete-file (expand-file-name "built-from" (file-name-directory file)))
        (should (equal (emrs-install--present 'm "1.0.0" file) file))
        (should (equal (list asked built) '(2 (m))))))))

(ert-deftest emrs-install-test-build-finished-hook ()
  (emrs-install-test--with-env (dir _root)
    (emrs-install-register 'm :version "2.0.0")
    (let* ((lib (expand-file-name "lib.so" dir))
           (name (emrs-install--buffer-name 'm))
           (buffer (get-buffer-create name))
           (start (time-subtract nil 3600))
           (co (file-name-as-directory dir))
           (pkg (file-name-as-directory (expand-file-name "pkg" dir)))
           (entry (list 'm "1.0.0"
                        (list :library lib :checkout co :package-dir pkg
                              :start start)))
           (emrs-install--builds (make-hash-table :test 'equal)))
      (write-region "lib" nil lib)
      (unwind-protect
          (cl-letf (((symbol-function 'message) #'ignore))
            (puthash name entry emrs-install--builds)
            ;; Another compilation buffer is ignored.
            (with-temp-buffer
              (emrs-install--build-finished (current-buffer) "finished\n"))
            (should (gethash name emrs-install--builds))
            ;; A failed build installs nothing and forgets the build.
            (emrs-install--build-finished buffer "exited abnormally with code 101\n")
            (should (zerop (hash-table-count emrs-install--builds)))
            (should-not (file-exists-p (emrs-install--installed-file 'm "1.0.0")))
            (should-not (file-exists-p (emrs-install--installed-file 'm "2.0.0")))
            ;; Success installs under the version captured at the start,
            ;; with the start time as modification time.
            (puthash name entry emrs-install--builds)
            (emrs-install--build-finished buffer "finished\n")
            (should (zerop (hash-table-count emrs-install--builds)))
            (let ((file (emrs-install--installed-file 'm "1.0.0")))
              (should (file-exists-p file))
              (should-not (file-exists-p (emrs-install--installed-file 'm "2.0.0")))
              (should (< (abs (float-time
                               (time-subtract
                                (file-attribute-modification-time
                                 (file-attributes file))
                                start)))
                         1))
              (should (equal (emrs-install-test--built-from file)
                             (concat co "\n" pkg "\n")))))
        (kill-buffer buffer)))))

(ert-deftest emrs-install-test-metadata-error-output ()
  (skip-unless (not (memq system-type '(windows-nt ms-dos))))
  (emrs-install-test--with-env (dir _root)
    (let ((cargo (expand-file-name "fake-cargo" dir))
          (checkout (expand-file-name "co/" dir)))
      (make-directory checkout t)
      (write-region "" nil (expand-file-name "Cargo.toml" checkout))
      (write-region "" nil (expand-file-name "m.el" checkout))
      (write-region "#!/bin/sh\necho 'error: boom in the manifest' >&2\nexit 101\n"
                    nil cargo)
      (set-file-modes cargo #o755)
      (emrs-install-register 'm :version "1.0.0"
                             :source (expand-file-name "m.el" checkout))
      (let* ((emrs-install-cargo-program cargo)
             (err (should-error (emrs-install-build 'm) :type 'user-error)))
        (should (string-match-p "metadata. failed in .*: error: boom in the manifest"
                                (cadr err)))))))

(ert-deftest emrs-install-test-build-sync-unregistered ()
  (emrs-install-test--with-env (_dir _root)
    (let (warning)
      (cl-letf (((symbol-function 'display-warning)
                 (lambda (_type text &rest _) (setq warning text))))
        (should-not (emrs-install-build-sync 'nope)))
      (should (string-match-p "nope.*not registered" warning)))))

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
    (let ((deadline (+ (float-time) 600)))
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

(defun emrs-install-test--run (version root &optional setup)
  "Run a fresh Emacs that registers emrs-test at VERSION and loads it.
ROOT is the release dir.  SETUP is a form to run after registering,
before loading.  Return (EXIT . OUTPUT)."
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
                   :url ,(emrs-install-test--file-url
                          (directory-file-name root)))
                  ,setup
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

(ert-deftest emrs-install-test-load-failure ()
  (emrs-install-test--with-env (_dir root)
    (let ((res (emrs-install-test--run
                "0.1.0" root
                '(let ((f (emrs-install--installed-file 'emrs-test "0.1.0")))
                   (make-directory (file-name-directory f) t)
                   (write-region "not a module" nil f)))))
      (should-not (eql (car res) 0))
      (should (string-match-p "Cannot load the emrs-test module from .*/emrs-test/0\\.1\\.0/"
                              (cdr res)))
      (should (string-match-p "^Platform: .*; system-configuration: .*; Emacs "
                              (cdr res))))))

(ert-deftest emrs-install-test-load-version-warning-names-loaded-file ()
  ;; The module is already loaded from elsewhere; the registered version
  ;; is installed too.  The warning names the file that was loaded.
  (emrs-install-test--with-env (_dir root)
    (let ((res (emrs-install-test--run
                "0.1.1" root
                `(progn
                   (require 'emrs)
                   (emrs-load-module 'emrs-test ,emrs-install-test--module-file)
                   (let ((f (emrs-install--installed-file 'emrs-test "0.1.1")))
                     (make-directory (file-name-directory f) t)
                     (write-region "" nil f))))))
      (should (eql (car res) 0))
      (should (string-match-p
               (concat "reports version 0.1.0 but 0.1.1 is registered (file "
                       (regexp-quote emrs-install-test--module-file) ")")
               (cdr res))))))

(provide 'emrs-install-tests)
;;; emrs-install-tests.el ends here
