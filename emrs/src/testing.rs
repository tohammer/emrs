//! Run a service without Emacs.
//!
//! [`Host`] drives one instance through the real pool, mailbox, reply and
//! stream code. Only the Lisp conversion and the wake-up pipe are replaced:
//! requests are passed as Rust values and answers are returned as the Rust
//! values the service produced.

use crate::error::{Error, ErrorKind};
use crate::runtime::core::{Core, Delivery, StreamEnd};
use crate::runtime::instance::{Actor, AnyActor, Mode, Service, State};
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

type AnyBox = Box<dyn Any + Send>;

enum Outcome {
    Reply(AnyBox),
    Error(Error),
    End(StreamEnd, Option<Error>),
}

#[derive(Default)]
struct CallRecord {
    items: Vec<AnyBox>,
    outcome: Option<Outcome>,
}

/// The collected result of a stream.
#[derive(Debug)]
pub struct StreamResult<T> {
    pub items: Vec<T>,
    pub status: StreamEnd,
    pub error: Option<Error>,
}

/// Hosts one instance of `S` for tests.
pub struct Host<S: Service> {
    core: Arc<Core>,
    actor: Arc<Actor<S>>,
    signal: Arc<(Mutex<bool>, Condvar)>,
    timeout: Duration,
    next_call: u64,
    events: VecDeque<AnyBox>,
    calls: HashMap<u64, CallRecord>,
    state: State,
    exit: Option<Option<Error>>,
    logs: Vec<String>,
}

fn downcast<T: 'static>(value: AnyBox, what: &str) -> T {
    match value.downcast::<T>() {
        Ok(value) => *value,
        Err(_) => panic!("{what} does not have type {}", std::any::type_name::<T>()),
    }
}

impl<S: Service> Host<S> {
    /// Start an instance. Operations wait up to five seconds by default.
    pub fn start(args: S::Args) -> Host<S> {
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let waker = signal.clone();
        let core = Core::new("test", move || {
            *waker.0.lock().unwrap() = true;
            waker.1.notify_all();
        });
        let actor = Actor::<S>::spawn(&core, args);
        Host {
            core,
            actor,
            signal,
            timeout: Duration::from_secs(5),
            next_call: 0,
            events: VecDeque::new(),
            calls: HashMap::new(),
            state: State::Starting,
            exit: None,
            logs: Vec::new(),
        }
    }

    /// Change how long operations wait before giving up.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn absorb(&mut self) {
        *self.signal.0.lock().unwrap() = false;
        for delivery in self.core.take(usize::MAX) {
            match delivery {
                Delivery::Event { payload, .. } => self.events.push_back(payload.into_any()),
                Delivery::Reply { call, payload, .. } => {
                    self.calls.entry(call).or_default().outcome = Some(Outcome::Reply(payload.into_any()));
                }
                Delivery::Error { call, error, .. } => {
                    self.calls.entry(call).or_default().outcome = Some(Outcome::Error(error));
                }
                Delivery::Item { call, payload, .. } => {
                    self.calls.entry(call).or_default().items.push(payload.into_any());
                }
                Delivery::End { call, status, error, .. } => {
                    self.calls.entry(call).or_default().outcome = Some(Outcome::End(status, error));
                }
                Delivery::State { state, error, .. } => {
                    self.state = state;
                    if matches!(state, State::Stopped | State::Failed) {
                        self.exit = Some(error);
                    }
                }
                Delivery::Log { level, message } => self.logs.push(format!("[{}] {message}", level.as_str())),
            }
        }
    }

    /// Absorb deliveries until `done` holds or the timeout passes.
    fn wait(&mut self, done: impl Fn(&Self) -> bool) -> bool {
        let deadline = Instant::now() + self.timeout;
        loop {
            self.absorb();
            if done(self) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let signalled = self.signal.0.lock().unwrap();
            if !*signalled {
                let _ = self.signal.1.wait_timeout(signalled, deadline - now).unwrap();
            }
        }
    }

    fn begin(&mut self, request: S::Request, mode: Mode) -> Result<u64, Error> {
        self.next_call += 1;
        self.actor.send(request, Some(self.next_call), mode)?;
        Ok(self.next_call)
    }

    /// Send a request without expecting an answer, as `emrs-send` does.
    pub fn send(&mut self, request: S::Request) {
        let _ = self.actor.send(request, None, Mode::Send);
    }

    /// Make a call and wait for its reply, as `emrs-call-sync` does.
    /// Panics if the reply does not have type `T`.
    pub fn call<T: 'static>(&mut self, request: S::Request) -> Result<T, Error> {
        let call = self.begin(request, Mode::Call)?;
        if !self.wait(|host| host.calls.get(&call).is_some_and(|c| c.outcome.is_some())) {
            self.cancel(call);
            return Err(Error::with_kind(ErrorKind::Timeout, "no reply within the host timeout"));
        }
        match self.calls.remove(&call).and_then(|c| c.outcome) {
            Some(Outcome::Reply(value)) => Ok(downcast(value, "the reply")),
            Some(Outcome::Error(error)) => Err(error),
            _ => panic!("a call was answered with a stream end"),
        }
    }

    /// Start a stream call and return its id, as `emrs-call-stream` does.
    pub fn begin_stream(&mut self, request: S::Request) -> Result<u64, Error> {
        self.begin(request, Mode::Stream)
    }

    /// Cancel a call or stream, as `emrs-cancel` does.
    pub fn cancel(&mut self, call: u64) {
        self.actor.shared().cancel_call(call);
    }

    /// Number of stream items received so far for `call`.
    pub fn items_received(&mut self, call: u64) -> usize {
        self.absorb();
        self.calls.get(&call).map_or(0, |c| c.items.len())
    }

    /// Wait for a stream to end and return everything it delivered.
    /// Panics if an item does not have type `T` or the stream does not end.
    pub fn finish_stream<T: 'static>(&mut self, call: u64) -> StreamResult<T> {
        if !self.wait(|host| host.calls.get(&call).is_some_and(|c| c.outcome.is_some())) {
            panic!("stream {call} did not end within the host timeout");
        }
        let record = self.calls.remove(&call).expect("record exists after wait");
        let items = record.items.into_iter().map(|item| downcast(item, "a stream item")).collect();
        match record.outcome {
            Some(Outcome::End(status, error)) => StreamResult { items, status, error },
            Some(Outcome::Error(error)) => StreamResult { items, status: StreamEnd::Failed, error: Some(error) },
            _ => panic!("a stream call was answered with a plain reply"),
        }
    }

    /// Make a stream call and collect it to the end.
    pub fn call_stream<T: 'static>(&mut self, request: S::Request) -> Result<StreamResult<T>, Error> {
        let call = self.begin_stream(request)?;
        Ok(self.finish_stream(call))
    }

    /// The next event, waiting if necessary. `None` after the timeout.
    pub fn next_event(&mut self) -> Option<S::Event> {
        if !self.wait(|host| !host.events.is_empty()) {
            return None;
        }
        self.events.pop_front().map(|event| downcast(event, "the event"))
    }

    /// The instance state as last reported.
    pub fn state(&mut self) -> State {
        self.absorb();
        self.state
    }

    /// Wait until the instance reports `state`.
    pub fn wait_for_state(&mut self, state: State) -> bool {
        self.wait(|host| host.state == state)
    }

    /// Request a stop, as `emrs-stop` does, and wait for the terminal state.
    pub fn stop(&mut self) -> State {
        self.actor.request_stop();
        self.wait_exit().0
    }

    /// Wait for the instance to stop or fail; returns the terminal state and
    /// the failure, if any. Panics if it keeps running.
    pub fn wait_exit(&mut self) -> (State, Option<Error>) {
        if !self.wait(|host| host.exit.is_some()) {
            panic!("the instance did not terminate within the host timeout");
        }
        (self.state, self.exit.take().flatten())
    }

    /// Log lines written so far, formatted as `[level] message`.
    pub fn logs(&mut self) -> Vec<String> {
        self.absorb();
        self.logs.clone()
    }
}

impl<S: Service> Drop for Host<S> {
    fn drop(&mut self) {
        self.actor.request_stop();
    }
}
