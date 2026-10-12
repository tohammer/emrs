;;; emrs.el --- Message-passing services in Rust dynamic modules  -*- lexical-binding: t; -*-

;; Copyright (C) 2026  The emrs authors

;; Version: 0.1.0
;; Package-Requires: ((emacs "30.1"))
;; Keywords: extensions, processes
;; SPDX-License-Identifier: GPL-3.0-or-later

;; This program is free software; you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;;; Commentary:

;; Companion library for Emacs modules built with the Rust crate `emrs'.
;; A module hosts services; this library starts instances of them, sends
;; requests, and dispatches replies, stream items and events to callbacks.
;;
;;   (setq inst (emrs-start 'echo-rs "echo" nil))
;;   (emrs-call-sync inst '(:echo "hi") 1.0)   ; => "hi"
;;   (emrs-stop inst)

;;; Code:

(require 'cl-lib)

(defgroup emrs nil
  "Message-passing services in Rust dynamic modules."
  :group 'extensions
  :prefix "emrs-")

(defcustom emrs-drain-max 512
  "Maximum number of deliveries converted to Lisp in one drain.
Emacs returns to its command loop between drains, so a lower value
keeps Emacs more responsive under load at some cost in throughput."
  :type 'natnum)

(defcustom emrs-shutdown-timeout 0.5
  "Seconds to wait at Emacs exit for services to run their stop hooks."
  :type 'number)

(defconst emrs-protocol-max 1
  "Highest module protocol version this library supports.")

(define-error 'emrs-error "emrs error")

(cl-defstruct (emrs--module (:constructor emrs--make-module) (:copier nil))
  name info pipe file mtime
  (instances (make-hash-table :test 'eql))
  ;; Deliveries fetched from the module and not yet dispatched, oldest first.
  pending
  ;; Non-nil while the dispatch loop of this module is running.
  dispatching)

(cl-defstruct (emrs-instance (:constructor emrs--make-instance) (:copier nil))
  "A running service instance.  Create with `emrs-start'."
  module id (state 'starting) on-event on-exit
  (calls (make-hash-table :test 'eql))
  (next-call 0))

(cl-defstruct (emrs-call (:constructor emrs--make-call) (:copier nil))
  "An open call or stream.  Pass it to `emrs-cancel'."
  instance id mode callback on-batch on-end done)

(defvar emrs--modules (make-hash-table :test 'eq)
  "Loaded modules by name symbol.")

;;;; Helpers

(defun emrs--fn (module op)
  "Return the protocol function OP of MODULE (a symbol)."
  (intern (format "%s--emrs-%s" module op)))

(defun emrs--hash-to-alist (table)
  "Return the entries of hash TABLE as an alist.  Called by modules."
  (let (alist)
    (maphash (lambda (k v) (push (cons k v) alist)) table)
    (nreverse alist)))

(defun emrs--err (kind message)
  "Return an error plist of KIND with MESSAGE."
  (list :kind kind :message message :data nil))

(defun emrs--signal (err)
  "Signal the error plist ERR as `emrs-error'."
  (signal 'emrs-error
          (list (plist-get err :kind) (plist-get err :message) (plist-get err :data))))

(defun emrs--log (format-string &rest args)
  "Append a line built from FORMAT-STRING and ARGS to *emrs-log*."
  (with-current-buffer (get-buffer-create "*emrs-log*")
    (goto-char (point-max))
    (insert (format-time-string "%T ") (apply #'format format-string args) "\n")))

(defun emrs--protect (what function &rest args)
  "Call FUNCTION with ARGS; log and warn instead of signalling.
WHAT names the callback for the message."
  (condition-case err
      (apply function args)
    (error
     (emrs--log "error in %s: %S" what err)
     (display-warning 'emrs (format "Error in %s: %S" what err) :error))))

(defun emrs--mtime (file)
  "Return the modification time of FILE, or nil."
  (and file (file-attribute-modification-time (file-attributes file))))

;;;; Loading

(defun emrs--attach (m)
  "Create the wake-up pipe of module M and hand it to the module."
  (let* ((name (emrs--module-name m))
         ;; Each wake-up is a single byte.  With adaptive read buffering
         ;; (the default before Emacs 31) Emacs delays reading after such
         ;; short reads by up to 50 ms, which would throttle every stream.
         ;; The setting is captured when the process is created.
         (process-adaptive-read-buffering nil)
         (pipe (make-pipe-process
                :name (format " emrs-%s" name)
                :buffer nil
                :noquery t
                :coding 'binary
                :filter (lambda (_process _string) (emrs--drain m))
                :sentinel (lambda (process _event)
                            (when (and (not (process-live-p process))
                                       (eq process (emrs--module-pipe m)))
                              (emrs--attach m)
                              (emrs--drain m))))))
    (setf (emrs--module-pipe m) pipe)
    (funcall (emrs--fn name 'attach) pipe)))

(defun emrs--warn-if-stale (m)
  "Warn once if the file of module M changed after it was loaded."
  (let ((file (emrs--module-file m)))
    (when (and file (emrs--module-mtime m)
               (time-less-p (emrs--module-mtime m) (or (emrs--mtime file) 0)))
      (setf (emrs--module-mtime m) nil)
      (display-warning
       'emrs
       (format "%s changed on disk; Emacs cannot unload modules, restart to use the new build"
               file)))))

(defun emrs-load-module (module &optional file)
  "Load the emrs module MODULE (a symbol) and return its handle.
With FILE, load that file; otherwise `require' the feature
MODULE-module, so the file MODULE-module plus `module-file-suffix'
must be on `load-path'.  Loading again is a no-op."
  (let ((loaded (gethash module emrs--modules)))
    (if loaded
        (progn (emrs--warn-if-stale loaded) loaded)
      (let* ((feature (intern (format "%s-module" module)))
             (path (if file (expand-file-name file) (locate-library (symbol-name feature)))))
        (if file (module-load path) (require feature))
        (let* ((info (funcall (emrs--fn module 'info)))
               (protocol (plist-get info :protocol)))
          (unless (and (integerp protocol) (<= 1 protocol emrs-protocol-max))
            (emrs--signal
             (emrs--err 'protocol
                        (format "Module %s uses emrs protocol %s but this emrs.el supports 1 to %d; upgrade the emrs Elisp package"
                                module protocol emrs-protocol-max))))
          (let ((m (emrs--make-module :name module :info info :file path
                                      :mtime (emrs--mtime path))))
            (emrs--attach m)
            (puthash module m emrs--modules)
            m))))))

(defun emrs-module-info (module)
  "Return the info plist of MODULE (a symbol), loading it if needed.
Keys: :protocol :emrs :module :version :services :target."
  (emrs--module-info (emrs-load-module module)))

;;;; Dispatch

(defun emrs--complete (call value err)
  "Finish CALL exactly once with VALUE and ERR."
  (unless (emrs-call-done call)
    (setf (emrs-call-done call) t)
    (remhash (emrs-call-id call) (emrs-instance-calls (emrs-call-instance call)))
    (pcase (emrs-call-mode call)
      ('call (when-let* ((f (emrs-call-callback call)))
               (emrs--protect "call callback" f value err)))
      ('stream (when-let* ((f (emrs-call-on-end call)))
                 (emrs--protect "on-end" f value err))))))

(defun emrs--fail-call (call err)
  "Finish CALL with the error plist ERR."
  (if (eq (emrs-call-mode call) 'stream)
      (emrs--complete call 'failed err)
    (emrs--complete call nil err)))

(defun emrs--set-state (inst state err)
  "Record that INST entered STATE with error ERR."
  (unless (and (eq state 'running) (not (eq (emrs-instance-state inst) 'starting)))
    (setf (emrs-instance-state inst) state))
  (when (memq state '(stopped failed))
    (let (open)
      (maphash (lambda (_id call) (push call open)) (emrs-instance-calls inst))
      (dolist (call open)
        (emrs--fail-call call (emrs--err state "The instance terminated"))))
    (remhash (emrs-instance-id inst) (emrs--module-instances (emrs-instance-module inst)))
    (when-let* ((f (emrs-instance-on-exit inst)))
      (emrs--protect "on-exit" f inst state err))))

(defun emrs--dispatch (m delivery)
  "Hand DELIVERY, a vector [KIND INSTANCE CALL PAYLOAD] from M, to its callback."
  (let* ((kind (aref delivery 0))
         (payload (aref delivery 3))
         (inst (and (aref delivery 1)
                    (gethash (aref delivery 1) (emrs--module-instances m))))
         (call (and inst (aref delivery 2)
                    (gethash (aref delivery 2) (emrs-instance-calls inst)))))
    (cond
     ((eq kind 'log)
      (emrs--log "%s [%s] %s" (emrs--module-name m) (car payload) (cdr payload)))
     ((null inst) nil)
     ((eq kind 'event)
      (when-let* ((f (emrs-instance-on-event inst)))
        (emrs--protect "on-event" f inst payload)))
     ((eq kind 'state) (emrs--set-state inst (car payload) (cdr payload)))
     ((null call) nil)
     ((eq kind 'reply) (emrs--complete call payload nil))
     ((eq kind 'error) (emrs--fail-call call payload))
     ((eq kind 'items)
      (when-let* ((f (emrs-call-on-batch call)))
        (emrs--protect "on-batch" f payload)))
     ((eq kind 'end) (emrs--complete call (car payload) (cdr payload))))))

(defun emrs--fetch (m)
  "Move deliveries from module M into its Lisp queue.
Quitting is inhibited: the module has already removed these deliveries
from its own queue, so a quit during the conversion would lose them."
  (let ((new (let ((inhibit-quit t))
               (funcall (emrs--fn (emrs--module-name m) 'drain) emrs-drain-max))))
    (setf (emrs--module-pending m) (nconc (emrs--module-pending m) new))))

(defun emrs--drain (m)
  "Fetch the deliveries of module M and dispatch them in order.
When called from inside a callback, only fetch.  The dispatch loop that
is already running continues with the queue, which keeps deliveries in
order and bounds the nesting depth."
  (emrs--fetch m)
  (unless (emrs--module-dispatching m)
    (setf (emrs--module-dispatching m) t)
    (unwind-protect
        (while (emrs--module-pending m)
          (emrs--dispatch m (pop (emrs--module-pending m))))
      (setf (emrs--module-dispatching m) nil)
      ;; A non-local exit from a callback leaves deliveries queued.
      (when (emrs--module-pending m)
        (run-at-time 0 nil #'emrs--drain m)))))

(defun emrs--dispatch-own (m call)
  "Dispatch the queued deliveries of CALL in module M ahead of their turn.
Used by the synchronous calls when they run inside a callback, where the
dispatch loop is suspended."
  (let ((instance-id (emrs-instance-id (emrs-call-instance call)))
        (call-id (emrs-call-id call))
        (own nil) (rest nil))
    (dolist (delivery (emrs--module-pending m))
      (if (and (memq (aref delivery 0) '(reply error items end))
               (eql (aref delivery 1) instance-id)
               (eql (aref delivery 2) call-id))
          (push delivery own)
        (push delivery rest)))
    (when own
      (setf (emrs--module-pending m) (nreverse rest))
      (dolist (delivery (nreverse own))
        (emrs--dispatch m delivery)))))

(defun emrs--wait (call timeout finished-p message)
  "Wait until FINISHED-P returns non-nil, which the callbacks of CALL cause.
With TIMEOUT (seconds), signal `emrs-error' of kind `timeout' with
MESSAGE after that long.  On that and on any other non-local exit, such
as a quit, remove the callbacks of CALL and cancel it."
  (let ((m (emrs-instance-module (emrs-call-instance call)))
        (deadline (and timeout (+ (float-time) timeout)))
        (returned nil))
    (unwind-protect
        (progn
          (while (not (funcall finished-p))
            (emrs--drain m)
            (when (and (not (funcall finished-p)) (emrs--module-dispatching m))
              (emrs--dispatch-own m call))
            (unless (funcall finished-p)
              (let ((remaining (and deadline (- deadline (float-time)))))
                (when (and remaining (<= remaining 0))
                  (emrs--signal (emrs--err 'timeout message)))
                (accept-process-output (emrs--module-pipe m)
                                       (if remaining (min remaining 0.1) 0.1)))))
          (setq returned t))
      (unless returned
        (setf (emrs-call-callback call) nil
              (emrs-call-on-batch call) nil
              (emrs-call-on-end call) nil)
        (emrs-cancel call)))))

;;;; Public API

(cl-defun emrs-start (module service args &key on-event on-exit)
  "Start an instance of SERVICE (a string) in MODULE (a symbol).
ARGS is decoded to the service's argument type.  Return the instance
at once, in state `starting'; requests sent meanwhile are queued.

ON-EVENT is called as (ON-EVENT INSTANCE EVENT) for each event.
ON-EXIT is called once as (ON-EXIT INSTANCE STATUS ERR) where STATUS
is `stopped' or `failed'."
  (let* ((m (emrs-load-module module))
         (id (funcall (emrs--fn module 'start) service args))
         (inst (emrs--make-instance :module m :id id :on-event on-event :on-exit on-exit)))
    (puthash id inst (emrs--module-instances m))
    inst))

(defun emrs--request (inst request mode &rest slots)
  "Send REQUEST to INST in MODE; SLOTS initialise the call object.
Return the call object, or nil when MODE is nil."
  (unless (memq (emrs-instance-state inst) '(starting running))
    (emrs--signal (emrs--err 'stopped "The instance is not running")))
  (let* ((send (emrs--fn (emrs--module-name (emrs-instance-module inst)) 'send))
         (id (emrs-instance-id inst)))
    (if (null mode)
        (progn (funcall send id request nil nil) nil)
      (let* ((call-id (cl-incf (emrs-instance-next-call inst)))
             (call (apply #'emrs--make-call :instance inst :id call-id :mode mode slots)))
        (puthash call-id call (emrs-instance-calls inst))
        (let ((sent nil))
          (unwind-protect
              (progn (funcall send id request call-id mode)
                     (setq sent t))
            ;; Reached with SENT nil on any non-local exit, including a quit.
            (unless sent
              (setf (emrs-call-done call) t)
              (remhash call-id (emrs-instance-calls inst)))))
        call))))

(defun emrs-send (instance request)
  "Send REQUEST to INSTANCE without expecting an answer."
  (emrs--request instance request nil))

(defun emrs-call (instance request callback)
  "Send REQUEST to INSTANCE and return a call object.
CALLBACK is called exactly once as (CALLBACK REPLY ERR); ERR is nil
on success and an error plist (:kind :message :data) otherwise."
  (emrs--request instance request 'call :callback callback))

(cl-defun emrs-call-stream (instance request &key on-batch on-end)
  "Send REQUEST to INSTANCE, expecting a stream; return a call object.
ON-BATCH is called as (ON-BATCH ITEMS) with a non-empty list, in order.
ON-END is called exactly once as (ON-END STATUS ERR) where STATUS is
`done', `failed' or `cancelled'; no batch follows it."
  (emrs--request instance request 'stream :on-batch on-batch :on-end on-end))

(defun emrs-cancel (call)
  "Cancel CALL, an object returned by `emrs-call' or `emrs-call-stream'.
The call completes at once: a call's callback gets an error of kind
`cancelled', a stream's ON-END gets status `cancelled'.  Does nothing
if CALL already completed."
  (unless (emrs-call-done call)
    (let ((inst (emrs-call-instance call)))
      (funcall (emrs--fn (emrs--module-name (emrs-instance-module inst)) 'cancel)
               (emrs-instance-id inst) (emrs-call-id call)))
    (if (eq (emrs-call-mode call) 'stream)
        (emrs--complete call 'cancelled nil)
      (emrs--complete call nil (emrs--err 'cancelled "The call was cancelled")))))

(defun emrs-call-sync (instance request &optional timeout)
  "Send REQUEST to INSTANCE, wait for the reply and return it.
Signal `emrs-error' with data (KIND MESSAGE DATA) on failure.  With
TIMEOUT (seconds), give up after that long with kind `timeout';
otherwise wait until the reply arrives.  \\[keyboard-quit] cancels the call.

While waiting, other deliveries of the same module are dispatched, so
event handlers and other callbacks may run.  The exception is a call
made from inside an emrs callback: there, only this call's reply is
dispatched and everything else waits, in order, until that callback
returns."
  (let* ((finished nil) (reply nil) (failure nil)
         (call (emrs-call instance request
                          (lambda (value err)
                            (setq reply value failure err finished t)))))
    (emrs--wait call timeout (lambda () finished) "No reply within the timeout")
    (when failure (emrs--signal failure))
    reply))

(defun emrs-call-stream-sync (instance request &optional timeout)
  "Send REQUEST to INSTANCE, wait for its stream to end and return the items.
The items of all batches are returned as one list, in order.  Signal
`emrs-error' with data (KIND MESSAGE DATA) when the stream fails or is
cancelled by the service; the items received until then are dropped.
With TIMEOUT (seconds), give up after that long with kind `timeout';
otherwise wait until the stream ends.  \\[keyboard-quit] cancels the call.

The wait is on the module only, so the filters of other processes do
not run.  Deliveries of the same module are dispatched as described for
`emrs-call-sync', including the exception inside an emrs callback."
  (let* ((batches nil) (status nil) (failure nil)
         (call (emrs-call-stream instance request
                                 :on-batch (lambda (items) (push items batches))
                                 :on-end (lambda (end err)
                                           (setq status end failure err)))))
    (emrs--wait call timeout (lambda () status) "The stream did not end within the timeout")
    (pcase status
      ('done (let ((items nil))
               ;; BATCHES is newest first.
               (dolist (batch batches)
                 (setq items (nconc batch items)))
               items))
      ('cancelled (emrs--signal (emrs--err 'cancelled "The stream was cancelled")))
      (_ (emrs--signal (or failure (emrs--err 'failed "The stream failed")))))))

(defun emrs-stop (instance)
  "Stop INSTANCE.  Return at once; ON-EXIT runs when it has stopped."
  (when (memq (emrs-instance-state instance) '(starting running))
    (setf (emrs-instance-state instance) 'stopping)
    (funcall (emrs--fn (emrs--module-name (emrs-instance-module instance)) 'stop)
             (emrs-instance-id instance)))
  nil)

;;;; Emacs exit

(defun emrs--shutdown ()
  "Stop all instances of all modules, waiting `emrs-shutdown-timeout'."
  (maphash (lambda (name _m)
             (ignore-errors
               (funcall (emrs--fn name 'shutdown) emrs-shutdown-timeout)))
           emrs--modules))

(add-hook 'kill-emacs-hook #'emrs--shutdown)

(provide 'emrs)
;;; emrs.el ends here
