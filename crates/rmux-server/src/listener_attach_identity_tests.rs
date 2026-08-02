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

async fn attach_delegated_peer(label: &str) -> AttachedPeer {
    let handler = Arc::new(RequestHandler::new());
    let target = start_quiet_pane_sized(&handler, label, CLIENT_SIZE).await;
    let session = target.session_name().clone();

    let peer_uid = delegated_peer_uid();
    handler
        .set_test_access_mode_for_uid(peer_uid, AccessMode::ReadWrite)
        .expect("the delegated peer can be granted access");
    // No periodic tick may repair or repeat what the attach frame carries.
    set_global(&handler, OptionName::StatusInterval, "0").await;
    set_global(&handler, OptionName::SetTitlesString, IDENTITY_TITLE_FORMAT).await;
    set_global(&handler, OptionName::SetTitles, "on").await;

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
