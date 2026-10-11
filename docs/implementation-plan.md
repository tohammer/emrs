# emrs Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `emrs` Rust crate and the `emrs.el` Elisp library, which let authors write Emacs dynamic modules as message-passing services that never run on the Emacs thread.

**Architecture:** One crate, `emrs`, holds a private FFI layer, a `serde` serializer and deserializer that work directly on Lisp values, and a per-module runtime (elastic thread pool, actor instances, replies, bounded streams). A module exposes eight protocol functions; `emrs.el` drives them, is woken through a pipe process opened with `open_channel`, and dispatches deliveries to callbacks. A fake Emacs environment lets the conversion code be unit-tested under `cargo test`; an ERT suite covers the full path in real Emacs.

**Tech Stack:** Rust (edition 2024), `serde`, `libc`; Emacs Lisp (Emacs 30.1+), ERT; shell scripts for the Emacs test runs; GitHub Actions.

**Status: executed.** Tasks 1 to 12 are committed on the branch `emrs-core`. Later commits on that branch supersede some of the code blocks below: "fix: review findings" (changes `elisp/emrs.el`, `emrs/src/module.rs`, `emrs/src/runtime/{core,instance,reply}.rs` and the tests), "ci: run only on pull requests", and "fix: disable adaptive read buffering on the wake-up pipe" (`elisp/emrs.el`). The repository is the current source; this document is kept as the record of how the first version was built. The statements under "Verification status" and "Not verified" describe the day the plan was written; `docs/specs/2026-10-03-emrs-core.md` section 13 has the current status.

**Spec:** `docs/specs/2026-10-03-emrs-core.md`. Executors read both documents; the plan implements the spec section by section.

**Verification status of this plan:** every code block below was built and every test was run on 2026-10-03 with Rust 1.97 and Emacs 31.1 on Linux x86_64, at each task boundary (Tasks 1 to 9 as separate staged workspaces, Tasks 10 and 11 on the complete workspace). Final results: `cargo test --workspace` 51 unit tests and 24 runtime tests pass, `cargo clippy -- -D warnings` and `cargo fmt --check` are clean, `tests/run-ert.sh` passes 37 of 37 with `--module-assertions`, `examples/run-examples.sh` passes 3 of 3. **Not verified:** the GitHub Actions workflow (Task 12), Rust 1.85, Emacs 30, macOS, and Windows.

**Spikes already done (spec section 13):**

- 13.2 raw bytes: verified. `copy_string_contents` returns the raw bytes of a unibyte string, including bytes above 127. No fallback is needed.
- 13.3 wake-up inside a filter: verified. `accept-process-output` on the pipe returns as soon as the byte arrives (0.10 s for a 100 ms delay), also when called from inside that pipe's own filter, and the filter runs re-entrantly.
- 13.1 Windows descriptor: not verified. The Windows CI job in Task 12 is the test and is allowed to fail.

## Global Constraints

- Crate name `emrs`; Elisp library `emrs.el`; every Elisp symbol starts with `emrs-` (private ones with `emrs--`).
- Minimum Emacs 30.1, declared as `Package-Requires: ((emacs "30.1"))`. The Rust side checks only for the Emacs 28 environment layout.
- Rust edition 2024, `rust-version = "1.85"`. Dependencies of `emrs`: `serde` and `libc` only. No proc-macro crate.
- Rust licence `MIT OR Apache-2.0`; `emrs.el` licence `GPL-3.0-or-later`.
- Modules must be built with `panic = "unwind"`; `module!` emits `compile_error!` under `#[cfg(panic = "abort")]`.
- Service code (`start`, `handle`, `stop`, spawned tasks) never runs on the Emacs thread. Lisp values are created and read only on the Emacs thread inside a module function call and never outlive it.
- A module function called from Emacs never blocks on service code, except the time-bounded `shutdown`.
- The FFI bindings stay private (`pub(crate)`); nothing in the public API mentions `emacs_env` or `emacs_value`.
- Module protocol version is the integer 1. Protocol functions are named `<module>--emrs-<op>`; the module provides the feature `<module>-module`.
- Wire format: keyword-headed lists and plists with kebab-case keywords (spec section 8).
- Formatting: `rustfmt.toml` with `max_width = 120` and `use_small_heuristics = "Max"`; `cargo fmt --all -- --check` must pass at every commit.
- Platforms: Linux and macOS (x86_64, aarch64) must pass; Windows is best effort.

## Review Focus

These are inputs and conditions the spec implies but does not spell out as requirements. Each has a test in the task named.

1. **A circular or dotted list sent as a request.** A person expects an error, not a frozen Emacs. The decoder calls `proper-list-p` once per list before reading it. Unit test `sequences_from_lists_and_vectors` (Task 6) covers dotted lists; ERT `emrs-test-circular-and-dotted-lists-are-rejected` (Task 10) covers a truly circular list in real Emacs. This is stricter than the spec, which only said circular lists "must not be sent".
2. **`C-g` while `emrs-call-sync` waits.** The quit must propagate, the call must be cancelled, and no call record may leak. ERT `emrs-test-quit-during-call-sync-cancels-the-call` (Task 10).
3. **Cancelling a stream from inside its own `on-batch`.** This is the natural way to stop after the first results. Exactly one batch and one `cancelled` end must be seen. ERT `emrs-test-cancel-stream-from-inside-on-batch` (Task 10).
4. **Emacs exits while a handler is blocked.** Shutdown must return within its timeout instead of hanging Emacs. ERT `emrs-test-zy-shutdown-does-not-wait-for-a-blocked-handler` (Task 10).
5. **Unusual volumes and strings.** Empty strings, embedded NUL, non-BMP characters, a 100,000-character string, and 50,000 events with a small drain size must all arrive intact. ERT `emrs-test-unusual-strings-round-trip` and `emrs-test-event-flood-arrives-completely` (Task 10).

## File Structure

| Path | Responsibility |
|---|---|
| `Cargo.toml`, `rustfmt.toml`, `.gitignore` | Workspace definition and formatting rules. |
| `emrs/Cargo.toml`, `emrs/build.rs` | Crate manifest; `build.rs` records the target triple as `EMRS_TARGET`. |
| `emrs/src/lib.rs` | Module declarations and the public re-exports. |
| `emrs/src/names.rs` | `to_kebab`: Rust identifier to keyword name. |
| `emrs/src/ffi/sys.rs` | Raw `emacs_env` / `emacs_runtime` layout. |
| `emrs/src/ffi/env.rs` | `Env` and `Value`: checked wrappers; every call checks for a non-local exit. |
| `emrs/src/ffi/fake.rs` | Test-only fake Emacs with a tiny Lisp heap, reader and printer. |
| `emrs/src/convert/ser.rs` | `serde::Serializer` producing Lisp values. |
| `emrs/src/convert/de.rs` | `serde::Deserializer` reading Lisp values. |
| `emrs/src/error.rs` | `Error`, `ErrorKind`, and the internal `Fail`. |
| `emrs/src/payload.rs` | `Payload`: type-erased values awaiting conversion on the Emacs thread. |
| `emrs/src/runtime/cancel.rs` | `CancelToken` tree. |
| `emrs/src/runtime/pool.rs` | Uncapped elastic thread pool. |
| `emrs/src/runtime/core.rs` | `Core`: outbound FIFO, wake-up flag, instance table, shutdown; `Delivery`. |
| `emrs/src/runtime/instance.rs` | `Service`, `Context`, `Address`, `State`; the actor loop and lifecycle. |
| `emrs/src/runtime/reply.rs` | `Reply`, `StreamSender`, backpressure accounting. |
| `emrs/src/testing.rs` | `Host`: runs a service without Emacs. |
| `emrs/src/module.rs` | Protocol functions, `init`, the `module!` macro. |
| `emrs/tests/runtime.rs` | Runtime behaviour tests through `Host`. |
| `elisp/emrs.el` | The Elisp library. |
| `tests/emrs-test-module/` | `cdylib` with the `types` and `probe` services for ERT. |
| `tests/elisp/emrs-tests.el`, `tests/run-ert.sh` | ERT suite and its runner. |
| `examples/echo/`, `examples/grep/` | The two example modules of spec section 10. |
| `examples/examples-test.el`, `examples/run-examples.sh` | Smoke tests for the examples. |
| `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`, `.github/workflows/ci.yml` | Project documentation, licences, CI. |

Two names differ from the layout sketched in spec section 3.1, and the spec has been updated to match: the conversion code lives in `emrs/src/convert/` (a module named `serde` would shadow the `serde` crate), and the outbound queue lives in `runtime/core.rs`.

Work on a feature branch, not on `master`.

---

### Task 1: Workspace scaffold and name conversion

**Files:**
- Create: `Cargo.toml`, `rustfmt.toml`, `.gitignore`
- Create: `emrs/Cargo.toml`, `emrs/build.rs`, `emrs/src/lib.rs`
- Create: `emrs/src/names.rs` (tests inside)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) fn names::to_kebab(name: &str) -> String`; the environment variable `EMRS_TARGET` at compile time (used in Task 9).

- [ ] **Step 1: Create the workspace files**

`Cargo.toml`:

````toml
[workspace]
resolver = "3"
members = ["emrs"]

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"

[profile.dev]
panic = "unwind"

[profile.release]
panic = "unwind"
````

`rustfmt.toml`:

````toml
max_width = 120
use_small_heuristics = "Max"
````

`.gitignore`:

````text
/target
Cargo.lock
*.elc
````

`emrs/Cargo.toml`:

````toml
[package]
name = "emrs"
version = "0.1.0"
description = "Message-passing services for Emacs dynamic modules"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
libc = "0.2"
serde = "1"

[dev-dependencies]
serde = { version = "1", features = ["derive"] }
````

`emrs/build.rs`:

````rust
fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    println!("cargo:rustc-env=EMRS_TARGET={target}");
}
````

`emrs/src/lib.rs`:

````rust
//! Message-passing services for Emacs dynamic modules.
#![allow(dead_code)] // Removed in Task 9, when every item has a user.

mod names;
````

- [ ] **Step 2: Write the failing tests**

Create `emrs/src/names.rs` with only the tests:

````rust
//! Conversion of Rust identifiers to Lisp keyword names.

#[cfg(test)]
mod tests {
    use super::to_kebab;

    #[test]
    fn snake_case_fields() {
        assert_eq!(to_kebab("line_number"), "line-number");
        assert_eq!(to_kebab("path"), "path");
    }

    #[test]
    fn camel_case_variants() {
        assert_eq!(to_kebab("Stop"), "stop");
        assert_eq!(to_kebab("SearchResult"), "search-result");
        assert_eq!(to_kebab("V2Result"), "v2-result");
    }

    #[test]
    fn acronyms_stay_together() {
        assert_eq!(to_kebab("HTTPServer"), "http-server");
        assert_eq!(to_kebab("ParseURL"), "parse-url");
    }

    #[test]
    fn kebab_input_is_unchanged() {
        assert_eq!(to_kebab("already-kebab"), "already-kebab");
    }

    #[test]
    fn empty_and_underscores() {
        assert_eq!(to_kebab(""), "");
        assert_eq!(to_kebab("_private_Name"), "-private-name");
    }
}
````

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p emrs`

Expected: compilation fails with `unresolved import super::to_kebab`.

- [ ] **Step 4: Write the implementation**

Replace the top of `emrs/src/names.rs` (keep the tests module below it) so the file starts with:

````rust
//! Conversion of Rust identifiers to Lisp keyword names.

/// Convert a `snake_case` or `CamelCase` identifier to kebab-case.
pub(crate) fn to_kebab(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            out.push('-');
        } else if c.is_uppercase() {
            if i > 0 {
                let prev = chars[i - 1];
                let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
                let after_word = prev.is_lowercase() || prev.is_ascii_digit();
                let ends_acronym = prev.is_uppercase() && next_is_lower;
                if after_word || ends_acronym {
                    out.push('-');
                }
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}
````

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p emrs`

Expected: `test result: ok. 5 passed`. A dead-code warning for `to_kebab` is suppressed by the crate-level `allow`.

- [ ] **Step 6: Commit**

````bash
git add Cargo.toml rustfmt.toml .gitignore emrs
git commit -m "feat: workspace scaffold and kebab-case name conversion"
````

---

### Task 2: CancelToken

**Files:**
- Create: `emrs/src/runtime/mod.rs`, `emrs/src/runtime/cancel.rs` (tests inside)
- Modify: `emrs/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub struct CancelToken` (`Clone + Send + Sync + Default`) with `pub fn is_cancelled(&self) -> bool`, `pub fn wait_timeout(&self, timeout: Duration) -> bool`, and crate-private `new()`, `child(&self) -> CancelToken`, `on_cancel(&self, f: impl FnOnce() + Send + 'static)`, `cancel(&self)`.

- [ ] **Step 1: Register the module**

`emrs/src/runtime/mod.rs`:

````rust
//! The per-module runtime: pool, outbound queue, instances, replies.

pub(crate) mod cancel;
````

`emrs/src/lib.rs` (whole file):

````rust
//! Message-passing services for Emacs dynamic modules.
#![allow(dead_code)] // Removed in Task 9, when every item has a user.

mod names;
mod runtime;

pub use runtime::cancel::CancelToken;
````

- [ ] **Step 2: Write the failing tests**

Create `emrs/src/runtime/cancel.rs` with only the tests:

````rust
//! Cooperative cancellation tokens arranged in a tree.

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
````

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p emrs cancel`

Expected: compilation fails with `unresolved import super::CancelToken`.

- [ ] **Step 4: Write the implementation**

Put this above the tests module in `emrs/src/runtime/cancel.rs`, replacing the one-line doc comment:

````rust
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
````

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p emrs`

Expected: `test result: ok. 10 passed`.

- [ ] **Step 6: Commit**

````bash
git add emrs/src
git commit -m "feat: cancellation token tree"
````

---

### Task 3: Elastic thread pool

**Files:**
- Create: `emrs/src/runtime/pool.rs` (tests inside)
- Modify: `emrs/src/runtime/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) struct Pool` (`Clone`) with `Pool::new(name: &str, idle_timeout: Duration) -> Pool` and `submit(&self, job: impl FnOnce() + Send + 'static)`. Worker threads are named `emrs-<name>-<n>`. A job never waits for a busy worker.

- [ ] **Step 1: Register the module**

`emrs/src/runtime/mod.rs` (whole file):

````rust
//! The per-module runtime: pool, outbound queue, instances, replies.

pub(crate) mod cancel;
pub(crate) mod pool;
````

- [ ] **Step 2: Write the failing tests**

Create `emrs/src/runtime/pool.rs` with only the tests:

````rust
//! Uncapped elastic thread pool.

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
````

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p emrs pool`

Expected: compilation fails with `unresolved import super::Pool`.

- [ ] **Step 4: Write the implementation**

Put this above the tests module, replacing the one-line doc comment:

````rust
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
````

The spawn decision is `queue.len() > idle`: `idle` counts workers that are actually blocked in `wait_timeout`, so two quick submissions with one idle worker start a second thread instead of queueing behind the first.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p emrs`

Expected: `test result: ok. 14 passed`.

- [ ] **Step 6: Commit**

````bash
git add emrs/src
git commit -m "feat: uncapped elastic thread pool"
````

---

### Task 4: FFI layer and fake Emacs

**Files:**
- Create: `emrs/src/ffi/mod.rs`, `emrs/src/ffi/sys.rs` (test inside), `emrs/src/ffi/env.rs`, `emrs/src/ffi/fake.rs` (tests inside)
- Modify: `emrs/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces (all `pub(crate)`):
  - `sys::{emacs_env, emacs_runtime, emacs_value, emacs_function, emacs_limb_t}`.
  - `env::Exit` (a non-local exit is pending), `env::Res<T> = Result<T, Exit>`, `env::Value` (`Copy`, `#[repr(transparent)]` over `emacs_value`).
  - `env::Env` with `unsafe fn from_raw(*mut sys::emacs_env) -> Env` and the methods `intern`, `nil`, `t`, `funcall`, `call(name, args)`, `is_not_nil`, `type_name`, `symbol_name`, `make_i64`, `extract_i64`, `make_big(negative, magnitude: u128)`, `extract_big -> Res<Option<(bool, u128)>>`, `make_float`, `extract_float`, `make_string`, `make_unibyte`, `string_bytes`, `string`, `vec_size`, `vec_get`, `cons`, `list`, `car`, `cdr`, `make_function`, `open_channel`, `signal`, `clear_exit`.
  - Test only: `fake::Fake` with `Fake::new()`, `env(&mut self) -> Env`, `read(&mut self, text: &str) -> Value`, `print(&self, Value) -> String`, `pending(&self) -> Option<String>`. The fake implements the Lisp functions `list`, `vector`, `cons`, `car`, `cdr`, `symbol-name`, `proper-list-p` and `emrs--hash-to-alist`.

- [ ] **Step 1: Register the module**

`emrs/src/ffi/mod.rs`:

````rust
//! Private bindings to the Emacs module API.

pub(crate) mod env;
#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod sys;
````

`emrs/src/lib.rs` (whole file):

````rust
//! Message-passing services for Emacs dynamic modules.
#![allow(dead_code)] // Removed in Task 9, when every item has a user.

mod ffi;
mod names;
mod runtime;

pub use runtime::cancel::CancelToken;
````

- [ ] **Step 2: Write the raw layout with its test**

`emrs/src/ffi/sys.rs`. The member order must match `emacs-module.h` exactly; the test pins the size.

````rust
//! Raw layout of the Emacs module API (`emacs-module.h`), Emacs 28 level.
//!
//! The environment structs of Emacs 29 and 30 add no members, so this layout
//! is valid for every Emacs from 28 on. Members the crate does not call are
//! declared as untyped function pointers to keep the layout exact.
#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_void};

pub(crate) type emacs_value = *mut c_void;
pub(crate) type emacs_limb_t = usize;

/// Placeholder for a member that is never called.
pub(crate) type Unused = Option<unsafe extern "C" fn()>;

pub(crate) type emacs_function =
    unsafe extern "C" fn(env: *mut emacs_env, nargs: isize, args: *mut emacs_value, data: *mut c_void) -> emacs_value;

pub(crate) const FUNCALL_EXIT_RETURN: c_int = 0;

#[repr(C)]
pub(crate) struct emacs_runtime {
    pub size: isize,
    pub private_members: *mut c_void,
    pub get_environment: unsafe extern "C" fn(rt: *mut emacs_runtime) -> *mut emacs_env,
}

#[repr(C)]
pub(crate) struct emacs_env {
    pub size: isize,
    pub private_members: *mut c_void,
    // Emacs 25
    pub make_global_ref: Unused,
    pub free_global_ref: Unused,
    pub non_local_exit_check: unsafe extern "C" fn(env: *mut emacs_env) -> c_int,
    pub non_local_exit_clear: unsafe extern "C" fn(env: *mut emacs_env),
    pub non_local_exit_get: Unused,
    pub non_local_exit_signal: unsafe extern "C" fn(env: *mut emacs_env, symbol: emacs_value, data: emacs_value),
    pub non_local_exit_throw: Unused,
    pub make_function: unsafe extern "C" fn(
        env: *mut emacs_env,
        min_arity: isize,
        max_arity: isize,
        func: emacs_function,
        docstring: *const c_char,
        data: *mut c_void,
    ) -> emacs_value,
    pub funcall: unsafe extern "C" fn(
        env: *mut emacs_env,
        func: emacs_value,
        nargs: isize,
        args: *mut emacs_value,
    ) -> emacs_value,
    pub intern: unsafe extern "C" fn(env: *mut emacs_env, name: *const c_char) -> emacs_value,
    pub type_of: unsafe extern "C" fn(env: *mut emacs_env, arg: emacs_value) -> emacs_value,
    pub is_not_nil: unsafe extern "C" fn(env: *mut emacs_env, arg: emacs_value) -> bool,
    pub eq: unsafe extern "C" fn(env: *mut emacs_env, a: emacs_value, b: emacs_value) -> bool,
    pub extract_integer: unsafe extern "C" fn(env: *mut emacs_env, arg: emacs_value) -> i64,
    pub make_integer: unsafe extern "C" fn(env: *mut emacs_env, n: i64) -> emacs_value,
    pub extract_float: unsafe extern "C" fn(env: *mut emacs_env, arg: emacs_value) -> f64,
    pub make_float: unsafe extern "C" fn(env: *mut emacs_env, d: f64) -> emacs_value,
    pub copy_string_contents:
        unsafe extern "C" fn(env: *mut emacs_env, value: emacs_value, buf: *mut c_char, len: *mut isize) -> bool,
    pub make_string: unsafe extern "C" fn(env: *mut emacs_env, s: *const c_char, len: isize) -> emacs_value,
    pub make_user_ptr: Unused,
    pub get_user_ptr: Unused,
    pub set_user_ptr: Unused,
    pub get_user_finalizer: Unused,
    pub set_user_finalizer: Unused,
    pub vec_get: unsafe extern "C" fn(env: *mut emacs_env, vector: emacs_value, index: isize) -> emacs_value,
    pub vec_set: Unused,
    pub vec_size: unsafe extern "C" fn(env: *mut emacs_env, vector: emacs_value) -> isize,
    // Emacs 26
    pub should_quit: Unused,
    // Emacs 27
    pub process_input: Unused,
    pub extract_time: Unused,
    pub make_time: Unused,
    pub extract_big_integer: unsafe extern "C" fn(
        env: *mut emacs_env,
        arg: emacs_value,
        sign: *mut c_int,
        count: *mut isize,
        magnitude: *mut emacs_limb_t,
    ) -> bool,
    pub make_big_integer: unsafe extern "C" fn(
        env: *mut emacs_env,
        sign: c_int,
        count: isize,
        magnitude: *const emacs_limb_t,
    ) -> emacs_value,
    // Emacs 28
    pub get_function_finalizer: Unused,
    pub set_function_finalizer: Unused,
    pub open_channel: unsafe extern "C" fn(env: *mut emacs_env, pipe_process: emacs_value) -> c_int,
    pub make_interactive: Unused,
    pub make_unibyte_string: unsafe extern "C" fn(env: *mut emacs_env, s: *const c_char, len: isize) -> emacs_value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_emacs_28_header() {
        let ptr = std::mem::size_of::<*mut c_void>();
        // size, private_members, then 27 (Emacs 25) + 1 (26) + 5 (27) + 5 (28) members.
        assert_eq!(std::mem::size_of::<emacs_env>(), ptr * (2 + 27 + 1 + 5 + 5));
        assert_eq!(std::mem::size_of::<emacs_runtime>(), ptr * 3);
    }
}
````

- [ ] **Step 3: Write the failing tests for the wrapper**

Create `emrs/src/ffi/fake.rs` with only its tests, and an empty `emrs/src/ffi/env.rs`:

````rust
//! A minimal in-process stand-in for Emacs, for unit tests.

#[cfg(test)]
mod tests {
    use super::Fake;

    #[test]
    fn read_print_round_trip() {
        let mut fake = Fake::new();
        for text in [
            "nil",
            "42",
            "-7",
            "1.5",
            "\"a \\\"q\\\" b\"",
            ":key",
            "(1 2 3)",
            "(a . b)",
            "(:x 1 :y (2 3))",
            "[1 \"two\" three]",
            "#b(0 127 255)",
            "#h(\"a\" 1 \"b\" 2)",
        ] {
            let value = fake.read(text);
            assert_eq!(fake.print(value), text);
        }
    }

    #[test]
    fn env_calls_work_against_the_fake() {
        let mut fake = Fake::new();
        let env = fake.env();
        let one = env.make_i64(1).unwrap();
        let s = env.make_string("hé").unwrap();
        let list = env.list(&[one, s]).unwrap();
        assert_eq!(env.type_name(list).unwrap(), "cons");
        assert_eq!(env.extract_i64(env.car(list).unwrap()).unwrap(), 1);
        assert_eq!(env.string(env.car(env.cdr(list).unwrap()).unwrap()).unwrap(), "hé");
        let big = env.make_big(true, u64::MAX as u128 + 5).unwrap();
        assert_eq!(env.extract_big(big).unwrap(), Some((true, u64::MAX as u128 + 5)));
        assert_eq!(fake.print(list), "(1 \"hé\")");
    }

    #[test]
    fn errors_become_pending_exits() {
        let mut fake = Fake::new();
        let env = fake.env();
        let s = env.make_string("x").unwrap();
        assert!(env.extract_i64(s).is_err());
        assert!(env.intern("still-pending").is_err(), "calls fail while an exit is pending");
        env.clear_exit();
        assert!(env.intern("ok").is_ok());
        assert!(fake.pending().is_none());
    }
}
````

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p emrs ffi`

Expected: compilation fails with `unresolved import super::Fake`.

- [ ] **Step 5: Write the checked wrapper**

`emrs/src/ffi/env.rs`:

````rust
//! Checked wrapper around a live `emacs_env`.
//!
//! An `Env` is valid only during the module function call it was created for
//! and only on the calling thread. `Env` and `Value` hold raw pointers and are
//! therefore neither `Send` nor `Sync`.

use super::sys;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};

/// A non-local exit (signal or throw) is pending in the environment. The
/// caller must return to Emacs without making further API calls.
#[derive(Debug)]
pub(crate) struct Exit;

pub(crate) type Res<T> = Result<T, Exit>;

#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
pub(crate) struct Value(pub(crate) sys::emacs_value);

pub(crate) struct Env {
    raw: *mut sys::emacs_env,
}

/// Number of limbs needed for a 128-bit magnitude.
const MAX_LIMBS: usize = 16 / std::mem::size_of::<sys::emacs_limb_t>();

macro_rules! raw {
    ($self:ident . $f:ident ( $($arg:expr),* )) => {
        unsafe { ((*$self.raw).$f)($self.raw $(, $arg)*) }
    };
}

impl Env {
    /// # Safety
    /// `raw` must be the environment pointer Emacs passed to the module
    /// function that is currently executing on this thread.
    pub(crate) unsafe fn from_raw(raw: *mut sys::emacs_env) -> Env {
        Env { raw }
    }

    fn check(&self) -> Res<()> {
        if raw!(self.non_local_exit_check()) == sys::FUNCALL_EXIT_RETURN { Ok(()) } else { Err(Exit) }
    }

    fn checked(&self, value: sys::emacs_value) -> Res<Value> {
        self.check().map(|()| Value(value))
    }

    pub(crate) fn intern(&self, name: &str) -> Res<Value> {
        let name = CString::new(name).expect("symbol name contains NUL");
        self.checked(raw!(self.intern(name.as_ptr())))
    }

    pub(crate) fn nil(&self) -> Res<Value> {
        self.intern("nil")
    }

    pub(crate) fn t(&self) -> Res<Value> {
        self.intern("t")
    }

    pub(crate) fn funcall(&self, function: Value, args: &[Value]) -> Res<Value> {
        let args_ptr = args.as_ptr() as *mut sys::emacs_value;
        self.checked(raw!(self.funcall(function.0, args.len() as isize, args_ptr)))
    }

    /// Call the Lisp function named `name`.
    pub(crate) fn call(&self, name: &str, args: &[Value]) -> Res<Value> {
        let function = self.intern(name)?;
        self.funcall(function, args)
    }

    pub(crate) fn is_not_nil(&self, value: Value) -> bool {
        raw!(self.is_not_nil(value.0))
    }

    /// Name of the symbol `type-of` returns for `value`, e.g. `"integer"`.
    pub(crate) fn type_name(&self, value: Value) -> Res<String> {
        let symbol = self.checked(raw!(self.type_of(value.0)))?;
        self.symbol_name(symbol)
    }

    pub(crate) fn symbol_name(&self, symbol: Value) -> Res<String> {
        let name = self.call("symbol-name", &[symbol])?;
        self.string(name)
    }

    pub(crate) fn make_i64(&self, n: i64) -> Res<Value> {
        self.checked(raw!(self.make_integer(n)))
    }

    pub(crate) fn extract_i64(&self, value: Value) -> Res<i64> {
        let n = raw!(self.extract_integer(value.0));
        self.check().map(|()| n)
    }

    /// Make an integer of any size up to 128 bits from sign and magnitude.
    pub(crate) fn make_big(&self, negative: bool, magnitude: u128) -> Res<Value> {
        if magnitude <= i64::MAX as u128 {
            let n = magnitude as i64;
            return self.make_i64(if negative { -n } else { n });
        }
        let limb_bits = 8 * std::mem::size_of::<sys::emacs_limb_t>();
        let mut limbs = [0 as sys::emacs_limb_t; MAX_LIMBS];
        for (i, limb) in limbs.iter_mut().enumerate() {
            *limb = (magnitude >> (i * limb_bits)) as sys::emacs_limb_t;
        }
        let sign: c_int = if negative { -1 } else { 1 };
        self.checked(raw!(self.make_big_integer(sign, MAX_LIMBS as isize, limbs.as_ptr())))
    }

    /// Sign and magnitude of an integer. `Ok(None)` if it needs more than
    /// 128 bits. Signals a Lisp error if `value` is not an integer.
    pub(crate) fn extract_big(&self, value: Value) -> Res<Option<(bool, u128)>> {
        let mut sign: c_int = 0;
        let mut count: isize = 0;
        raw!(self.extract_big_integer(value.0, &mut sign, &mut count, std::ptr::null_mut()));
        self.check()?;
        if sign == 0 {
            return Ok(Some((false, 0)));
        }
        if count as usize > MAX_LIMBS {
            return Ok(None);
        }
        let mut limbs = [0 as sys::emacs_limb_t; MAX_LIMBS];
        let mut count = MAX_LIMBS as isize;
        raw!(self.extract_big_integer(value.0, &mut sign, &mut count, limbs.as_mut_ptr()));
        self.check()?;
        let limb_bits = 8 * std::mem::size_of::<sys::emacs_limb_t>();
        let mut magnitude: u128 = 0;
        for (i, limb) in limbs.iter().enumerate().take(count as usize) {
            magnitude |= (*limb as u128) << (i * limb_bits);
        }
        Ok(Some((sign < 0, magnitude)))
    }

    pub(crate) fn make_float(&self, d: f64) -> Res<Value> {
        self.checked(raw!(self.make_float(d)))
    }

    pub(crate) fn extract_float(&self, value: Value) -> Res<f64> {
        let d = raw!(self.extract_float(value.0));
        self.check().map(|()| d)
    }

    /// Make a multibyte string from UTF-8 text.
    pub(crate) fn make_string(&self, s: &str) -> Res<Value> {
        self.checked(raw!(self.make_string(s.as_ptr() as *const c_char, s.len() as isize)))
    }

    /// Make a unibyte string holding exactly `bytes`.
    pub(crate) fn make_unibyte(&self, bytes: &[u8]) -> Res<Value> {
        self.checked(raw!(self.make_unibyte_string(bytes.as_ptr() as *const c_char, bytes.len() as isize)))
    }

    /// UTF-8 contents of a Lisp string, as Emacs encodes them.
    pub(crate) fn string_bytes(&self, value: Value) -> Res<Vec<u8>> {
        let mut len: isize = 0;
        raw!(self.copy_string_contents(value.0, std::ptr::null_mut(), &mut len));
        self.check()?;
        let mut buf = vec![0u8; len as usize];
        raw!(self.copy_string_contents(value.0, buf.as_mut_ptr() as *mut c_char, &mut len));
        self.check()?;
        buf.truncate((len as usize).saturating_sub(1)); // drop the trailing NUL
        Ok(buf)
    }

    pub(crate) fn string(&self, value: Value) -> Res<String> {
        let bytes = self.string_bytes(value)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub(crate) fn vec_size(&self, vector: Value) -> Res<isize> {
        let n = raw!(self.vec_size(vector.0));
        self.check().map(|()| n)
    }

    pub(crate) fn vec_get(&self, vector: Value, index: isize) -> Res<Value> {
        self.checked(raw!(self.vec_get(vector.0, index)))
    }

    pub(crate) fn cons(&self, car: Value, cdr: Value) -> Res<Value> {
        self.call("cons", &[car, cdr])
    }

    pub(crate) fn list(&self, items: &[Value]) -> Res<Value> {
        self.call("list", items)
    }

    pub(crate) fn car(&self, cons: Value) -> Res<Value> {
        self.call("car", &[cons])
    }

    pub(crate) fn cdr(&self, cons: Value) -> Res<Value> {
        self.call("cdr", &[cons])
    }

    pub(crate) fn make_function(
        &self,
        min_arity: isize,
        max_arity: isize,
        function: sys::emacs_function,
        data: *mut c_void,
    ) -> Res<Value> {
        self.checked(raw!(self.make_function(min_arity, max_arity, function, std::ptr::null(), data)))
    }

    /// Open a channel to a pipe process; the descriptor is writable from any
    /// thread.
    pub(crate) fn open_channel(&self, pipe_process: Value) -> Res<c_int> {
        let fd = raw!(self.open_channel(pipe_process.0));
        self.check().map(|()| fd)
    }

    /// Leave a pending `(signal SYMBOL DATA)` in the environment.
    pub(crate) fn signal(&self, symbol: Value, data: Value) {
        raw!(self.non_local_exit_signal(symbol.0, data.0));
    }

    /// Discard a pending non-local exit.
    pub(crate) fn clear_exit(&self) {
        raw!(self.non_local_exit_clear());
    }
}
````

`extract_big` is called twice by design: the first call with a null magnitude asks for the limb count. Emacs returns early for zero without writing the count, which is why `sign == 0` is handled before the count is used.

- [ ] **Step 6: Write the fake Emacs**

Put this above the tests module in `emrs/src/ffi/fake.rs`, replacing the one-line doc comment:

````rust
//! A minimal in-process stand-in for Emacs, for unit tests.
//!
//! `Fake` owns a tiny Lisp heap and an `emacs_env` whose function pointers
//! operate on it, so the serializer and deserializer run under `cargo test`
//! without Emacs. `read` and `print` use Lisp syntax plus two extensions:
//! `#b(1 2 255)` is a unibyte string and `#h(k v ...)` is a hash table.

use super::env::{Env, Value};
use super::sys::{self, emacs_env, emacs_value};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};

#[derive(Clone, Debug, PartialEq)]
enum Obj {
    Sym(String),
    Int { neg: bool, mag: u128 },
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Cons(usize, usize),
    Vector(Vec<usize>),
    Hash(Vec<(usize, usize)>),
}

#[derive(Default)]
struct Heap {
    objs: Vec<Obj>,
    symbols: HashMap<String, usize>,
    pending: Option<(usize, usize)>,
}

impl Heap {
    fn alloc(&mut self, obj: Obj) -> usize {
        self.objs.push(obj);
        self.objs.len() - 1
    }

    fn sym(&mut self, name: &str) -> usize {
        if let Some(&i) = self.symbols.get(name) {
            return i;
        }
        let i = self.alloc(Obj::Sym(name.to_string()));
        self.symbols.insert(name.to_string(), i);
        i
    }

    fn int(&mut self, n: i128) -> usize {
        self.alloc(Obj::Int { neg: n < 0, mag: n.unsigned_abs() })
    }

    fn is_nil(&self, i: usize) -> bool {
        self.objs[i] == Obj::Sym("nil".into())
    }

    fn list(&mut self, items: &[usize], tail: Option<usize>) -> usize {
        let mut list = tail.unwrap_or_else(|| self.sym("nil"));
        for &item in items.iter().rev() {
            list = self.alloc(Obj::Cons(item, list));
        }
        list
    }

    fn signal(&mut self, symbol: &str, data: usize) {
        let symbol = self.sym(symbol);
        self.pending = Some((symbol, data));
    }

    fn print(&self, i: usize, out: &mut String) {
        match &self.objs[i] {
            Obj::Sym(name) => out.push_str(name),
            Obj::Int { neg, mag } => {
                if *neg {
                    out.push('-');
                }
                out.push_str(&mag.to_string());
            }
            Obj::Float(d) => out.push_str(&format!("{d:?}")),
            Obj::Str(s) => {
                out.push('"');
                for c in s.chars() {
                    if c == '"' || c == '\\' {
                        out.push('\\');
                    }
                    out.push(c);
                }
                out.push('"');
            }
            Obj::Bytes(bytes) => {
                let parts: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
                out.push_str(&format!("#b({})", parts.join(" ")));
            }
            Obj::Cons(..) => {
                out.push('(');
                let mut cur = i;
                let mut first = true;
                loop {
                    match &self.objs[cur] {
                        Obj::Cons(car, cdr) => {
                            if !first {
                                out.push(' ');
                            }
                            first = false;
                            self.print(*car, out);
                            cur = *cdr;
                        }
                        _ if self.is_nil(cur) => break,
                        _ => {
                            out.push_str(" . ");
                            self.print(cur, out);
                            break;
                        }
                    }
                }
                out.push(')');
            }
            Obj::Vector(items) => {
                out.push('[');
                for (n, item) in items.iter().enumerate() {
                    if n > 0 {
                        out.push(' ');
                    }
                    self.print(*item, out);
                }
                out.push(']');
            }
            Obj::Hash(pairs) => {
                out.push_str("#h(");
                for (n, (k, v)) in pairs.iter().enumerate() {
                    if n > 0 {
                        out.push(' ');
                    }
                    self.print(*k, out);
                    out.push(' ');
                    self.print(*v, out);
                }
                out.push(')');
            }
        }
    }
}

struct Reader<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    heap: &'a mut Heap,
}

impl Reader<'_> {
    fn skip_ws(&mut self) {
        while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
            self.chars.next();
        }
    }

    fn items_until(&mut self, close: char) -> (Vec<usize>, Option<usize>) {
        let mut items = Vec::new();
        let mut tail = None;
        loop {
            self.skip_ws();
            match self.chars.peek() {
                Some(&c) if c == close => {
                    self.chars.next();
                    return (items, tail);
                }
                None => panic!("fake reader: missing {close}"),
                _ => {}
            }
            let item = self.read();
            if matches!(&self.heap.objs[item], Obj::Sym(s) if s == ".") {
                tail = Some(self.read());
            } else {
                items.push(item);
            }
        }
    }

    fn read(&mut self) -> usize {
        self.skip_ws();
        match self.chars.next().expect("fake reader: unexpected end") {
            '(' => {
                let (items, tail) = self.items_until(')');
                self.heap.list(&items, tail)
            }
            '[' => {
                let (items, _) = self.items_until(']');
                self.heap.alloc(Obj::Vector(items))
            }
            '"' => {
                let mut s = String::new();
                loop {
                    match self.chars.next().expect("fake reader: unterminated string") {
                        '"' => break,
                        '\\' => s.push(self.chars.next().unwrap()),
                        c => s.push(c),
                    }
                }
                self.heap.alloc(Obj::Str(s))
            }
            '#' => {
                let kind = self.chars.next().unwrap();
                assert_eq!(self.chars.next(), Some('('), "fake reader: expected ( after #{kind}");
                let (items, _) = self.items_until(')');
                match kind {
                    'b' => {
                        let bytes = items
                            .iter()
                            .map(|&i| match self.heap.objs[i] {
                                Obj::Int { mag, .. } => mag as u8,
                                _ => panic!("fake reader: #b takes integers"),
                            })
                            .collect();
                        self.heap.alloc(Obj::Bytes(bytes))
                    }
                    'h' => {
                        let pairs = items.chunks(2).map(|kv| (kv[0], kv[1])).collect();
                        self.heap.alloc(Obj::Hash(pairs))
                    }
                    other => panic!("fake reader: unknown #{other}"),
                }
            }
            '?' => {
                let c = self.chars.next().unwrap();
                self.heap.int(c as i128)
            }
            first => {
                let mut token = String::from(first);
                while let Some(&c) = self.chars.peek() {
                    if c.is_whitespace() || "()[]\"".contains(c) {
                        break;
                    }
                    token.push(c);
                    self.chars.next();
                }
                let digits = token.strip_prefix('-').unwrap_or(&token);
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    let mag: u128 = digits.parse().expect("fake reader: integer too large");
                    self.heap.alloc(Obj::Int { neg: token.starts_with('-') && mag != 0, mag })
                } else if let (true, Ok(d)) = (digits.starts_with(|c: char| c.is_ascii_digit()), token.parse::<f64>()) {
                    self.heap.alloc(Obj::Float(d))
                } else {
                    self.heap.sym(&token)
                }
            }
        }
    }
}

/// A fake Emacs. Create one per test.
pub(crate) struct Fake {
    heap: Box<Heap>,
    env: Box<emacs_env>,
}

impl Fake {
    pub(crate) fn new() -> Fake {
        let mut heap = Box::new(Heap::default());
        heap.sym("nil");
        heap.sym("t");
        let env = Box::new(emacs_env {
            size: std::mem::size_of::<emacs_env>() as isize,
            private_members: &mut *heap as *mut Heap as *mut c_void,
            make_global_ref: None,
            free_global_ref: None,
            non_local_exit_check,
            non_local_exit_clear,
            non_local_exit_get: None,
            non_local_exit_signal,
            non_local_exit_throw: None,
            make_function,
            funcall,
            intern,
            type_of,
            is_not_nil,
            eq,
            extract_integer,
            make_integer,
            extract_float,
            make_float,
            copy_string_contents,
            make_string,
            make_user_ptr: None,
            get_user_ptr: None,
            set_user_ptr: None,
            get_user_finalizer: None,
            set_user_finalizer: None,
            vec_get,
            vec_set: None,
            vec_size,
            should_quit: None,
            process_input: None,
            extract_time: None,
            make_time: None,
            extract_big_integer,
            make_big_integer,
            get_function_finalizer: None,
            set_function_finalizer: None,
            open_channel,
            make_interactive: None,
            make_unibyte_string,
        });
        Fake { heap, env }
    }

    pub(crate) fn env(&mut self) -> Env {
        unsafe { Env::from_raw(&mut *self.env) }
    }

    /// Parse one Lisp expression into the fake heap.
    pub(crate) fn read(&mut self, text: &str) -> Value {
        let index = Reader { chars: text.chars().peekable(), heap: &mut self.heap }.read();
        Value(to_value(index))
    }

    /// Print a value in Lisp syntax.
    pub(crate) fn print(&self, value: Value) -> String {
        let mut out = String::new();
        self.heap.print(to_index(value.0), &mut out);
        out
    }

    /// The pending signal as `(SYMBOL . DATA)`, if any.
    pub(crate) fn pending(&self) -> Option<String> {
        self.heap.pending.map(|(symbol, data)| {
            let mut out = String::from("(");
            self.heap.print(symbol, &mut out);
            out.push_str(" . ");
            self.heap.print(data, &mut out);
            out.push(')');
            out
        })
    }
}

fn to_value(index: usize) -> emacs_value {
    (index + 1) as emacs_value
}

fn to_index(value: emacs_value) -> usize {
    value as usize - 1
}

unsafe fn heap<'a>(env: *mut emacs_env) -> &'a mut Heap {
    unsafe { &mut *((*env).private_members as *mut Heap) }
}

unsafe extern "C" fn non_local_exit_check(env: *mut emacs_env) -> c_int {
    unsafe { heap(env).pending.is_some() as c_int }
}

unsafe extern "C" fn non_local_exit_clear(env: *mut emacs_env) {
    unsafe { heap(env).pending = None }
}

unsafe extern "C" fn non_local_exit_signal(env: *mut emacs_env, symbol: emacs_value, data: emacs_value) {
    unsafe { heap(env).pending = Some((to_index(symbol), to_index(data))) }
}

unsafe extern "C" fn make_function(
    env: *mut emacs_env,
    _min: isize,
    _max: isize,
    _func: sys::emacs_function,
    _doc: *const c_char,
    _data: *mut c_void,
) -> emacs_value {
    unsafe { to_value(heap(env).sym("fake-function")) }
}

unsafe extern "C" fn funcall(
    env: *mut emacs_env,
    func: emacs_value,
    nargs: isize,
    args: *mut emacs_value,
) -> emacs_value {
    let h = unsafe { heap(env) };
    let args: Vec<usize> = (0..nargs as usize).map(|i| to_index(unsafe { *args.add(i) })).collect();
    let name = match &h.objs[to_index(func)] {
        Obj::Sym(name) => name.clone(),
        _ => String::new(),
    };
    let nil = h.sym("nil");
    let result = match name.as_str() {
        "list" => h.list(&args, None),
        "vector" => h.alloc(Obj::Vector(args)),
        "cons" => h.alloc(Obj::Cons(args[0], args[1])),
        "car" | "cdr" => match h.objs[args[0]] {
            Obj::Cons(car, cdr) => {
                if name == "car" {
                    car
                } else {
                    cdr
                }
            }
            _ if h.is_nil(args[0]) => nil,
            _ => {
                h.signal("wrong-type-argument", args[0]);
                nil
            }
        },
        "proper-list-p" => {
            let mut cur = args[0];
            let mut len = 0;
            while let Obj::Cons(_, cdr) = h.objs[cur] {
                cur = cdr;
                len += 1;
            }
            if h.is_nil(cur) { h.int(len) } else { nil }
        }
        "symbol-name" => match h.objs[args[0]].clone() {
            Obj::Sym(s) => h.alloc(Obj::Str(s)),
            _ => {
                h.signal("wrong-type-argument", args[0]);
                nil
            }
        },
        "emrs--hash-to-alist" => match h.objs[args[0]].clone() {
            Obj::Hash(pairs) => {
                let conses: Vec<usize> = pairs.iter().map(|&(k, v)| h.alloc(Obj::Cons(k, v))).collect();
                h.list(&conses, None)
            }
            _ => {
                h.signal("wrong-type-argument", args[0]);
                nil
            }
        },
        _ => {
            h.signal("void-function", to_index(func));
            nil
        }
    };
    to_value(result)
}

unsafe extern "C" fn intern(env: *mut emacs_env, name: *const c_char) -> emacs_value {
    let name = unsafe { CStr::from_ptr(name) }.to_str().unwrap();
    unsafe { to_value(heap(env).sym(name)) }
}

unsafe extern "C" fn type_of(env: *mut emacs_env, arg: emacs_value) -> emacs_value {
    let h = unsafe { heap(env) };
    let name = match &h.objs[to_index(arg)] {
        Obj::Sym(_) => "symbol",
        Obj::Int { .. } => "integer",
        Obj::Float(_) => "float",
        Obj::Str(_) | Obj::Bytes(_) => "string",
        Obj::Cons(..) => "cons",
        Obj::Vector(_) => "vector",
        Obj::Hash(_) => "hash-table",
    };
    to_value(h.sym(name))
}

unsafe extern "C" fn is_not_nil(env: *mut emacs_env, arg: emacs_value) -> bool {
    unsafe { !heap(env).is_nil(to_index(arg)) }
}

unsafe extern "C" fn eq(env: *mut emacs_env, a: emacs_value, b: emacs_value) -> bool {
    let h = unsafe { heap(env) };
    let (a, b) = (to_index(a), to_index(b));
    a == b || matches!((&h.objs[a], &h.objs[b]), (Obj::Int { .. }, Obj::Int { .. }) if h.objs[a] == h.objs[b])
}

unsafe extern "C" fn extract_integer(env: *mut emacs_env, arg: emacs_value) -> i64 {
    let h = unsafe { heap(env) };
    match h.objs[to_index(arg)] {
        Obj::Int { neg, mag } if mag <= i64::MAX as u128 => {
            if neg {
                -(mag as i64)
            } else {
                mag as i64
            }
        }
        Obj::Int { .. } => {
            h.signal("overflow-error", to_index(arg));
            0
        }
        _ => {
            h.signal("wrong-type-argument", to_index(arg));
            0
        }
    }
}

unsafe extern "C" fn make_integer(env: *mut emacs_env, n: i64) -> emacs_value {
    unsafe { to_value(heap(env).int(n as i128)) }
}

unsafe extern "C" fn extract_float(env: *mut emacs_env, arg: emacs_value) -> f64 {
    let h = unsafe { heap(env) };
    match h.objs[to_index(arg)] {
        Obj::Float(d) => d,
        _ => {
            h.signal("wrong-type-argument", to_index(arg));
            0.0
        }
    }
}

unsafe extern "C" fn make_float(env: *mut emacs_env, d: f64) -> emacs_value {
    unsafe { to_value(heap(env).alloc(Obj::Float(d))) }
}

unsafe extern "C" fn copy_string_contents(
    env: *mut emacs_env,
    value: emacs_value,
    buf: *mut c_char,
    len: *mut isize,
) -> bool {
    let h = unsafe { heap(env) };
    let bytes = match &h.objs[to_index(value)] {
        Obj::Str(s) => s.as_bytes().to_vec(),
        Obj::Bytes(b) => b.clone(),
        _ => {
            h.signal("wrong-type-argument", to_index(value));
            return false;
        }
    };
    let needed = bytes.len() as isize + 1;
    unsafe {
        if buf.is_null() {
            *len = needed;
            return true;
        }
        if *len < needed {
            *len = needed;
            h.signal("args-out-of-range", to_index(value));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, bytes.len());
        *buf.add(bytes.len()) = 0;
        *len = needed;
    }
    true
}

unsafe extern "C" fn make_string(env: *mut emacs_env, s: *const c_char, len: isize) -> emacs_value {
    let bytes = unsafe { std::slice::from_raw_parts(s as *const u8, len as usize) };
    let s = String::from_utf8(bytes.to_vec()).unwrap();
    unsafe { to_value(heap(env).alloc(Obj::Str(s))) }
}

unsafe extern "C" fn make_unibyte_string(env: *mut emacs_env, s: *const c_char, len: isize) -> emacs_value {
    let bytes = unsafe { std::slice::from_raw_parts(s as *const u8, len as usize) };
    unsafe { to_value(heap(env).alloc(Obj::Bytes(bytes.to_vec()))) }
}

unsafe extern "C" fn vec_get(env: *mut emacs_env, vector: emacs_value, index: isize) -> emacs_value {
    let h = unsafe { heap(env) };
    match &h.objs[to_index(vector)] {
        Obj::Vector(items) if (index as usize) < items.len() => to_value(items[index as usize]),
        _ => {
            h.signal("args-out-of-range", to_index(vector));
            to_value(h.sym("nil"))
        }
    }
}

unsafe extern "C" fn vec_size(env: *mut emacs_env, vector: emacs_value) -> isize {
    let h = unsafe { heap(env) };
    match &h.objs[to_index(vector)] {
        Obj::Vector(items) => items.len() as isize,
        _ => {
            h.signal("wrong-type-argument", to_index(vector));
            0
        }
    }
}

unsafe extern "C" fn extract_big_integer(
    env: *mut emacs_env,
    arg: emacs_value,
    sign: *mut c_int,
    count: *mut isize,
    magnitude: *mut sys::emacs_limb_t,
) -> bool {
    let h = unsafe { heap(env) };
    let Obj::Int { neg, mag } = h.objs[to_index(arg)] else {
        h.signal("wrong-type-argument", to_index(arg));
        return false;
    };
    let limb_bits = 8 * std::mem::size_of::<sys::emacs_limb_t>();
    unsafe {
        *sign = if mag == 0 {
            0
        } else if neg {
            -1
        } else {
            1
        };
        if mag == 0 {
            return true;
        }
        let needed = (128 - mag.leading_zeros() as usize).div_ceil(limb_bits) as isize;
        if magnitude.is_null() {
            *count = needed;
            return true;
        }
        if *count < needed {
            h.signal("args-out-of-range", to_index(arg));
            return false;
        }
        for i in 0..needed as usize {
            *magnitude.add(i) = (mag >> (i * limb_bits)) as sys::emacs_limb_t;
        }
        *count = needed;
    }
    true
}

unsafe extern "C" fn make_big_integer(
    env: *mut emacs_env,
    sign: c_int,
    count: isize,
    magnitude: *const sys::emacs_limb_t,
) -> emacs_value {
    let limb_bits = 8 * std::mem::size_of::<sys::emacs_limb_t>();
    let mut mag: u128 = 0;
    for i in 0..count as usize {
        mag |= (unsafe { *magnitude.add(i) } as u128) << (i * limb_bits);
    }
    unsafe { to_value(heap(env).alloc(Obj::Int { neg: sign < 0 && mag != 0, mag })) }
}

unsafe extern "C" fn open_channel(_env: *mut emacs_env, _pipe: emacs_value) -> c_int {
    -1
}
````

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p emrs`

Expected: `test result: ok. 18 passed`.

- [ ] **Step 8: Commit**

````bash
git add emrs/src
git commit -m "feat: private FFI wrapper and fake Emacs for unit tests"
````

---

### Task 5: Serializer (Rust to Lisp)

**Files:**
- Create: `emrs/src/convert/mod.rs`, `emrs/src/convert/ser.rs` (tests inside)
- Modify: `emrs/src/lib.rs`

**Interfaces:**
- Consumes: `ffi::env::{Env, Exit, Value}`, `ffi::fake::Fake` (tests), `names::to_kebab`.
- Produces: `pub(crate) fn convert::ser::to_lisp<T: Serialize + ?Sized>(env: &Env, value: &T) -> Result<Value, SerError>`; `pub(crate) enum SerError { Exit, Msg(String) }` implementing `serde::ser::Error`, `Display`, `From<Exit>`.

- [ ] **Step 1: Register the module**

`emrs/src/convert/mod.rs`:

````rust
//! Direct conversion between `serde` data and Lisp values.

pub(crate) mod ser;
````

`emrs/src/lib.rs` (whole file):

````rust
//! Message-passing services for Emacs dynamic modules.
#![allow(dead_code)] // Removed in Task 9, when every item has a user.

mod convert;
mod ffi;
mod names;
mod runtime;

pub use runtime::cancel::CancelToken;
````

- [ ] **Step 2: Write the failing tests**

Create `emrs/src/convert/ser.rs` with only the tests. They state the mapping of spec section 8.2 in Lisp syntax.

````rust
//! `serde::Serializer` that builds Lisp values directly.

#[cfg(test)]
mod tests {
    use super::to_lisp;
    use crate::ffi::fake::Fake;
    use serde::Serialize;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn ser<T: Serialize>(value: &T) -> String {
        let mut fake = Fake::new();
        let env = fake.env();
        let lisp = to_lisp(&env, value).expect("serialization failed");
        fake.print(lisp)
    }

    #[derive(Serialize)]
    struct Match {
        path: PathBuf,
        line_number: u64,
        preview: Option<String>,
    }

    #[derive(Serialize)]
    struct Unit;

    #[derive(Serialize)]
    struct Meters(f64);

    #[derive(Serialize)]
    struct Pair(i32, String);

    #[derive(Serialize)]
    enum Event {
        Ready,
        Result(String),
        Pair(i32, i32),
        SearchStarted { id: u64, case_fold: bool },
        Found(Match),
    }

    #[test]
    fn scalars() {
        assert_eq!(ser(&true), "t");
        assert_eq!(ser(&false), "nil");
        assert_eq!(ser(&-5i8), "-5");
        assert_eq!(ser(&70000u32), "70000");
        assert_eq!(ser(&1.5f64), "1.5");
        assert_eq!(ser(&'a'), "97");
        assert_eq!(ser(&"hé \"x\""), "\"hé \\\"x\\\"\"");
        assert_eq!(ser(&()), "nil");
    }

    #[test]
    fn large_integers_become_bignums() {
        assert_eq!(ser(&u64::MAX), "18446744073709551615");
        assert_eq!(ser(&i64::MIN), "-9223372036854775808");
        assert_eq!(ser(&u128::MAX), "340282366920938463463374607431768211455");
        assert_eq!(ser(&i128::MIN), "-170141183460469231731687303715884105728");
    }

    #[test]
    fn options_and_unit_types() {
        assert_eq!(ser(&None::<i32>), "nil");
        assert_eq!(ser(&Some(3)), "3");
        assert_eq!(ser(&Unit), "nil");
        assert_eq!(ser(&Meters(2.5)), "2.5");
    }

    #[test]
    fn sequences_and_tuples_are_lists() {
        assert_eq!(ser(&vec![1, 2, 3]), "(1 2 3)");
        assert_eq!(ser(&Vec::<i32>::new()), "nil");
        assert_eq!(ser(&(1, "a")), "(1 \"a\")");
        assert_eq!(ser(&Pair(1, "a".into())), "(1 \"a\")");
    }

    #[test]
    fn bytes_need_serde_bytes_style_serialization() {
        struct Raw(Vec<u8>);
        impl Serialize for Raw {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_bytes(&self.0)
            }
        }
        assert_eq!(ser(&Raw(vec![0, 200, 255])), "#b(0 200 255)");
        assert_eq!(ser(&vec![0u8, 200]), "(0 200)", "plain Vec<u8> is a list");
    }

    #[test]
    fn maps_are_alists() {
        let mut map = BTreeMap::new();
        map.insert("a".to_string(), 1);
        map.insert("b".to_string(), 2);
        assert_eq!(ser(&map), "((\"a\" . 1) (\"b\" . 2))");
        assert_eq!(ser(&BTreeMap::<String, i32>::new()), "nil");
    }

    #[test]
    fn structs_are_plists_with_kebab_keywords() {
        let m = Match { path: "/a.rs".into(), line_number: 42, preview: None };
        assert_eq!(ser(&m), "(:path \"/a.rs\" :line-number 42 :preview nil)");
    }

    #[test]
    fn enum_variants_are_keyword_headed_lists() {
        assert_eq!(ser(&Event::Ready), "(:ready)");
        assert_eq!(ser(&Event::Result("x".into())), "(:result \"x\")");
        assert_eq!(ser(&Event::Pair(1, 2)), "(:pair 1 2)");
        assert_eq!(ser(&Event::SearchStarted { id: 7, case_fold: true }), "(:search-started :id 7 :case-fold t)");
        let m = Match { path: "/a".into(), line_number: 1, preview: Some("p".into()) };
        assert_eq!(ser(&Event::Found(m)), "(:found (:path \"/a\" :line-number 1 :preview \"p\"))");
    }

    #[test]
    fn custom_serialize_errors_are_reported() {
        struct Bad;
        impl Serialize for Bad {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("cannot encode"))
            }
        }
        let mut fake = Fake::new();
        let env = fake.env();
        let err = to_lisp(&env, &vec![Bad]).unwrap_err();
        assert_eq!(err.to_string(), "cannot encode");
    }
}
````

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p emrs ser`

Expected: compilation fails with `unresolved import super::to_lisp`.

- [ ] **Step 4: Write the implementation**

Put this above the tests module, replacing the one-line doc comment:

````rust
//! `serde::Serializer` that builds Lisp values directly.

use crate::ffi::env::{Env, Exit, Value};
use crate::names::to_kebab;
use serde::ser::{self, Serialize};
use std::fmt;

#[derive(Debug)]
pub(crate) enum SerError {
    /// A Lisp non-local exit is pending.
    Exit,
    Msg(String),
}

impl fmt::Display for SerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SerError::Exit => f.write_str("Lisp non-local exit"),
            SerError::Msg(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for SerError {}

impl ser::Error for SerError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        SerError::Msg(msg.to_string())
    }
}

impl From<Exit> for SerError {
    fn from(_: Exit) -> Self {
        SerError::Exit
    }
}

type Result<T> = std::result::Result<T, SerError>;

/// Convert `value` to a Lisp value. Must run on the Emacs thread.
pub(crate) fn to_lisp<T: Serialize + ?Sized>(env: &Env, value: &T) -> Result<Value> {
    value.serialize(Ser { env })
}

fn keyword(env: &Env, name: &str) -> Result<Value> {
    Ok(env.intern(&format!(":{}", to_kebab(name)))?)
}

#[derive(Clone, Copy)]
struct Ser<'e> {
    env: &'e Env,
}

/// Collects elements and ends as a Lisp list. Used for sequences, tuples,
/// structs (keyword/value pairs) and all non-unit enum variants (keyword
/// first).
struct ListSer<'e> {
    env: &'e Env,
    items: Vec<Value>,
}

impl<'e> ListSer<'e> {
    fn new(env: &'e Env, capacity: usize) -> Self {
        ListSer { env, items: Vec::with_capacity(capacity) }
    }

    fn variant(env: &'e Env, variant: &str, capacity: usize) -> Result<Self> {
        let mut list = ListSer::new(env, capacity + 1);
        list.items.push(keyword(env, variant)?);
        Ok(list)
    }

    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.items.push(to_lisp(self.env, value)?);
        Ok(())
    }

    fn field<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) -> Result<()> {
        self.items.push(keyword(self.env, key)?);
        self.push(value)
    }

    fn finish(self) -> Result<Value> {
        Ok(self.env.list(&self.items)?)
    }
}

/// Collects `(KEY . VALUE)` pairs and ends as an alist.
struct MapSer<'e> {
    env: &'e Env,
    pairs: Vec<Value>,
    key: Option<Value>,
}

impl<'e> ser::Serializer for Ser<'e> {
    type Ok = Value;
    type Error = SerError;
    type SerializeSeq = ListSer<'e>;
    type SerializeTuple = ListSer<'e>;
    type SerializeTupleStruct = ListSer<'e>;
    type SerializeTupleVariant = ListSer<'e>;
    type SerializeMap = MapSer<'e>;
    type SerializeStruct = ListSer<'e>;
    type SerializeStructVariant = ListSer<'e>;

    fn serialize_bool(self, v: bool) -> Result<Value> {
        Ok(if v { self.env.t()? } else { self.env.nil()? })
    }

    fn serialize_i8(self, v: i8) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i16(self, v: i16) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i32(self, v: i32) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i64(self, v: i64) -> Result<Value> {
        Ok(self.env.make_i64(v)?)
    }

    fn serialize_i128(self, v: i128) -> Result<Value> {
        Ok(self.env.make_big(v < 0, v.unsigned_abs())?)
    }

    fn serialize_u8(self, v: u8) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u16(self, v: u16) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u32(self, v: u32) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u64(self, v: u64) -> Result<Value> {
        Ok(self.env.make_big(false, v as u128)?)
    }

    fn serialize_u128(self, v: u128) -> Result<Value> {
        Ok(self.env.make_big(false, v)?)
    }

    fn serialize_f32(self, v: f32) -> Result<Value> {
        self.serialize_f64(v as f64)
    }

    fn serialize_f64(self, v: f64) -> Result<Value> {
        Ok(self.env.make_float(v)?)
    }

    fn serialize_char(self, v: char) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_str(self, v: &str) -> Result<Value> {
        Ok(self.env.make_string(v)?)
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Value> {
        Ok(self.env.make_unibyte(v)?)
    }

    fn serialize_none(self) -> Result<Value> {
        self.serialize_unit()
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Value> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Value> {
        Ok(self.env.nil()?)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(self, _name: &'static str, _index: u32, variant: &'static str) -> Result<Value> {
        ListSer::variant(self.env, variant, 0)?.finish()
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _name: &'static str, value: &T) -> Result<Value> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value> {
        let mut list = ListSer::variant(self.env, variant, 1)?;
        list.push(value)?;
        list.finish()
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<ListSer<'e>> {
        Ok(ListSer::new(self.env, len.unwrap_or(0)))
    }

    fn serialize_tuple(self, len: usize) -> Result<ListSer<'e>> {
        Ok(ListSer::new(self.env, len))
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<ListSer<'e>> {
        Ok(ListSer::new(self.env, len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<ListSer<'e>> {
        ListSer::variant(self.env, variant, len)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<MapSer<'e>> {
        Ok(MapSer { env: self.env, pairs: Vec::with_capacity(len.unwrap_or(0)), key: None })
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<ListSer<'e>> {
        Ok(ListSer::new(self.env, len * 2))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<ListSer<'e>> {
        ListSer::variant(self.env, variant, len * 2)
    }
}

impl ser::SerializeSeq for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeTuple for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeTupleStruct for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeTupleVariant for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeStruct for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.field(key, value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.field(key, value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeMap for MapSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        self.key = Some(to_lisp(self.env, key)?);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let key = self.key.take().expect("serialize_value called before serialize_key");
        let value = to_lisp(self.env, value)?;
        self.pairs.push(self.env.cons(key, value)?);
        Ok(())
    }

    fn end(self) -> Result<Value> {
        Ok(self.env.list(&self.pairs)?)
    }
}
````

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p emrs`

Expected: `test result: ok. 27 passed`.

- [ ] **Step 6: Commit**

````bash
git add emrs/src
git commit -m "feat: serde serializer producing Lisp values"
````

---

### Task 6: Deserializer (Lisp to Rust)

**Files:**
- Create: `emrs/src/convert/de.rs` (tests inside)
- Modify: `emrs/src/convert/mod.rs`

**Interfaces:**
- Consumes: `ffi::env::{Env, Exit, Value}`, `ffi::fake::Fake` (tests), `names::to_kebab`.
- Produces: `pub(crate) fn convert::de::from_lisp<T: DeserializeOwned>(env: &Env, value: Value) -> Result<T, DeError>`; `pub(crate) enum DeError { Exit, Msg { msg, path } }` whose `Display` reads `at <path>: <message>`. Depends on the Lisp functions `car`, `cdr`, `symbol-name`, `proper-list-p` and `emrs--hash-to-alist` (the last is defined by `emrs.el` in Task 10).

- [ ] **Step 1: Register the module**

`emrs/src/convert/mod.rs` (whole file):

````rust
//! Direct conversion between `serde` data and Lisp values.

pub(crate) mod de;
pub(crate) mod ser;
````

- [ ] **Step 2: Write the failing tests**

Create `emrs/src/convert/de.rs` with only the tests. They state spec section 8.3, including every error message.

````rust
//! `serde::Deserializer` that reads Lisp values directly.

#[cfg(test)]
mod tests {
    use super::from_lisp;
    use crate::ffi::fake::Fake;
    use serde::Deserialize;
    use serde::de::DeserializeOwned;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn de<T: DeserializeOwned>(text: &str) -> Result<T, String> {
        let mut fake = Fake::new();
        let value = fake.read(text);
        let env = fake.env();
        from_lisp(&env, value).map_err(|e| e.to_string())
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct Match {
        path: PathBuf,
        line_number: u64,
        preview: Option<String>,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Strict {
        a: i32,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    enum Request {
        Stop,
        Search(String),
        Pair(i32, i32),
        Log { rev: String, limit: usize },
        HTTPGet(Match),
    }

    #[test]
    fn scalars() {
        assert_eq!(de::<bool>("t"), Ok(true));
        assert_eq!(de::<bool>("nil"), Ok(false));
        assert_eq!(de::<bool>("0"), Ok(true), "any non-nil value is true");
        assert_eq!(de::<i8>("-5"), Ok(-5));
        assert_eq!(de::<u64>("18446744073709551615"), Ok(u64::MAX));
        assert_eq!(de::<i128>("-170141183460469231731687303715884105728"), Ok(i128::MIN));
        assert_eq!(de::<u128>("340282366920938463463374607431768211455"), Ok(u128::MAX));
        assert_eq!(de::<f64>("1.5"), Ok(1.5));
        assert_eq!(de::<f64>("2"), Ok(2.0), "integers are accepted for floats");
        assert_eq!(de::<String>("\"hé\""), Ok("hé".to_string()));
        assert_eq!(de::<()>("nil"), Ok(()));
    }

    #[test]
    fn integer_range_and_type_errors() {
        assert_eq!(de::<u8>("256"), Err("integer 256 out of range for u8".into()));
        assert_eq!(de::<i8>("-129"), Err("integer -129 out of range for i8".into()));
        assert_eq!(de::<u32>("-1"), Err("expected a non-negative integer".into()));
        assert_eq!(de::<i32>("1.5"), Err("expected integer, got float".into()));
        assert_eq!(de::<i32>("nil"), Err("expected integer, got nil".into()));
        assert_eq!(de::<String>("sym"), Err("expected string, got symbol".into()));
        assert_eq!(de::<()>("1"), Err("expected nil, got integer".into()));
    }

    #[test]
    fn chars() {
        assert_eq!(de::<char>("?a"), Ok('a'));
        assert_eq!(de::<char>("\"é\""), Ok('é'));
        assert_eq!(de::<char>("\"ab\""), Err("expected a string of length 1".into()));
        assert_eq!(de::<char>("55296"), Err("integer is not a valid character".into()));
    }

    #[test]
    fn options() {
        assert_eq!(de::<Option<i32>>("nil"), Ok(None));
        assert_eq!(de::<Option<i32>>("3"), Ok(Some(3)));
    }

    #[test]
    fn bytes_from_unibyte_and_multibyte_strings() {
        #[derive(Debug, PartialEq)]
        struct Raw(Vec<u8>);
        impl<'de> Deserialize<'de> for Raw {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Raw;
                    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                        f.write_str("bytes")
                    }
                    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Raw, E> {
                        Ok(Raw(v))
                    }
                }
                d.deserialize_byte_buf(V)
            }
        }
        assert_eq!(de::<Raw>("#b(0 200 255)"), Ok(Raw(vec![0, 200, 255])));
        assert_eq!(de::<Raw>("\"é\""), Ok(Raw(vec![0xC3, 0xA9])));
    }

    #[test]
    fn sequences_from_lists_and_vectors() {
        assert_eq!(de::<Vec<i32>>("(1 2 3)"), Ok(vec![1, 2, 3]));
        assert_eq!(de::<Vec<i32>>("[1 2 3]"), Ok(vec![1, 2, 3]));
        assert_eq!(de::<Vec<i32>>("nil"), Ok(vec![]));
        assert_eq!(de::<Vec<i32>>("(1 . 2)"), Err("expected a proper list, got a dotted or circular list".into()));
        assert_eq!(de::<Request>("(:pair 1 . 2)"), Err("expected a proper list, got a dotted or circular list".into()));
        assert_eq!(de::<Vec<i32>>("(1 \"x\")"), Err("at [1]: expected integer, got string".into()));
        assert_eq!(de::<Vec<i32>>("5"), Err("expected list or vector, got integer".into()));
    }

    #[test]
    fn tuples_check_their_length() {
        assert_eq!(de::<(i32, String)>("(1 \"a\")"), Ok((1, "a".to_string())));
        assert_eq!(de::<(i32, i32)>("(1 2 3)"), Err("expected 2 elements, got more".into()));
        assert!(de::<(i32, i32)>("(1)").unwrap_err().contains("invalid length 1"));
    }

    #[test]
    fn maps_from_alists_and_hash_tables() {
        let expected: BTreeMap<String, i32> = [("a".to_string(), 1), ("b".to_string(), 2)].into();
        assert_eq!(de::<BTreeMap<String, i32>>("((\"a\" . 1) (\"b\" . 2))"), Ok(expected.clone()));
        assert_eq!(de::<BTreeMap<String, i32>>("#h(\"a\" 1 \"b\" 2)"), Ok(expected));
        assert_eq!(de::<BTreeMap<String, i32>>("nil"), Ok(BTreeMap::new()));
        assert_eq!(de::<BTreeMap<String, i32>>("(1 2)"), Err("alist entry must be a cons".into()));
    }

    #[test]
    fn structs_from_plists() {
        let expected = Match { path: "/a.rs".into(), line_number: 42, preview: None };
        assert_eq!(de::<Match>("(:path \"/a.rs\" :line-number 42 :preview nil)"), Ok(expected));
        let reordered = de::<Match>("(:line-number 1 :path \"/b\")").unwrap();
        assert_eq!(reordered, Match { path: "/b".into(), line_number: 1, preview: None });
    }

    #[test]
    fn struct_errors_name_the_path() {
        assert_eq!(
            de::<Match>("(:path \"/a\" :line-number \"x\")"),
            Err("at :line-number: expected integer, got string".into())
        );
        assert!(de::<Match>("(:path \"/a\")").unwrap_err().contains("missing field `line_number`"));
        assert_eq!(de::<Match>("(:path)"), Err("missing value for :path".into()));
        assert_eq!(de::<Match>("(path \"/a\")"), Err("expected a keyword key in plist".into()));
        assert_eq!(de::<Match>("5"), Err("expected plist, got integer".into()));
        assert_eq!(de::<Vec<Match>>("((:path 1))"), Err("at [0] :path: expected string, got integer".into()));
    }

    #[test]
    fn unknown_keys_follow_serde_attributes() {
        assert!(de::<Match>("(:path \"/a\" :line-number 1 :extra 9)").is_ok(), "ignored by default");
        assert!(de::<Strict>("(:a 1 :extra 9)").unwrap_err().contains("unknown field `extra`"));
    }

    #[test]
    fn enums_from_keyword_lists() {
        assert_eq!(de::<Request>("(:stop)"), Ok(Request::Stop));
        assert_eq!(de::<Request>(":stop"), Ok(Request::Stop), "bare keyword for a unit variant");
        assert_eq!(de::<Request>("(:search \"hello\")"), Ok(Request::Search("hello".into())));
        assert_eq!(de::<Request>("(:pair 1 2)"), Ok(Request::Pair(1, 2)));
        assert_eq!(de::<Request>("(:log :rev \"HEAD\" :limit 20)"), Ok(Request::Log { rev: "HEAD".into(), limit: 20 }));
        assert_eq!(
            de::<Request>("(:http-get (:path \"/a\" :line-number 1))"),
            Ok(Request::HTTPGet(Match { path: "/a".into(), line_number: 1, preview: None }))
        );
    }

    #[test]
    fn enum_errors() {
        assert_eq!(
            de::<Request>("(:nope)"),
            Err("at (:nope): unknown variant, expected one of :stop :search :pair :log :http-get".into())
        );
        assert_eq!(de::<Request>("(:stop 1)"), Err("at (:stop): expected no values".into()));
        assert_eq!(de::<Request>("(:search)"), Err("at (:search): expected exactly one value".into()));
        assert_eq!(de::<Request>("(:search \"a\" \"b\")"), Err("at (:search): expected exactly one value".into()));
        assert_eq!(de::<Request>("(:pair 1 2 3)"), Err("at (:pair): expected 2 elements, got more".into()));
        assert_eq!(de::<Request>("(:log :rev 5 :limit 1)"), Err("at (:log) :rev: expected string, got integer".into()));
        assert_eq!(de::<Request>("(stop)"), Err("expected a keyword at the head of the list".into()));
        assert_eq!(de::<Request>("7"), Err("expected keyword or keyword-headed list, got integer".into()));
    }

    #[test]
    fn conflicting_names_are_rejected() {
        #[derive(Deserialize, Debug)]
        #[allow(non_snake_case, dead_code)]
        struct Clash {
            fooBar: i32,
            foo_bar: i32,
        }
        assert_eq!(de::<Clash>("(:foo-bar 1)").unwrap_err(), "names `fooBar` and `foo_bar` both map to :foo-bar");
    }

    #[test]
    fn nesting_is_limited() {
        #[derive(Deserialize, Debug)]
        struct Nest(#[allow(dead_code)] Vec<Nest>);
        let ok = format!("{}{}", "(".repeat(100), ")".repeat(100));
        assert!(de::<Nest>(&ok).is_ok());
        let deep = format!("{}{}", "(".repeat(200), ")".repeat(200));
        assert_eq!(
            de::<Nest>(&deep).unwrap_err().rsplit(": ").next().unwrap(),
            "value is nested deeper than 128 levels"
        );
    }

    #[test]
    fn self_describing_values() {
        #[derive(Deserialize, Debug, PartialEq, Clone)]
        #[serde(untagged)]
        enum Any {
            Bool(bool),
            Int(i64),
            Float(f64),
            Text(String),
            List(Vec<Any>),
            Map(BTreeMap<String, Any>),
            Unit(()),
        }
        assert_eq!(de::<Any>("nil"), Ok(Any::Unit(())));
        assert_eq!(de::<Any>("t"), Ok(Any::Bool(true)));
        assert_eq!(de::<Any>("-3"), Ok(Any::Int(-3)));
        assert_eq!(de::<Any>("1.5"), Ok(Any::Float(1.5)));
        assert_eq!(de::<Any>("\"s\""), Ok(Any::Text("s".into())));
        assert_eq!(de::<Any>("sym"), Ok(Any::Text("sym".into())));
        assert_eq!(de::<Any>("[1 (2)]"), Ok(Any::List(vec![Any::Int(1), Any::List(vec![Any::Int(2)])])));
        let map: BTreeMap<String, Any> = [("a".to_string(), Any::Int(1))].into();
        assert_eq!(de::<Any>("(:a 1)"), Ok(Any::Map(map.clone())));
        assert_eq!(de::<Any>("#h(\"a\" 1)"), Ok(Any::Map(map)));
    }
}
````

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p emrs de::`

Expected: compilation fails with `unresolved import super::from_lisp`.

- [ ] **Step 4: Write the implementation**

Put this above the tests module, replacing the one-line doc comment:

````rust
//! `serde::Deserializer` that reads Lisp values directly.

use crate::ffi::env::{Env, Exit, Value};
use crate::names::to_kebab;
use serde::de::{self, DeserializeOwned, DeserializeSeed, IntoDeserializer, Visitor};
use std::fmt;

/// Maximum nesting depth of a decoded value.
const MAX_DEPTH: usize = 128;

#[derive(Debug)]
pub(crate) enum DeError {
    /// A Lisp non-local exit is pending.
    Exit,
    /// `path` holds the location of the offending value, innermost first.
    Msg { msg: String, path: Vec<String> },
}

impl DeError {
    fn msg(msg: impl Into<String>) -> DeError {
        DeError::Msg { msg: msg.into(), path: Vec::new() }
    }

    /// Record that the error happened inside `segment`.
    fn at(self, segment: impl Into<String>) -> DeError {
        match self {
            DeError::Exit => DeError::Exit,
            DeError::Msg { msg, mut path } => {
                path.push(segment.into());
                DeError::Msg { msg, path }
            }
        }
    }
}

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeError::Exit => f.write_str("Lisp non-local exit"),
            DeError::Msg { msg, path } if path.is_empty() => f.write_str(msg),
            DeError::Msg { msg, path } => {
                let path: Vec<&str> = path.iter().rev().map(String::as_str).collect();
                write!(f, "at {}: {}", path.join(" "), msg)
            }
        }
    }
}

impl std::error::Error for DeError {}

impl de::Error for DeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        DeError::msg(msg.to_string())
    }
}

impl From<Exit> for DeError {
    fn from(_: Exit) -> Self {
        DeError::Exit
    }
}

type Result<T> = std::result::Result<T, DeError>;

/// Convert a Lisp value to `T`. Must run on the Emacs thread.
pub(crate) fn from_lisp<T: DeserializeOwned>(env: &Env, value: Value) -> Result<T> {
    T::deserialize(De { env, value, depth: 0 })
}

#[derive(PartialEq)]
enum Kind {
    Nil,
    Symbol,
    Int,
    Float,
    Str,
    Cons,
    Vector,
    Hash,
    Other(String),
}

fn kind_of(env: &Env, value: Value) -> Result<Kind> {
    if !env.is_not_nil(value) {
        return Ok(Kind::Nil);
    }
    Ok(match env.type_name(value)?.as_str() {
        "symbol" | "boolean" => Kind::Symbol,
        "integer" => Kind::Int,
        "float" => Kind::Float,
        "string" => Kind::Str,
        "cons" => Kind::Cons,
        "vector" => Kind::Vector,
        "hash-table" => Kind::Hash,
        other => Kind::Other(other.to_string()),
    })
}

fn describe(kind: &Kind) -> &str {
    match kind {
        Kind::Nil => "nil",
        Kind::Symbol => "symbol",
        Kind::Int => "integer",
        Kind::Float => "float",
        Kind::Str => "string",
        Kind::Cons => "list",
        Kind::Vector => "vector",
        Kind::Hash => "hash-table",
        Kind::Other(name) => name,
    }
}

/// The name of `value` without the colon if it is a keyword.
fn keyword_name(env: &Env, value: Value) -> Result<Option<String>> {
    if kind_of(env, value)? != Kind::Symbol {
        return Ok(None);
    }
    let name = env.symbol_name(value)?;
    Ok(name.strip_prefix(':').map(str::to_string))
}

/// Kebab-case lookup table for the field or variant names of a type.
fn name_table(names: &'static [&'static str]) -> Result<Vec<(String, &'static str)>> {
    let mut table: Vec<(String, &'static str)> = Vec::with_capacity(names.len());
    for name in names {
        let kebab = to_kebab(name);
        if let Some((_, other)) = table.iter().find(|(k, _)| *k == kebab) {
            return Err(DeError::msg(format!("names `{other}` and `{name}` both map to :{kebab}")));
        }
        table.push((kebab, name));
    }
    Ok(table)
}

#[derive(Clone, Copy)]
struct De<'e> {
    env: &'e Env,
    value: Value,
    depth: usize,
}

impl<'e> De<'e> {
    fn kind(&self) -> Result<Kind> {
        kind_of(self.env, self.value)
    }

    fn mismatch(&self, expected: &str) -> DeError {
        match self.kind() {
            Ok(kind) => DeError::msg(format!("expected {expected}, got {}", describe(&kind))),
            Err(e) => e,
        }
    }

    fn int_parts(&self) -> Result<(bool, u128)> {
        if self.kind()? != Kind::Int {
            return Err(self.mismatch("integer"));
        }
        self.env.extract_big(self.value)?.ok_or_else(|| DeError::msg("integer out of range"))
    }

    fn signed(&self) -> Result<i128> {
        let (negative, magnitude) = self.int_parts()?;
        let limit = 1u128 << 127;
        if negative && magnitude <= limit {
            Ok((magnitude as i128).wrapping_neg())
        } else if !negative && magnitude < limit {
            Ok(magnitude as i128)
        } else {
            Err(DeError::msg("integer out of range"))
        }
    }

    fn unsigned(&self) -> Result<u128> {
        match self.int_parts()? {
            (true, magnitude) if magnitude != 0 => Err(DeError::msg("expected a non-negative integer")),
            (_, magnitude) => Ok(magnitude),
        }
    }

    fn seq(&self) -> Result<Seq<'e>> {
        let src = match self.kind()? {
            Kind::Nil | Kind::Cons => {
                ensure_proper_list(self.env, self.value)?;
                Src::List(self.value)
            }
            Kind::Vector => Src::Vector { vector: self.value, next: 0, len: self.env.vec_size(self.value)? },
            _ => return Err(self.mismatch("list or vector")),
        };
        Ok(Seq { env: self.env, depth: self.depth, index: 0, src })
    }

    fn plist(&self, fields: &'static [&'static str]) -> Result<Plist<'e>> {
        match self.kind()? {
            Kind::Nil | Kind::Cons => {
                ensure_proper_list(self.env, self.value)?;
                Ok(Plist { env: self.env, depth: self.depth, cur: self.value, names: name_table(fields)?, value: None })
            }
            _ => Err(self.mismatch("plist")),
        }
    }

    fn alist(&self) -> Result<Alist<'e>> {
        let cur = match self.kind()? {
            Kind::Nil | Kind::Cons => {
                ensure_proper_list(self.env, self.value)?;
                self.value
            }
            Kind::Hash => self.env.call("emrs--hash-to-alist", &[self.value])?,
            _ => return Err(self.mismatch("alist or hash table")),
        };
        Ok(Alist { env: self.env, depth: self.depth, cur, value: None })
    }
}

/// A deserializer for a value nested one level below `depth`.
fn child<'e>(env: &'e Env, value: Value, depth: usize) -> Result<De<'e>> {
    if depth >= MAX_DEPTH {
        return Err(DeError::msg(format!("value is nested deeper than {MAX_DEPTH} levels")));
    }
    Ok(De { env, value, depth: depth + 1 })
}

/// Fail unless `value` is nil or a proper list. One call into Lisp rejects
/// dotted and circular lists before any element is read.
fn ensure_proper_list(env: &Env, value: Value) -> Result<()> {
    if !env.is_not_nil(value) || env.is_not_nil(env.call("proper-list-p", &[value])?) {
        Ok(())
    } else {
        Err(DeError::msg("expected a proper list, got a dotted or circular list"))
    }
}

/// Pop the next element of a proper list.
fn pop(env: &Env, cur: &mut Value) -> Result<Option<Value>> {
    match kind_of(env, *cur)? {
        Kind::Nil => Ok(None),
        Kind::Cons => {
            let car = env.car(*cur)?;
            *cur = env.cdr(*cur)?;
            Ok(Some(car))
        }
        _ => Err(DeError::msg("improper list")),
    }
}

macro_rules! de_signed {
    ($($method:ident $visit:ident $ty:ty),*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            let n = self.signed()?;
            let n = <$ty>::try_from(n)
                .map_err(|_| DeError::msg(format!("integer {n} out of range for {}", stringify!($ty))))?;
            visitor.$visit(n)
        }
    )*};
}

macro_rules! de_unsigned {
    ($($method:ident $visit:ident $ty:ty),*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            let n = self.unsigned()?;
            let n = <$ty>::try_from(n)
                .map_err(|_| DeError::msg(format!("integer {n} out of range for {}", stringify!($ty))))?;
            visitor.$visit(n)
        }
    )*};
}

impl<'de> de::Deserializer<'de> for De<'_> {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Nil => visitor.visit_unit(),
            Kind::Symbol => {
                let name = self.env.symbol_name(self.value)?;
                if name == "t" { visitor.visit_bool(true) } else { visitor.visit_string(name) }
            }
            Kind::Int => match self.int_parts()? {
                (false, m) if m <= u64::MAX as u128 => visitor.visit_u64(m as u64),
                (false, m) => visitor.visit_u128(m),
                (true, _) => match i64::try_from(self.signed()?) {
                    Ok(n) => visitor.visit_i64(n),
                    Err(_) => visitor.visit_i128(self.signed()?),
                },
            },
            Kind::Float => self.deserialize_f64(visitor),
            Kind::Str => self.deserialize_string(visitor),
            Kind::Vector => self.deserialize_seq(visitor),
            Kind::Hash => self.deserialize_map(visitor),
            Kind::Cons => {
                ensure_proper_list(self.env, self.value)?;
                let head = self.env.car(self.value)?;
                if keyword_name(self.env, head)?.is_some() {
                    visitor.visit_map(self.plist(&[])?)
                } else {
                    self.deserialize_seq(visitor)
                }
            }
            Kind::Other(name) => Err(DeError::msg(format!("cannot decode a value of type {name}"))),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_bool(self.env.is_not_nil(self.value))
    }

    de_signed!(
        deserialize_i8 visit_i8 i8,
        deserialize_i16 visit_i16 i16,
        deserialize_i32 visit_i32 i32,
        deserialize_i64 visit_i64 i64,
        deserialize_i128 visit_i128 i128
    );

    de_unsigned!(
        deserialize_u8 visit_u8 u8,
        deserialize_u16 visit_u16 u16,
        deserialize_u32 visit_u32 u32,
        deserialize_u64 visit_u64 u64,
        deserialize_u128 visit_u128 u128
    );

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Float => visitor.visit_f64(self.env.extract_float(self.value)?),
            Kind::Int => visitor.visit_f64(self.signed()? as f64),
            _ => Err(self.mismatch("number")),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Int => {
                let c = u32::try_from(self.unsigned()?).ok().and_then(char::from_u32);
                visitor.visit_char(c.ok_or_else(|| DeError::msg("integer is not a valid character"))?)
            }
            Kind::Str => {
                let s = self.env.string(self.value)?;
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => visitor.visit_char(c),
                    _ => Err(DeError::msg("expected a string of length 1")),
                }
            }
            _ => Err(self.mismatch("character")),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Str => visitor.visit_string(self.env.string(self.value)?),
            _ => Err(self.mismatch("string")),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_byte_buf(visitor)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Str => visitor.visit_byte_buf(self.env.string_bytes(self.value)?),
            _ => Err(self.mismatch("string")),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if self.env.is_not_nil(self.value) { visitor.visit_some(self) } else { visitor.visit_none() }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if self.env.is_not_nil(self.value) { Err(self.mismatch("nil")) } else { visitor.visit_unit() }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_seq(self.seq()?)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        let mut seq = self.seq()?;
        let value = visitor.visit_seq(&mut seq)?;
        seq.expect_end(len)?;
        Ok(value)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_map(self.alist()?)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_map(self.plist(fields)?)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        let (name, rest) = if let Some(name) = keyword_name(self.env, self.value)? {
            (name, None)
        } else if self.kind()? == Kind::Cons {
            ensure_proper_list(self.env, self.value)?;
            let head = self.env.car(self.value)?;
            let name = keyword_name(self.env, head)?
                .ok_or_else(|| DeError::msg("expected a keyword at the head of the list"))?;
            (name, Some(self.env.cdr(self.value)?))
        } else {
            return Err(self.mismatch("keyword or keyword-headed list"));
        };
        let segment = format!("(:{name})");
        visitor.visit_enum(EnumDe { env: self.env, depth: self.depth, name, rest, variants }).map_err(|e| e.at(segment))
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_unit()
    }
}

enum Src {
    List(Value),
    Vector { vector: Value, next: isize, len: isize },
}

struct Seq<'e> {
    env: &'e Env,
    depth: usize,
    index: usize,
    src: Src,
}

impl Seq<'_> {
    fn next_value(&mut self) -> Result<Option<Value>> {
        match &mut self.src {
            Src::List(cur) => pop(self.env, cur),
            Src::Vector { vector, next, len } => {
                if *next >= *len {
                    return Ok(None);
                }
                let value = self.env.vec_get(*vector, *next)?;
                *next += 1;
                Ok(Some(value))
            }
        }
    }

    /// Fail if elements remain after a fixed-length read of `len`.
    fn expect_end(&mut self, len: usize) -> Result<()> {
        match self.next_value()? {
            None => Ok(()),
            Some(_) => Err(DeError::msg(format!("expected {len} elements, got more"))),
        }
    }
}

impl<'de> de::SeqAccess<'de> for Seq<'_> {
    type Error = DeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        let Some(value) = self.next_value()? else {
            return Ok(None);
        };
        let index = self.index;
        self.index += 1;
        seed.deserialize(child(self.env, value, self.depth)?).map(Some).map_err(|e| e.at(format!("[{index}]")))
    }
}

struct Plist<'e> {
    env: &'e Env,
    depth: usize,
    cur: Value,
    names: Vec<(String, &'static str)>,
    /// Value and keyword name of the key returned last.
    value: Option<(Value, String)>,
}

impl<'de> de::MapAccess<'de> for Plist<'_> {
    type Error = DeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        let Some(key) = pop(self.env, &mut self.cur)? else {
            return Ok(None);
        };
        let keyword = keyword_name(self.env, key)?.ok_or_else(|| DeError::msg("expected a keyword key in plist"))?;
        let value =
            pop(self.env, &mut self.cur)?.ok_or_else(|| DeError::msg(format!("missing value for :{keyword}")))?;
        let name = match self.names.iter().find(|(kebab, _)| *kebab == keyword) {
            Some((_, original)) => original.to_string(),
            None => keyword.clone(),
        };
        self.value = Some((value, keyword));
        let de: de::value::StringDeserializer<DeError> = name.into_deserializer();
        seed.deserialize(de).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let (value, keyword) = self.value.take().expect("next_value_seed called before next_key_seed");
        seed.deserialize(child(self.env, value, self.depth)?).map_err(|e| e.at(format!(":{keyword}")))
    }
}

struct Alist<'e> {
    env: &'e Env,
    depth: usize,
    cur: Value,
    value: Option<Value>,
}

impl<'de> de::MapAccess<'de> for Alist<'_> {
    type Error = DeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        let Some(entry) = pop(self.env, &mut self.cur)? else {
            return Ok(None);
        };
        if kind_of(self.env, entry)? != Kind::Cons {
            return Err(DeError::msg("alist entry must be a cons"));
        }
        self.value = Some(self.env.cdr(entry)?);
        seed.deserialize(child(self.env, self.env.car(entry)?, self.depth)?).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let value = self.value.take().expect("next_value_seed called before next_key_seed");
        seed.deserialize(child(self.env, value, self.depth)?)
    }
}

struct EnumDe<'e> {
    env: &'e Env,
    depth: usize,
    /// Variant keyword without the colon.
    name: String,
    /// The list after the keyword; `None` for a bare keyword.
    rest: Option<Value>,
    variants: &'static [&'static str],
}

impl<'de, 'e> de::EnumAccess<'de> for EnumDe<'e> {
    type Error = DeError;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self)> {
        let table = name_table(self.variants)?;
        let Some((_, original)) = table.iter().find(|(kebab, _)| *kebab == self.name) else {
            let known: Vec<String> = table.iter().map(|(kebab, _)| format!(":{kebab}")).collect();
            return Err(DeError::msg(format!("unknown variant, expected one of {}", known.join(" "))));
        };
        let de: de::value::StrDeserializer<DeError> = (*original).into_deserializer();
        let value = seed.deserialize(de)?;
        Ok((value, self))
    }
}

impl<'de> de::VariantAccess<'de> for EnumDe<'_> {
    type Error = DeError;

    fn unit_variant(self) -> Result<()> {
        match self.rest {
            Some(rest) if self.env.is_not_nil(rest) => Err(DeError::msg("expected no values")),
            _ => Ok(()),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        let mut rest = self.rest.ok_or_else(|| DeError::msg("expected exactly one value"))?;
        let value = pop(self.env, &mut rest)?.ok_or_else(|| DeError::msg("expected exactly one value"))?;
        if self.env.is_not_nil(rest) {
            return Err(DeError::msg("expected exactly one value"));
        }
        seed.deserialize(child(self.env, value, self.depth)?)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        let rest = self.rest.ok_or_else(|| DeError::msg(format!("expected {len} values")))?;
        let mut seq = Seq { env: self.env, depth: self.depth, index: 0, src: Src::List(rest) };
        let value = visitor.visit_seq(&mut seq)?;
        seq.expect_end(len)?;
        Ok(value)
    }

    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value> {
        let cur = match self.rest {
            Some(rest) => rest,
            None => self.env.nil()?,
        };
        visitor.visit_map(Plist { env: self.env, depth: self.depth, cur, names: name_table(fields)?, value: None })
    }
}
````

Notes for the reviewer: `ensure_proper_list` is the guard against circular lists (Review Focus 1). Field and variant names are matched by converting the names serde supplies to kebab-case, so `#[serde(rename)]` names go through the same conversion.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p emrs`

Expected: `test result: ok. 43 passed`.

- [ ] **Step 6: Commit**

````bash
git add emrs/src
git commit -m "feat: serde deserializer reading Lisp values"
````

---

### Task 7: Error and Payload

**Files:**
- Create: `emrs/src/error.rs` (tests inside), `emrs/src/payload.rs`
- Modify: `emrs/src/lib.rs`

**Interfaces:**
- Consumes: `convert::ser::{to_lisp, SerError}`, `convert::de::DeError`, `ffi::env::{Env, Exit, Value}`.
- Produces:
  - `pub enum ErrorKind { Service, NoReply, Cancelled, Timeout, Stopped, Failed, Decode, Encode, Protocol }` with `pub fn as_str(self) -> &'static str` (the Lisp symbol name).
  - `pub struct Error` with `Error::new(message)`, `Error::with_data(message, data)`, `kind()`, `message()`, `take_data::<T>()`, crate-private `Error::with_kind(kind, message)` and crate-visible fields `kind`, `message`, `data`. `impl<E: std::error::Error> From<E> for Error`.
  - `pub(crate) enum Fail { Exit, Error(Error) }` with `From<Exit>`, `From<Error>`, `From<DeError>`.
  - `pub(crate) trait Payload: Send + 'static { fn to_lisp(&self, env: &Env) -> Result<Value, SerError>; fn into_any(self: Box<Self>) -> Box<dyn Any + Send>; }`, implemented for every `T: Serialize + Send + 'static`.

- [ ] **Step 1: Register the modules**

`emrs/src/lib.rs` (whole file):

````rust
//! Message-passing services for Emacs dynamic modules.
#![allow(dead_code)] // Removed in Task 9, when every item has a user.

mod convert;
mod error;
mod ffi;
mod names;
mod payload;
mod runtime;

pub use error::{Error, ErrorKind};
pub use runtime::cancel::CancelToken;
````

- [ ] **Step 2: Write the payload trait**

`emrs/src/payload.rs`:

````rust
//! Type-erased values waiting to be converted to Lisp on the Emacs thread.

use crate::convert::ser::{SerError, to_lisp};
use crate::ffi::env::{Env, Value};
use serde::Serialize;
use std::any::Any;

pub(crate) trait Payload: Send + 'static {
    fn to_lisp(&self, env: &Env) -> Result<Value, SerError>;
    /// Recover the concrete value; used by the testing host.
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send>;
}

impl<T: Serialize + Send + 'static> Payload for T {
    fn to_lisp(&self, env: &Env) -> Result<Value, SerError> {
        to_lisp(env, self)
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }
}
````

- [ ] **Step 3: Write the failing tests**

Create `emrs/src/error.rs` with only the tests:

````rust
//! The error type crossing the Rust/Lisp boundary.

#[cfg(test)]
mod tests {
    use super::{Error, ErrorKind};

    #[test]
    fn new_is_a_service_error() {
        let e = Error::new("boom");
        assert_eq!(e.kind(), ErrorKind::Service);
        assert_eq!(e.message(), "boom");
        assert_eq!(e.to_string(), "service: boom");
    }

    #[test]
    fn question_mark_converts_std_errors() {
        fn parse() -> Result<i32, Error> {
            Ok("x".parse::<i32>()?)
        }
        let e = parse().unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Service);
        assert!(e.message().contains("invalid digit"));
    }

    #[test]
    fn data_can_be_taken_back_by_type() {
        let mut e = Error::with_data("bad", 42u32);
        assert_eq!(e.take_data::<String>(), None);
        let mut e2 = Error::with_data("bad", 42u32);
        assert_eq!(e2.take_data::<u32>(), Some(42));
        assert_eq!(e.take_data::<u32>(), None, "a failed downcast consumes the data");
    }

    #[test]
    fn kind_names_are_kebab_symbols() {
        assert_eq!(ErrorKind::NoReply.as_str(), "no-reply");
        assert_eq!(ErrorKind::Protocol.as_str(), "protocol");
    }
}
````

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p emrs error`

Expected: compilation fails with `unresolved imports super::Error, super::ErrorKind`.

- [ ] **Step 5: Write the implementation**

Put this above the tests module, replacing the one-line doc comment:

````rust
//! The error type crossing the Rust/Lisp boundary.

use crate::convert::de::DeError;
use crate::ffi::env::Exit;
use crate::payload::Payload;
use serde::Serialize;
use std::fmt;

/// Why a call, stream or instance ended unsuccessfully. Shown to Elisp as the
/// `:kind` symbol of an error plist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The handler answered with an error.
    Service,
    /// The reply or stream sender was dropped unanswered.
    NoReply,
    /// The call was cancelled.
    Cancelled,
    /// A synchronous wait timed out.
    Timeout,
    /// The instance stopped before answering, or was not running.
    Stopped,
    /// The instance failed before answering.
    Failed,
    /// A Lisp value could not be converted to the Rust type.
    Decode,
    /// A Rust value could not be converted to Lisp.
    Encode,
    /// Version or mode mismatch, unknown service, or internal fault.
    Protocol,
}

impl ErrorKind {
    /// The Lisp symbol name of this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Service => "service",
            ErrorKind::NoReply => "no-reply",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Stopped => "stopped",
            ErrorKind::Failed => "failed",
            ErrorKind::Decode => "decode",
            ErrorKind::Encode => "encode",
            ErrorKind::Protocol => "protocol",
        }
    }
}

/// An error reported to Elisp.
///
/// `Error` deliberately does not implement `std::error::Error`, which allows
/// the blanket `From` conversion below so that `?` works on any error type.
pub struct Error {
    pub(crate) kind: ErrorKind,
    pub(crate) message: String,
    pub(crate) data: Option<Box<dyn Payload>>,
}

impl Error {
    /// A service error with a message.
    pub fn new(message: impl Into<String>) -> Error {
        Error::with_kind(ErrorKind::Service, message)
    }

    /// A service error with a message and a payload for the `:data` slot.
    pub fn with_data(message: impl Into<String>, data: impl Serialize + Send + 'static) -> Error {
        Error { kind: ErrorKind::Service, message: message.into(), data: Some(Box::new(data)) }
    }

    pub(crate) fn with_kind(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into(), data: None }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Take the data payload if it has type `T`. Intended for tests.
    pub fn take_data<T: 'static>(&mut self) -> Option<T> {
        let data = self.data.take()?;
        data.into_any().downcast::<T>().ok().map(|b| *b)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("has_data", &self.data.is_some())
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl<E: std::error::Error> From<E> for Error {
    fn from(error: E) -> Error {
        Error::new(error.to_string())
    }
}

/// Why a protocol function could not complete.
pub(crate) enum Fail {
    /// A Lisp non-local exit is pending; return to Emacs untouched.
    Exit,
    /// Signal this error to Lisp as `emrs-error`.
    Error(Error),
}

impl From<Exit> for Fail {
    fn from(_: Exit) -> Fail {
        Fail::Exit
    }
}

impl From<Error> for Fail {
    fn from(error: Error) -> Fail {
        Fail::Error(error)
    }
}

impl From<DeError> for Fail {
    fn from(error: DeError) -> Fail {
        match error {
            DeError::Exit => Fail::Exit,
            other => Fail::Error(Error::with_kind(ErrorKind::Decode, other.to_string())),
        }
    }
}
````

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p emrs`

Expected: `test result: ok. 47 passed`.

- [ ] **Step 7: Commit**

````bash
git add emrs/src
git commit -m "feat: boundary error type and type-erased payloads"
````

---

### Task 8: Runtime: instances, replies, streams, test host

**Files:**
- Create: `emrs/src/runtime/core.rs`, `emrs/src/runtime/instance.rs`, `emrs/src/runtime/reply.rs` (tests inside), `emrs/src/testing.rs`
- Create: `emrs/tests/runtime.rs`
- Modify: `emrs/src/runtime/mod.rs`, `emrs/src/lib.rs`

These four source files reference each other (`Core` holds instances, instances push deliveries, replies complete calls), so they form one task.

**Interfaces:**
- Consumes: `CancelToken` (`new`, `child`, `on_cancel`, `cancel`), `Pool`, `Error`/`ErrorKind`/`Fail`, `Payload`, `convert::de::from_lisp`, `ffi::env::{Env, Value}`.
- Produces (public): `trait Service` (spec 4.2), `Context<S>` with `emit`, `spawn`, `address`, `cancel_token`, `stop`, `log`; `Address<S>::send`; `Reply` with `ok`, `err`, `stream::<T>(capacity)`, `token`, `is_wanted`; `StreamSender<T>` with `send`, `try_send`, `finish`, `fail`, `token`; `StreamClosed`; `TrySendError<T>`; `State`; `Level`; `StreamEnd`; `testing::Host<S>` and `testing::StreamResult<T>`.
- Produces (crate-private, used by Task 9):
  - `Core::new(name: &str, waker: impl Fn() + Send + Sync + 'static) -> Arc<Core>`, `core.take(max: usize) -> Vec<Delivery>`, `core.get(id: u64) -> Option<Arc<dyn AnyActor>>`, `core.shutdown(timeout: Duration) -> bool`.
  - `enum Delivery { Event, Reply, Error, Item, End, State, Log }` with the fields shown in `core.rs`.
  - `trait AnyActor { fn shared(&self) -> &Arc<Shared>; fn request_stop(&self); fn send_lisp(&self, env: &Env, request: Value, call: Option<u64>, mode: Mode) -> Result<(), Fail>; }`
  - `Actor::<S>::spawn(core: &Arc<Core>, args: S::Args) -> Arc<Actor<S>>`; `Shared { id, core, token, .. }` with `cancel_call(call: u64)`; `enum Mode { Send, Call, Stream }`; `State::as_str`, `Level::as_str`.

- [ ] **Step 1: Write the failing behaviour tests**

`emrs/tests/runtime.rs`. The `Probe` service misbehaves on request; each test states one rule of spec sections 4 and 7.

````rust
//! Runtime behaviour, driven through `emrs::testing::Host` without Emacs.

use emrs::testing::Host;
use emrs::{Context, Error, ErrorKind, Reply, Service, State, StreamEnd};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
enum Start {
    Ok,
    Fail,
    Panic,
}

#[derive(Deserialize)]
enum Request {
    Echo(String),
    Fail,
    Forget,
    Panic,
    Emit(u32),
    Slow(u64),
    SpawnReply,
    SpawnPanic,
    SelfPing,
    #[serde(skip_deserializing)]
    Bump,
    Count {
        n: u32,
        capacity: usize,
    },
    Endless,
    DropStream,
    StopSelf,
}

#[derive(Serialize, Debug, PartialEq)]
enum Event {
    Tick(u32),
    Bumped { count: u32, wanted: bool },
    ProducerStopped,
    StopHookRan,
}

struct Probe {
    ctx: Context<Probe>,
    count: u32,
}

impl Service for Probe {
    const NAME: &'static str = "probe";
    type Args = Start;
    type Request = Request;
    type Event = Event;

    fn start(args: Start, ctx: &Context<Self>) -> Result<Self, Error> {
        match args {
            Start::Ok => Ok(Probe { ctx: ctx.clone(), count: 0 }),
            Start::Fail => Err(Error::new("cannot start")),
            Start::Panic => panic!("start exploded"),
        }
    }

    fn handle(&mut self, request: Request, reply: Reply, ctx: &Context<Self>) {
        match request {
            Request::Echo(text) => reply.ok(text),
            Request::Fail => reply.err(Error::with_data("nope", 7u32)),
            Request::Forget => drop(reply),
            Request::Panic => panic!("handler exploded"),
            Request::Emit(n) => {
                for i in 0..n {
                    ctx.emit(Event::Tick(i));
                }
                reply.ok(n);
            }
            Request::Slow(ms) => {
                std::thread::sleep(Duration::from_millis(ms));
                reply.ok(ms);
            }
            Request::SpawnReply => ctx.spawn(move |_| reply.ok("spawned".to_string())),
            Request::SpawnPanic => ctx.spawn(move |_| {
                let _reply = reply;
                panic!("task exploded");
            }),
            Request::SelfPing => {
                ctx.address().send(Request::Bump);
                reply.ok(());
            }
            Request::Bump => {
                self.count += 1;
                ctx.emit(Event::Bumped { count: self.count, wanted: reply.is_wanted() });
            }
            Request::Count { n, capacity } => {
                let out = reply.stream::<u32>(capacity);
                ctx.spawn(move |_| {
                    for i in 0..n {
                        if out.send(i).is_err() {
                            return;
                        }
                    }
                    out.finish();
                });
            }
            Request::Endless => {
                let out = reply.stream::<u32>(4);
                let ctx = ctx.clone();
                ctx.clone().spawn(move |_| {
                    let mut i = 0;
                    while out.send(i).is_ok() {
                        i += 1;
                    }
                    ctx.emit(Event::ProducerStopped);
                });
            }
            Request::DropStream => {
                let out = reply.stream::<u32>(4);
                out.send(1).unwrap();
            }
            Request::StopSelf => {
                ctx.stop();
                reply.ok(());
            }
        }
    }

    fn stop(&mut self) {
        self.ctx.emit(Event::StopHookRan);
    }
}

fn host() -> Host<Probe> {
    Host::start(Start::Ok)
}

#[test]
fn instance_reaches_running() {
    let mut host = host();
    assert!(host.wait_for_state(State::Running));
}

#[test]
fn call_returns_the_reply() {
    let mut host = host();
    assert_eq!(host.call::<String>(Request::Echo("hi".into())).unwrap(), "hi");
}

#[test]
fn service_error_carries_message_and_data() {
    let mut host = host();
    let mut error = host.call::<()>(Request::Fail).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Service);
    assert_eq!(error.message(), "nope");
    assert_eq!(error.take_data::<u32>(), Some(7));
}

#[test]
fn dropped_reply_reports_no_reply() {
    let mut host = host();
    assert_eq!(host.call::<()>(Request::Forget).unwrap_err().kind(), ErrorKind::NoReply);
}

#[test]
fn reply_can_be_sent_from_a_spawned_task() {
    let mut host = host();
    assert_eq!(host.call::<String>(Request::SpawnReply).unwrap(), "spawned");
}

#[test]
fn events_arrive_in_order_before_the_reply_is_returned() {
    let mut host = host();
    assert_eq!(host.call::<u32>(Request::Emit(3)).unwrap(), 3);
    assert_eq!(host.next_event(), Some(Event::Tick(0)));
    assert_eq!(host.next_event(), Some(Event::Tick(1)));
    assert_eq!(host.next_event(), Some(Event::Tick(2)));
}

#[test]
fn self_messages_reach_the_handler_without_a_wanted_reply() {
    let mut host = host();
    host.call::<()>(Request::SelfPing).unwrap();
    assert_eq!(host.next_event(), Some(Event::Bumped { count: 1, wanted: false }));
}

#[test]
fn start_error_fails_the_instance_and_its_queued_calls() {
    let mut host = Host::<Probe>::start(Start::Fail);
    let error = host.call::<String>(Request::Echo("x".into())).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Failed | ErrorKind::Stopped), "{error:?}");
    let (state, error) = host.wait_exit();
    assert_eq!(state, State::Failed);
    assert_eq!(error.unwrap().message(), "cannot start");
}

#[test]
fn start_panic_fails_the_instance() {
    let mut host = Host::<Probe>::start(Start::Panic);
    let (state, error) = host.wait_exit();
    assert_eq!(state, State::Failed);
    assert_eq!(error.unwrap().message(), "panic in start: start exploded");
}

#[test]
fn handler_panic_fails_the_instance_and_completes_every_open_call() {
    let mut host = host();
    assert_eq!(host.call::<()>(Request::Panic).unwrap_err().kind(), ErrorKind::NoReply);
    let (state, error) = host.wait_exit();
    assert_eq!(state, State::Failed);
    assert_eq!(error.unwrap().message(), "panic in handle: handler exploded");
    assert_eq!(host.call::<String>(Request::Echo("x".into())).unwrap_err().kind(), ErrorKind::Stopped);
}

#[test]
fn spawned_task_panic_is_logged_and_the_instance_survives() {
    let mut host = host();
    assert_eq!(host.call::<()>(Request::SpawnPanic).unwrap_err().kind(), ErrorKind::NoReply);
    assert_eq!(host.call::<String>(Request::Echo("alive".into())).unwrap(), "alive");
    assert!(host.logs().iter().any(|l| l.contains("spawned task panicked: task exploded")), "{:?}", host.logs());
}

#[test]
fn stop_finishes_the_running_handler_and_rejects_queued_requests() {
    let mut host = host();
    assert!(host.wait_for_state(State::Running));
    host.send(Request::Slow(300));
    std::thread::sleep(Duration::from_millis(100)); // the slow handler is now running
    host.send(Request::Emit(1)); // queued behind it; must not be handled
    assert_eq!(host.stop(), State::Stopped);
    assert_eq!(host.next_event(), Some(Event::StopHookRan), "queued Emit was skipped, stop hook ran");
    assert_eq!(host.call::<String>(Request::Echo("x".into())).unwrap_err().kind(), ErrorKind::Stopped);
}

#[test]
fn service_can_stop_itself() {
    let mut host = host();
    host.call::<()>(Request::StopSelf).unwrap();
    assert_eq!(host.wait_exit().0, State::Stopped);
}

#[test]
fn stream_delivers_all_items_in_order_then_done() {
    let mut host = host();
    let result = host.call_stream::<u32>(Request::Count { n: 5000, capacity: 8 }).unwrap();
    assert_eq!(result.status, StreamEnd::Done);
    assert!(result.error.is_none());
    assert_eq!(result.items, (0..5000).collect::<Vec<u32>>());
}

#[test]
fn zero_capacity_is_treated_as_one() {
    let mut host = host();
    let result = host.call_stream::<u32>(Request::Count { n: 10, capacity: 0 }).unwrap();
    assert_eq!(result.items.len(), 10);
}

#[test]
fn empty_stream_ends_done() {
    let mut host = host();
    let result = host.call_stream::<u32>(Request::Count { n: 0, capacity: 4 }).unwrap();
    assert_eq!((result.items.len(), result.status), (0, StreamEnd::Done));
}

#[test]
fn dropped_stream_ends_failed_with_no_reply() {
    let mut host = host();
    let result = host.call_stream::<u32>(Request::DropStream).unwrap();
    assert_eq!(result.items, vec![1]);
    assert_eq!(result.status, StreamEnd::Failed);
    assert_eq!(result.error.unwrap().kind(), ErrorKind::NoReply);
}

#[test]
fn cancelling_a_stream_stops_the_producer_promptly() {
    let mut host = host();
    let call = host.begin_stream(Request::Endless).unwrap();
    while host.items_received(call) < 10 {
        std::thread::sleep(Duration::from_millis(1));
    }
    let cancelled_at = Instant::now();
    host.cancel(call);
    assert_eq!(host.next_event(), Some(Event::ProducerStopped));
    assert!(cancelled_at.elapsed() < Duration::from_secs(2));
    let received = host.items_received(call);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(host.items_received(call), received, "no items after cancellation");
}

#[test]
fn plain_reply_to_a_stream_call_is_one_item_then_done() {
    let mut host = host();
    let result = host.call_stream::<String>(Request::Echo("one".into())).unwrap();
    assert_eq!(result.items, vec!["one".to_string()]);
    assert_eq!(result.status, StreamEnd::Done);
}

#[test]
fn error_reply_to_a_stream_call_ends_failed() {
    let mut host = host();
    let result = host.call_stream::<String>(Request::Fail).unwrap();
    assert!(result.items.is_empty());
    assert_eq!(result.status, StreamEnd::Failed);
    assert_eq!(result.error.unwrap().kind(), ErrorKind::Service);
}

#[test]
fn stream_answer_to_a_plain_call_is_a_protocol_error() {
    let mut host = host();
    let error = host.call::<u32>(Request::Count { n: 3, capacity: 4 }).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Protocol);
}

#[test]
fn stopping_ends_open_streams_as_failed_with_kind_stopped() {
    let mut host = host();
    let call = host.begin_stream(Request::Endless).unwrap();
    while host.items_received(call) < 1 {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(host.stop(), State::Stopped);
    let result = host.finish_stream::<u32>(call);
    assert_eq!(result.status, StreamEnd::Failed);
    assert_eq!(result.error.unwrap().kind(), ErrorKind::Stopped);
}

#[test]
fn a_slow_instance_does_not_delay_another() {
    let mut slow = host();
    let mut fast = host();
    slow.send(Request::Slow(1000));
    let started = Instant::now();
    assert_eq!(fast.call::<String>(Request::Echo("quick".into())).unwrap(), "quick");
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn many_instances_run_side_by_side() {
    let mut hosts: Vec<Host<Probe>> = (0..50).map(|_| host()).collect();
    for (i, host) in hosts.iter_mut().enumerate() {
        assert_eq!(host.call::<String>(Request::Echo(i.to_string())).unwrap(), i.to_string());
    }
}
````

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p emrs --test runtime`

Expected: compilation fails with `unresolved import emrs::testing`.

- [ ] **Step 3: Write the runtime core**

`emrs/src/runtime/core.rs`:

````rust
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
````

`take` clears `wake_pending` before it pops. A producer that pushes after the clear sets the flag again and wakes Emacs; one that pushed before is popped by this call. Either way nothing is left unannounced.

- [ ] **Step 4: Write the instance actor**

`emrs/src/runtime/instance.rs`:

````rust
//! Service instances: the actor loop, lifecycle, and the handles given to
//! service code.

use super::cancel::CancelToken;
use super::core::{Core, Delivery, Level, StreamEnd};
use super::reply::Reply;
use crate::convert::de::from_lisp;
use crate::error::{Error, ErrorKind, Fail};
use crate::ffi::env::{Env, Value};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, Weak};

/// A message-passing service hosted in an Emacs module.
///
/// For one instance, `start`, `handle` and `stop` never run concurrently and
/// never on the Emacs thread.
pub trait Service: Sized + Send + 'static {
    /// The name Elisp passes to `emrs-start`.
    const NAME: &'static str;
    /// Start arguments, decoded from the `ARGS` of `emrs-start`.
    type Args: DeserializeOwned + Send + 'static;
    /// Requests from Elisp, and self-messages sent through [`Address`].
    type Request: DeserializeOwned + Send + 'static;
    /// Unsolicited events for the instance's `on-event` handler.
    type Event: Serialize + Send + 'static;

    /// Build the instance state. May block.
    fn start(args: Self::Args, ctx: &Context<Self>) -> Result<Self, Error>;

    /// Handle one request. May block; move long work to [`Context::spawn`].
    fn handle(&mut self, request: Self::Request, reply: Reply, ctx: &Context<Self>);

    /// Called once after the last `handle` when the instance stops normally.
    fn stop(&mut self) {}
}

/// Lifecycle state of an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

impl State {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Running => "running",
            State::Stopping => "stopping",
            State::Stopped => "stopped",
            State::Failed => "failed",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, State::Stopped | State::Failed)
    }
}

/// How the sender of a request wants to be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// No answer expected.
    Send,
    /// Exactly one reply or error.
    Call,
    /// Items followed by one end.
    Stream,
}

struct CallEntry {
    mode: Mode,
    token: CancelToken,
}

struct Calls {
    state: State,
    open: HashMap<u64, CallEntry>,
}

/// The part of an instance that does not depend on the service type.
pub(crate) struct Shared {
    pub(crate) id: u64,
    pub(crate) core: Arc<Core>,
    pub(crate) token: CancelToken,
    /// Guards the state and the open calls together, and is held while
    /// pushing their deliveries so that completions and the final state
    /// reach Emacs in a consistent order.
    calls: Mutex<Calls>,
}

impl Shared {
    pub(crate) fn state(&self) -> State {
        self.calls.lock().unwrap().state
    }

    /// Register an open call and return its reply handle.
    fn begin(self: &Arc<Self>, call: Option<u64>, mode: Mode) -> Result<Reply, Error> {
        let mut calls = self.calls.lock().unwrap();
        if !matches!(calls.state, State::Starting | State::Running) {
            return Err(Error::with_kind(ErrorKind::Stopped, "the instance is not running"));
        }
        let token = self.token.child();
        if let Some(id) = call {
            calls.open.insert(id, CallEntry { mode, token: token.clone() });
        }
        Ok(Reply::new(self.clone(), call, mode, token))
    }

    /// Push the deliveries that complete `call`, unless it is already
    /// completed or cancelled.
    pub(crate) fn complete_with(&self, call: u64, make: impl FnOnce() -> Vec<Delivery>) {
        let mut calls = self.calls.lock().unwrap();
        if calls.open.remove(&call).is_some() {
            for delivery in make() {
                self.core.push(delivery);
            }
        }
    }

    pub(crate) fn complete(&self, call: u64, make: impl FnOnce() -> Delivery) {
        self.complete_with(call, || vec![make()]);
    }

    /// Forget `call` and cancel its token. Nothing is delivered for it.
    pub(crate) fn cancel_call(&self, call: u64) {
        let entry = self.calls.lock().unwrap().open.remove(&call);
        if let Some(entry) = entry {
            entry.token.cancel();
        }
    }

    /// Move from `Starting` to `Running`. Returns false if a stop was
    /// requested meanwhile.
    fn set_running(&self) -> bool {
        let mut calls = self.calls.lock().unwrap();
        if calls.state != State::Starting {
            return false;
        }
        calls.state = State::Running;
        self.core.push(Delivery::State { instance: self.id, state: State::Running, error: None });
        true
    }

    /// Move to `Stopping`. Returns false if the instance was already
    /// stopping or terminal.
    fn set_stopping(&self) -> bool {
        let mut calls = self.calls.lock().unwrap();
        if !matches!(calls.state, State::Starting | State::Running) {
            return false;
        }
        calls.state = State::Stopping;
        true
    }

    /// Enter a terminal state: fail every open call, announce the state and
    /// drop the instance from the table.
    fn terminate(&self, state: State, error: Option<Error>) {
        let kind = if state == State::Failed { ErrorKind::Failed } else { ErrorKind::Stopped };
        let message = match &error {
            Some(error) => error.message.clone(),
            None => "the instance stopped".to_string(),
        };
        let mut calls = self.calls.lock().unwrap();
        if calls.state.is_terminal() {
            return;
        }
        calls.state = state;
        self.token.cancel();
        let mut open: Vec<(u64, CallEntry)> = calls.open.drain().collect();
        open.sort_by_key(|(id, _)| *id);
        for (call, entry) in open {
            let error = Error::with_kind(kind, message.clone());
            self.core.push(match entry.mode {
                Mode::Stream => {
                    Delivery::End { instance: self.id, call, status: StreamEnd::Failed, error: Some(error) }
                }
                _ => Delivery::Error { instance: self.id, call, error },
            });
        }
        self.core.push(Delivery::State { instance: self.id, state, error });
        drop(calls);
        self.core.unregister(self.id);
    }
}

enum Envelope<S: Service> {
    Start(S::Args),
    Request(S::Request, Reply),
    Stop,
}

struct Mailbox<S: Service> {
    queue: VecDeque<Envelope<S>>,
    /// A pool job is running or queued for this instance.
    scheduled: bool,
    /// No further requests are accepted.
    closed: bool,
}

pub(crate) struct Actor<S: Service> {
    shared: Arc<Shared>,
    mailbox: Mutex<Mailbox<S>>,
    /// The service state; touched only by the worker running this instance.
    service: Mutex<Option<S>>,
    me: Weak<Actor<S>>,
}

/// Operations on an instance that do not need its service type.
pub(crate) trait AnyActor: Send + Sync {
    fn shared(&self) -> &Arc<Shared>;
    fn request_stop(&self);
    /// Decode `request` on the Emacs thread and enqueue it.
    fn send_lisp(&self, env: &Env, request: Value, call: Option<u64>, mode: Mode) -> Result<(), Fail>;
}

fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

impl<S: Service> Actor<S> {
    /// Create an instance and queue its start.
    pub(crate) fn spawn(core: &Arc<Core>, args: S::Args) -> Arc<Actor<S>> {
        let shared = Arc::new(Shared {
            id: core.next_id(),
            core: core.clone(),
            token: CancelToken::new(),
            calls: Mutex::new(Calls { state: State::Starting, open: HashMap::new() }),
        });
        let actor = Arc::new_cyclic(|me| Actor {
            shared,
            mailbox: Mutex::new(Mailbox { queue: VecDeque::new(), scheduled: false, closed: false }),
            service: Mutex::new(None),
            me: me.clone(),
        });
        core.register(actor.shared.id, actor.clone());
        actor.enqueue(Envelope::Start(args));
        actor
    }

    fn context(&self) -> Context<S> {
        Context { shared: self.shared.clone(), actor: self.me.clone() }
    }

    /// Enqueue a typed request.
    pub(crate) fn send(&self, request: S::Request, call: Option<u64>, mode: Mode) -> Result<(), Error> {
        let reply = self.shared.begin(call, mode)?;
        self.enqueue(Envelope::Request(request, reply));
        Ok(())
    }

    fn enqueue(&self, envelope: Envelope<S>) {
        let mut mailbox = self.mailbox.lock().unwrap();
        if mailbox.closed && !matches!(envelope, Envelope::Stop) {
            drop(mailbox);
            if let Envelope::Request(_, reply) = envelope {
                reply.fail(ErrorKind::Stopped, "the instance is stopping");
            }
            return;
        }
        mailbox.queue.push_back(envelope);
        if !mailbox.scheduled {
            mailbox.scheduled = true;
            drop(mailbox);
            let actor = self.me.upgrade().expect("actor is alive while it is used");
            self.shared.core.pool.submit(move || actor.run());
        }
    }

    /// Pool job: process messages until the mailbox is empty.
    fn run(self: Arc<Self>) {
        loop {
            let envelope = {
                let mut mailbox = self.mailbox.lock().unwrap();
                match mailbox.queue.pop_front() {
                    Some(envelope) => envelope,
                    None => {
                        mailbox.scheduled = false;
                        return;
                    }
                }
            };
            if !self.process(envelope) {
                return;
            }
        }
    }

    /// Returns false once the instance has terminated.
    fn process(&self, envelope: Envelope<S>) -> bool {
        let ctx = self.context();
        let mut service = self.service.lock().unwrap();
        match envelope {
            Envelope::Start(args) => match catch_unwind(AssertUnwindSafe(|| S::start(args, &ctx))) {
                Ok(Ok(state)) => {
                    *service = Some(state);
                    self.shared.set_running();
                    true
                }
                Ok(Err(error)) => self.fail(Error::with_kind(ErrorKind::Failed, error.message)),
                Err(panic) => self.fail_panic("start", panic),
            },
            Envelope::Request(request, reply) => {
                let Some(state) = service.as_mut() else {
                    return true;
                };
                match catch_unwind(AssertUnwindSafe(|| state.handle(request, reply, &ctx))) {
                    Ok(()) => true,
                    Err(panic) => {
                        let dropped = catch_unwind(AssertUnwindSafe(|| drop(service.take())));
                        if dropped.is_err() {
                            self.shared.core.log(Level::Error, "service state panicked while being dropped");
                        }
                        self.fail_panic("handle", panic)
                    }
                }
            }
            Envelope::Stop => {
                let stopped = catch_unwind(AssertUnwindSafe(|| {
                    if let Some(mut state) = service.take() {
                        state.stop();
                    }
                }));
                match stopped {
                    Ok(()) => {
                        self.shared.terminate(State::Stopped, None);
                        false
                    }
                    Err(panic) => self.fail_panic("stop", panic),
                }
            }
        }
    }

    fn fail_panic(&self, phase: &str, panic: Box<dyn Any + Send>) -> bool {
        let message = format!("panic in {phase}: {}", panic_message(panic));
        self.fail(Error::with_kind(ErrorKind::Failed, message))
    }

    /// Terminate as failed and discard queued requests. Always returns false.
    fn fail(&self, error: Error) -> bool {
        self.shared.terminate(State::Failed, Some(error));
        let queued: Vec<Envelope<S>> = {
            let mut mailbox = self.mailbox.lock().unwrap();
            mailbox.closed = true;
            mailbox.queue.drain(..).collect()
        };
        drop(queued);
        false
    }
}

impl<S: Service> AnyActor for Actor<S> {
    fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    fn request_stop(&self) {
        if !self.shared.set_stopping() {
            return;
        }
        self.shared.token.cancel();
        let queued: Vec<Envelope<S>> = {
            let mut mailbox = self.mailbox.lock().unwrap();
            mailbox.closed = true;
            mailbox.queue.drain(..).collect()
        };
        for envelope in queued {
            if let Envelope::Request(_, reply) = envelope {
                reply.fail(ErrorKind::Stopped, "the instance stopped before handling the request");
            }
        }
        self.enqueue(Envelope::Stop);
    }

    fn send_lisp(&self, env: &Env, request: Value, call: Option<u64>, mode: Mode) -> Result<(), Fail> {
        let request: S::Request = from_lisp(env, request)?;
        Ok(self.send(request, call, mode)?)
    }
}

/// The handle through which service code talks to the runtime.
pub struct Context<S: Service> {
    shared: Arc<Shared>,
    actor: Weak<Actor<S>>,
}

impl<S: Service> Clone for Context<S> {
    fn clone(&self) -> Self {
        Context { shared: self.shared.clone(), actor: self.actor.clone() }
    }
}

impl<S: Service> Context<S> {
    /// Queue an event for the instance's `on-event` handler. Never blocks.
    /// Does nothing once the instance has stopped or failed.
    pub fn emit(&self, event: S::Event) {
        if !self.shared.state().is_terminal() {
            self.shared.core.push(Delivery::Event { instance: self.shared.id, payload: Box::new(event) });
        }
    }

    /// Run `task` on the runtime's pool. Its token is cancelled when the
    /// instance stops.
    pub fn spawn(&self, task: impl FnOnce(CancelToken) + Send + 'static) {
        let token = self.shared.token.child();
        let shared = self.shared.clone();
        self.shared.core.pool.submit(move || {
            if let Err(panic) = catch_unwind(AssertUnwindSafe(|| task(token))) {
                let message = format!("instance {}: spawned task panicked: {}", shared.id, panic_message(panic));
                shared.core.log(Level::Error, message);
            }
        });
    }

    /// A handle for sending requests to this instance from other threads.
    pub fn address(&self) -> Address<S> {
        Address { actor: self.actor.clone() }
    }

    /// Cancelled when the instance stops or fails.
    pub fn cancel_token(&self) -> CancelToken {
        self.shared.token.clone()
    }

    /// Stop this instance, as `emrs-stop` does.
    pub fn stop(&self) {
        if let Some(actor) = self.actor.upgrade() {
            actor.request_stop();
        }
    }

    /// Write a line to the `*emrs-log*` buffer.
    pub fn log(&self, level: Level, message: impl Into<String>) {
        self.shared.core.log(level, format!("instance {}: {}", self.shared.id, message.into()));
    }
}

/// Sends requests into an instance's own mailbox.
pub struct Address<S: Service> {
    actor: Weak<Actor<S>>,
}

impl<S: Service> Clone for Address<S> {
    fn clone(&self) -> Self {
        Address { actor: self.actor.clone() }
    }
}

impl<S: Service> Address<S> {
    /// Enqueue a request. Its `Reply` reports `is_wanted() == false`. Does
    /// nothing once the instance has stopped or failed.
    pub fn send(&self, request: S::Request) {
        if let Some(actor) = self.actor.upgrade() {
            let _ = actor.send(request, None, Mode::Send);
        }
    }
}
````

The `calls` mutex guards the state and the open calls together and is held while their deliveries are pushed. That is what guarantees that every completion reaches Emacs before the terminal `State` delivery, so `emrs.el` never drops a reply for an instance it has already removed.

- [ ] **Step 5: Write replies and streams, with their tests**

`emrs/src/runtime/reply.rs`:

````rust
//! Answering calls: `Reply` and `StreamSender`.

use super::cancel::CancelToken;
use super::core::{Delivery, Level, StreamEnd};
use super::instance::{Mode, Shared};
use crate::error::{Error, ErrorKind};
use crate::payload::Payload;
use serde::Serialize;
use std::marker::PhantomData;
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
    done: bool,
}

impl Reply {
    pub(crate) fn new(shared: Arc<Shared>, call: Option<u64>, mode: Mode, token: CancelToken) -> Reply {
        Reply { shared, call, mode, token, done: false }
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
                self.shared.complete_with(call, || {
                    vec![
                        Delivery::Item { instance, call, payload, stream: None, token },
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
````

Append the tests, which control draining directly through `Core::take` to observe backpressure:

````rust
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
````

The two `Drop` impls skip the `no-reply` completion when the token is cancelled. Without that, a producer that notices a stop and returns would report `no-reply` before the instance's termination reports `stopped`.

- [ ] **Step 6: Write the test host**

`emrs/src/testing.rs`:

````rust
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
````

- [ ] **Step 7: Register everything**

`emrs/src/runtime/mod.rs` (whole file):

````rust
//! The per-module runtime: pool, outbound queue, instances, replies.

pub(crate) mod cancel;
pub(crate) mod core;
pub(crate) mod instance;
pub(crate) mod pool;
pub(crate) mod reply;
````

`emrs/src/lib.rs` (whole file):

````rust
//! Message-passing services for Emacs dynamic modules.
#![allow(dead_code)] // Removed in Task 9, when every item has a user.

mod convert;
mod error;
mod ffi;
mod names;
mod payload;
mod runtime;
pub mod testing;

pub use error::{Error, ErrorKind};
pub use runtime::cancel::CancelToken;
pub use runtime::core::{Level, StreamEnd};
pub use runtime::instance::{Address, Context, Service, State};
pub use runtime::reply::{Reply, StreamClosed, StreamSender, TrySendError};
````

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p emrs`

Expected: `test result: ok. 51 passed` for the unit tests and `test result: ok. 24 passed` for `tests/runtime.rs`. Panic messages from the tests that provoke panics appear on stderr; that is expected.

Then run the suite five times to check for timing-dependent failures:

````bash
for i in 1 2 3 4 5; do cargo test -p emrs 2>&1 | grep -E '^test result|FAILED'; done
````

Expected: only `test result: ok` lines.

- [ ] **Step 9: Commit**

````bash
git add emrs
git commit -m "feat: actor runtime with replies, bounded streams and test host"
````

---

### Task 9: Module protocol, `module!` macro, test module

**Files:**
- Create: `emrs/src/module.rs`
- Create: `tests/emrs-test-module/Cargo.toml`, `tests/emrs-test-module/src/lib.rs`
- Create: `tests/run-ert.sh` (executable)
- Modify: `emrs/src/lib.rs`, `Cargo.toml`

**Interfaces:**
- Consumes: everything from Tasks 4 to 8, and `env!("EMRS_TARGET")` from Task 1.
- Produces:
  - The macro `emrs::module!("name", [ServiceA, ServiceB])`.
  - `#[doc(hidden)] emrs::__private::{init, entry, ServiceEntry}`.
  - For a module named `M`, the Lisp functions `M--emrs-info ()`, `M--emrs-attach (PIPE)`, `M--emrs-start (SERVICE ARGS)`, `M--emrs-send (ID REQUEST CALL-ID MODE)`, `M--emrs-cancel (ID CALL-ID)`, `M--emrs-stop (ID)`, `M--emrs-drain (MAX)`, `M--emrs-shutdown (TIMEOUT)`, and the feature `M-module`.
  - `drain` returns a list of vectors `[KIND INSTANCE-ID CALL-ID PAYLOAD]` as in spec 6.1. Errors are signalled as `(emrs-error KIND MESSAGE DATA)`.
  - The test module `emrs-test` with services `"types"` and `"probe"`, installed by `tests/run-ert.sh` as `target/elisp/emrs-test-module.<suffix>`.

- [ ] **Step 1: Write the test module**

`Cargo.toml` (whole file; adds the member):

````toml
[workspace]
resolver = "3"
members = ["emrs", "tests/emrs-test-module"]

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"

[profile.dev]
panic = "unwind"

[profile.release]
panic = "unwind"
````

`tests/emrs-test-module/Cargo.toml`:

````toml
[package]
name = "emrs-test-module"
version = "0.1.0"
publish = false
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
emrs = { path = "../../emrs" }
serde = { version = "1", features = ["derive"] }
serde_bytes = "0.11"
````

`tests/emrs-test-module/src/lib.rs`:

````rust
//! Module used by the ERT suite in `tests/elisp`.

use emrs::{Context, Error, Reply, Service};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::collections::BTreeMap;
use std::time::Duration;

// ---- "types": echoes values back to exercise the Lisp data mapping ----

#[derive(Serialize, Deserialize)]
struct Scalars {
    flag: bool,
    small: i8,
    big: u64,
    huge: i128,
    ratio: f64,
    letter: char,
    text: String,
    nothing: (),
    maybe: Option<i32>,
}

#[derive(Serialize, Deserialize)]
enum Shape {
    Dot,
    Circle(f64),
    Rect(f64, f64),
    Label { text: String, font_size: u32 },
}

#[derive(Serialize, Deserialize)]
struct Shapes {
    list: Vec<i32>,
    pair: (i32, String),
    map: BTreeMap<String, i32>,
    shapes: Vec<Shape>,
}

#[derive(Deserialize)]
enum TypesRequest {
    Scalars(Scalars),
    Shapes(Shapes),
    Shape(Shape),
    Bytes(ByteBuf),
}

struct Types;

impl Service for Types {
    const NAME: &'static str = "types";
    type Args = ();
    type Request = TypesRequest;
    type Event = ();

    fn start(_: (), _: &Context<Self>) -> Result<Self, Error> {
        Ok(Types)
    }

    fn handle(&mut self, request: TypesRequest, reply: Reply, _: &Context<Self>) {
        match request {
            TypesRequest::Scalars(v) => reply.ok(v),
            TypesRequest::Shapes(v) => reply.ok(v),
            TypesRequest::Shape(v) => reply.ok(v),
            TypesRequest::Bytes(v) => reply.ok(v),
        }
    }
}

// ---- "probe": behaves well or badly on request ----

/// A value whose serialization always fails.
struct Unencodable;

impl Serialize for Unencodable {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("cannot encode"))
    }
}

#[derive(Deserialize)]
enum Start {
    Ok,
    Fail,
    Panic,
}

#[derive(Deserialize)]
enum ProbeRequest {
    Echo(String),
    Fail,
    Forget,
    Panic,
    Emit(u32),
    Slow(u64),
    Count { n: u32, capacity: usize },
    Endless,
    BadReply,
    BadEvent,
    BadItem,
}

#[derive(Serialize)]
enum ProbeEvent {
    Tick(u32),
    ProducerStopped,
    StopHookRan,
    Bad(Unencodable),
}

/// A stream item that fails to encode when `bad` is set.
enum Item {
    Good(u32),
    Bad,
}

impl Serialize for Item {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Item::Good(n) => serializer.serialize_u32(*n),
            Item::Bad => Unencodable.serialize(serializer),
        }
    }
}

struct Probe {
    ctx: Context<Probe>,
}

impl Service for Probe {
    const NAME: &'static str = "probe";
    type Args = Start;
    type Request = ProbeRequest;
    type Event = ProbeEvent;

    fn start(args: Start, ctx: &Context<Self>) -> Result<Self, Error> {
        match args {
            Start::Ok => Ok(Probe { ctx: ctx.clone() }),
            Start::Fail => Err(Error::new("cannot start")),
            Start::Panic => panic!("start exploded"),
        }
    }

    fn handle(&mut self, request: ProbeRequest, reply: Reply, ctx: &Context<Self>) {
        match request {
            ProbeRequest::Echo(text) => reply.ok(text),
            ProbeRequest::Fail => reply.err(Error::with_data("nope", 7u32)),
            ProbeRequest::Forget => drop(reply),
            ProbeRequest::Panic => panic!("handler exploded"),
            ProbeRequest::Emit(n) => {
                for i in 0..n {
                    ctx.emit(ProbeEvent::Tick(i));
                }
                reply.ok(n);
            }
            ProbeRequest::Slow(ms) => {
                std::thread::sleep(Duration::from_millis(ms));
                reply.ok(ms);
            }
            ProbeRequest::Count { n, capacity } => {
                let out = reply.stream::<u32>(capacity);
                ctx.spawn(move |_| {
                    for i in 0..n {
                        if out.send(i).is_err() {
                            return;
                        }
                    }
                    out.finish();
                });
            }
            ProbeRequest::Endless => {
                let out = reply.stream::<u32>(64);
                let ctx = ctx.clone();
                ctx.clone().spawn(move |_| {
                    let mut i = 0;
                    while out.send(i).is_ok() {
                        i += 1;
                    }
                    ctx.emit(ProbeEvent::ProducerStopped);
                });
            }
            ProbeRequest::BadReply => reply.ok(Unencodable),
            ProbeRequest::BadEvent => {
                ctx.emit(ProbeEvent::Bad(Unencodable));
                reply.ok(());
            }
            ProbeRequest::BadItem => {
                let out = reply.stream::<Item>(8);
                let _ = out.send(Item::Good(1));
                let _ = out.send(Item::Bad);
                let _ = out.send(Item::Good(3));
                out.finish();
            }
        }
    }

    fn stop(&mut self) {
        self.ctx.emit(ProbeEvent::StopHookRan);
    }
}

emrs::module!("emrs-test", [Types, Probe]);
````

- [ ] **Step 2: Run the build to verify it fails**

Run: `cargo build -p emrs-test-module`

Expected: fails with `cannot find macro module in crate emrs` (wording varies by compiler version).

- [ ] **Step 3: Write the protocol implementation**

`emrs/src/module.rs`:

````rust
//! The module side of the protocol between `emrs.el` and a module:
//! initialisation, the `M--emrs-*` functions, and the wake-up pipe.

use crate::convert::de::from_lisp;
use crate::convert::ser::SerError;
use crate::error::{Error, ErrorKind, Fail};
use crate::ffi::env::{Env, Exit, Value};
use crate::ffi::sys;
use crate::runtime::core::{Core, Delivery, StreamEnd};
use crate::runtime::instance::{Actor, AnyActor, Mode, Service};
use std::os::raw::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

/// Version of the interface between `emrs.el` and modules.
pub const PROTOCOL: i64 = 1;

/// A service registered by [`crate::module!`].
pub struct ServiceEntry {
    name: &'static str,
    start: fn(&Arc<Core>, &Env, Value) -> Result<u64, Fail>,
}

/// Registry entry for service `S`.
pub fn entry<S: Service>() -> ServiceEntry {
    ServiceEntry { name: S::NAME, start: start_service::<S> }
}

fn start_service<S: Service>(core: &Arc<Core>, env: &Env, args: Value) -> Result<u64, Fail> {
    let args: S::Args = from_lisp(env, args)?;
    Ok(Actor::<S>::spawn(core, args).shared().id)
}

struct Module {
    name: &'static str,
    version: &'static str,
    core: Arc<Core>,
    /// Write end of the wake-up pipe, or -1 before `attach`.
    fd: Arc<AtomicI32>,
    services: Vec<ServiceEntry>,
}

fn write_wake_byte(fd: c_int) {
    // A failed write means the pipe is gone; emrs.el re-attaches and drains.
    unsafe {
        libc::write(fd, b"!".as_ptr() as *const c_void, 1);
    }
}

type Handler = fn(&Env, &Module, &[Value]) -> Result<Value, Fail>;

/// Body shared by all protocol functions: build the safe wrappers, contain
/// panics, and turn failures into Lisp signals.
unsafe fn run(
    env: *mut sys::emacs_env,
    nargs: isize,
    args: *mut sys::emacs_value,
    data: *mut c_void,
    handler: Handler,
) -> sys::emacs_value {
    let env = unsafe { Env::from_raw(env) };
    let module = unsafe { &*(data as *const Module) };
    let args: &[Value] =
        if nargs == 0 { &[] } else { unsafe { std::slice::from_raw_parts(args as *const Value, nargs as usize) } };
    let fail = match catch_unwind(AssertUnwindSafe(|| handler(&env, module, args))) {
        Ok(Ok(value)) => return value.0,
        Ok(Err(fail)) => fail,
        Err(_) => Fail::Error(Error::with_kind(ErrorKind::Protocol, "panic inside an emrs protocol function")),
    };
    if let Fail::Error(error) = fail {
        signal_error(&env, &error);
    }
    std::ptr::null_mut()
}

/// Leave `(signal 'emrs-error (KIND MESSAGE DATA))` pending.
fn signal_error(env: &Env, error: &Error) {
    let build = || -> Result<(Value, Value), Exit> {
        let data = match &error.data {
            Some(data) => match data.to_lisp(env) {
                Ok(value) => value,
                Err(_) => {
                    env.clear_exit();
                    env.nil()?
                }
            },
            None => env.nil()?,
        };
        let payload = env.list(&[env.intern(error.kind.as_str())?, env.make_string(&error.message)?, data])?;
        Ok((env.intern("emrs-error")?, payload))
    };
    if let Ok((symbol, payload)) = build() {
        env.signal(symbol, payload);
    }
}

fn error_plist(env: &Env, error: &Error) -> Result<Value, Exit> {
    let data = match &error.data {
        Some(data) => match data.to_lisp(env) {
            Ok(value) => value,
            Err(SerError::Exit) => return Err(Exit),
            Err(SerError::Msg(_)) => env.nil()?,
        },
        None => env.nil()?,
    };
    env.list(&[
        env.intern(":kind")?,
        env.intern(error.kind.as_str())?,
        env.intern(":message")?,
        env.make_string(&error.message)?,
        env.intern(":data")?,
        data,
    ])
}

fn optional_error(env: &Env, error: &Option<Error>) -> Result<Value, Exit> {
    match error {
        Some(error) => error_plist(env, error),
        None => env.nil(),
    }
}

fn record(env: &Env, kind: &str, instance: Option<u64>, call: Option<u64>, payload: Value) -> Result<Value, Exit> {
    let id = |n: Option<u64>| match n {
        Some(n) => env.make_big(false, n as u128),
        None => env.nil(),
    };
    env.call("vector", &[env.intern(kind)?, id(instance)?, id(call)?, payload])
}

fn instance_arg(env: &Env, module: &Module, value: Value) -> Result<Arc<dyn AnyActor>, Fail> {
    let id = env.extract_i64(value)?;
    let actor = u64::try_from(id).ok().and_then(|id| module.core.get(id));
    actor.ok_or_else(|| Fail::Error(Error::with_kind(ErrorKind::Stopped, "the instance is not running")))
}

fn call_arg(env: &Env, value: Value) -> Result<u64, Fail> {
    u64::try_from(env.extract_i64(value)?)
        .map_err(|_| Fail::Error(Error::with_kind(ErrorKind::Protocol, "call id must not be negative")))
}

fn info(env: &Env, module: &Module, _: &[Value]) -> Result<Value, Fail> {
    let mut services = Vec::new();
    for service in &module.services {
        services.push(env.make_string(service.name)?);
    }
    Ok(env.list(&[
        env.intern(":protocol")?,
        env.make_i64(PROTOCOL)?,
        env.intern(":emrs")?,
        env.make_string(env!("CARGO_PKG_VERSION"))?,
        env.intern(":module")?,
        env.make_string(module.name)?,
        env.intern(":version")?,
        env.make_string(module.version)?,
        env.intern(":services")?,
        env.list(&services)?,
        env.intern(":target")?,
        env.make_string(env!("EMRS_TARGET"))?,
    ])?)
}

fn attach(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let fd = env.open_channel(args[0])?;
    let old = module.fd.swap(fd, Ordering::SeqCst);
    if old >= 0 {
        unsafe {
            libc::close(old);
        }
    }
    Ok(env.nil()?)
}

fn start(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let name = env.string(args[0])?;
    let Some(service) = module.services.iter().find(|s| s.name == name) else {
        let message = format!("module {} has no service named \"{name}\"", module.name);
        return Err(Fail::Error(Error::with_kind(ErrorKind::Protocol, message)));
    };
    let id = (service.start)(&module.core, env, args[1])?;
    Ok(env.make_big(false, id as u128)?)
}

fn send(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let actor = instance_arg(env, module, args[0])?;
    let (call, mode) = if env.is_not_nil(args[3]) {
        let mode = match env.symbol_name(args[3])?.as_str() {
            "call" => Mode::Call,
            "stream" => Mode::Stream,
            other => {
                let message = format!("unknown request mode `{other}`");
                return Err(Fail::Error(Error::with_kind(ErrorKind::Protocol, message)));
            }
        };
        (Some(call_arg(env, args[2])?), mode)
    } else {
        (None, Mode::Send)
    };
    actor.send_lisp(env, args[1], call, mode)?;
    Ok(env.nil()?)
}

fn cancel(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let call = call_arg(env, args[1])?;
    if let Ok(actor) = instance_arg(env, module, args[0]) {
        actor.shared().cancel_call(call);
    }
    Ok(env.nil()?)
}

fn stop(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    if let Ok(actor) = instance_arg(env, module, args[0]) {
        actor.request_stop();
    }
    Ok(env.nil()?)
}

fn drain(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let max = usize::try_from(env.extract_i64(args[0])?).unwrap_or(1).max(1);
    let mut records = Vec::new();
    let mut deliveries = module.core.take(max).into_iter().peekable();
    while let Some(delivery) = deliveries.next() {
        match delivery {
            Delivery::Event { instance, payload } => match payload.to_lisp(env) {
                Ok(value) => records.push(record(env, "event", Some(instance), None, value)?),
                Err(SerError::Exit) => return Err(Fail::Exit),
                Err(SerError::Msg(message)) => {
                    let text = format!("instance {instance}: an event could not be encoded: {message}");
                    let payload = env.cons(env.intern("error")?, env.make_string(&text)?)?;
                    records.push(record(env, "log", None, None, payload)?);
                }
            },
            Delivery::Reply { instance, call, payload } => match payload.to_lisp(env) {
                Ok(value) => records.push(record(env, "reply", Some(instance), Some(call), value)?),
                Err(SerError::Exit) => return Err(Fail::Exit),
                Err(SerError::Msg(message)) => {
                    let error = Error::with_kind(ErrorKind::Encode, message);
                    records.push(record(env, "error", Some(instance), Some(call), error_plist(env, &error)?)?);
                }
            },
            Delivery::Error { instance, call, error } => {
                records.push(record(env, "error", Some(instance), Some(call), error_plist(env, &error)?)?);
            }
            Delivery::Item { instance, call, payload, token, .. } => {
                // Merge the run of items of this stream into one record.
                let mut run = vec![payload];
                while let Some(Delivery::Item { instance: i, call: c, .. }) = deliveries.peek() {
                    if (*i, *c) != (instance, call) {
                        break;
                    }
                    if let Some(Delivery::Item { payload, .. }) = deliveries.next() {
                        run.push(payload);
                    }
                }
                if token.is_cancelled() {
                    continue;
                }
                let mut values = Vec::with_capacity(run.len());
                let mut failure = None;
                for payload in run {
                    match payload.to_lisp(env) {
                        Ok(value) => values.push(value),
                        Err(SerError::Exit) => return Err(Fail::Exit),
                        Err(SerError::Msg(message)) => {
                            failure = Some(message);
                            break;
                        }
                    }
                }
                if !values.is_empty() {
                    records.push(record(env, "items", Some(instance), Some(call), env.list(&values)?)?);
                }
                if let Some(message) = failure {
                    if let Some(actor) = module.core.get(instance) {
                        actor.shared().cancel_call(call);
                    }
                    token.cancel();
                    let error = Error::with_kind(ErrorKind::Encode, message);
                    let payload = env.cons(env.intern("failed")?, error_plist(env, &error)?)?;
                    records.push(record(env, "end", Some(instance), Some(call), payload)?);
                }
            }
            Delivery::End { instance, call, status, error } => {
                let status = env.intern(if status == StreamEnd::Done { "done" } else { "failed" })?;
                let payload = env.cons(status, optional_error(env, &error)?)?;
                records.push(record(env, "end", Some(instance), Some(call), payload)?);
            }
            Delivery::State { instance, state, error } => {
                let payload = env.cons(env.intern(state.as_str())?, optional_error(env, &error)?)?;
                records.push(record(env, "state", Some(instance), None, payload)?);
            }
            Delivery::Log { level, message } => {
                let payload = env.cons(env.intern(level.as_str())?, env.make_string(&message)?)?;
                records.push(record(env, "log", None, None, payload)?);
            }
        }
    }
    Ok(env.list(&records)?)
}

fn shutdown(env: &Env, module: &Module, args: &[Value]) -> Result<Value, Fail> {
    let seconds = env.extract_float(env.call("float", &[args[0]])?)?;
    let all_stopped = module.core.shutdown(Duration::from_secs_f64(seconds.clamp(0.0, 3600.0)));
    Ok(if all_stopped { env.t()? } else { env.nil()? })
}

macro_rules! protocol_fn {
    ($name:ident => $handler:path) => {
        unsafe extern "C" fn $name(
            env: *mut sys::emacs_env,
            nargs: isize,
            args: *mut sys::emacs_value,
            data: *mut c_void,
        ) -> sys::emacs_value {
            unsafe { run(env, nargs, args, data, $handler) }
        }
    };
}

protocol_fn!(raw_info => info);
protocol_fn!(raw_attach => attach);
protocol_fn!(raw_start => start);
protocol_fn!(raw_send => send);
protocol_fn!(raw_cancel => cancel);
protocol_fn!(raw_stop => stop);
protocol_fn!(raw_drain => drain);
protocol_fn!(raw_shutdown => shutdown);

fn define(env: &Env, module: &'static Module) -> Result<(), Exit> {
    let functions: [(&str, isize, sys::emacs_function); 8] = [
        ("info", 0, raw_info),
        ("attach", 1, raw_attach),
        ("start", 2, raw_start),
        ("send", 4, raw_send),
        ("cancel", 2, raw_cancel),
        ("stop", 1, raw_stop),
        ("drain", 1, raw_drain),
        ("shutdown", 1, raw_shutdown),
    ];
    let data = module as *const Module as *mut c_void;
    for (op, arity, function) in functions {
        let symbol = env.intern(&format!("{}--emrs-{op}", module.name))?;
        let function = env.make_function(arity, arity, function, data)?;
        env.call("fset", &[symbol, function])?;
    }
    env.call("provide", &[env.intern(&format!("{}-module", module.name))?])?;
    Ok(())
}

/// Entry point called by the code [`crate::module!`] generates.
///
/// # Safety
/// `runtime` must be the pointer Emacs passed to `emacs_module_init`.
pub unsafe fn init(
    runtime: *mut c_void,
    name: &'static str,
    version: &'static str,
    services: Vec<ServiceEntry>,
) -> c_int {
    let runtime = runtime as *mut sys::emacs_runtime;
    if unsafe { (*runtime).size } < std::mem::size_of::<sys::emacs_runtime>() as isize {
        return 1;
    }
    let raw_env = unsafe { ((*runtime).get_environment)(runtime) };
    if unsafe { (*raw_env).size } < std::mem::size_of::<sys::emacs_env>() as isize {
        return 2;
    }
    let mut names: Vec<&str> = services.iter().map(|s| s.name).collect();
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return 3;
    }
    let fd = Arc::new(AtomicI32::new(-1));
    let wake_fd = fd.clone();
    let core = Core::new(name, move || {
        let fd = wake_fd.load(Ordering::SeqCst);
        if fd >= 0 {
            write_wake_byte(fd);
        }
    });
    let module: &'static Module = Box::leak(Box::new(Module { name, version, core, fd, services }));
    let env = unsafe { Env::from_raw(raw_env) };
    match catch_unwind(AssertUnwindSafe(|| define(&env, module))) {
        Ok(Ok(())) => 0,
        _ => 4,
    }
}

/// Define the entry point of an Emacs module that hosts the given services.
///
/// ```ignore
/// emrs::module!("git-rs", [Git, Blame]);
/// ```
///
/// The module must be built as a `cdylib` with `panic = "unwind"` and
/// installed as `git-rs-module` plus the platform's module suffix.
#[macro_export]
macro_rules! module {
    ($name:literal, [$($service:ty),+ $(,)?]) => {
        #[cfg(panic = "abort")]
        compile_error!("emrs modules must be built with panic = \"unwind\"; a panic would otherwise abort Emacs");

        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static plugin_is_GPL_compatible: ::std::os::raw::c_int = 0;

        /// # Safety
        /// Called by Emacs when it loads the module.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn emacs_module_init(runtime: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int {
            unsafe {
                $crate::__private::init(
                    runtime,
                    $name,
                    env!("CARGO_PKG_VERSION"),
                    vec![$($crate::__private::entry::<$service>()),+],
                )
            }
        }
    };
}
````

Points to check in review: every `extern "C"` function goes through `run`, which contains panics; a `Fail::Exit` returns to Emacs without touching the pending signal; `drain` merges consecutive items of one stream into one `items` record; an item that cannot be encoded ends that stream with `(failed . encode-error)` and cancels it.

- [ ] **Step 4: Finalise the crate root**

`emrs/src/lib.rs` (whole file). The `allow(dead_code)` line is gone.

````rust
//! Message-passing services for Emacs dynamic modules.
//!
//! Implement [`Service`], register it with [`module!`], and talk to it from
//! Elisp through `emrs.el`. Service code never runs on the Emacs thread and
//! never sees the Emacs module API.

mod convert;
mod error;
mod ffi;
mod module;
mod names;
mod payload;
mod runtime;
pub mod testing;

pub use error::{Error, ErrorKind};
pub use runtime::cancel::CancelToken;
pub use runtime::core::{Level, StreamEnd};
pub use runtime::instance::{Address, Context, Service, State};
pub use runtime::reply::{Reply, StreamClosed, StreamSender, TrySendError};

#[doc(hidden)]
pub mod __private {
    pub use crate::module::{ServiceEntry, entry, init};
}
````

- [ ] **Step 5: Build and lint**

Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check && cargo test --workspace`

Expected: no warnings, no formatting differences, `51 passed` and `24 passed`.

- [ ] **Step 6: Write the runner script**

`tests/run-ert.sh`:

````sh
#!/bin/sh
# Build the test module, install it under the name Emacs expects, and run
# the ERT suite with module assertions enabled.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build -p emrs-test-module
out="$root/target/elisp"
mkdir -p "$out"
for ext in so dylib dll; do
    for built in "target/debug/libemrs_test_module.$ext" "target/debug/emrs_test_module.$ext"; do
        if [ -f "$built" ]; then
            cp "$built" "$out/emrs-test-module.$ext"
        fi
    done
done
exec "${EMACS:-emacs}" --batch --module-assertions \
    -L "$root/elisp" -L "$out" \
    -l "$root/tests/elisp/emrs-tests.el" \
    -f ert-run-tests-batch-and-exit
````

Make it executable:

````bash
chmod +x tests/run-ert.sh
````

- [ ] **Step 7: Smoke-test the module in Emacs**

The ERT file does not exist yet, so install the module by hand and call a protocol function directly:

````bash
cargo build -p emrs-test-module
mkdir -p target/elisp
cp target/debug/libemrs_test_module.so target/elisp/emrs-test-module.so   # .dylib on macOS
emacs --batch --module-assertions --eval '(progn (module-load (expand-file-name "target/elisp/emrs-test-module.so")) (princ (emrs-test--emrs-info)))'
````

Expected output (the target triple is that of your machine):

````text
(:protocol 1 :emrs 0.1.0 :module emrs-test :version 0.1.0 :services (types probe) :target x86_64-unknown-linux-gnu)
````

- [ ] **Step 8: Commit**

````bash
git add Cargo.toml emrs tests
git commit -m "feat: module protocol functions, module! macro and test module"
````

---

### Task 10: Elisp library and end-to-end tests

**Files:**
- Create: `elisp/emrs.el`
- Create: `tests/elisp/emrs-tests.el`

**Interfaces:**
- Consumes: the protocol functions and delivery vectors of Task 9; the test module's services. `probe` requests: `(:echo S)`, `(:fail)`, `(:forget)`, `(:panic)`, `(:emit N)`, `(:slow MS)`, `(:count :n N :capacity C)`, `(:endless)`, `(:bad-reply)`, `(:bad-event)`, `(:bad-item)`; start args `:ok`, `:fail`, `:panic`; events `(:tick N)`, `(:producer-stopped)`, `(:stop-hook-ran)`. `types` requests: `(:scalars PLIST)`, `(:shapes PLIST)`, `(:shape SHAPE)`, `(:bytes STRING)`.
- Produces: the public functions of spec section 5: `emrs-load-module`, `emrs-module-info`, `emrs-start`, `emrs-send`, `emrs-call`, `emrs-call-sync`, `emrs-call-stream`, `emrs-cancel`, `emrs-stop`, `emrs-instance-state`; the options `emrs-drain-max` and `emrs-shutdown-timeout`; the error `emrs-error`; the helper `emrs--hash-to-alist` that modules call.

- [ ] **Step 1: Write the failing tests**

`tests/elisp/emrs-tests.el`:

````elisp
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

;;;; Events

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
````

- [ ] **Step 2: Run the tests to verify they fail**

Run: `tests/run-ert.sh`

Expected: Emacs exits with `Cannot open load file: No such file or directory, emrs`.

- [ ] **Step 3: Write the library**

`elisp/emrs.el`:

````elisp
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
  (instances (make-hash-table :test 'eql)))

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

(defvar emrs--shutting-down nil
  "Non-nil while Emacs is exiting.")

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
         (pipe (make-pipe-process
                :name (format " emrs-%s" name)
                :buffer nil
                :noquery t
                :coding 'binary
                :filter (lambda (_process _string) (emrs--drain m))
                :sentinel (lambda (process _event)
                            (when (and (not emrs--shutting-down)
                                       (not (process-live-p process))
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

(defun emrs--drain (m)
  "Fetch pending deliveries of module M and dispatch them."
  (dolist (delivery (funcall (emrs--fn (emrs--module-name m) 'drain) emrs-drain-max))
    (emrs--dispatch m delivery)))

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
        (condition-case err
            (funcall send id request call-id mode)
          (error
           (remhash call-id (emrs-instance-calls inst))
           (signal (car err) (cdr err))))
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
event handlers and other callbacks may run."
  (let* ((m (emrs-instance-module instance))
         (deadline (and timeout (+ (float-time) timeout)))
         (finished nil) (reply nil) (failure nil)
         (call (emrs-call instance request
                          (lambda (value err)
                            (setq reply value failure err finished t)))))
    (unwind-protect
        (while (not finished)
          (emrs--drain m)
          (unless finished
            (let ((remaining (and deadline (- deadline (float-time)))))
              (if (and remaining (<= remaining 0))
                  (progn
                    (setf (emrs-call-callback call) nil)
                    (emrs-cancel call)
                    (setq failure (emrs--err 'timeout "No reply within the timeout")
                          finished t))
                (accept-process-output (emrs--module-pipe m)
                                       (if remaining (min remaining 0.1) 0.1))))))
      ;; Reached with FINISHED nil only on a non-local exit such as a quit.
      (unless finished
        (setf (emrs-call-callback call) nil)
        (emrs-cancel call)))
    (when failure (emrs--signal failure))
    reply))

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
  (setq emrs--shutting-down t)
  (maphash (lambda (name _m)
             (ignore-errors
               (funcall (emrs--fn name 'shutdown) emrs-shutdown-timeout)))
           emrs--modules))

(add-hook 'kill-emacs-hook #'emrs--shutdown)

(provide 'emrs)
;;; emrs.el ends here
````

`emrs-call-sync` drains the module directly on every loop turn and uses `accept-process-output` only as a sleep that ends early when the wake-up byte arrives, in slices of at most 0.1 s. The `unwind-protect` is what cancels the call on `C-g`.

- [ ] **Step 4: Byte-compile with warnings as errors**

Run: `emacs --batch -L elisp --eval '(setq byte-compile-error-on-warn t)' -f batch-byte-compile elisp/emrs.el && rm elisp/emrs.elc`

Expected: no output and exit status 0.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `tests/run-ert.sh`

Expected: `Ran 37 tests, 37 results as expected, 0 unexpected`. The output also shows one `Error in on-event: (error "Boom")` warning and two Rust panic messages (`handler exploded`, `start exploded`); the tests provoke these on purpose.

Run it five times to check for timing-dependent failures:

````bash
for i in 1 2 3 4 5; do tests/run-ert.sh 2>&1 | grep -E '^Ran |unexpected'; done
````

Expected: five identical `Ran 37 tests, 37 results as expected` lines.

- [ ] **Step 6: Commit**

````bash
git add elisp tests
git commit -m "feat: emrs.el and end-to-end ERT suite"
````

---

### Task 11: Example modules

**Files:**
- Create: `examples/echo/Cargo.toml`, `examples/echo/src/lib.rs`
- Create: `examples/grep/Cargo.toml`, `examples/grep/src/lib.rs`
- Create: `examples/examples-test.el`, `examples/run-examples.sh` (executable)
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: the public Rust API and `emrs.el`.
- Produces: modules `echo-rs` (service `"echo"`, request `(:echo STRING)`) and `grep-rs` (service `"grep"`, request `(:grep :pattern STRING :root DIR)`, stream items `(:path P :line-number N :preview S)`). These are the examples of spec section 10 and the evidence for acceptance criteria 1 and 6.

- [ ] **Step 1: Write the failing smoke tests**

`examples/examples-test.el`:

````elisp
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
````

`examples/run-examples.sh`:

````sh
#!/bin/sh
# Build the example modules, install them under the names Emacs expects,
# and run their smoke tests.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build -p echo-rs -p grep-rs
out="$root/target/elisp"
mkdir -p "$out"
for name in echo grep; do
    for ext in so dylib dll; do
        for built in "target/debug/lib${name}_rs.$ext" "target/debug/${name}_rs.$ext"; do
            if [ -f "$built" ]; then
                cp "$built" "$out/$name-rs-module.$ext"
            fi
        done
    done
done
exec "${EMACS:-emacs}" --batch --module-assertions \
    -L "$root/elisp" -L "$out" \
    -l "$root/examples/examples-test.el" \
    -f ert-run-tests-batch-and-exit
````

````bash
chmod +x examples/run-examples.sh
````

- [ ] **Step 2: Run the tests to verify they fail**

Run: `examples/run-examples.sh`

Expected: cargo fails with `package ID specification echo-rs did not match any packages`.

- [ ] **Step 3: Write the echo example**

`Cargo.toml` (whole file; adds the two members):

````toml
[workspace]
resolver = "3"
members = ["emrs", "tests/emrs-test-module", "examples/echo", "examples/grep"]

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"

[profile.dev]
panic = "unwind"

[profile.release]
panic = "unwind"
````

`examples/echo/Cargo.toml`:

````toml
[package]
name = "echo-rs"
version = "0.1.0"
publish = false
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
emrs = { path = "../../emrs" }
serde = { version = "1", features = ["derive"] }
````

`examples/echo/src/lib.rs`:

````rust
//! The smallest emrs module: one service that echoes a string.
//!
//! ```elisp
//! (setq e (emrs-start 'echo-rs "echo" nil))
//! (emrs-call-sync e '(:echo "hi") 1.0)   ; => "hi"
//! ```

use emrs::{Context, Error, Reply, Service};
use serde::Deserialize;

#[derive(Deserialize)]
enum Request {
    Echo(String),
}

struct Echo;

impl Service for Echo {
    const NAME: &'static str = "echo";
    type Args = ();
    type Request = Request;
    type Event = ();

    fn start(_: (), _: &Context<Self>) -> Result<Self, Error> {
        Ok(Echo)
    }

    fn handle(&mut self, request: Request, reply: Reply, _: &Context<Self>) {
        match request {
            Request::Echo(text) => reply.ok(text),
        }
    }
}

emrs::module!("echo-rs", [Echo]);
````

- [ ] **Step 4: Write the grep example**

`examples/grep/Cargo.toml`:

````toml
[package]
name = "grep-rs"
version = "0.1.0"
publish = false
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
emrs = { path = "../../emrs" }
serde = { version = "1", features = ["derive"] }
````

`examples/grep/src/lib.rs`:

````rust
//! A streaming search service: finds lines containing a substring in all
//! files below a directory and streams the matches to Emacs.
//!
//! ```elisp
//! (setq g (emrs-start 'grep-rs "grep" nil))
//! (setq call (emrs-call-stream g '(:grep :pattern "TODO" :root "/src")
//!              :on-batch (lambda (items) (message "%d matches" (length items)))
//!              :on-end (lambda (status err) (message "grep: %s" status))))
//! (emrs-cancel call)   ; e.g. when the user changes the query
//! ```

use emrs::{CancelToken, Context, Error, Reply, Service, StreamSender};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
enum Request {
    Grep { pattern: String, root: PathBuf },
}

#[derive(Serialize)]
struct Match {
    path: PathBuf,
    line_number: u64,
    preview: String,
}

struct Grep;

/// Search `dir` recursively. Returns false once the stream is closed.
fn search(dir: &Path, pattern: &str, out: &StreamSender<Match>, token: &CancelToken) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    for entry in entries.flatten() {
        if token.is_cancelled() {
            return false;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            if !search(&path, pattern, out, token) {
                return false;
            }
        } else if file_type.is_file() {
            let Ok(file) = std::fs::File::open(&path) else { continue };
            for (index, line) in BufReader::new(file).lines().enumerate() {
                // A line that is not UTF-8 means a binary file: skip the rest.
                let Ok(line) = line else { break };
                if line.contains(pattern) {
                    let found = Match { path: path.clone(), line_number: index as u64 + 1, preview: line };
                    if out.send(found).is_err() {
                        return false;
                    }
                }
            }
        }
    }
    true
}

impl Service for Grep {
    const NAME: &'static str = "grep";
    type Args = ();
    type Request = Request;
    type Event = ();

    fn start(_: (), _: &Context<Self>) -> Result<Self, Error> {
        Ok(Grep)
    }

    fn handle(&mut self, request: Request, reply: Reply, ctx: &Context<Self>) {
        match request {
            Request::Grep { pattern, root } => {
                if !root.is_dir() {
                    reply.err(Error::new(format!("{} is not a directory", root.display())));
                    return;
                }
                let out = reply.stream::<Match>(1024);
                ctx.spawn(move |_| {
                    let token = out.token();
                    if search(&root, &pattern, &out, &token) {
                        out.finish();
                    }
                });
            }
        }
    }
}

emrs::module!("grep-rs", [Grep]);
````

- [ ] **Step 5: Run the tests to verify they pass**

Run: `examples/run-examples.sh && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`

Expected: `Ran 3 tests, 3 results as expected, 0 unexpected`, no lint warnings, no formatting differences.

- [ ] **Step 6: Commit**

````bash
git add Cargo.toml examples
git commit -m "feat: echo and streaming grep example modules"
````

---

### Task 12: README, licences and CI

**Files:**
- Create: `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `tests/run-ert.sh`, `examples/run-examples.sh`.
- Produces: the CI matrix of spec section 11. This task has no local test; its test is the first CI run.

- [ ] **Step 1: Write the README**

`README.md`:

````markdown
# emrs

Write Emacs dynamic modules in Rust as message-passing services.

Rust owns threads and state, Emacs owns its event loop and UI, and typed
messages are the only boundary between them. Service code never runs on the
Emacs thread and never sees the Emacs module API.

- `emrs/` is the Rust crate.
- `elisp/emrs.el` is the companion Elisp library (Emacs 30.1 or later).
- `examples/` holds two complete modules: `echo` and a streaming `grep`.
- `docs/specs/2026-10-03-emrs-core.md` is the full specification.

## A module in 25 lines

```rust
use emrs::{Context, Error, Reply, Service};
use serde::Deserialize;

#[derive(Deserialize)]
enum Request {
    Echo(String),
}

struct Echo;

impl Service for Echo {
    const NAME: &'static str = "echo";
    type Args = ();
    type Request = Request;
    type Event = ();

    fn start(_: (), _: &Context<Self>) -> Result<Self, Error> {
        Ok(Echo)
    }

    fn handle(&mut self, request: Request, reply: Reply, _: &Context<Self>) {
        match request {
            Request::Echo(text) => reply.ok(text),
        }
    }
}

emrs::module!("echo-rs", [Echo]);
```

```elisp
(require 'emrs)
(setq e (emrs-start 'echo-rs "echo" nil))
(emrs-call-sync e '(:echo "hi") 1.0)   ; => "hi"
```

## Building a module

1. Make the crate a `cdylib` and keep `panic = "unwind"` (the default).
2. `cargo build --release`.
3. Copy the library to a directory on `load-path` under the name
   `<module>-module` plus Emacs's `module-file-suffix`, for example
   `target/release/libecho_rs.so` to `echo-rs-module.so`.

Emacs cannot unload a module; restart Emacs to load a new build.

## Tests

```sh
cargo test --workspace     # runtime and data mapping, no Emacs needed
tests/run-ert.sh           # end to end in Emacs, with module assertions
examples/run-examples.sh   # the example modules
```

## Licence

The Rust crates are licensed under either MIT or Apache-2.0, at your option.
`elisp/emrs.el` is licensed under GPL-3.0-or-later.
````

- [ ] **Step 2: Add the licence files**

`LICENSE-MIT`:

````text
MIT License

Copyright (c) 2026 The emrs authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
````

`LICENSE-APACHE` is the unmodified Apache License 2.0 text:

````bash
curl -fsSL https://www.apache.org/licenses/LICENSE-2.0.txt -o LICENSE-APACHE
grep -c 'Apache License' LICENSE-APACHE
````

Expected: a count of at least 1.

- [ ] **Step 3: Write the CI workflow**

`.github/workflows/ci.yml`:

````yaml
name: CI

on:
  push:
  pull_request:

jobs:
  rust:
    name: Rust checks
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace

  msrv:
    name: Minimum supported Rust
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.85
      - run: cargo check -p emrs

  emacs:
    name: Emacs ${{ matrix.emacs }} on ${{ matrix.os }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, ubuntu-24.04-arm, macos-latest]
        emacs: ["30.1", "31.1"]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: purcell/setup-emacs@master
        with:
          version: ${{ matrix.emacs }}
      - run: tests/run-ert.sh
      - run: examples/run-examples.sh

  macos-x86_64-build:
    name: Build for macOS x86_64
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-apple-darwin
      - run: cargo build --workspace --target x86_64-apple-darwin

  windows:
    name: Windows (best effort)
    runs-on: windows-latest
    continue-on-error: true
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: jcs090218/setup-emacs-windows@master
        with:
          version: "30.1"
      - run: tests/run-ert.sh
        shell: bash
````

This file could not be run while the plan was written. On the first push, check three things and adjust the file if needed: that `purcell/setup-emacs` offers versions `30.1` and `31.1` on all three runners (use the nearest available patch release otherwise); that the `ubuntu-24.04-arm` runner label is available to the repository; and whether the Windows job passes. A passing Windows job settles spec item 13.1; a failing one stays allowed through `continue-on-error`.

- [ ] **Step 4: Run the complete local verification**

````bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
tests/run-ert.sh
examples/run-examples.sh
````

Expected: no formatting differences, no warnings, `51 passed` and `24 passed`, `Ran 37 tests, 37 results as expected`, `Ran 3 tests, 3 results as expected`.

- [ ] **Step 5: Commit**

````bash
git add README.md LICENSE-MIT LICENSE-APACHE .github
git commit -m "docs: README, licences and CI workflow"
````

---

## Acceptance criteria (spec section 12) and where they are checked

| # | Criterion | Check |
|---|---|---|
| 1 | The two examples build, load and behave as shown | `examples/run-examples.sh` (Task 11) |
| 2 | A handler sleeping for seconds does not delay Emacs or other instances | `a_slow_instance_does_not_delay_another` (Task 8), `emrs-test-slow-instance-does-not-delay-another` (Task 10) |
| 3 | A large stream completes with bounded memory and Emacs stays responsive | `producer_blocks_at_capacity_until_emacs_drains` (Task 8), `emrs-test-large-stream-keeps-emacs-responsive` with 200,000 items and a timer that must keep firing (Task 10) |
| 4 | Cancelling a stream stops deliveries at once and the producer notices | `cancel_wakes_a_blocked_producer`, `cancelling_a_stream_stops_the_producer_promptly` (Task 8), `emrs-test-cancel-stream` (Task 10) |
| 5 | A handler panic fails only that instance and completes its open calls | `handler_panic_fails_the_instance_and_completes_every_open_call` (Task 8), `emrs-test-handler-panic-fails-only-that-instance` (Task 10) |
| 6 | Two modules run side by side | `examples-modules-coexist` (Task 11). The modules share one `emrs` version here; modules built against different versions cannot be tested until a second release exists, and `emrs-test-newer-protocol-is-rejected` covers the version check. |
| 7 | The ERT suite passes under `--module-assertions` | `tests/run-ert.sh` (Task 10) |

Criterion 3 in the spec says one million items. The ERT test uses 200,000 to keep the suite fast; raising `:n` to 1000000 is a one-word change for a manual check.
