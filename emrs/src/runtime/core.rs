//! Runtime state shared by all instances of one module: the pool, the
//! outbound FIFO with its wake-up, and the instance table.

use super::cancel::CancelToken;
use super::instance::{AnyActor, State};
use super::pool::Pool;
use super::reply::StreamState;
use crate::error::Error;
use crate::payload::Payload;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long an idle pool worker lives.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Severity of a line in `*emrs-log*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// How a stream ended on the Rust side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEnd {
    Done,
    Failed,
}

/// One record handed to the Emacs side by a drain.
pub(crate) enum Delivery {
    Event {
        instance: u64,
        payload: Box<dyn Payload>,
    },
    Reply {
        instance: u64,
        call: u64,
        payload: Box<dyn Payload>,
    },
    Error {
        instance: u64,
        call: u64,
        error: Error,
    },
    Item {
        instance: u64,
        call: u64,
        payload: Box<dyn Payload>,
        /// Backpressure accounting; `None` for an item that is not counted.
        stream: Option<Arc<StreamState>>,
        /// Items of a cancelled call are discarded by `take`.
        token: CancelToken,
    },
    End {
        instance: u64,
        call: u64,
        status: StreamEnd,
        error: Option<Error>,
    },
    State {
        instance: u64,
        state: State,
        error: Option<Error>,
    },
    Log {
        level: Level,
        message: String,
    },
}

pub(crate) struct Core {
    pub(crate) pool: Pool,
    out: Mutex<VecDeque<Delivery>>,
    wake_pending: AtomicBool,
    waker: Box<dyn Fn() + Send + Sync>,
    instances: Mutex<HashMap<u64, Arc<dyn AnyActor>>>,
    instances_changed: Condvar,
    next_id: AtomicU64,
}

impl Core {
    /// `waker` is called, from any thread, when the outbound queue goes from
    /// "nothing announced" to "something to drain".
    pub(crate) fn new(name: &str, waker: impl Fn() + Send + Sync + 'static) -> Arc<Core> {
        Arc::new(Core {
            pool: Pool::new(name, IDLE_TIMEOUT),
            out: Mutex::new(VecDeque::new()),
            wake_pending: AtomicBool::new(false),
            waker: Box::new(waker),
            instances: Mutex::new(HashMap::new()),
            instances_changed: Condvar::new(),
            next_id: AtomicU64::new(1),
        })
    }

    fn wake(&self) {
        if !self.wake_pending.swap(true, Ordering::SeqCst) {
            (self.waker)();
        }
    }

    pub(crate) fn push(&self, delivery: Delivery) {
        self.out.lock().unwrap().push_back(delivery);
        self.wake();
    }

    pub(crate) fn log(&self, level: Level, message: impl Into<String>) {
        self.push(Delivery::Log { level, message: message.into() });
    }

    /// Remove up to `max` deliveries in order. Items of cancelled calls are
    /// dropped and do not count. Re-arms the wake-up if deliveries remain.
    pub(crate) fn take(&self, max: usize) -> Vec<Delivery> {
        self.wake_pending.store(false, Ordering::SeqCst);
        let mut taken = Vec::new();
        let mut out = self.out.lock().unwrap();
        while taken.len() < max {
            let Some(delivery) = out.pop_front() else { break };
            if let Delivery::Item { stream, token, .. } = &delivery {
                if let Some(stream) = stream {
                    stream.release();
                }
                if token.is_cancelled() {
                    continue;
                }
            }
            taken.push(delivery);
        }
        let more = !out.is_empty();
        drop(out);
        if more {
            self.wake();
        }
        taken
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn register(&self, id: u64, actor: Arc<dyn AnyActor>) {
        self.instances.lock().unwrap().insert(id, actor);
    }

    pub(crate) fn unregister(&self, id: u64) {
        let removed = self.instances.lock().unwrap().remove(&id);
        self.instances_changed.notify_all();
        drop(removed);
    }

    pub(crate) fn get(&self, id: u64) -> Option<Arc<dyn AnyActor>> {
        self.instances.lock().unwrap().get(&id).cloned()
    }

    /// Ask every instance to stop and wait up to `timeout` until all have
    /// terminated. Returns true if none is left.
    pub(crate) fn shutdown(&self, timeout: Duration) -> bool {
        let actors: Vec<Arc<dyn AnyActor>> = self.instances.lock().unwrap().values().cloned().collect();
        for actor in actors {
            actor.request_stop();
        }
        let deadline = Instant::now() + timeout;
        let mut instances = self.instances.lock().unwrap();
        while !instances.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            instances = self.instances_changed.wait_timeout(instances, deadline - now).unwrap().0;
        }
        true
    }
}
