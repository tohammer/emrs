;;; examples-test.el --- Smoke tests for the example modules  -*- lexical-binding: t; -*-

;;; Commentary:

;; Run with examples/run-examples.sh.  Loads both example modules into
;; one Emacs, so it also checks that modules coexist.

;;; Code:

(require 'ert)
(require 'emrs)

(ert-deftest examples-echo ()
  (let ((e (emrs-start 'echo-rs "echo" nil)))
    (unwind-protect
        (should (equal (emrs-call-sync e '(:echo "hi") 5.0) "hi"))
      (emrs-stop e))))

(ert-deftest examples-grep ()
  (let* ((root (make-temp-file "emrs-grep" t))
         (g (emrs-start 'grep-rs "grep" nil))
         (matches nil) (ends nil))
    (unwind-protect
        (progn
          (make-directory (expand-file-name "sub" root))
          (write-region "one\nTODO first\nthree\n" nil (expand-file-name "a.txt" root))
          (write-region "TODO second\n" nil (expand-file-name "sub/b.txt" root))
          (emrs-call-stream g (list :grep :pattern "TODO" :root root)
                            :on-batch (lambda (items) (setq matches (append matches items)))
                            :on-end (lambda (status err) (push (list status err) ends)))
          (let ((deadline (+ (float-time) 10)))
            (while (and (not ends) (< (float-time) deadline))
              (accept-process-output nil 0.01)))
          (should (equal ends '((done nil))))
          (should (equal (sort (mapcar (lambda (m)
                                         (list (file-name-nondirectory (plist-get m :path))
                                               (plist-get m :line-number)
                                               (plist-get m :preview)))
                                       matches)
                               (lambda (a b) (string< (car a) (car b))))
                         '(("a.txt" 2 "TODO first") ("b.txt" 1 "TODO second"))))
          (should (equal (condition-case err
                             (emrs-call-sync g '(:grep :pattern "x" :root "/no/such/dir") 5.0)
                           (emrs-error (seq-take (cdr err) 1)))
                         '(service))))
      (emrs-stop g)
      (delete-directory root t))))

(ert-deftest examples-modules-coexist ()
  (should (equal (plist-get (emrs-module-info 'echo-rs) :services) '("echo")))
  (should (equal (plist-get (emrs-module-info 'grep-rs) :services) '("grep"))))

;;; examples-test.el ends here
