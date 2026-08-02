//! Issue #182, driven through the production listener: the first attach frame
//! must describe the peer the connection authenticated, and the listener must
//! carry that frame's title into the registration it performs afterwards.
//!
//! `handler_attach_tests::set_titles` builds an attach target itself and hands
//! it to a registration helper, so it pins the render and the dedup rule but
//! never reaches the connection loop. Replacing the listener's seed with `None`
//! or letting the render describe the server owner leaves those tests green.
//! Everything here goes the whole way: a real wire request, the real
//! authenticated peer, the real upgraded byte stream and the real registration.

use rmux_os::identity::UserIdentity;
use rmux_proto::request::AttachSessionExt3Request;
use rmux_proto::{
    ClientTerminalContext, ListClientsRequest, OptionName, ScopeSelector, SetOptionMode,
    SetOptionRequest, TerminalSize,
};

use super::connection_test_support::{
    connected_streams, finish_connection, read_response_leaving_raw_bytes, spawn_connection,
    start_quiet_pane_sized, write_test_request, TestClientStream,
};
use super::*;
use crate::server_access::AccessMode;

const TITLE_OPEN: &str = "\u{1b}]0;";
const TITLE_CLOSE: char = '\u{7}';
const IDENTITY_TITLE_FORMAT: &str = "U=#{client_user}|ID=#{client_uid}|N=#{client_name}";
const CLIENT_SIZE: TerminalSize = TerminalSize { cols: 40, rows: 10 };

/// A local identity that is neither the server owner nor a reserved
/// superuser, so granting it access models a delegated peer.
#[cfg(unix)]
fn delegated_peer_uid() -> u32 {
    rmux_os::identity::real_user_id().saturating_add(18_200)
}

#[cfg(windows)]
fn delegated_peer_uid() -> u32 {
    // Windows keys the access store by SID, so no uid entry can collide with
    // the server owner or with a reserved identity.
    18_200
}

/// A second delegated identity, for the connection that reaches a numeric pid
/// the previous peer is still holding.
fn successor_peer_uid() -> u32 {
    delegated_peer_uid().saturating_add(1)
}

/// The OSC 0 payloads in a raw slice of the upgraded attach stream, in order.
fn titles_in(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split(TITLE_OPEN)
        .skip(1)
        .filter_map(|rest| rest.split_once(TITLE_CLOSE))
        .map(|(payload, _)| payload.to_owned())
        .collect()
}

fn attach_request(session: &rmux_proto::SessionName) -> Request {
    Request::AttachSessionExt3(Box::new(AttachSessionExt3Request {
        target: Some(session.clone()),
        target_spec: None,
        detach_other_clients: false,
        kill_other_clients: false,
        read_only: false,
        skip_environment_update: true,
        flags: None,
        working_directory: None,
        client_terminal: ClientTerminalContext {
            // A real client advertises what its outer terminal can do; without
            // this a Unix test peer has no TERM and no title template at all.
            terminal_features: vec!["title".to_owned()],
            utf8: true,
        },
        client_size: Some(CLIENT_SIZE),
        attach_capabilities: Vec::new(),
    }))
}

async fn set_global(handler: &Arc<RequestHandler>, option: OptionName, value: &str) {
    let response = handler
        .handle(Request::SetOption(SetOptionRequest {
            scope: ScopeSelector::Global,
            option,
            value: value.to_owned(),
            mode: SetOptionMode::Replace,
        }))
        .await;
    assert!(matches!(response, Response::SetOption(_)), "set {option:?}");
}

/// The same bindings, resolved through the independent `list-clients` path.
/// This is the oracle the first frame must already agree with.
async fn listed_identity(handler: &Arc<RequestHandler>, attach_pid: u32) -> String {
    let response = handler
        .handle(Request::ListClients(Box::new(ListClientsRequest {
            target_session: None,
            format: Some(format!("#{{client_pid}}\t{IDENTITY_TITLE_FORMAT}")),
            filter: None,
            sort_order: None,
            reversed: false,
        })))
        .await;
    let Response::ListClients(list) = response else {
        panic!("list-clients must answer, got {response:?}");
    };
    String::from_utf8_lossy(list.output.stdout())
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find(|(pid, _)| *pid == attach_pid.to_string())
        .map(|(_, bindings)| bindings.to_owned())
        .expect("the attached client is listed")
}

/// Reads upgraded attach bytes until this client's outer terminal has been
/// given a title, and returns that first title.
///
/// Waiting for one expected value instead would turn a wrong first frame into a
/// timeout; the identity the frame actually carried is the evidence.
async fn read_first_title(client: &mut TestClientStream) -> String {
    let mut seen = Vec::new();
    let mut buffer = [0_u8; 4096];
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let bytes_read = client
                .read(&mut buffer)
                .await
                .expect("the upgraded attach stream stays readable");
            assert_ne!(bytes_read, 0, "the attach stream closed early");
            seen.extend_from_slice(&buffer[..bytes_read]);
            if let Some(title) = titles_in(&seen).into_iter().next() {
                return title;
            }
        }
    })
    .await
    .expect("the attach frame carries a title")
}

/// Reads upgraded attach bytes until `wanted` arrives, returning everything
/// read. Fails the test rather than hanging if the stream goes quiet.
async fn read_until_title(client: &mut TestClientStream, wanted: &str) -> Vec<u8> {
    let mut seen = Vec::new();
    let mut buffer = [0_u8; 4096];
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let bytes_read = client
                .read(&mut buffer)
                .await
                .expect("the upgraded attach stream stays readable");
            assert_ne!(bytes_read, 0, "the attach stream closed early");
            seen.extend_from_slice(&buffer[..bytes_read]);
            if titles_in(&seen).iter().any(|title| title == wanted) {
                return seen;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for the title {wanted:?}"))
}

/// Arms a session, the identity title and a delegated peer, then attaches that
/// peer through the real listener and returns everything the client observed
/// up to and including its first title.
struct AttachedPeer {
    handler: Arc<RequestHandler>,
    client: TestClientStream,
    peer_pid: u32,
    first_title: String,
    shutdown_tx: watch::Sender<()>,
    connection: tokio::task::JoinHandle<io::Result<()>>,
}

/// A server with one quiet session, the identity title armed and the delegated
/// peer allowed in.
async fn armed_handler(label: &str) -> (Arc<RequestHandler>, rmux_proto::SessionName) {
    let handler = Arc::new(RequestHandler::new());
    let target = start_quiet_pane_sized(&handler, label, CLIENT_SIZE).await;
    let session = target.session_name().clone();

    handler
        .set_test_access_mode_for_uid(delegated_peer_uid(), AccessMode::ReadWrite)
        .expect("the delegated peer can be granted access");
    // No periodic tick may repair or repeat what the attach frame carries.
    set_global(&handler, OptionName::StatusInterval, "0").await;
    set_global(&handler, OptionName::SetTitlesString, IDENTITY_TITLE_FORMAT).await;
    set_global(&handler, OptionName::SetTitles, "on").await;

    (handler, session)
}

async fn attach_delegated_peer(label: &str) -> AttachedPeer {
    let (handler, session) = armed_handler(label).await;
    let peer_uid = delegated_peer_uid();

    let peer = PeerIdentity {
        // Not this process: the peer must be the connection's identity, never
        // whatever the server itself happens to be.
        pid: std::process::id().wrapping_add(18_207),
        uid: peer_uid,
        user: UserIdentity::Uid(peer_uid),
    };
    let peer_pid = peer.pid;
    let (server, mut client) = connected_streams(label)
        .await
        .expect("a connected stream pair");
    let (shutdown_tx, connection) = spawn_connection(&handler, peer, server);

    write_test_request(&mut client, attach_request(&session))
        .await
        .expect("the attach request reaches the listener");
    let response = read_response_leaving_raw_bytes(&mut client)
        .await
        .expect("the listener answers the attach");
    assert!(
        matches!(&response, Response::AttachSession(attached) if attached.session_name == session),
        "the delegated peer must be allowed to attach, got {response:?}"
    );

    let expected = listed_identity(&handler, peer_pid).await;
    let observed = read_until_title(&mut client, &expected).await;
    let first_title = titles_in(&observed)
        .into_iter()
        .next()
        .expect("the attach frame carries a title");

    AttachedPeer {
        handler,
        client,
        peer_pid,
        first_title,
        shutdown_tx,
        connection,
    }
}

impl AttachedPeer {
    async fn finish(self) -> io::Result<()> {
        drop(self.client);
        let _ = self.shutdown_tx.send(());
        finish_connection(self.connection).await
    }
}

/// The blocking finding: the frame rendered before registration must describe
/// the authenticated peer, not the server owner.
#[tokio::test]
async fn the_first_attach_frame_describes_the_authenticated_peer() -> io::Result<()> {
    let attached = attach_delegated_peer("listener-attach-identity").await;

    let listed = listed_identity(&attached.handler, attached.peer_pid).await;
    assert_eq!(
        attached.first_title, listed,
        "the first frame and the registered client must report one identity"
    );
    assert_eq!(
        attached
            .handler
            .remembered_client_title_for_test(attached.peer_pid)
            .await,
        Some(attached.first_title.clone()),
        "registration must remember the title the listener already forwarded"
    );
    assert_ne!(
        delegated_peer_uid(),
        crate::server_access::current_owner_uid(),
        "this test only means something while the peer is not the server owner"
    );

    attached.finish().await
}

/// One authenticated connection, still live, plus the first title its outer
/// terminal was given.
struct ListenerPeer {
    client: TestClientStream,
    first_title: String,
    shutdown_tx: watch::Sender<()>,
    connection: tokio::task::JoinHandle<io::Result<()>>,
}

impl ListenerPeer {
    async fn finish(self) -> io::Result<()> {
        drop(self.client);
        let _ = self.shutdown_tx.send(());
        finish_connection(self.connection).await
    }
}

/// Attaches `peer` through the production connection loop and stops as soon as
/// its first frame has been observed, leaving the connection — and therefore
/// its authenticated scope — open.
async fn attach_peer_through_listener(
    handler: &Arc<RequestHandler>,
    label: &str,
    peer: PeerIdentity,
    session: &rmux_proto::SessionName,
) -> ListenerPeer {
    let (server, mut client) = connected_streams(label)
        .await
        .expect("a connected stream pair");
    let (shutdown_tx, connection) = spawn_connection(handler, peer, server);

    write_test_request(&mut client, attach_request(session))
        .await
        .expect("the attach request reaches the listener");
    let response = read_response_leaving_raw_bytes(&mut client)
        .await
        .expect("the listener answers the attach");
    assert!(
        matches!(&response, Response::AttachSession(attached) if &attached.session_name == session),
        "the delegated peer must be allowed to attach, got {response:?}"
    );
    let first_title = read_first_title(&mut client).await;

    ListenerPeer {
        client,
        first_title,
        shutdown_tx,
        connection,
    }
}

fn delegated_peer(pid: u32, uid: u32) -> PeerIdentity {
    PeerIdentity {
        pid,
        uid,
        user: UserIdentity::Uid(uid),
    }
}

/// The reused-pid finding: one numeric pid can carry two connections the
/// listener authenticated as different local peers.
///
/// `serve_connection` opens its authenticated scope before dispatch and keeps
/// it open across registration, `forward_attach` and `finish_attach`, so an
/// exited client's scope outlives the registration `finish_attach` removes.
/// This parks the first connection inside attach registration to hold exactly
/// that overlap open: its scope is live and it owns no registration, which is
/// the state a recycled pid reaches. The second peer's frame is then built
/// while the pid-keyed scope set disagrees.
///
/// Ambiguity must not become the server owner. The frame the second peer is
/// shown has to carry the identity the listener authenticated for *its*
/// connection and then registers it under — the one `list-clients` reports.
#[tokio::test]
async fn a_reused_pid_never_renders_the_owner_into_a_new_peer_s_first_frame() -> io::Result<()> {
    let (handler, session) = armed_handler("listener-attach-reused-pid").await;
    let successor_uid = successor_peer_uid();
    handler
        .set_test_access_mode_for_uid(successor_uid, AccessMode::ReadWrite)
        .expect("the successor peer can be granted access");
    assert_ne!(
        successor_uid,
        crate::server_access::current_owner_uid(),
        "this test only means something while the successor is not the owner"
    );

    // Both connections reach the server under one numeric pid, as the OS
    // permits once an exited client's pid has been recycled.
    let shared_pid = std::process::id().wrapping_add(18_209);
    let registration_pause = crate::server_access::install_access_registration_pause(
        crate::server_access::AccessRegistrationKind::Attach,
        shared_pid,
    );

    let (first_server, mut first_client) = connected_streams("listener-attach-reused-pid-first")
        .await
        .expect("a connected stream pair");
    let (first_shutdown_tx, first_connection) = spawn_connection(
        &handler,
        delegated_peer(shared_pid, delegated_peer_uid()),
        first_server,
    );
    write_test_request(&mut first_client, attach_request(&session))
        .await
        .expect("the first attach request reaches the listener");
    tokio::time::timeout(
        Duration::from_secs(20),
        registration_pause.reached.notified(),
    )
    .await
    .expect("the first attach reaches its registration boundary");

    // The first connection now holds an authenticated scope on `shared_pid`
    // and owns no registration, so the successor takes the fresh-attach path.
    let successor = attach_peer_through_listener(
        &handler,
        "listener-attach-reused-pid-successor",
        delegated_peer(shared_pid, successor_uid),
        &session,
    )
    .await;

    let listed = listed_identity(&handler, shared_pid).await;
    assert!(
        listed.starts_with(&format!("U={successor_uid}|")),
        "registration must publish the successor's authenticated identity, got {listed:?}"
    );
    assert_eq!(
        successor.first_title, listed,
        "the successor's first frame and its registration must report one identity"
    );
    assert_eq!(
        handler.remembered_client_title_for_test(shared_pid).await,
        Some(successor.first_title.clone()),
        "registration must remember the title the successor was actually shown"
    );

    registration_pause.release.notify_one();
    successor.finish().await?;
    drop(first_client);
    let _ = first_shutdown_tx.send(());
    finish_connection(first_connection).await
}

/// The listener seam the direct unit test cannot reach: the title the frame
/// already delivered has to arrive in the registration, or the client's next
/// redraw writes it a second time.
#[tokio::test]
async fn the_listener_seeds_registration_with_the_title_it_forwarded() -> io::Result<()> {
    let mut attached = attach_delegated_peer("listener-attach-seed").await;

    // Force a redraw that changes no title. A client whose registration missed
    // the seed repeats its first title here.
    set_global(&attached.handler, OptionName::StatusInterval, "13").await;
    // Then a real title change, so the wait below is bounded by a value that
    // must arrive rather than by a timeout.
    let second = "SECOND-TITLE";
    set_global(&attached.handler, OptionName::SetTitlesString, second).await;

    let observed = read_until_title(&mut attached.client, second).await;
    let titles = titles_in(&observed);
    assert!(
        !titles.contains(&attached.first_title),
        "the seeded title must not be written twice, got {titles:?}"
    );
    assert_eq!(
        titles.last().map(String::as_str),
        Some(second),
        "the changed title still reaches the client, got {titles:?}"
    );
    assert_eq!(
        attached
            .handler
            .remembered_client_title_for_test(attached.peer_pid)
            .await,
        Some(second.to_owned()),
        "the memory follows what this client was last told"
    );

    attached.finish().await
}
