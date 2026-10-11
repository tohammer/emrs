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
    // The reply is dropped while the task unwinds, so `no-reply` can arrive
    // before the log line is queued.
    let deadline = Instant::now() + Duration::from_secs(5);
    let logged =
        |host: &mut Host<Probe>| host.logs().iter().any(|l| l.contains("spawned task panicked: task exploded"));
    while !logged(&mut host) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(logged(&mut host), "{:?}", host.logs());
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
fn finished_stream_keeps_its_items_when_the_instance_stops() {
    let mut host = host();
    let call = host.begin_stream(Request::Count { n: 10, capacity: 64 }).unwrap();
    std::thread::sleep(Duration::from_millis(200)); // the producer has finished; nothing is drained yet
    assert_eq!(host.stop(), State::Stopped);
    let result = host.finish_stream::<u32>(call);
    assert_eq!(result.status, StreamEnd::Done);
    assert_eq!(result.items, (0..10).collect::<Vec<u32>>());
}

/// Panics in `stop` and again when its state is dropped.
struct Fragile {
    armed: bool,
}

impl Drop for Fragile {
    fn drop(&mut self) {
        if self.armed {
            panic!("drop exploded");
        }
    }
}

impl Service for Fragile {
    const NAME: &'static str = "fragile";
    type Args = ();
    type Request = ();
    type Event = ();

    fn start(_: (), _: &Context<Self>) -> Result<Self, Error> {
        Ok(Fragile { armed: false })
    }

    fn handle(&mut self, _: (), reply: Reply, _: &Context<Self>) {
        reply.ok(());
    }

    fn stop(&mut self) {
        self.armed = true;
        panic!("stop exploded");
    }
}

#[test]
fn panic_in_stop_followed_by_panic_in_drop_does_not_abort() {
    let mut host = Host::<Fragile>::start(());
    assert!(host.wait_for_state(State::Running));
    assert_eq!(host.stop(), State::Failed);
    assert!(host.logs().iter().any(|l| l.contains("panicked while being dropped")), "{:?}", host.logs());
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
