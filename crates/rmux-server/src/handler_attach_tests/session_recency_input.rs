//! Accepted attached-client interaction is what advances session recency.
//!
//! The commit sits at the input admission boundary, before the server decides
//! whether the bytes become a prefix, a key binding, a mode key or a pane
//! write. Committing after pane I/O instead would drop every key the server
//! consumes itself, which is the defect these tests pin.
//!
//! Measured against tmux 3.7b on this host: a bare prefix key and a copy-mode
//! navigation key each advanced `session_activity`, while `send-keys` to a
//! detached session and `rename-session` did not.
//!
//! Every fixture pins the public whole seconds *after* the event under test,
//! because that is the condition targetless ranking has to survive: with the
//! seconds equal, only the internal recency order can still separate the
//! sessions. `used` is deliberately the alphabetically last name and `spare`
//! the first, so the legacy name tiebreak cannot produce the expected answer.

use super::*;
use rmux_core::{TargetFindContext, TargetFindFlags, TargetFindType, UnresolvedTarget};

/// One arbitrary but fixed second shared by every session in a fixture.
const PINNED_SECOND: i64 = 1_785_500_000;

/// The session an attached client interacts with. Alphabetically last, so the
/// legacy `activity_at`/`created_at`/name ranking can never select it.
const USED: &str = "zulu";
/// A detached session created after `USED`, so it starts out the most recent.
const SPARE: &str = "alfa";

/// Collapses every public timestamp onto one second, leaving only the internal
/// recency order able to rank the sessions.
async fn pin_public_seconds(handler: &RequestHandler) {
    let mut state = handler.state.lock().await;
    let names = state
        .sessions
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    for name in names {
        state
            .sessions
            .session_mut(&name)
            .expect("listed session exists")
            .pin_public_times_for_tests(PINNED_SECOND);
    }
    assert!(
        state
            .sessions
            .iter()
            .all(|(_, session)| session.created_at() == PINNED_SECOND
                && session.activity_at() == PINNED_SECOND),
        "the fixture must leave no public second able to order the sessions"
    );
}

/// Resolves a targetless session through the core reader, which is the reader a
/// detached scripting client reaches.
async fn default_session(handler: &RequestHandler) -> SessionName {
    let state = handler.state.lock().await;
    let target = state
        .sessions
        .resolve_unresolved_target(
            &UnresolvedTarget::none(),
            TargetFindType::Session,
            TargetFindFlags::NONE,
            &TargetFindContext::new(None),
        )
        .expect("default session resolves");
    let Target::Session(name) = target else {
        panic!("default session target did not resolve to a session: {target:?}");
    };
    name
}

/// Attaches a client to `USED`, then creates `SPARE` afterwards so that `SPARE`
/// — not the attached session — is the session a targetless command selects.
/// Any later win by `USED` therefore has to come from the interaction itself.
async fn interaction_fixture(
    attach_pid: u32,
) -> (RequestHandler, mpsc::UnboundedReceiver<AttachControl>) {
    let handler = RequestHandler::new();
    let control_rx = create_quiet_attached_session(&handler, attach_pid, &session_name(USED)).await;
    create_quiet_session(&handler, &session_name(SPARE)).await;
    pin_public_seconds(&handler).await;
    assert_eq!(
        default_session(&handler).await,
        session_name(SPARE),
        "the fixture must start with the untouched session winning"
    );
    (handler, control_rx)
}

#[tokio::test]
async fn an_accepted_prefix_key_advances_targetless_session_recency() {
    let attach_pid = std::process::id();
    let (handler, _control_rx) = interaction_fixture(attach_pid).await;

    // A bare prefix is consumed by the server and never reaches a pane, so a
    // post-pane-write commit cannot see it at all.
    handler
        .handle_attached_live_input_for_test(attach_pid, b"\x02")
        .await
        .expect("prefix key input succeeds");
    pin_public_seconds(&handler).await;

    assert_eq!(default_session(&handler).await, session_name(USED));
}

#[tokio::test]
async fn an_accepted_key_binding_advances_targetless_session_recency() {
    let attach_pid = std::process::id();
    let (handler, _control_rx) = interaction_fixture(attach_pid).await;

    // `prefix c` is dispatched locally as new-window; no byte reaches a pane.
    handler
        .handle_attached_live_input_for_test(attach_pid, b"\x02c")
        .await
        .expect("prefix c input succeeds");
    pin_public_seconds(&handler).await;

    assert_eq!(default_session(&handler).await, session_name(USED));
}

#[tokio::test]
async fn a_locally_consumed_mode_key_advances_targetless_session_recency() {
    let attach_pid = std::process::id();
    let handler = RequestHandler::new();
    let _control_rx =
        create_quiet_attached_session(&handler, attach_pid, &session_name(USED)).await;
    handler
        .handle_attached_live_input_for_test(attach_pid, b"\x02[")
        .await
        .expect("prefix [ enters copy mode");
    assert!(
        pane_mode_status(&handler, &session_name(USED))
            .await
            .contains("copy-mode"),
        "the fixture must actually be in a mode for its keys to be consumed locally"
    );

    // Only now is the rival created, so copy mode itself cannot be the cause.
    create_quiet_session(&handler, &session_name(SPARE)).await;
    pin_public_seconds(&handler).await;
    assert_eq!(default_session(&handler).await, session_name(SPARE));

    handler
        .handle_attached_live_input_for_test(attach_pid, b"\x1b[A")
        .await
        .expect("copy-mode cursor key succeeds");
    assert!(
        pane_mode_status(&handler, &session_name(USED))
            .await
            .contains("copy-mode"),
        "the cursor key must have been consumed by the mode, not sent to the pane"
    );
    pin_public_seconds(&handler).await;

    assert_eq!(default_session(&handler).await, session_name(USED));
}

#[tokio::test]
async fn successful_pane_input_advances_targetless_session_recency() {
    let attach_pid = std::process::id();
    let (handler, _control_rx) = interaction_fixture(attach_pid).await;

    handler
        .handle_attached_live_input_for_test(attach_pid, b"x")
        .await
        .expect("plain pane input succeeds");
    pin_public_seconds(&handler).await;

    assert_eq!(default_session(&handler).await, session_name(USED));
}

#[tokio::test]
async fn synchronized_pane_input_advances_the_session_at_the_admission_boundary() {
    let attach_pid = std::process::id();
    let (handler, _control_rx) = interaction_fixture(attach_pid).await;
    split_used_window(&handler).await;
    set_synchronize_panes(&handler).await;
    // Splitting is window bookkeeping rather than client interaction, so the
    // rival must still be winning when the one synchronized input arrives.
    pin_public_seconds(&handler).await;
    assert_eq!(default_session(&handler).await, session_name(SPARE));

    handler
        .handle_attached_live_input_for_test(attach_pid, b"y")
        .await
        .expect("synchronized pane input succeeds");
    pin_public_seconds(&handler).await;

    // The commit happens once, before the input fans out to either pane.
    assert_eq!(default_session(&handler).await, session_name(USED));
}

#[tokio::test]
async fn switching_a_client_advances_the_target_session_recency() {
    let attach_pid = std::process::id();
    let handler = RequestHandler::new();
    create_quiet_session(&handler, &session_name(USED)).await;
    create_quiet_session(&handler, &session_name(SPARE)).await;
    let (control_tx, _control_rx) = mpsc::unbounded_channel();
    handler
        .register_attach(attach_pid, session_name(SPARE), control_tx)
        .await;
    pin_public_seconds(&handler).await;
    assert_eq!(default_session(&handler).await, session_name(SPARE));

    let switched = handler
        .handle(Request::SwitchClient(SwitchClientRequest {
            target: session_name(USED),
        }))
        .await;
    assert!(
        matches!(switched, Response::SwitchClient(_)),
        "{switched:?}"
    );
    pin_public_seconds(&handler).await;

    // tmux 3.7b updates session activity when a client switches into a
    // session, exactly as it does on attach.
    assert_eq!(default_session(&handler).await, session_name(USED));
}

#[tokio::test]
async fn empty_attached_input_does_not_advance_targetless_session_recency() {
    let attach_pid = std::process::id();
    let (handler, _control_rx) = interaction_fixture(attach_pid).await;

    handler
        .handle_attached_live_input_for_test(attach_pid, b"")
        .await
        .expect("empty attached input succeeds");
    pin_public_seconds(&handler).await;

    assert_eq!(default_session(&handler).await, session_name(SPARE));
}

#[tokio::test]
async fn read_only_client_input_does_not_advance_targetless_session_recency() {
    let attach_pid = std::process::id();
    let handler = RequestHandler::new();
    create_quiet_session(&handler, &session_name(USED)).await;
    let (control_tx, _control_rx) = mpsc::unbounded_channel();
    handler
        .register_attach_with_closing(
            attach_pid,
            session_name(USED),
            control_tx,
            Arc::new(AtomicBool::new(false)),
            OuterTerminalContext::default(),
            crate::client_flags::ClientFlags::READONLY,
        )
        .await;
    create_quiet_session(&handler, &session_name(SPARE)).await;
    pin_public_seconds(&handler).await;
    assert_eq!(default_session(&handler).await, session_name(SPARE));

    let _ = handler
        .handle_attached_live_input_for_test(attach_pid, b"x")
        .await;
    pin_public_seconds(&handler).await;

    // Read-only clients already do not advance client activity; the session
    // commit reuses that policy rather than inventing a second one.
    assert_eq!(default_session(&handler).await, session_name(SPARE));
}

#[tokio::test]
async fn input_from_a_vanished_attach_does_not_advance_targetless_session_recency() {
    let attach_pid = std::process::id();
    let (handler, _control_rx) = interaction_fixture(attach_pid).await;

    let error = handler
        .handle_attached_live_input_for_test(attach_pid.wrapping_add(1), b"x")
        .await
        .expect_err("input for an unregistered attach must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::Other, "{error:?}");
    pin_public_seconds(&handler).await;

    assert_eq!(default_session(&handler).await, session_name(SPARE));
}

#[tokio::test]
async fn input_must_not_advance_a_same_name_session_recreated_under_the_client() {
    let attach_pid = std::process::id();
    let (handler, _control_rx) = interaction_fixture(attach_pid).await;
    let original_id = session_id(&handler, USED).await;

    // Destroy and recreate the attached name. The attach record still carries
    // the destroyed session's id, so its input belongs to a lifetime that no
    // longer exists and must not be credited to the new one.
    {
        let mut state = handler.state.lock().await;
        state
            .sessions
            .remove_session(&session_name(USED))
            .expect("session removal succeeds");
        state
            .sessions
            .create_session(session_name(USED), TerminalSize { cols: 80, rows: 24 })
            .expect("replacement session creation succeeds");
    }
    assert_ne!(
        session_id(&handler, USED).await,
        original_id,
        "the recreated session must be a new identity"
    );
    pin_public_seconds(&handler).await;
    assert_eq!(default_session(&handler).await, session_name(USED));

    // Make the rival the most recent again, then replay the stale client.
    touch_spare(&handler).await;
    pin_public_seconds(&handler).await;
    assert_eq!(default_session(&handler).await, session_name(SPARE));

    let _ = handler
        .handle_attached_live_input_for_test(attach_pid, b"x")
        .await;
    pin_public_seconds(&handler).await;

    assert_eq!(default_session(&handler).await, session_name(SPARE));
}

#[tokio::test]
async fn detaching_does_not_advance_targetless_session_recency() {
    let attach_pid = std::process::id();
    let handler = RequestHandler::new();
    create_quiet_session(&handler, &session_name(USED)).await;
    let (control_tx, _control_rx) = mpsc::unbounded_channel();
    let attach_id = handler
        .register_attach(attach_pid, session_name(USED), control_tx)
        .await;
    create_quiet_session(&handler, &session_name(SPARE)).await;
    pin_public_seconds(&handler).await;
    assert_eq!(default_session(&handler).await, session_name(SPARE));

    handler.finish_attach(attach_pid, attach_id).await;
    pin_public_seconds(&handler).await;

    // Leaving a session is not using it.
    assert_eq!(default_session(&handler).await, session_name(SPARE));
}

#[tokio::test]
async fn explicit_send_keys_to_a_detached_session_does_not_advance_its_recency() {
    let handler = RequestHandler::new();
    create_quiet_session(&handler, &session_name(USED)).await;
    create_quiet_session(&handler, &session_name(SPARE)).await;
    pin_public_seconds(&handler).await;
    assert_eq!(default_session(&handler).await, session_name(SPARE));

    let response = handler
        .handle(Request::SendKeys(SendKeysRequest {
            target: PaneTarget::new(session_name(USED), 0),
            keys: vec!["x".to_owned()],
        }))
        .await;
    assert!(matches!(response, Response::SendKeys(_)), "{response:?}");
    pin_public_seconds(&handler).await;

    // tmux 3.7b measured: `send-keys` to a detached session left its
    // `session_activity` unchanged. Command input is not client interaction.
    assert_eq!(default_session(&handler).await, session_name(SPARE));
}

async fn session_id(handler: &RequestHandler, name: &str) -> rmux_proto::SessionId {
    handler
        .state
        .lock()
        .await
        .sessions
        .session(&session_name(name))
        .expect("session exists")
        .id()
}

/// Marks `SPARE` as used through the ordinary attach path.
async fn touch_spare(handler: &RequestHandler) {
    handler
        .state
        .lock()
        .await
        .sessions
        .session_mut(&session_name(SPARE))
        .expect("spare session exists")
        .touch_attached();
}

async fn split_used_window(handler: &RequestHandler) {
    let response = handler
        .handle(Request::SplitWindow(SplitWindowRequest {
            target: SplitWindowTarget::Session(session_name(USED)),
            direction: rmux_proto::SplitDirection::Vertical,
            before: false,
            environment: None,
        }))
        .await;
    assert!(matches!(response, Response::SplitWindow(_)), "{response:?}");
}

async fn set_synchronize_panes(handler: &RequestHandler) {
    let response = handler
        .handle(Request::SetOption(SetOptionRequest {
            scope: ScopeSelector::Window(WindowTarget::with_window(session_name(USED), 0)),
            option: OptionName::SynchronizePanes,
            value: "on".to_owned(),
            mode: SetOptionMode::Replace,
        }))
        .await;
    assert!(matches!(response, Response::SetOption(_)), "{response:?}");
}
