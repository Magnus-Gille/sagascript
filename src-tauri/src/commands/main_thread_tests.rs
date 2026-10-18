use std::sync::mpsc;
use std::time::Duration;

use super::*;

const SHORT: Duration = Duration::from_millis(30);
const LONG: Duration = Duration::from_secs(30);

fn runtime() -> tokio::runtime::Runtime {
    // One thread, like a worker with nothing else to run it: a wait that
    // blocks it cannot be answered by anything scheduled on the same runtime.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

#[test]
fn an_answer_from_the_main_thread_is_returned() {
    let reply = runtime().block_on(await_main_thread(LONG, |sender| {
        std::thread::spawn(move || {
            let _ = sender.send("installed");
        });
        Ok(())
    }));
    assert_eq!(reply, Ok(MainThreadReply::Done("installed")));
}

#[test]
fn no_answer_times_out_instead_of_waiting_for_ever() {
    // The sender stays alive past the wait: the main thread is busy, not gone.
    let (keep_alive, kept) = mpsc::channel();
    let reply = runtime().block_on(await_main_thread::<()>(SHORT, |sender| {
        keep_alive.send(sender).expect("the receiver is alive");
        Ok(())
    }));
    assert_eq!(reply, Ok(MainThreadReply::TimedOut));
    drop(kept);
}

#[test]
fn a_dropped_sender_means_the_app_is_shutting_down() {
    let reply = runtime().block_on(await_main_thread::<()>(LONG, |sender| {
        drop(sender);
        Ok(())
    }));
    assert_eq!(reply, Ok(MainThreadReply::Disconnected));
}

#[test]
fn a_main_thread_that_cannot_be_reached_is_an_error() {
    let reply = runtime().block_on(await_main_thread::<()>(LONG, |_sender| {
        Err(MainThreadUnreachable(
            "the event loop is closed".to_string(),
        ))
    }));
    let unreachable = reply.expect_err("the dispatch failed");
    assert_eq!(unreachable.to_string(), "the event loop is closed");
    // A command reports it as unavailable, with the runtime's text.
    let error = CommandError::from(unreachable);
    assert_eq!(error.code(), "unavailable");
    assert_eq!(error.to_string(), "the event loop is closed");
}

/// The wait yields its worker. The answer only exists once a task on the same
/// single-thread runtime has run, which a wait that blocks the thread (the
/// plain `recv()` this replaced) never lets happen.
#[test]
fn waiting_does_not_block_the_worker() {
    let reply = runtime().block_on(async {
        let (go, answer_after) = mpsc::channel::<()>();
        let waiting = await_main_thread(Duration::from_secs(5), move |sender| {
            std::thread::spawn(move || {
                // "The main thread" answers once the runtime's other task ran.
                if answer_after.recv().is_ok() {
                    let _ = sender.send(7);
                }
            });
            Ok(())
        });
        let other_task = async move {
            tokio::task::yield_now().await;
            go.send(()).expect("the answering thread is waiting");
        };
        let (reply, ()) = tokio::join!(waiting, other_task);
        reply
    });
    assert_eq!(reply, Ok(MainThreadReply::Done(7)));
}
