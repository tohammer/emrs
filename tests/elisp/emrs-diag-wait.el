;;; emrs-diag-wait.el --- Per-step timing of `emrs--wait'  -*- lexical-binding: t; -*-

;;; Commentary:

;; Temporary diagnostic, not part of the test suite.  Load it after
;; emrs-tests.el.  It times the steps of every outermost `emrs--wait' and
;; prints a report for each wait that took longer than
;; `emrs-diag-report-above' seconds, to find out why
;; `emrs-test-stream-sync-timeout-cancels-the-stream' overshoots its
;; timeout on the Linux CI runners.

;;; Code:

(require 'emrs)

(defvar emrs-diag-report-above 0.15
  "Report a wait that took longer than this many seconds.")

(defvar emrs-diag-slow-step 0.05
  "List every single step that took longer than this many seconds.")

(defvar emrs-diag--depth 0 "Nesting depth of `emrs--wait'.")
(defvar emrs-diag--in-accept nil "Non-nil inside `accept-process-output'.")
(defvar emrs-diag--start 0.0 "Start time of the outermost wait.")
(defvar emrs-diag--stats nil "Alist of (STEP COUNT TOTAL WORST).")
(defvar emrs-diag--slow nil "List of (OFFSET STEP SECONDS NOTE), newest first.")
(defvar emrs-diag--records 0 "Number of records fetched during the wait.")
(defvar emrs-diag--items 0 "Number of stream items fetched during the wait.")

(defun emrs-diag--note (step seconds &optional note)
  "Account SECONDS for STEP; remember it with NOTE if it was slow."
  (let ((cell (or (assq step emrs-diag--stats)
                  (car (push (list step 0 0.0 0.0) emrs-diag--stats)))))
    (setcar (nthcdr 1 cell) (1+ (nth 1 cell)))
    (setcar (nthcdr 2 cell) (+ (nth 2 cell) seconds))
    (setcar (nthcdr 3 cell) (max (nth 3 cell) seconds)))
  (when (> seconds emrs-diag-slow-step)
    (push (list (- (float-time) emrs-diag--start seconds) step seconds note)
          emrs-diag--slow)))

(defun emrs-diag--timed (step function args &optional note-fn)
  "Apply FUNCTION to ARGS and account the time for STEP.
NOTE-FN, called with the result, returns a note for a slow step."
  (if (zerop emrs-diag--depth)
      (apply function args)
    (let ((start (float-time)) (result nil))
      (unwind-protect
          (setq result (apply function args))
        (emrs-diag--note step (- (float-time) start)
                         (and note-fn (funcall note-fn result))))
      result)))

(defun emrs-diag--fetch (function m)
  "Around advice for `emrs--fetch': time FUNCTION on M and count what arrived."
  (let ((before (length (emrs--module-pending m))))
    (emrs-diag--timed
     (if emrs-diag--in-accept 'fetch-in-filter 'fetch)
     function (list m)
     (lambda (_result)
       (let ((new (nthcdr before (emrs--module-pending m))) (items 0))
         (dolist (delivery new)
           (when (eq (aref delivery 0) 'items)
             (setq items (+ items (length (aref delivery 3))))))
         (setq emrs-diag--records (+ emrs-diag--records (length new))
               emrs-diag--items (+ emrs-diag--items items))
         (format "%d records, %d items" (length new) items))))))

(defun emrs-diag--accept (function &rest args)
  "Around advice for `accept-process-output': time FUNCTION on ARGS."
  (if emrs-diag--in-accept
      (apply function args)
    (let ((emrs-diag--in-accept t))
      (emrs-diag--timed 'accept function args
                        (lambda (result)
                          (format "asked for %S s, returned %S" (nth 1 args) result))))))

(defun emrs-diag--wait (function call timeout &rest args)
  "Around advice for `emrs--wait': time FUNCTION and report a slow wait."
  (if (> emrs-diag--depth 0)
      (let ((emrs-diag--depth (1+ emrs-diag--depth)))
        (apply function call timeout args))
    (setq emrs-diag--stats nil emrs-diag--slow nil
          emrs-diag--records 0 emrs-diag--items 0
          emrs-diag--start (float-time))
    (let ((gcs gcs-done) (gc gc-elapsed))
      (unwind-protect
          (let ((emrs-diag--depth 1))
            (apply function call timeout args))
        (let ((elapsed (- (float-time) emrs-diag--start)))
          (when (> elapsed emrs-diag-report-above)
            (message "DIAG wait: %.3f s, timeout %S, %d records, %d items, %d gcs in %.3f s"
                     elapsed timeout emrs-diag--records emrs-diag--items
                     (- gcs-done gcs) (- gc-elapsed gc))
            (dolist (cell (sort (copy-sequence emrs-diag--stats)
                                (lambda (a b) (> (nth 2 a) (nth 2 b)))))
              (message "DIAG   %-16s n=%-6d total=%.3f worst=%.3f"
                       (nth 0 cell) (nth 1 cell) (nth 2 cell) (nth 3 cell)))
            (dolist (slow (reverse emrs-diag--slow))
              (message "DIAG   slow at +%.3f: %s %.3f s (%s)"
                       (nth 0 slow) (nth 1 slow) (nth 2 slow) (or (nth 3 slow) "")))))))))

(advice-add 'emrs--wait :around #'emrs-diag--wait)
(advice-add 'emrs--fetch :around #'emrs-diag--fetch)
(advice-add 'accept-process-output :around #'emrs-diag--accept)
(dolist (step '(emrs--dispatch emrs--dispatch-own emrs-cancel))
  (advice-add step :around
              (lambda (function &rest args)
                (emrs-diag--timed (if emrs-diag--in-accept
                                      (intern (format "%s-in-filter" step))
                                    step)
                                  function args))))

(message "DIAG %s, %s, %d cpus, read-process-output-max %S, adaptive %S"
         emacs-version system-configuration (num-processors)
         read-process-output-max process-adaptive-read-buffering)

(provide 'emrs-diag-wait)
;;; emrs-diag-wait.el ends here
