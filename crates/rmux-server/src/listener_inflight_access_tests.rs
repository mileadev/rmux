//! Listener-level coverage for an access admission that goes stale while a
//! pane-stream open is still in flight.
//!
//! `handler_pane_stream_tests::revoked_raw_subscribe_keeps_its_identity_and_only_replaces_the_keyframe`
//! calls [`RequestHandler::revoke_inflight_pane_stream_response`] directly, so
//! it pins the substitution but never proves that the connection loop reaches
//! it: deleting the recheck in [`serve_connection`] leaves that test green.
//! The test below drives a real connection through the real listener and only
//! observes the wire, so the recheck itself is what is under test.

use std::sync::Mutex as StdMutex;

use rmux_os::identity::UserIdentity;
use rmux_proto::{
    NewSessionExtRequest, PaneStreamEndReason, PaneStreamEvent, PaneStreamMode, PaneTarget,
    PaneTargetRef, SessionName, SubscribePaneStreamRequest, TerminalSize,
    UnsubscribePaneStreamRequest, UnsubscribePaneStreamResponse,
};
use tokio::sync::Notify;

use super::*;
use crate::server_access::AccessMode;

/// Parks a connection loop between its dispatched pane-stream response and the
/// inflight admission recheck.
///
/// The revocation window the listener guards is exactly that gap, and nothing
/// a client can send widens it. This is the only way to make the race
/// deterministic, so it stays `#[cfg(test)]`, one-shot, and scoped to a single
/// handler.
#[derive(Debug, Default)]
pub(super) struct InflightPaneStreamPause {
    reached: Notify,
    release: Notify,
    dispatched: StdMutex<Option<Response>>,
}

impl InflightPaneStreamPause {
    /// Waits until the connection loop parks and returns the untouched
    /// response it is about to revalidate.
    async fn wait_until_reached(&self) -> Response {
        self.reached.notified().await;
        self.dispatched
            .lock()
            .expect("inflight pane-stream pause mutex must not be poisoned")
            .clone()
            .expect("a parked connection publishes its dispatched response")
    }

    fn release(&self) {
        self.release.notify_one();
    }
}

static INFLIGHT_PANE_STREAM_PAUSES: StdMutex<Vec<(usize, Arc<InflightPaneStreamPause>)>> =
    StdMutex::new(Vec::new());

fn install_inflight_pane_stream_pause(
    handler: &Arc<RequestHandler>,
) -> Arc<InflightPaneStreamPause> {
    let handler_key = std::ptr::from_ref::<RequestHandler>(handler).addr();
    let pause = Arc::new(InflightPaneStreamPause::default());
    let mut pauses = INFLIGHT_PANE_STREAM_PAUSES
        .lock()
        .expect("inflight pane-stream pause registry lock");
    let previous = pauses.iter().any(|(key, _)| *key == handler_key);
    assert!(!previous, "inflight pane-stream pause already installed");
    pauses.push((handler_key, Arc::clone(&pause)));
    pause
}

/// Called by [`serve_connection`] just before it revalidates the admission a
/// pane-stream response was dispatched under. Returns immediately unless a
/// test installed a pause for this handler.
pub(super) async fn pause_before_inflight_pane_stream_recheck(
    handler: &RequestHandler,
    response: &Response,
) {
    if !matches!(
        response,
        Response::SubscribePaneStream(_) | Response::PaneStreamCursor(_)
    ) {
        return;
    }
    let handler_key = std::ptr::from_ref(handler).addr();
    let pause = {
        let mut pauses = INFLIGHT_PANE_STREAM_PAUSES
            .lock()
            .expect("inflight pane-stream pause registry lock");
        let position = pauses.iter().position(|(key, _)| *key == handler_key);
        position.map(|position| pauses.swap_remove(position).1)
    };
    let Some(pause) = pause else {
        return;
    };
    *pause
        .dispatched
        .lock()
        .expect("inflight pane-stream pause mutex must not be poisoned") = Some(response.clone());
    pause.reached.notify_one();
    pause.release.notified().await;
}

/// Proves the listener path the direct unit test cannot reach.
///
/// The connection really subscribes, the admission it was dispatched under is
/// really rotated out while the response is still in flight, and the client
/// only ever observes the substituted opening event.
#[tokio::test]
async fn a_stale_admission_turns_an_inflight_raw_open_into_a_revoked_end() -> io::Result<()> {
    let peer_uid = revocable_peer_uid();
    let peer = PeerIdentity {
        pid: std::process::id(),
        uid: peer_uid,
        user: UserIdentity::Uid(peer_uid),
    };
    let handler = Arc::new(RequestHandler::new());
    let target = start_quiet_pane(&handler, "inflight-access-revoked").await;
    handler
        .set_test_access_mode_for_uid(peer_uid, AccessMode::ReadWrite)
        .expect("test peer starts read-write");

    let (server, mut client) = connected_streams("listener-inflight-access").await?;
    let (_shutdown_tx, connection_task) = spawn_connection(&handler, peer, server);
    let pause = install_inflight_pane_stream_pause(&handler);

    write_test_request(
        &mut client,
        Request::SubscribePaneStream(SubscribePaneStreamRequest {
            target: PaneTargetRef::slot(target.clone()),
            mode: PaneStreamMode::Raw,
            include_snapshot: false,
        }),
    )
    .await?;

    // The connection loop now holds a real opening response and has not yet
    // revalidated the admission it was dispatched under.
    let Response::SubscribePaneStream(opened) = pause.wait_until_reached().await else {
        panic!("the parked response must be the real pane-stream opening");
    };
    assert_eq!(opened.target, target);
    assert!(
        matches!(opened.event, PaneStreamEvent::RawRebase(_)),
        "a Raw open answers with a keyframe before revocation: {:?}",
        opened.event
    );
    assert!(
        handler
            .pane_output_subscription_key_for_test(opened.subscription_id)
            .is_some(),
        "the dispatched open still holds its subscription while the admission is current"
    );

    // Rotate the admission out. A mode downgrade would not do: `set_mode`
    // keeps the entry's epoch, so only removing the identity makes the
    // snapshot the connection captured fail revalidation.
    handler
        .remove_test_access_for_uid(peer_uid)
        .expect("test peer access can be revoked");
    pause.release();

    let mut revoked = opened.clone();
    revoked.event = PaneStreamEvent::End(PaneStreamEndReason::AccessRevoked);
    assert_eq!(
        read_test_response(&mut client).await?,
        Response::SubscribePaneStream(revoked),
        "the listener must replace the opening event of the real response and nothing else"
    );
    assert!(
        handler
            .pane_output_subscription_key_for_test(opened.subscription_id)
            .is_none(),
        "the subscription and its stream quota are released before the client observes the end"
    );

    // Nothing is left for the client to release. The sibling test
    // `revoked_connection_can_release_its_owned_pane_stream` answers `removed:
    // true` here because its stream survived the revocation; a revoked
    // inflight open owes the client no cursor and no unsubscribe.
    write_test_request(
        &mut client,
        Request::UnsubscribePaneStream(UnsubscribePaneStreamRequest {
            subscription_id: opened.subscription_id,
        }),
    )
    .await?;
    assert_eq!(
        read_test_response(&mut client).await?,
        Response::UnsubscribePaneStream(UnsubscribePaneStreamResponse {
            subscription_id: opened.subscription_id,
            removed: false,
        })
    );

    drop(client);
    finish_connection(connection_task).await
}

/// A uid that is neither the server owner nor a reserved superuser, so its
/// access entry can be removed while the connection is mid-request.
#[cfg(unix)]
fn revocable_peer_uid() -> u32 {
    rmux_os::identity::real_user_id().saturating_add(13_000)
}

#[cfg(windows)]
fn revocable_peer_uid() -> u32 {
    // Windows keys the access store by SID, so no uid entry can collide with
    // the server owner or with a reserved identity.
    13_000
}

#[cfg(unix)]
fn quiet_command() -> Vec<String> {
    vec!["/bin/sh".to_owned(), "-c".to_owned(), "sleep 60".to_owned()]
}

#[cfg(windows)]
fn quiet_command() -> Vec<String> {
    let system_root =
        std::env::var_os("SystemRoot").unwrap_or_else(|| std::ffi::OsString::from(r"C:\Windows"));
    let cmd = std::path::PathBuf::from(system_root)
        .join("System32")
        .join("cmd.exe");
    vec![
        cmd.to_string_lossy().into_owned(),
        "/d".to_owned(),
        "/q".to_owned(),
        "/c".to_owned(),
        "ping -n 120 127.0.0.1 >NUL".to_owned(),
    ]
}

async fn start_quiet_pane(handler: &Arc<RequestHandler>, name: &str) -> PaneTarget {
    let session = SessionName::new(name).expect("valid session name");
    let response = handler
        .handle(Request::NewSessionExt(Box::new(NewSessionExtRequest {
            session_name: Some(session.clone()),
            working_directory: None,
            detached: true,
            size: Some(TerminalSize { cols: 12, rows: 4 }),
            environment: None,
            group_target: None,
            attach_if_exists: false,
            detach_other_clients: false,
            kill_other_clients: false,
            flags: None,
            window_name: None,
            print_session_info: false,
            print_format: None,
            command: Some(quiet_command()),
            process_command: None,
            client_environment: None,
            skip_environment_update: false,
        })))
        .await;
    assert!(matches!(response, Response::NewSession(_)), "{response:?}");
    let target = PaneTarget::with_window(session, 0, 0);
    handler
        .wait_for_pane_startup_to_finish_for_test(&target)
        .await;
    target
}

/// Async client end of the connection under test.
#[cfg(unix)]
type TestClientStream = LocalStream;
#[cfg(windows)]
type TestClientStream = rmux_ipc::WindowsPipeClient;

#[cfg(unix)]
async fn connected_streams(_label: &str) -> io::Result<(LocalStream, TestClientStream)> {
    LocalStream::pair()
}

#[cfg(windows)]
async fn connected_streams(label: &str) -> io::Result<(LocalStream, TestClientStream)> {
    let endpoint = rmux_ipc::endpoint_for_label(format!("{label}-{}", std::process::id()))?;
    let listener = LocalListener::bind(&endpoint)?;
    let client_endpoint = endpoint.clone();
    let client = tokio::spawn(async move {
        rmux_ipc::connect_windows_pipe(client_endpoint.as_pipe_name()).await
    });
    // The accepted peer identity is the test process itself; the connection is
    // served under the synthetic revocable identity instead, exactly like the
    // Unix access-revocation tests.
    let (server, _accepted_peer) = listener.accept().await?;
    let client = client
        .await
        .map_err(|error| io::Error::other(format!("named-pipe client task failed: {error}")))??;
    Ok((server, client))
}

fn spawn_connection(
    handler: &Arc<RequestHandler>,
    peer: PeerIdentity,
    server: LocalStream,
) -> (watch::Sender<()>, tokio::task::JoinHandle<io::Result<()>>) {
    let handler = Arc::clone(handler);
    let (shutdown_tx, shutdown_rx) = watch::channel(());
    let (shutdown_handle, _shutdown_request_rx) = ShutdownHandle::new();
    let connection_id = handler.allocate_connection_id();
    let task = tokio::spawn(async move {
        run_connection_with_cleanup(
            server,
            peer,
            handler,
            connection_id,
            shutdown_rx,
            shutdown_handle,
        )
        .await
    });
    (shutdown_tx, task)
}

async fn finish_connection(task: tokio::task::JoinHandle<io::Result<()>>) -> io::Result<()> {
    match task.await.expect("connection task") {
        Ok(()) => Ok(()),
        // A dropped named-pipe client surfaces as a peer disconnect rather
        // than a clean end-of-stream.
        Err(error) if rmux_ipc::is_peer_disconnect(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

async fn write_test_request<S>(stream: &mut S, request: Request) -> io::Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    let frame = encode_frame(&request).map_err(io::Error::other)?;
    stream.write_all(&frame).await
}

async fn read_test_response<S>(stream: &mut S) -> io::Result<Response>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut decoder = FrameDecoder::new();
    let mut buffer = [0_u8; 512];

    loop {
        if let Some(response) = decoder.next_frame::<Response>().map_err(io::Error::other)? {
            return Ok(response);
        }

        let bytes_read = stream.read(&mut buffer).await?;
        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "server closed before response frame",
            ));
        }
        decoder.push_bytes(&buffer[..bytes_read]);
    }
}
