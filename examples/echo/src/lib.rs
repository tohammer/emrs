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
