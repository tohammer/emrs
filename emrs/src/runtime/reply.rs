//! Answering calls: `Reply` and `StreamSender`.

use super::cancel::CancelToken;
use super::core::{Delivery, Level, StreamEnd};
use super::instance::{Mode, Shared};
use crate::error::{Error, ErrorKind};
use crate::payload::Payload;
use serde::Serialize;
use std::marker::PhantomData;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex};

/// Counts the items of one stream that are queued but not yet drained.
pub(crate) struct StreamState {
    capacity: usize,
    in_flight: Mutex<usize>,
    cv: Condvar,
}

impl StreamState {
    fn new(capacity: usize, token: &CancelToken) -> Arc<StreamState> {
        let state = Arc::new(StreamState { capacity, in_flight: Mutex::new(0), cv: Condvar::new() });
        let waker = state.clone();
        token.on_cancel(move || {
            let _guard = waker.in_flight.lock().unwrap();
            waker.cv.notify_all();
        });
        state
    }

    /// Reserve a slot, blocking while the stream is full.
    fn acquire(&self, token: &CancelToken) -> Result<(), StreamClosed> {
        let mut in_flight = self.in_flight.lock().unwrap();
        loop {
            if token.is_cancelled() {
                return Err(StreamClosed);
            }
            if *in_flight < self.capacity {
                *in_flight += 1;
                return Ok(());
            }
            in_flight = self.cv.wait(in_flight).unwrap();
        }
    }

    fn try_acquire(&self) -> bool {
        let mut in_flight = self.in_flight.lock().unwrap();
        if *in_flight < self.capacity {
            *in_flight += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn release(&self) {
        let mut in_flight = self.in_flight.lock().unwrap();
        *in_flight = in_flight.saturating_sub(1);
        self.cv.notify_one();
    }
}

/// The stream's call was cancelled or its instance stopped.
#[derive(Debug, PartialEq, Eq)]
pub struct StreamClosed;

/// Why [`StreamSender::try_send`] did not queue the item.
#[derive(Debug, PartialEq, Eq)]
pub enum TrySendError<T> {
    /// The stream is at capacity; try again later.
    Full(T),
    /// The stream's call was cancelled or its instance stopped.
    Closed(T),
}

/// The handle a handler uses to answer one request. Consumed by answering;
/// dropping it unanswered reports `no-reply` to the caller.
pub struct Reply {
    shared: Arc<Shared>,
    call: Option<u64>,
    mode: Mode,
    token: CancelToken,
    discard: Arc<AtomicBool>,
    done: bool,
}

impl Reply {
    pub(crate) fn new(
        shared: Arc<Shared>,
        call: Option<u64>,
        mode: Mode,
        token: CancelToken,
        discard: Arc<AtomicBool>,
    ) -> Reply {
        Reply { shared, call, mode, token, discard, done: false }
    }

    /// Answer with a value.
    pub fn ok(mut self, value: impl Serialize + Send + 'static) {
        self.finish(Ok(Box::new(value)));
    }

    /// Answer with an error; the caller sees kind `service`.
    pub fn err(mut self, error: impl Into<Error>) {
        self.finish(Err(error.into()));
    }

    /// Answer with a stream of `T`. At most `capacity` items (minimum 1) wait
    /// for Emacs at any time; `send` blocks beyond that.
    pub fn stream<T: Serialize + Send + 'static>(mut self, capacity: usize) -> StreamSender<T> {
        self.done = true;
        let live = match (self.mode, self.call) {
            (Mode::Stream, Some(_)) => true,
            (Mode::Call, Some(call)) => {
                let instance = self.shared.id;
                self.shared.complete(call, || Delivery::Error {
                    instance,
                    call,
                    error: Error::with_kind(
                        ErrorKind::Protocol,
                        "the handler answered with a stream; use emrs-call-stream",
                    ),
                });
                false
            }
            _ => false,
        };
        if !live {
            self.token.cancel();
        }
        StreamSender {
            shared: self.shared.clone(),
            call: self.call.unwrap_or(0),
            state: StreamState::new(capacity.max(1), &self.token),
            token: self.token.clone(),
            discard: self.discard.clone(),
            done: !live,
            _item: PhantomData,
        }
    }

    /// Cancelled when Elisp cancels this call or the instance stops.
    pub fn token(&self) -> CancelToken {
        self.token.clone()
    }

    /// False if nobody waits for the answer (`emrs-send` or a self-message).
    pub fn is_wanted(&self) -> bool {
        self.call.is_some()
    }

    pub(crate) fn fail(mut self, kind: ErrorKind, message: &str) {
        self.finish(Err(Error::with_kind(kind, message)));
    }

    fn finish(&mut self, outcome: Result<Box<dyn Payload>, Error>) {
        self.done = true;
        let instance = self.shared.id;
        let Some(call) = self.call else {
            if let Err(error) = outcome {
                self.shared.core.log(Level::Warn, format!("instance {instance}: unanswered request failed: {error}"));
            }
            return;
        };
        match (self.mode, outcome) {
            (Mode::Stream, Ok(payload)) => {
                let token = self.token.clone();
                let discard = self.discard.clone();
                self.shared.complete_with(call, || {
                    vec![
                        Delivery::Item { instance, call, payload, stream: None, discard, token },
                        Delivery::End { instance, call, status: StreamEnd::Done, error: None },
                    ]
                });
            }
            (Mode::Stream, Err(error)) => self.shared.complete(call, || Delivery::End {
                instance,
                call,
                status: StreamEnd::Failed,
                error: Some(error),
            }),
            (_, Ok(payload)) => self.shared.complete(call, || Delivery::Reply { instance, call, payload }),
            (_, Err(error)) => self.shared.complete(call, || Delivery::Error { instance, call, error }),
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        // After a cancellation the call is completed by whoever cancelled it:
        // Elisp for a cancelled call, the instance's termination for a stop.
        if !self.done && self.call.is_some() && !self.token.is_cancelled() {
            self.finish(Err(Error::with_kind(ErrorKind::NoReply, "the handler did not answer")));
        }
    }
}

/// Sends the items of a stream. Dropping it without [`StreamSender::finish`]
/// or [`StreamSender::fail`] ends the stream as failed with kind `no-reply`.
pub struct StreamSender<T> {
    shared: Arc<Shared>,
    call: u64,
    state: Arc<StreamState>,
    token: CancelToken,
    discard: Arc<AtomicBool>,
    done: bool,
    _item: PhantomData<fn(T)>,
}

impl<T: Serialize + Send + 'static> StreamSender<T> {
    /// Queue an item. Blocks while the stream is at capacity; returns an
    /// error once the call is cancelled or the instance stopped.
    pub fn send(&self, item: T) -> Result<(), StreamClosed> {
        self.state.acquire(&self.token)?;
        self.push(item);
        Ok(())
    }

    /// Like [`StreamSender::send`] but never blocks.
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        if self.token.is_cancelled() {
            return Err(TrySendError::Closed(item));
        }
        if !self.state.try_acquire() {
            return Err(TrySendError::Full(item));
        }
        self.push(item);
        Ok(())
    }

    fn push(&self, item: T) {
        self.shared.core.push(Delivery::Item {
            instance: self.shared.id,
            call: self.call,
            payload: Box::new(item),
            stream: Some(self.state.clone()),
            discard: self.discard.clone(),
            token: self.token.clone(),
        });
    }

    /// End the stream successfully.
    pub fn finish(mut self) {
        self.end(StreamEnd::Done, None);
    }

    /// End the stream with an error.
    pub fn fail(mut self, error: impl Into<Error>) {
        self.end(StreamEnd::Failed, Some(error.into()));
    }

    /// Cancelled when Elisp cancels the call or the instance stops.
    pub fn token(&self) -> CancelToken {
        self.token.clone()
    }
}

impl<T> StreamSender<T> {
    fn end(&mut self, status: StreamEnd, error: Option<Error>) {
        if self.done {
            return;
        }
        self.done = true;
        let (instance, call) = (self.shared.id, self.call);
        self.shared.complete(call, || Delivery::End { instance, call, status, error });
    }
}

impl<T> Drop for StreamSender<T> {
    fn drop(&mut self) {
        // See `Reply::drop`: a cancelled stream is completed by the canceller.
        if self.token.is_cancelled() {
            return;
        }
        self.end(
            StreamEnd::Failed,
            Some(Error::with_kind(ErrorKind::NoReply, "the stream was dropped without finish or fail")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::core::Core;
    use crate::runtime::instance::{Actor, AnyActor, Context, Service};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Streams `n` numbers with the given capacity, then reports on `done`.
    struct Producer;

    struct Job {
        n: u32,
        capacity: usize,
        done: mpsc::Sender<&'static str>,
    }

    impl<'de> serde::Deserialize<'de> for Job {
        fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
            Err(serde::de::Error::custom("Job is only sent from Rust"))
        }
    }

    impl Service for Producer {
        const NAME: &'static str = "producer";
        type Args = ();
        type Request = Job;
        type Event = ();

        fn start(_: (), _: &Context<Self>) -> Result<Self, Error> {
            Ok(Producer)
        }

        fn handle(&mut self, job: Job, reply: Reply, _: &Context<Self>) {
            let out = reply.stream::<u32>(job.capacity);
            for i in 0..job.n {
                if out.send(i).is_err() {
                    job.done.send("closed").unwrap();
                    return;
                }
            }
            out.finish();
            job.done.send("finished").unwrap();
        }
    }

    fn items(deliveries: &[Delivery]) -> usize {
        deliveries.iter().filter(|d| matches!(d, Delivery::Item { .. })).count()
    }

    #[test]
    fn producer_blocks_at_capacity_until_emacs_drains() {
        let core = Core::new("test", || {});
        let actor = Actor::<Producer>::spawn(&core, ());
        let (done, finished) = mpsc::channel();
        actor.send(Job { n: 20, capacity: 4, done }, Some(1), Mode::Stream).unwrap();

        std::thread::sleep(Duration::from_millis(200));
        assert!(finished.try_recv().is_err(), "producer must still be blocked");
        let first = core.take(usize::MAX);
        assert_eq!(items(&first), 4, "exactly `capacity` items are queued");

        let mut total = items(&first);
        while finished.try_recv().is_err() {
            total += items(&core.take(usize::MAX));
            std::thread::sleep(Duration::from_millis(1));
        }
        total += items(&core.take(usize::MAX));
        assert_eq!(total, 20);
    }

    #[test]
    fn cancel_wakes_a_blocked_producer() {
        let core = Core::new("test", || {});
        let actor = Actor::<Producer>::spawn(&core, ());
        let (done, finished) = mpsc::channel();
        actor.send(Job { n: 20, capacity: 2, done }, Some(1), Mode::Stream).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        actor.shared().cancel_call(1);
        assert_eq!(finished.recv_timeout(Duration::from_secs(5)), Ok("closed"));
        assert_eq!(items(&core.take(usize::MAX)), 0, "queued items of a cancelled call are discarded");
    }

    #[test]
    fn try_send_reports_full_and_closed() {
        let token = CancelToken::new();
        let state = StreamState::new(1, &token);
        assert!(state.try_acquire());
        assert!(!state.try_acquire(), "full at capacity");
        state.release();
        assert!(state.try_acquire(), "a drained slot can be reused");
        token.cancel();
        assert_eq!(state.acquire(&token), Err(StreamClosed));
    }

    #[test]
    fn take_respects_max_and_rearms_the_wakeup() {
        let wakes = Arc::new(Mutex::new(0));
        let counter = wakes.clone();
        let core = Core::new("test", move || *counter.lock().unwrap() += 1);
        for i in 0..5 {
            core.log(Level::Info, format!("line {i}"));
        }
        assert_eq!(*wakes.lock().unwrap(), 1, "one wake-up for a burst");
        assert_eq!(core.take(2).len(), 2);
        assert_eq!(*wakes.lock().unwrap(), 2, "re-armed because deliveries remain");
        assert_eq!(core.take(10).len(), 3);
        assert_eq!(*wakes.lock().unwrap(), 2, "queue empty: no further wake-up");
        core.log(Level::Info, "again");
        assert_eq!(*wakes.lock().unwrap(), 3);
    }
}
