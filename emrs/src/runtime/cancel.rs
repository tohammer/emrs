//! Cooperative cancellation tokens arranged in a tree.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

type Callback = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Listeners {
    callbacks: Vec<Callback>,
    children: Vec<Weak<Inner>>,
}

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    listeners: Mutex<Listeners>,
    cv: Condvar,
}

/// A token that reports whether work should stop.
///
/// Cancellation is cooperative: code checks [`CancelToken::is_cancelled`] or
/// sleeps with [`CancelToken::wait_timeout`].
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

impl CancelToken {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// True once this token or one of its ancestors was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Sleep for at most `timeout`. Returns true as soon as the token is
    /// cancelled, false if the timeout elapsed first.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let guard = self.inner.listeners.lock().unwrap();
        let (_guard, _) = self.inner.cv.wait_timeout_while(guard, timeout, |_| !self.is_cancelled()).unwrap();
        self.is_cancelled()
    }

    /// A token that is cancelled when this one is, and can also be cancelled
    /// on its own.
    pub(crate) fn child(&self) -> CancelToken {
        let child = CancelToken::new();
        let mut listeners = self.inner.listeners.lock().unwrap();
        if self.is_cancelled() {
            drop(listeners);
            child.cancel();
        } else {
            listeners.children.retain(|c| c.strong_count() > 0);
            listeners.children.push(Arc::downgrade(&child.inner));
        }
        child
    }

    /// Run `f` when the token is cancelled; immediately if it already is.
    pub(crate) fn on_cancel(&self, f: impl FnOnce() + Send + 'static) {
        let mut listeners = self.inner.listeners.lock().unwrap();
        if self.is_cancelled() {
            drop(listeners);
            f();
        } else {
            listeners.callbacks.push(Box::new(f));
        }
    }

    pub(crate) fn cancel(&self) {
        let taken = {
            let mut listeners = self.inner.listeners.lock().unwrap();
            if self.inner.cancelled.swap(true, Ordering::SeqCst) {
                return;
            }
            self.inner.cv.notify_all();
            std::mem::take(&mut *listeners)
        };
        for callback in taken.callbacks {
            callback();
        }
        for child in taken.children {
            if let Some(inner) = child.upgrade() {
                CancelToken { inner }.cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CancelToken;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn starts_uncancelled_and_cancel_is_sticky() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
        assert!(token.clone().is_cancelled());
    }

    #[test]
    fn cancelling_parent_cancels_children_but_not_reverse() {
        let parent = CancelToken::new();
        let a = parent.child();
        let b = parent.child();
        a.cancel();
        assert!(!parent.is_cancelled());
        assert!(!b.is_cancelled());
        parent.cancel();
        assert!(b.is_cancelled());
        assert!(parent.child().is_cancelled(), "child of cancelled parent");
    }

    #[test]
    fn callbacks_run_exactly_once() {
        let token = CancelToken::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        token.on_cancel(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        token.cancel();
        token.cancel();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let h = hits.clone();
        token.on_cancel(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(hits.load(Ordering::SeqCst), 2, "runs at once when already cancelled");
    }

    #[test]
    fn wait_timeout_returns_early_on_cancel() {
        let token = CancelToken::new();
        let t = token.clone();
        let start = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            t.cancel();
        });
        assert!(token.wait_timeout(Duration::from_secs(10)));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn wait_timeout_expires() {
        let token = CancelToken::new();
        assert!(!token.wait_timeout(Duration::from_millis(20)));
    }
}
