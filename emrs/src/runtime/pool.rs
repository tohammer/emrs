//! Uncapped elastic thread pool.
//!
//! A job never waits for a busy worker: if no worker is idle when a job is
//! submitted, a new thread starts. Idle workers exit after `idle_timeout`.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub(crate) type Job = Box<dyn FnOnce() + Send + 'static>;

struct State {
    queue: VecDeque<Job>,
    /// Workers currently blocked waiting for a job.
    idle: usize,
    /// Workers alive.
    threads: usize,
    next_id: usize,
}

struct Inner {
    name: String,
    idle_timeout: Duration,
    state: Mutex<State>,
    cv: Condvar,
}

#[derive(Clone)]
pub(crate) struct Pool {
    inner: Arc<Inner>,
}

impl Pool {
    pub(crate) fn new(name: &str, idle_timeout: Duration) -> Pool {
        Pool {
            inner: Arc::new(Inner {
                name: name.to_string(),
                idle_timeout,
                state: Mutex::new(State { queue: VecDeque::new(), idle: 0, threads: 0, next_id: 0 }),
                cv: Condvar::new(),
            }),
        }
    }

    pub(crate) fn submit(&self, job: impl FnOnce() + Send + 'static) {
        let mut state = self.inner.state.lock().unwrap();
        state.queue.push_back(Box::new(job));
        if state.queue.len() > state.idle {
            state.threads += 1;
            state.next_id += 1;
            let name = format!("emrs-{}-{}", self.inner.name, state.next_id);
            drop(state);
            let inner = self.inner.clone();
            std::thread::Builder::new()
                .name(name)
                .spawn(move || worker(inner))
                .expect("failed to spawn emrs worker thread");
        } else {
            self.inner.cv.notify_one();
        }
    }

    /// Number of live worker threads.
    #[cfg(test)]
    pub(crate) fn threads(&self) -> usize {
        self.inner.state.lock().unwrap().threads
    }
}

fn worker(inner: Arc<Inner>) {
    let mut state = inner.state.lock().unwrap();
    loop {
        if let Some(job) = state.queue.pop_front() {
            drop(state);
            job();
            state = inner.state.lock().unwrap();
            continue;
        }
        state.idle += 1;
        let (guard, result) = inner.cv.wait_timeout(state, inner.idle_timeout).unwrap();
        state = guard;
        state.idle -= 1;
        if result.timed_out() && state.queue.is_empty() {
            state.threads -= 1;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Pool;
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    #[test]
    fn runs_submitted_jobs() {
        let pool = Pool::new("t", Duration::from_secs(1));
        let (tx, rx) = mpsc::channel();
        for i in 0..10 {
            let tx = tx.clone();
            pool.submit(move || tx.send(i).unwrap());
        }
        let mut got: Vec<i32> = (0..10).map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap()).collect();
        got.sort();
        assert_eq!(got, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn blocked_jobs_do_not_starve_new_jobs() {
        let pool = Pool::new("t", Duration::from_secs(1));
        let barrier = Arc::new(Barrier::new(9));
        // Eight jobs that block until the ninth party arrives.
        for _ in 0..8 {
            let b = barrier.clone();
            pool.submit(move || {
                b.wait();
            });
        }
        let (tx, rx) = mpsc::channel();
        pool.submit(move || tx.send(()).unwrap());
        rx.recv_timeout(Duration::from_secs(5)).expect("ninth job must run while eight are blocked");
        barrier.wait();
    }

    #[test]
    fn idle_workers_are_reused_and_then_exit() {
        let pool = Pool::new("t", Duration::from_millis(100));
        let (tx, rx) = mpsc::channel();
        for _ in 0..3 {
            let tx = tx.clone();
            pool.submit(move || tx.send(()).unwrap());
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(pool.threads(), 1, "sequential jobs reuse one idle worker");
        let deadline = Instant::now() + Duration::from_secs(5);
        while pool.threads() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(pool.threads(), 0, "idle worker exits after the timeout");
    }

    #[test]
    fn worker_names_carry_the_pool_name() {
        let pool = Pool::new("git-rs", Duration::from_secs(1));
        let (tx, rx) = mpsc::channel();
        pool.submit(move || tx.send(std::thread::current().name().map(String::from)).unwrap());
        let name = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert!(name.starts_with("emrs-git-rs-"), "{name}");
    }
}
