;;; emrs-tests.el --- End-to-end tests for emrs  -*- lexical-binding: t; -*-

;;; Commentary:

;; Run with tests/run-ert.sh, which builds tests/emrs-test-module and
;; puts it on `load-path' as emrs-test-module.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'emrs)

(defun emrs-test--wait (predicate &optional timeout)
  "Process output until PREDICATE returns non-nil or TIMEOUT seconds pass.
Return the last value of PREDICATE."
  (let ((deadline (+ (float-time) (or timeout 5.0))) value)
    (while (and (not (setq value (funcall predicate)))
                (< (float-time) deadline))
      (accept-process-output nil 0.01))
    value))

(defmacro emrs-test--with-instance (spec &rest body)
  "Run BODY with an instance bound per SPEC, then stop it.
SPEC is (VAR SERVICE ARGS &rest KEYS) as for `emrs-start'."
  (declare (indent 1))
  (let ((var (car spec)))
    `(let ((,var (emrs-start 'emrs-test ,@(cdr spec))))
       (unwind-protect (progn ,@body)
         (emrs-stop ,var)))))

(defun emrs-test--error (form-fn)
  "Call FORM-FN and return the (KIND MESSAGE DATA) of its `emrs-error'."
  (condition-case err
      (progn (funcall form-fn) nil)
    (emrs-error (cdr err))))

;;;; Loading

(ert-deftest emrs-test-module-info ()
  (let ((info (emrs-module-info 'emrs-test)))
    (should (equal (plist-get info :protocol) 1))
    (should (equal (plist-get info :module) "emrs-test"))
    (should (equal (plist-get info :services) '("types" "probe")))
    (should (stringp (plist-get info :emrs)))
    (should (string-match-p "-" (plist-get info :target)))))

(ert-deftest emrs-test-newer-protocol-is-rejected ()
  (defalias 'emrs-future--emrs-info (lambda () '(:protocol 99)))
  (provide 'emrs-future-module)
  (let ((err (emrs-test--error (lambda () (emrs-load-module 'emrs-future)))))
    (should (eq (car err) 'protocol))
    (should (string-match-p "upgrade the emrs Elisp package" (cadr err)))))

(ert-deftest emrs-test-unknown-service ()
  (let ((err (emrs-test--error (lambda () (emrs-start 'emrs-test "nope" nil)))))
    (should (eq (car err) 'protocol))
    (should (string-match-p "no service named \"nope\"" (cadr err)))))

(ert-deftest emrs-test-bad-start-args ()
  (let ((err (emrs-test--error (lambda () (emrs-start 'emrs-test "probe" 42)))))
    (should (eq (car err) 'decode))))

;;;; Data mapping

(ert-deftest emrs-test-scalars-round-trip ()
  (emrs-test--with-instance (inst "types" nil)
    (let ((value (list :flag t :small -5 :big 18446744073709551615
                       :huge -170141183460469231731687303715884105728
                       :ratio 1.5 :letter ?é :text "hé \"q\"" :nothing nil :maybe nil)))
      (should (equal (emrs-call-sync inst (list :scalars value) 5) value)))))

(ert-deftest emrs-test-shapes-round-trip ()
  (emrs-test--with-instance (inst "types" nil)
    (let ((table (make-hash-table :test 'equal)))
      (puthash "b" 2 table)
      (puthash "a" 1 table)
      (should (equal (emrs-call-sync
                      inst
                      `(:shapes (:list [1 2 3] :pair (7 "x") :map ,table
                                 :shapes (:dot (:circle 2) (:rect 1.0 2.5)
                                          (:label :text "t" :font-size 9))))
                      5)
                     '(:list (1 2 3) :pair (7 "x") :map (("a" . 1) ("b" . 2))
                       :shapes ((:dot) (:circle 2.0) (:rect 1.0 2.5)
                                (:label :text "t" :font-size 9))))))))

(ert-deftest emrs-test-bytes-round-trip ()
  (emrs-test--with-instance (inst "types" nil)
    (let* ((bytes (unibyte-string 0 127 128 200 255))
           (back (emrs-call-sync inst (list :bytes bytes) 5)))
      (should (equal back bytes))
      (should-not (multibyte-string-p back)))))

(ert-deftest emrs-test-decode-error-is-synchronous ()
  (emrs-test--with-instance (inst "types" nil)
    (let ((err (emrs-test--error (lambda () (emrs-send inst '(:nope))))))
      (should (eq (car err) 'decode))
      (should (string-match-p "unknown variant" (cadr err))))
    (let ((err (emrs-test--error
                (lambda () (emrs-call inst '(:shape (:label :text 5 :font-size 1)) #'ignore)))))
      (should (eq (car err) 'decode))
      (should (string-match-p ":text: expected string, got integer" (cadr err))))
    (should (= 0 (hash-table-count (emrs-instance-calls inst))))))

(ert-deftest emrs-test-circular-and-dotted-lists-are-rejected ()
  (emrs-test--with-instance (inst "types" nil)
    (let ((circular (list 1 2 3)))
      (nconc circular circular)
      (dolist (bad (list circular '(1 2 . 3)))
        (let ((err (emrs-test--error
                    (lambda ()
                      (emrs-send inst `(:shapes (:list ,bad :pair (1 "x") :map nil :shapes nil)))))))
          (should (eq (car err) 'decode))
          (should (string-match-p "dotted or circular" (cadr err))))))))

(ert-deftest emrs-test-unusual-strings-round-trip ()
  (emrs-test--with-instance (inst "probe" :ok)
    (dolist (text (list "" "a\0b" "naïve 日本語 🦀" (make-string 100000 ?x)))
      (should (equal (emrs-call-sync inst (list :echo text) 5) text)))))

;;;; Calls

(ert-deftest emrs-test-call-runs-callback-once ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((results nil))
      (emrs-call inst '(:echo "hi") (lambda (reply err) (push (list reply err) results)))
      (should (emrs-test--wait (lambda () results)))
      (accept-process-output nil 0.05)
      (should (equal results '(("hi" nil)))))))

(ert-deftest emrs-test-service-error ()
  (emrs-test--with-instance (inst "probe" :ok)
    (should (equal (emrs-test--error (lambda () (emrs-call-sync inst '(:fail) 5)))
                   '(service "nope" 7)))))

(ert-deftest emrs-test-no-reply ()
  (emrs-test--with-instance (inst "probe" :ok)
    (should (eq (car (emrs-test--error (lambda () (emrs-call-sync inst '(:forget) 5))))
                'no-reply))))

(ert-deftest emrs-test-call-sync-timeout ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((start (float-time)))
      (should (eq (car (emrs-test--error (lambda () (emrs-call-sync inst '(:slow 2000) 0.2))))
                  'timeout))
      (should (< (- (float-time) start) 1.0))
      (should (= 0 (hash-table-count (emrs-instance-calls inst)))))))

(ert-deftest emrs-test-cancel-call ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let* ((results nil)
           (call (emrs-call inst '(:slow 300) (lambda (reply err) (push (list reply err) results)))))
      (emrs-cancel call)
      (should (equal (plist-get (cadr (car results)) :kind) 'cancelled))
      (emrs-cancel call)
      (accept-process-output nil 0.5)
      (should (= 1 (length results))))))

(ert-deftest emrs-test-slow-instance-does-not-delay-another ()
  (emrs-test--with-instance (slow "probe" :ok)
    (emrs-test--with-instance (fast "probe" :ok)
      (emrs-send slow '(:slow 1500))
      (let ((start (float-time)))
        (should (equal (emrs-call-sync fast '(:echo "quick") 5) "quick"))
        (should (< (- (float-time) start) 0.5))))))

(ert-deftest emrs-test-encode-failure-of-reply ()
  (emrs-test--with-instance (inst "probe" :ok)
    (should (equal (seq-take (emrs-test--error (lambda () (emrs-call-sync inst '(:bad-reply) 5))) 2)
                   '(encode "cannot encode")))
    (should (equal (emrs-call-sync inst '(:echo "alive") 5) "alive"))))

(ert-deftest emrs-test-quit-during-call-sync-cancels-the-call ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((timer (run-at-time 0.1 nil (lambda () (signal 'quit nil))))
          (start (float-time)))
      (unwind-protect
          (should (eq (condition-case nil
                          (emrs-call-sync inst '(:slow 3000))
                        (quit 'quit))
                      'quit))
        (cancel-timer timer))
      (should (< (- (float-time) start) 1.0))
      (should (= 0 (hash-table-count (emrs-instance-calls inst)))))))

(ert-deftest emrs-test-quit-while-sending-leaves-no-call-record ()
  (emrs-test--with-instance (inst "types" nil)
    (let ((results nil)
          (quit-once (lambda (&rest _) (signal 'quit nil))))
      (advice-add 'proper-list-p :before quit-once)
      (unwind-protect
          (should (eq (condition-case nil
                          (emrs-call inst '(:shape (:circle 1.0))
                                     (lambda (reply err) (push (list reply err) results)))
                        (quit 'quit))
                      'quit))
        (advice-remove 'proper-list-p quit-once))
      (should (= 0 (hash-table-count (emrs-instance-calls inst))))
      (should-not results))))

(ert-deftest emrs-test-module-drain-runs-with-quit-inhibited ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let* ((seen nil)
           (record (lambda (&rest _) (push inhibit-quit seen))))
      (advice-add 'vector :before record)
      (unwind-protect
          (should (equal (emrs-call-sync inst '(:echo "x") 5) "x"))
        (advice-remove 'vector record))
      (should seen)
      (should (cl-every #'identity seen)))))

(ert-deftest emrs-test-panicking-serializer-is-an-encode-error ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((other nil))
      (emrs-call inst '(:echo "other") (lambda (reply _err) (setq other reply)))
      (should (eq (car (emrs-test--error (lambda () (emrs-call-sync inst '(:panic-reply) 5))))
                  'encode))
      (should (emrs-test--wait (lambda () other)))
      (should (equal other "other"))
      (should (equal (emrs-call-sync inst '(:echo "alive") 5) "alive")))))

;;;; Events

(ert-deftest emrs-test-quit-in-a-callback-keeps-later-deliveries ()
  (let ((events nil))
    (emrs-test--with-instance (inst "probe" :ok
                                    :on-event (lambda (_i event)
                                                (when (equal event '(:tick 0)) (signal 'quit nil))
                                                (push event events)))
      (let* ((m (emrs-instance-module inst))
             (pipe (emrs--module-pipe m))
             (filter (process-filter pipe)))
        ;; Keep the filter from draining, so the quit happens in a drain
        ;; made from ordinary code, as it does inside `emrs-call-sync'.
        (set-process-filter pipe #'ignore)
        (unwind-protect
            (progn
              (emrs-send inst '(:emit 3))
              (sleep-for 0.2)
              (should (eq (condition-case nil (progn (emrs--drain m) nil) (quit 'quit)) 'quit)))
          (set-process-filter pipe filter))
        (should-not (emrs--module-dispatching m))
        (should (emrs-test--wait (lambda () (= 2 (length events)))))
        (should (equal (reverse events) '((:tick 1) (:tick 2))))))))

(ert-deftest emrs-test-events-in-order ()
  (let ((events nil) (seen-instance nil))
    (emrs-test--with-instance (inst "probe" :ok
                                    :on-event (lambda (i event)
                                                (setq seen-instance i)
                                                (push event events)))
      (should (= (emrs-call-sync inst '(:emit 3) 5) 3))
      (should (equal (reverse events) '((:tick 0) (:tick 1) (:tick 2))))
      (should (eq seen-instance inst)))))

(ert-deftest emrs-test-callback-error-does-not-lose-deliveries ()
  (let ((events nil) (warning-minimum-log-level :emergency))
    (emrs-test--with-instance (inst "probe" :ok
                                    :on-event (lambda (_i event)
                                                (when (equal event '(:tick 0)) (error "Boom"))
                                                (push event events)))
      (should (= (emrs-call-sync inst '(:emit 3) 5) 3))
      (should (equal (reverse events) '((:tick 1) (:tick 2))))
      (should (with-current-buffer "*emrs-log*"
                (save-excursion (goto-char (point-min))
                                (search-forward "error in on-event" nil t)))))))

(ert-deftest emrs-test-unencodable-event-is-logged ()
  (emrs-test--with-instance (inst "probe" :ok)
    (emrs-call-sync inst '(:bad-event) 5)
    (should (with-current-buffer "*emrs-log*"
              (save-excursion (goto-char (point-min))
                              (search-forward "an event could not be encoded: cannot encode" nil t))))))

(ert-deftest emrs-test-nested-call-sync-inside-a-callback ()
  (let ((inner nil))
    (emrs-test--with-instance (inst "probe" :ok
                                    :on-event (lambda (i event)
                                                (when (equal event '(:tick 0))
                                                  (setq inner (emrs-call-sync i '(:echo "inner") 5)))))
      (emrs-call-sync inst '(:emit 1) 5)
      (should (emrs-test--wait (lambda () inner)))
      (should (equal inner "inner")))))

(ert-deftest emrs-test-event-flood-arrives-completely ()
  (let ((count 0) (emrs-drain-max 100))
    (emrs-test--with-instance (inst "probe" :ok :on-event (lambda (_i _e) (cl-incf count)))
      (should (= (emrs-call-sync inst '(:emit 50000) 30) 50000))
      (should (emrs-test--wait (lambda () (= count 50000)) 30)))))

;;;; Streams

(ert-deftest emrs-test-stream-delivers-everything-in-order ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((batches nil) (ends nil))
      (emrs-call-stream inst '(:count :n 5000 :capacity 8)
                        :on-batch (lambda (items) (push items batches))
                        :on-end (lambda (status err) (push (list status err) ends)))
      (should (emrs-test--wait (lambda () ends) 20))
      (should (equal ends '((done nil))))
      (should (cl-every #'consp batches))
      (should (equal (apply #'append (reverse batches)) (number-sequence 0 4999))))))

(ert-deftest emrs-test-wake-up-is-not-delayed-by-read-buffering ()
  ;; 800 items at capacity 8 need at least 100 wake-ups.  Emacs 30 delays
  ;; reading after short reads (`process-adaptive-read-buffering'), which
  ;; would cost 10 ms or more per wake-up here.
  (emrs-test--with-instance (inst "probe" :ok)
    (emrs-call-sync inst '(:echo "warm up") 5)
    (let ((count 0) (ends nil) (start (float-time)))
      (emrs-call-stream inst '(:count :n 800 :capacity 8)
                        :on-batch (lambda (items) (cl-incf count (length items)))
                        :on-end (lambda (status _err) (push status ends)))
      (should (emrs-test--wait (lambda () ends) 20))
      (should (= count 800))
      (should (< (- (float-time) start) 0.5)))))

(ert-deftest emrs-test-large-stream-keeps-emacs-responsive ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let* ((count 0) (ends nil) (ticks 0)
           (timer (run-at-time 0 0.01 (lambda () (cl-incf ticks)))))
      (unwind-protect
          (progn
            (emrs-call-stream inst '(:count :n 200000 :capacity 1024)
                              :on-batch (lambda (items) (cl-incf count (length items)))
                              :on-end (lambda (status _err) (push status ends)))
            (should (emrs-test--wait (lambda () ends) 120))
            (should (equal ends '(done)))
            (should (= count 200000))
            (should (> ticks 5)))
        (cancel-timer timer)))))

(ert-deftest emrs-test-cancel-stream ()
  (let ((events nil))
    (emrs-test--with-instance (inst "probe" :ok :on-event (lambda (_i e) (push e events)))
      (let* ((received 0) (ends nil)
             (call (emrs-call-stream inst '(:endless)
                                     :on-batch (lambda (items) (cl-incf received (length items)))
                                     :on-end (lambda (status err) (push (list status err) ends)))))
        (should (emrs-test--wait (lambda () (> received 100))))
        (emrs-cancel call)
        (should (equal ends '((cancelled nil))))
        (let ((at-cancel received))
          (should (emrs-test--wait (lambda () (member '(:producer-stopped) events))))
          (should (= received at-cancel)))
        (emrs-cancel call)
        (should (= 1 (length ends)))))))

(ert-deftest emrs-test-cancel-stream-from-inside-on-batch ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let* ((batches 0) (ends nil) call)
      (setq call (emrs-call-stream inst '(:endless)
                                   :on-batch (lambda (_items)
                                               (cl-incf batches)
                                               (emrs-cancel call))
                                   :on-end (lambda (status _err) (push status ends))))
      (should (emrs-test--wait (lambda () ends)))
      (accept-process-output nil 0.1)
      (should (= batches 1))
      (should (equal ends '(cancelled))))))

(ert-deftest emrs-test-sync-call-inside-on-batch-keeps-stream-order ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((a nil) (b nil) (ends nil))
      (emrs-call-stream inst '(:count :n 3000 :capacity 8)
                        :on-batch (lambda (items)
                                    (setq a (append a items))
                                    (emrs-call-sync inst '(:echo "nested") 5))
                        :on-end (lambda (status _err) (push status ends)))
      (emrs-call-stream inst '(:count :n 3000 :capacity 8)
                        :on-batch (lambda (items) (setq b (append b items)))
                        :on-end (lambda (status _err) (push status ends)))
      (should (emrs-test--wait (lambda () (= 2 (length ends))) 60))
      (should (equal ends '(done done)))
      (should (equal a (number-sequence 0 2999)))
      (should (equal b (number-sequence 0 2999))))))

(ert-deftest emrs-test-sync-call-in-every-batch-does-not-recurse-without-bound ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((count 0) (ends nil) (depth 0) (max-depth 0))
      (emrs-call-stream inst '(:count :n 4000 :capacity 4)
                        :on-batch (lambda (items)
                                    (cl-incf count (length items))
                                    (cl-incf depth)
                                    (setq max-depth (max max-depth depth))
                                    (unwind-protect (emrs-call-sync inst '(:echo "nested") 5)
                                      (cl-decf depth)))
                        :on-end (lambda (status _err) (push status ends)))
      (should (emrs-test--wait (lambda () ends) 120))
      (should (equal ends '(done)))
      (should (= count 4000))
      (should (= max-depth 1)))))

(ert-deftest emrs-test-finished-stream-survives-stop ()
  (let ((items nil) (ends nil)
        (inst (emrs-start 'emrs-test "probe" :ok)))
    (emrs-call-stream inst '(:count :n 10 :capacity 64)
                      :on-batch (lambda (batch) (setq items (append items batch)))
                      :on-end (lambda (status err) (push (list status err) ends)))
    (sleep-for 0.3)
    (emrs-stop inst)
    (should (emrs-test--wait (lambda () ends)))
    (should (equal ends '((done nil))))
    (should (equal items (number-sequence 0 9)))))

(ert-deftest emrs-test-plain-reply-to-stream-call ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((batches nil) (ends nil))
      (emrs-call-stream inst '(:echo "one")
                        :on-batch (lambda (items) (push items batches))
                        :on-end (lambda (status err) (push (list status err) ends)))
      (should (emrs-test--wait (lambda () ends)))
      (should (equal batches '(("one"))))
      (should (equal ends '((done nil)))))))

(ert-deftest emrs-test-stream-answer-to-plain-call ()
  (emrs-test--with-instance (inst "probe" :ok)
    (should (eq (car (emrs-test--error
                      (lambda () (emrs-call-sync inst '(:count :n 3 :capacity 4) 5))))
                'protocol))))

(ert-deftest emrs-test-unencodable-item-fails-the-stream ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((batches nil) (ends nil))
      (emrs-call-stream inst '(:bad-item)
                        :on-batch (lambda (items) (push items batches))
                        :on-end (lambda (status err) (push (list status (plist-get err :kind)) ends)))
      (should (emrs-test--wait (lambda () ends)))
      (accept-process-output nil 0.05)
      (should (equal batches '((1))))
      (should (equal ends '((failed encode)))))))

;;;; Synchronous streams

(ert-deftest emrs-test-stream-sync-returns-all-items-in-order ()
  (emrs-test--with-instance (inst "probe" :ok)
    (should (equal (emrs-call-stream-sync inst '(:count :n 5000 :capacity 8) 20)
                   (number-sequence 0 4999)))
    (should (equal (emrs-call-stream-sync inst '(:count :n 0 :capacity 8) 5) nil))
    (should (equal (emrs-call-stream-sync inst '(:echo "one") 5) '("one")))
    (should (= 0 (hash-table-count (emrs-instance-calls inst))))))

(ert-deftest emrs-test-stream-sync-signals-a-failed-stream ()
  (emrs-test--with-instance (inst "probe" :ok)
    (should (eq (car (emrs-test--error (lambda () (emrs-call-stream-sync inst '(:bad-item) 5))))
                'encode))
    (should (equal (seq-take (emrs-test--error (lambda () (emrs-call-stream-sync inst '(:fail) 5))) 2)
                   '(service "nope")))
    (should (equal (emrs-call-stream-sync inst '(:count :n 3 :capacity 8) 5) '(0 1 2)))))

(ert-deftest emrs-test-stream-sync-timeout-cancels-the-stream ()
  (let ((events nil))
    (emrs-test--with-instance (inst "probe" :ok :on-event (lambda (_i e) (push e events)))
      (let ((start (float-time)))
        (should (eq (car (emrs-test--error (lambda () (emrs-call-stream-sync inst '(:endless) 0.2))))
                    'timeout))
        (should (< (- (float-time) start) 1.0)))
      (should (= 0 (hash-table-count (emrs-instance-calls inst))))
      (should (emrs-test--wait (lambda () (member '(:producer-stopped) events)))))))

(ert-deftest emrs-test-stream-sync-quit-cancels-the-stream ()
  (let ((events nil))
    (emrs-test--with-instance (inst "probe" :ok :on-event (lambda (_i e) (push e events)))
      (let ((timer (run-at-time 0.1 nil (lambda () (signal 'quit nil)))))
        (unwind-protect
            (should (eq (condition-case nil
                            (emrs-call-stream-sync inst '(:endless))
                          (quit 'quit))
                        'quit))
          (cancel-timer timer)))
      (should (= 0 (hash-table-count (emrs-instance-calls inst))))
      (should (emrs-test--wait (lambda () (member '(:producer-stopped) events)))))))

(ert-deftest emrs-test-stream-sync-inside-a-callback ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((result nil) (order nil))
      (emrs-call inst '(:echo "outer")
                 (lambda (_reply _err)
                   (emrs-call inst '(:echo "later") (lambda (&rest _) (push 'later order)))
                   (setq result (emrs-call-stream-sync inst '(:count :n 3000 :capacity 8) 20))
                   (push 'stream order)))
      (should (emrs-test--wait (lambda () (memq 'later order)) 20))
      (should (equal result (number-sequence 0 2999)))
      (should (equal order '(later stream))))))

(ert-deftest emrs-test-stream-sync-instance-stops-while-waiting ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((timer (run-at-time 0.1 nil #'emrs-stop inst)))
      (unwind-protect
          (should (eq (car (emrs-test--error (lambda () (emrs-call-stream-sync inst '(:endless) 5))))
                      'stopped))
        (cancel-timer timer)))))

;;;; Lifecycle

(ert-deftest emrs-test-stop ()
  (let* ((events nil) (exits nil)
         (inst (emrs-start 'emrs-test "probe" :ok
                           :on-event (lambda (_i e) (push e events))
                           :on-exit (lambda (i status err) (push (list i status err) exits)))))
    (should (eq (emrs-instance-state inst) 'starting))
    (should (emrs-test--wait (lambda () (eq (emrs-instance-state inst) 'running))))
    (emrs-stop inst)
    (should (eq (emrs-instance-state inst) 'stopping))
    (should (emrs-test--wait (lambda () exits)))
    (should (equal exits (list (list inst 'stopped nil))))
    (should (equal events '((:stop-hook-ran))))
    (should (eq (emrs-instance-state inst) 'stopped))
    (should (eq (car (emrs-test--error (lambda () (emrs-send inst '(:echo "x"))))) 'stopped))
    (emrs-stop inst)
    (accept-process-output nil 0.05)
    (should (= 1 (length exits)))))

(ert-deftest emrs-test-stop-fails-queued-calls ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((results nil))
      (emrs-send inst '(:slow 300))
      (emrs-call inst '(:echo "queued") (lambda (reply err) (push (list reply (plist-get err :kind)) results)))
      (emrs-stop inst)
      (should (emrs-test--wait (lambda () results)))
      (should (equal results '((nil stopped)))))))

(ert-deftest emrs-test-handler-panic-fails-only-that-instance ()
  (emrs-test--with-instance (other "probe" :ok)
    (let* ((exits nil) (results nil)
           (inst (emrs-start 'emrs-test "probe" :ok
                             :on-exit (lambda (_i status err)
                                        (push (list status (plist-get err :message)) exits)))))
      (emrs-send inst '(:slow 100))
      (emrs-call inst '(:panic) (lambda (_r err) (push (plist-get err :kind) results)))
      (emrs-call inst '(:echo "after") (lambda (_r err) (push (plist-get err :kind) results)))
      (should (emrs-test--wait (lambda () exits)))
      (should (equal exits '((failed "panic in handle: handler exploded"))))
      (should (equal (reverse results) '(no-reply failed)))
      (should (eq (emrs-instance-state inst) 'failed))
      (should (equal (emrs-call-sync other '(:echo "alive") 5) "alive")))))

(ert-deftest emrs-test-start-failure ()
  (dolist (case '((:fail . "cannot start") (:panic . "panic in start: start exploded")))
    (let* ((exits nil)
           (inst (emrs-start 'emrs-test "probe" (car case)
                             :on-exit (lambda (_i status err)
                                        (push (list status (plist-get err :message)) exits)))))
      (should (emrs-test--wait (lambda () exits)))
      (should (equal exits (list (list 'failed (cdr case)))))
      (should (eq (emrs-instance-state inst) 'failed)))))

(ert-deftest emrs-test-pipe-loss-is-repaired ()
  (emrs-test--with-instance (inst "probe" :ok)
    (let ((old (emrs--module-pipe (emrs-instance-module inst))))
      (delete-process old)
      (should (emrs-test--wait
               (lambda () (not (eq old (emrs--module-pipe (emrs-instance-module inst)))))))
      (should (equal (emrs-call-sync inst '(:echo "back") 5) "back")))))

(ert-deftest emrs-test-zy-shutdown-does-not-wait-for-a-blocked-handler ()
  (let ((inst (emrs-start 'emrs-test "probe" :ok))
        (start (float-time)))
    (emrs-send inst '(:slow 3000))
    (accept-process-output nil 0.1)
    (should-not (funcall 'emrs-test--emrs-shutdown 0.2))
    (should (< (- (float-time) start) 1.0))
    (should (emrs-test--wait (lambda () (eq (emrs-instance-state inst) 'stopped)) 10))))

(ert-deftest emrs-test-zz-shutdown-stops-everything ()
  (let* ((exits nil)
         (inst (emrs-start 'emrs-test "probe" :ok
                           :on-exit (lambda (_i status _err) (push status exits)))))
    (should (emrs-test--wait (lambda () (eq (emrs-instance-state inst) 'running))))
    (should (eq (funcall 'emrs-test--emrs-shutdown 5.0) t))
    (should (emrs-test--wait (lambda () exits)))
    (should (equal exits '(stopped)))))

(provide 'emrs-tests)
;;; emrs-tests.el ends here
