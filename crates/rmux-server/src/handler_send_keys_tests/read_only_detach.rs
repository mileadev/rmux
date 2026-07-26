use super::*;
use crate::pane_io::AttachControl;

async fn register_delegated_attach(
    handler: &RequestHandler,
    requester_pid: u32,
    session: &rmux_proto::SessionName,
) -> mpsc::UnboundedReceiver<AttachControl> {
    create_send_keys_test_session(handler, session).await;
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    handler
        .register_attach(requester_pid, session.clone(), control_tx)
        .await;
    let mut active_attach = handler.active_attach.lock().await;
    let active = active_attach
        .by_pid
        .get_mut(&requester_pid)
        .expect("delegated attach is active");
    active.can_write = false;
    active.flags = active.flags.with_read_only();
    control_rx
}

async fn recv_detach(control_rx: &mut mpsc::UnboundedReceiver<AttachControl>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(control) = control_rx.recv().await {
            if matches!(control, AttachControl::Detach) {
                return;
            }
        }
        panic!("attach control channel closed before detach");
    })
    .await
    .expect("delegated detach arrives");
}

fn assert_no_detach(control_rx: &mut mpsc::UnboundedReceiver<AttachControl>) {
    while let Ok(control) = control_rx.try_recv() {
        assert!(
            !matches!(control, AttachControl::Detach),
            "unauthorized input must not detach the client"
        );
    }
}

#[tokio::test]
async fn delegated_read_only_attach_can_detach_with_split_prefix_binding() {
    let handler = RequestHandler::new();
    let alpha = session_name("delegated-read-only-detach");
    let requester_pid = std::process::id();
    let mut control_rx = register_delegated_attach(&handler, requester_pid, &alpha).await;

    handler
        .handle_attached_live_input_for_test(requester_pid, b"\x02")
        .await
        .expect("read-only prefix input");
    assert_no_detach(&mut control_rx);
    handler
        .handle_attached_live_input_for_test(requester_pid, b"d")
        .await
        .expect("read-only detach input");

    recv_detach(&mut control_rx).await;
}

#[tokio::test]
async fn delegated_read_only_attach_still_rejects_content_and_mutating_bindings() {
    let handler = RequestHandler::new();
    let alpha = session_name("delegated-read-only-denied");
    let requester_pid = std::process::id();
    let mut control_rx = register_delegated_attach(&handler, requester_pid, &alpha).await;
    let capture =
        RawPaneInputProbe::start(&handler, &alpha, "delegated-read-only-content", 0).await;

    handler
        .handle_attached_live_input_for_test(requester_pid, b"content")
        .await
        .expect("read-only content input");
    handler
        .handle_attached_live_input_for_test(requester_pid, b"\x02")
        .await
        .expect("read-only prefix input");
    handler
        .handle_attached_live_input_for_test(requester_pid, b"c")
        .await
        .expect("read-only new-window binding");

    capture.finish(&handler, &alpha).await;
    capture.assert_contents(&handler, b"").await;
    assert_eq!(
        handler
            .state
            .lock()
            .await
            .sessions
            .session(&alpha)
            .expect("session remains")
            .windows()
            .len(),
        1,
        "read-only prefix c must not create a window"
    );
    assert_no_detach(&mut control_rx);
}

// tmux 3.7b executes both commands in this binding for an `attach-session -r`
// client. RMUX delegated access deliberately grants only the local detach
// capability, never the mutation chained after it.
#[tokio::test]
async fn delegated_read_only_attach_rejects_chained_detach_binding_product_divergence() {
    let handler = RequestHandler::new();
    let alpha = session_name("delegated-read-only-chained");
    let requester_pid = std::process::id();
    let mut control_rx = register_delegated_attach(&handler, requester_pid, &alpha).await;
    let rebound = handler
        .handle(Request::BindKey(Box::new(BindKeyRequest {
            table_name: "prefix".to_owned(),
            key: "d".to_owned(),
            note: Some("delegated detach must remain local".to_owned()),
            repeat: false,
            command: Some(vec![
                "detach-client".to_owned(),
                ";".to_owned(),
                "new-window".to_owned(),
            ]),
        })))
        .await;
    assert!(matches!(rebound, Response::BindKey(_)), "{rebound:?}");

    handler
        .handle_attached_live_input_for_test(requester_pid, b"\x02d")
        .await
        .expect("chained read-only binding input");

    assert_eq!(
        handler
            .state
            .lock()
            .await
            .sessions
            .session(&alpha)
            .expect("session remains")
            .windows()
            .len(),
        1,
        "the chained new-window command must not execute"
    );
    assert_no_detach(&mut control_rx);
}

#[tokio::test]
async fn delegated_read_only_attach_does_not_interpret_detach_keys_inside_fragmented_paste() {
    let handler = RequestHandler::new();
    let alpha = session_name("delegated-read-only-paste");
    let requester_pid = std::process::id();
    let mut control_rx = register_delegated_attach(&handler, requester_pid, &alpha).await;
    let mut pending_input = Vec::new();

    for chunk in [
        b"\x1b[20".as_slice(),
        b"0~\x02".as_slice(),
        b"d\x1b[201~".as_slice(),
    ] {
        handler
            .handle_attached_live_input(requester_pid, &mut pending_input, chunk)
            .await
            .expect("read-only fragmented paste input");
    }

    assert!(pending_input.is_empty(), "complete paste must be consumed");
    assert_no_detach(&mut control_rx);

    handler
        .handle_attached_live_input_for_test(requester_pid, b"\x02d")
        .await
        .expect("explicit detach after paste");
    recv_detach(&mut control_rx).await;
}
