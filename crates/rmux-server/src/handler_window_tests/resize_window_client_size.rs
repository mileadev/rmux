//! `resize-window -A` / `-a` selects a client size the way tmux 3.7b does.
//!
//! `cmd_resize_window_exec` hands both flags to `default_window_size(..,
//! WINDOW_SIZE_LARGEST | WINDOW_SIZE_SMALLEST)`, so the command shares the
//! automatic `window-size` selector: every client `ignore_client_size()`
//! rejects owns nothing, each remaining client's status rows come off its outer
//! terminal rows before it competes, and each dimension keeps its own extreme.
//!
//! Measured against the pinned tmux 3.7b (binary sha256 `eb05f981…`, the
//! `frozen_reference.yaml` build) on macOS 26.5.2 arm64 with real PTY clients of
//! the stated winsize. Every run first pinned the window with an explicit
//! `resize-window -x 70 -y 18`, so the reported geometry is what the flag itself
//! chose:
//!
//! ```text
//! status=2    client 80x24                      -A -> 80x22    -a -> 80x22
//! status=2    clients 80x24 + 100x40            -A -> 100x38   -a -> 80x22
//! status=off  ignore-size 80x24 + 100x40        -A -> 100x40   -a -> 100x40
//! status=off  read-only   80x24 + 100x40        -A -> 100x40   -a -> 100x40
//! status=off  clients 100x20 + 80x40            -A -> 100x40   -a -> 80x20
//! status=off  only an ignore-size 80x24 client  -A -> 80x24    -a -> 80x24
//! status=off  client 100x40, unlinked session   -A -> 80x24    -a -> 80x24
//! status=2    client 100x40 on a linked session -A -> 100x38   -a -> 100x38
//! ```
//!
//! The last two land on tmux's `manual:` fallback, which reads the `default-size`
//! option (`80x24` by default) without touching the status rows. rmux uses the
//! target session's own outer terminal size there; the fallback tests below keep
//! an 80x24 session so the two agree exactly.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::*;
use crate::client_flags::ClientFlags;
use crate::outer_terminal::OuterTerminalContext;

const STATUS_TWO: &str = "2";
const STATUS_OFF: &str = "off";

#[tokio::test]
async fn resize_window_largest_takes_the_status_rows_off_the_outer_client_terminal() {
    status_two_single_client_case(ResizeWindowAdjustment::LargestLinkedSession).await;
}

#[tokio::test]
async fn resize_window_smallest_takes_the_status_rows_off_the_outer_client_terminal() {
    status_two_single_client_case(ResizeWindowAdjustment::SmallestLinkedSession).await;
}

/// One 80x24 client and a two-line status: the window must become 80x22
/// content, never the client's outer 80x24.
async fn status_two_single_client_case(adjustment: ResizeWindowAdjustment) {
    let handler = RequestHandler::new();
    let alpha = session_name("alpha");
    let target = WindowTarget::with_window(alpha.clone(), 0);
    create_session_with_size(&handler, "alpha", TerminalSize { cols: 80, rows: 24 }).await;
    set_session_status(&handler, &alpha, STATUS_TWO).await;
    let _client = attach_declared_client(
        &handler,
        60_100,
        &alpha,
        TerminalSize { cols: 80, rows: 24 },
        ClientFlags::default(),
    )
    .await;

    pin_window_size(&handler, &target, TerminalSize { cols: 70, rows: 18 }).await;
    apply_adjustment(&handler, &target, adjustment).await;

    assert_eq!(
        window_size(&handler, &target).await,
        TerminalSize { cols: 80, rows: 22 },
        "{adjustment:?} must convert the client's outer terminal rows to \
         content rows exactly once"
    );
}

#[tokio::test]
async fn resize_window_ranks_clients_by_their_content_rows() {
    let handler = RequestHandler::new();
    let alpha = session_name("alpha");
    let target = WindowTarget::with_window(alpha.clone(), 0);
    create_session_with_size(&handler, "alpha", TerminalSize { cols: 80, rows: 24 }).await;
    set_session_status(&handler, &alpha, STATUS_TWO).await;
    let _small = attach_declared_client(
        &handler,
        60_110,
        &alpha,
        TerminalSize { cols: 80, rows: 24 },
        ClientFlags::default(),
    )
    .await;
    let _large = attach_declared_client(
        &handler,
        60_111,
        &alpha,
        TerminalSize {
            cols: 100,
            rows: 40,
        },
        ClientFlags::default(),
    )
    .await;

    for (adjustment, expected) in [
        (
            ResizeWindowAdjustment::LargestLinkedSession,
            TerminalSize {
                cols: 100,
                rows: 38,
            },
        ),
        (
            ResizeWindowAdjustment::SmallestLinkedSession,
            TerminalSize { cols: 80, rows: 22 },
        ),
    ] {
        pin_window_size(&handler, &target, TerminalSize { cols: 70, rows: 18 }).await;
        apply_adjustment(&handler, &target, adjustment).await;

        assert_eq!(
            window_size(&handler, &target).await,
            expected,
            "{adjustment:?} must rank the clients after each one lost its own \
             status rows"
        );
    }
}

#[tokio::test]
async fn resize_window_takes_each_dimension_from_its_own_client() {
    let handler = RequestHandler::new();
    let alpha = session_name("alpha");
    let target = WindowTarget::with_window(alpha.clone(), 0);
    create_session_with_size(&handler, "alpha", TerminalSize { cols: 80, rows: 24 }).await;
    set_session_status(&handler, &alpha, STATUS_OFF).await;
    let _wide = attach_declared_client(
        &handler,
        60_120,
        &alpha,
        TerminalSize {
            cols: 100,
            rows: 20,
        },
        ClientFlags::default(),
    )
    .await;
    let _tall = attach_declared_client(
        &handler,
        60_121,
        &alpha,
        TerminalSize { cols: 80, rows: 40 },
        ClientFlags::default(),
    )
    .await;

    for (adjustment, expected) in [
        (
            ResizeWindowAdjustment::LargestLinkedSession,
            TerminalSize {
                cols: 100,
                rows: 40,
            },
        ),
        (
            ResizeWindowAdjustment::SmallestLinkedSession,
            TerminalSize { cols: 80, rows: 20 },
        ),
    ] {
        pin_window_size(&handler, &target, TerminalSize { cols: 70, rows: 18 }).await;
        apply_adjustment(&handler, &target, adjustment).await;

        assert_eq!(
            window_size(&handler, &target).await,
            expected,
            "{adjustment:?} must take the extreme of each dimension on its own, \
             not one client's whole size"
        );
    }
}

/// Every way a client can be barred from sizing: `resize-window -a` must keep
/// the eligible 100x40 peer instead of shrinking to the barred 80x24 client.
///
/// Each kind is reported rather than asserted in place, so one run names every
/// kind that regressed instead of stopping at the first.
#[tokio::test]
async fn resize_window_gives_no_sizing_authority_to_an_ineligible_client() {
    let eligible_size = TerminalSize {
        cols: 100,
        rows: 40,
    };
    let mut regressions = Vec::new();
    for kind in [
        IneligibleClient::IgnoreSize,
        IneligibleClient::ReadOnly,
        IneligibleClient::Closing,
        IneligibleClient::Suspended,
        IneligibleClient::StaleSessionIdentity,
    ] {
        let handler = RequestHandler::new();
        let alpha = session_name("alpha");
        let target = WindowTarget::with_window(alpha.clone(), 0);
        create_session_with_size(&handler, "alpha", TerminalSize { cols: 80, rows: 24 }).await;
        set_session_status(&handler, &alpha, STATUS_OFF).await;
        let _eligible = attach_declared_client(
            &handler,
            60_130,
            &alpha,
            eligible_size,
            ClientFlags::default(),
        )
        .await;
        let barred = attach_declared_client(
            &handler,
            60_131,
            &alpha,
            TerminalSize { cols: 80, rows: 24 },
            kind.flags(),
        )
        .await;
        // Bar the client only after the window is pinned: a closing client is
        // reaped by the refresh a resize publishes, and the point here is that
        // the selector rejects it while it is still registered.
        pin_window_size(&handler, &target, TerminalSize { cols: 70, rows: 18 }).await;
        kind.bar_client(&handler, 60_131, &barred).await;
        apply_adjustment(
            &handler,
            &target,
            ResizeWindowAdjustment::SmallestLinkedSession,
        )
        .await;

        let selected = window_size(&handler, &target).await;
        if selected != eligible_size {
            regressions.push(format!("{kind:?} won with {selected:?}"));
        }
    }

    assert!(
        regressions.is_empty(),
        "no ineligible client may win `resize-window -a`, but {regressions:?}"
    );
}

/// The eligibility filter must not swallow the neighbours: with an ignored
/// client present, the *smallest eligible* client still wins `-a` and the
/// largest still wins `-A`.
#[tokio::test]
async fn resize_window_still_ranks_the_eligible_neighbours_of_an_ignored_client() {
    let handler = RequestHandler::new();
    let alpha = session_name("alpha");
    let target = WindowTarget::with_window(alpha.clone(), 0);
    create_session_with_size(&handler, "alpha", TerminalSize { cols: 80, rows: 24 }).await;
    set_session_status(&handler, &alpha, STATUS_OFF).await;
    let _ignored = attach_declared_client(
        &handler,
        60_140,
        &alpha,
        TerminalSize {
            cols: 200,
            rows: 60,
        },
        ClientFlags::IGNORESIZE,
    )
    .await;
    let _small = attach_declared_client(
        &handler,
        60_141,
        &alpha,
        TerminalSize { cols: 80, rows: 24 },
        ClientFlags::default(),
    )
    .await;
    let _large = attach_declared_client(
        &handler,
        60_142,
        &alpha,
        TerminalSize {
            cols: 100,
            rows: 40,
        },
        ClientFlags::default(),
    )
    .await;

    for (adjustment, expected) in [
        (
            ResizeWindowAdjustment::LargestLinkedSession,
            TerminalSize {
                cols: 100,
                rows: 40,
            },
        ),
        (
            ResizeWindowAdjustment::SmallestLinkedSession,
            TerminalSize { cols: 80, rows: 24 },
        ),
    ] {
        pin_window_size(&handler, &target, TerminalSize { cols: 70, rows: 18 }).await;
        apply_adjustment(&handler, &target, adjustment).await;

        assert_eq!(
            window_size(&handler, &target).await,
            expected,
            "{adjustment:?} must still rank the eligible clients around an \
             ignore-size client"
        );
    }
}

#[tokio::test]
async fn resize_window_without_an_eligible_client_falls_back_to_the_session_terminal_size() {
    for adjustment in [
        ResizeWindowAdjustment::LargestLinkedSession,
        ResizeWindowAdjustment::SmallestLinkedSession,
    ] {
        let handler = RequestHandler::new();
        let alpha = session_name("alpha");
        let target = WindowTarget::with_window(alpha.clone(), 0);
        create_session_with_size(&handler, "alpha", TerminalSize { cols: 80, rows: 24 }).await;
        set_session_status(&handler, &alpha, STATUS_OFF).await;
        let _ignored = attach_declared_client(
            &handler,
            60_150,
            &alpha,
            TerminalSize {
                cols: 132,
                rows: 50,
            },
            ClientFlags::IGNORESIZE,
        )
        .await;

        pin_window_size(&handler, &target, TerminalSize { cols: 70, rows: 18 }).await;
        apply_adjustment(&handler, &target, adjustment).await;

        assert_eq!(
            window_size(&handler, &target).await,
            TerminalSize { cols: 80, rows: 24 },
            "with no eligible client {adjustment:?} must fall back instead of \
             borrowing the ignored client's geometry"
        );
    }
}

#[tokio::test]
async fn resize_window_selects_a_client_attached_to_a_linked_session() {
    let handler = RequestHandler::new();
    let alpha = session_name("alpha");
    let beta = session_name("beta");
    let target = WindowTarget::with_window(alpha.clone(), 0);
    create_session_with_size(&handler, "alpha", TerminalSize { cols: 80, rows: 24 }).await;
    create_session_with_size(&handler, "beta", TerminalSize { cols: 80, rows: 24 }).await;
    set_session_status(&handler, &alpha, STATUS_TWO).await;
    set_session_status(&handler, &beta, STATUS_TWO).await;
    let link = handler
        .handle(Request::LinkWindow(LinkWindowRequest {
            source: target.clone(),
            target: WindowTarget::with_window(beta.clone(), 1),
            after: false,
            before: false,
            kill_destination: false,
            detached: false,
        }))
        .await;
    assert!(
        matches!(link, Response::LinkWindow(_)),
        "expected link-window success, got {link:?}"
    );
    let _client = attach_declared_client(
        &handler,
        60_160,
        &beta,
        TerminalSize {
            cols: 100,
            rows: 40,
        },
        ClientFlags::default(),
    )
    .await;

    pin_window_size(&handler, &target, TerminalSize { cols: 70, rows: 18 }).await;
    apply_adjustment(
        &handler,
        &target,
        ResizeWindowAdjustment::LargestLinkedSession,
    )
    .await;

    assert_eq!(
        window_size(&handler, &target).await,
        TerminalSize {
            cols: 100,
            rows: 38
        },
        "a client attached to a linked session still owns the window size, \
         minus its status rows"
    );
}

#[derive(Debug, Clone, Copy)]
enum IneligibleClient {
    IgnoreSize,
    ReadOnly,
    Closing,
    Suspended,
    StaleSessionIdentity,
}

impl IneligibleClient {
    fn flags(self) -> ClientFlags {
        match self {
            Self::IgnoreSize => ClientFlags::IGNORESIZE,
            Self::ReadOnly => ClientFlags::default().with_read_only(),
            Self::Closing | Self::Suspended | Self::StaleSessionIdentity => ClientFlags::default(),
        }
    }

    async fn bar_client(self, handler: &RequestHandler, pid: u32, client: &AttachedTestClient) {
        match self {
            Self::IgnoreSize | Self::ReadOnly => {}
            Self::Closing => client.closing.store(true, Ordering::SeqCst),
            Self::Suspended => {
                let mut active_attach = handler.active_attach.lock().await;
                active_attach
                    .by_pid
                    .get_mut(&pid)
                    .expect("registered attach exists")
                    .suspended = true;
            }
            Self::StaleSessionIdentity => {
                let mut active_attach = handler.active_attach.lock().await;
                let active = active_attach
                    .by_pid
                    .get_mut(&pid)
                    .expect("registered attach exists");
                active.session_id = rmux_proto::SessionId::new(active.session_id.as_u32() + 1_000);
            }
        }
    }
}

/// Keeps the attach's control channel and closing flag alive for the test.
struct AttachedTestClient {
    _control_rx: mpsc::UnboundedReceiver<AttachControl>,
    closing: Arc<AtomicBool>,
}

async fn attach_declared_client(
    handler: &RequestHandler,
    pid: u32,
    session: &SessionName,
    size: TerminalSize,
    flags: ClientFlags,
) -> AttachedTestClient {
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let closing = Arc::new(AtomicBool::new(false));
    handler
        .register_attach_with_closing(
            pid,
            session.clone(),
            control_tx,
            Arc::clone(&closing),
            OuterTerminalContext::default(),
            flags,
        )
        .await;
    let mut active_attach = handler.active_attach.lock().await;
    active_attach
        .by_pid
        .get_mut(&pid)
        .expect("registered attach exists")
        .set_declared_client_size(size);
    drop(active_attach);
    AttachedTestClient {
        _control_rx: control_rx,
        closing,
    }
}

async fn set_session_status(handler: &RequestHandler, session: &SessionName, value: &str) {
    let response = handler
        .handle(Request::SetOption(SetOptionRequest {
            scope: ScopeSelector::Session(session.clone()),
            option: OptionName::Status,
            value: value.to_owned(),
            mode: SetOptionMode::Replace,
        }))
        .await;
    assert!(matches!(response, Response::SetOption(_)), "{response:?}");
}

/// Pins the window at an explicit geometry, which also parks `window-size` on
/// `manual` so only the command under test can move it again.
async fn pin_window_size(handler: &RequestHandler, target: &WindowTarget, size: TerminalSize) {
    let response = handler
        .handle(Request::ResizeWindow(ResizeWindowRequest {
            target: target.clone(),
            width: Some(size.cols),
            height: Some(size.rows),
            adjustment: None,
        }))
        .await;
    assert!(
        matches!(response, Response::ResizeWindow(_)),
        "expected explicit resize success, got {response:?}"
    );
    assert_eq!(window_size(handler, target).await, size);
}

async fn apply_adjustment(
    handler: &RequestHandler,
    target: &WindowTarget,
    adjustment: ResizeWindowAdjustment,
) {
    let response = handler
        .handle(Request::ResizeWindow(ResizeWindowRequest {
            target: target.clone(),
            width: None,
            height: None,
            adjustment: Some(adjustment),
        }))
        .await;
    assert!(
        matches!(response, Response::ResizeWindow(_)),
        "expected {adjustment:?} success, got {response:?}"
    );
}

async fn window_size(handler: &RequestHandler, target: &WindowTarget) -> TerminalSize {
    let state = handler.state.lock().await;
    state
        .sessions
        .session(target.session_name())
        .and_then(|session| session.window_at(target.window_index()))
        .expect("window exists")
        .size()
}
