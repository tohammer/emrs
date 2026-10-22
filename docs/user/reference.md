# Reference

The Elisp API of `emrs.el`, the error kinds, how services behave and how
Lisp values map to Rust types. For the Rust API run
`cargo doc -p emrs --open` in a module crate.

- [Elisp functions](#elisp-functions)
- [Callbacks](#callbacks)
- [Instance states](#instance-states)
- [User options](#user-options)
- [Error kinds](#error-kinds)
- [Service behaviour](#service-behaviour)
- [Data mapping](#data-mapping)

## Elisp functions

`MODULE` is a symbol such as `echo-rs`; `SERVICE` is the string
`Service::NAME`.

| Function | Returns | Description |
|---|---|---|
| `(emrs-start MODULE SERVICE ARGS &key on-event on-exit)` | instance | Start an instance; `ARGS` is decoded to `Service::Args`. Loads the module if needed |
| `(emrs-stop INSTANCE)` | `nil` | Stop the instance; returns at once, `on-exit` runs when it has stopped. Does nothing unless the instance is `starting` or `running` |
| `(emrs-send INSTANCE REQUEST)` | `nil` | Send a request without expecting an answer |
| `(emrs-call INSTANCE REQUEST CALLBACK)` | call | Send a request; `CALLBACK` gets the reply |
| `(emrs-call-sync INSTANCE REQUEST &optional TIMEOUT)` | reply | Send a request and wait for the reply |
| `(emrs-call-stream INSTANCE REQUEST &key on-batch on-end)` | call | Send a request that is answered with a stream |
| `(emrs-call-stream-sync INSTANCE REQUEST &optional TIMEOUT)` | list | Send a request and wait for the stream to end; returns all items in order |
| `(emrs-cancel CALL)` | | Cancel a call of `emrs-call` or `emrs-call-stream`. Does nothing on a finished call |
| `(emrs-instance-state INSTANCE)` | symbol | `starting`, `running`, `stopping`, `stopped` or `failed` |
| `(emrs-load-module MODULE &optional FILE)` | handle | Load the module from `FILE`, or `<module>-module` on `load-path`. Loading again does nothing |
| `(emrs-module-info MODULE)` | plist | Keys `:protocol :emrs :module :version :services :target` |

- Requests and `ARGS` are decoded on the Emacs thread, so a malformed one
  signals `emrs-error` of kind `decode` at once and nothing is sent.
- Sending to an instance that is `stopping`, `stopped` or `failed` signals
  kind `stopped`.
- `emrs-cancel` completes the call at once: the callback gets kind
  `cancelled`, `on-end` gets status `cancelled`. Later deliveries for the
  call are dropped, and the service sees its cancel token set.

### Synchronous calls

`emrs-call-sync` and `emrs-call-stream-sync` signal `emrs-error` with the
data `(KIND MESSAGE DATA)` on failure.

- `TIMEOUT` is in seconds. On timeout the call is cancelled and kind
  `timeout` is signalled. Without it they wait until the answer arrives.
- `C-g` cancels the call and quits.
- They read only the module, so the filters of other processes do not run
  while they wait; timers do.
- Other deliveries of the same module are dispatched while they wait, so
  event handlers and other callbacks may run. Inside an emrs callback, only
  the call's own answer is dispatched; everything else waits, in order,
  until that callback returns.
- `emrs-call-stream-sync` signals the stream's error when it fails and kind
  `cancelled` when the service cancels it; the items received until then
  are dropped.

## Callbacks

| Callback | Called as | When |
|---|---|---|
| `on-event` | `(F INSTANCE EVENT)` | For each `ctx.emit(event)` |
| `on-exit` | `(F INSTANCE STATUS ERR)` | Once; `STATUS` is `stopped` or `failed` |
| `CALLBACK` of `emrs-call` | `(F REPLY ERR)` | Once; `ERR` is `nil` on success, otherwise an error plist |
| `on-batch` | `(F ITEMS)` | For each batch; `ITEMS` is a non-empty list, in order |
| `on-end` | `(F STATUS ERR)` | Once; `STATUS` is `done`, `failed` or `cancelled`; no batch follows |

An error plist is `(:kind KIND :message MESSAGE :data DATA)`. An error
signalled in a callback is written to `*emrs-log*` and shown as a warning;
the other deliveries still run.

## Instance states

```text
starting ──start ok──► running ──emrs-stop / ctx.stop()──► stopping ──► stopped
    │                     │                                    │
    └─ start Err / panic ─┴──────── panic in handle ───────────┴──► failed
```

- `starting`: `start` runs; requests wait.
- `running`: requests are handled one at a time, in order.
- `stopping`: requests still waiting are not handled; their calls end with
  `stopped`. A handler in progress finishes, then `stop` runs.
- `stopped`, `failed`: open calls end with that kind, then `on-exit` runs.
- There is no automatic restart; restart from `on-exit` if needed.
- At Emacs exit, all instances are stopped; Emacs waits up to
  `emrs-shutdown-timeout` for their `stop` to finish.

## User options

Group `emrs`.

| Option | Default | Description |
|---|---|---|
| `emrs-drain-max` | `512` | Deliveries converted to Lisp at a time; lower keeps Emacs more responsive under load |
| `emrs-shutdown-timeout` | `0.5` | Seconds to wait at Emacs exit for services to stop |

Log lines of services (`ctx.log`) and errors in callbacks go to the buffer
`*emrs-log*`.

## Error kinds

| Kind | Meaning |
|---|---|
| `service` | The service answered with `reply.err` or `fail`; `:data` holds the optional data of the error |
| `no-reply` | The service dropped the reply or stream without answering, for example after a panic in a spawned task |
| `cancelled` | The call was cancelled |
| `timeout` | A synchronous call timed out |
| `stopped` | The instance stopped before answering, or was not running |
| `failed` | The instance failed (panic or `start` error) before answering |
| `decode` | A request or `ARGS` did not fit the Rust type; the message names the path to the value |
| `encode` | A reply or item could not be converted to Lisp |
| `protocol` | Version or mode mismatch, unknown service, or internal fault |

## Service behaviour

### Answer and call mode

The caller chooses the mode; the service chooses how to answer.

| Caller used | Service did | Result |
|---|---|---|
| `emrs-call` | `reply.stream()` | Error `protocol` |
| `emrs-call-stream` | `reply.ok(v)` | One batch `(v)`, then `done` |
| `emrs-call-stream` | `reply.err(e)` | `failed` with `e`, no items |
| `emrs-send` | `reply.ok(v)` | Discarded |
| `emrs-send` | `reply.err(e)` | Logged to `*emrs-log*` |
| `emrs-send` | `reply.stream()` | The stream is closed at once |

### Panics

| Where | Result |
|---|---|
| `start` | The instance fails |
| `handle` or `stop` | The instance fails; that call ends with `no-reply`, other open calls with `failed`; `on-exit` runs |
| A spawned task | Logged; the instance keeps running; replies and streams the task held end with `no-reply` |

An event that cannot be converted to Lisp is logged and dropped.

## Data mapping

Requests, `ARGS`, replies, stream items and events are converted with
serde.

### Names

Struct fields and enum variants become keywords in kebab-case:
`line_number` → `:line-number`, `HTTPServer` → `:http-server`. Names
changed with `#[serde(rename …)]` are converted the same way.

### Rust to Lisp

| Rust | Lisp |
|---|---|
| `bool` | `t` / `nil` |
| integers up to 128 bit | integer |
| `f32`, `f64` | float |
| `char` | character |
| `str`, `String`, `PathBuf` | multibyte string |
| bytes (`serde_bytes`) | unibyte string |
| `None`, `()`, unit struct | `nil` |
| `Some(x)`, newtype struct | as `x` |
| sequence, tuple, tuple struct | list |
| map | alist `((K . V) …)` |
| struct | plist `(:field-a A :field-b B)` |
| unit variant | `(:variant)` |
| newtype variant | `(:variant X)` |
| tuple variant | `(:variant A B)` |
| struct variant | `(:variant :field-a A :field-b B)` |

- `Vec<u8>` without `serde_bytes` is a list of integers.
- A `None` field is `:field nil` unless `skip_serializing_if` leaves it out.

### Lisp to Rust

| Rust | Accepted Lisp |
|---|---|
| `bool` | `nil` is false, anything else true |
| integer | integer in range |
| float | float or integer |
| `char` | character, or string of length 1 |
| `String` | string |
| bytes | unibyte string, or multibyte string as UTF-8 |
| `Option<T>` | `nil` is `None`, anything else `Some` |
| unit | `nil` |
| sequence, tuple | proper list or vector; tuples check the length |
| map | alist or hash table |
| struct | plist with keyword keys; unknown and missing keys follow the type's serde attributes |
| enum | list headed by the variant keyword; a bare keyword for a unit variant |

- Dotted and circular lists, and nesting deeper than 128 levels, are
  `decode` errors.
- Self-describing types (`serde_json::Value`, untagged and internally or
  adjacently tagged enums) decode on a best-effort basis and can lose
  information: a symbol becomes a string, a list headed by a keyword
  becomes a map.
- `#[serde(flatten)]` is not supported.
