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
