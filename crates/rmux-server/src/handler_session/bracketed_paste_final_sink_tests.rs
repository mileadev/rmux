//! Final-sink proof for attached bracketed paste that arrives while a pane is
//! still starting.
//!
//! On Windows the initial ConPTY is created off the request path, so attached
//! input can reach `prepare_pane_input_write_with_encoding` before the pane has
//! a terminal. The typed `DeferredInitialPaneInput::BracketedPaste` entry and
//! the console branch of `flush_deferred_initial_pane_input` exist so the paste
//! keeps its bracketed intent across that boundary. The queued-variant test in
//! `pane_terminals/deferred_initial/tests.rs` stops at the enum; this one runs
//! a real child and compares the bytes it read after the flush.
//!
//! The starting window is held open deterministically by occupying the
//! runtime's single blocking thread, the same technique the deferred
//! `select-pane` tests use, instead of racing a timeout.

use rmux_proto::{PaneTarget, SessionName};

use crate::test_shell::final_sink::{
    create_final_sink_session, FinalSinkSlot, ENABLE_BRACKETED_PASTE,
};

use super::super::RequestHandler;

const OPEN: &[u8] = b"\x1b[200~";
const CLOSE: &[u8] = b"\x1b[201~";

fn wrapped(body: &[u8]) -> Vec<u8> {
    let mut wrapped = Vec::with_capacity(OPEN.len() + body.len() + CLOSE.len());
    wrapped.extend_from_slice(OPEN);
    wrapped.extend_from_slice(body);
    wrapped.extend_from_slice(CLOSE);
    wrapped
}

#[test]
fn deferred_bracketed_paste_reaches_the_child_with_its_delimiters() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("build isolated deferred-pane runtime");

    runtime.block_on(async {
        // Occupying the only blocking thread keeps the initial ConPTY
        // unstarted, so the paste below is genuinely queued rather than
        // written live.
        let (blocker_started_tx, blocker_started_rx) = tokio::sync::oneshot::channel();
        let (blocker_release_tx, blocker_release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = blocker_started_tx.send(());
            blocker_release_rx
                .recv()
                .expect("release deferred-pane blocking worker");
        });
        blocker_started_rx
            .await
            .expect("blocking worker reports that it is occupied");

        let handler = RequestHandler::new();
        let session = SessionName::new("final-sink-deferred").expect("valid session name");
        let expected = wrapped("deferred\r\nβ😀 body".as_bytes());
        let slot = FinalSinkSlot::new("deferred", &expected, true);
        create_final_sink_session(&handler, &session, &slot).await;

        let target = PaneTarget::with_window(session.clone(), 0, 0);
        {
            let mut state = handler.state.lock().await;
            assert!(
                state.pane_is_starting_in_window(&session, 0, 0),
                "the deferred pane must still be starting before the paste"
            );
            // The child cannot announce DECSET 2004 yet — it has no terminal.
            // Stamp the mode it will confirm once started, so the destination
            // is bracket-aware at the moment the paste is queued.
            state
                .append_bytes_to_pane_transcript_for_test(&session, 0, 0, ENABLE_BRACKETED_PASTE)
                .expect("bracketed paste mode reaches the starting pane transcript");
        }

        let requester_pid = std::process::id();
        let (control_tx, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let _attach_id = handler
            .register_attach(requester_pid, session.clone(), control_tx)
            .await;
        handler
            .handle_attached_live_input_for_test(requester_pid, &expected)
            .await
            .expect("bracketed paste is accepted while the pane starts");

        {
            let state = handler.state.lock().await;
            assert!(
                state.pane_is_starting_in_window(&session, 0, 0),
                "the paste must not have started the pane by itself"
            );
        }

        blocker_release_tx
            .send(())
            .expect("release deferred-pane blocking worker");
        blocker.await.expect("blocking worker joins");
        handler
            .wait_for_pane_startup_to_finish_for_test(&target)
            .await;

        slot.assert_application_bytes(
            "input queued before the pane started must reach the child with its delimiters",
        );
    });
}
