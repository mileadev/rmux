//! `switch-client` frames the target with the status of the session the client
//! is *joining*, never the one it is leaving.
//!
//! tmux 3.7b's `cmd_switch_client_exec` assigns `c->session = s` before it calls
//! `recalculate_sizes()` and `server_redraw_client(c)`, and all three happen
//! inside the command's own execution. A client migrating between two aliases of
//! one linked window therefore votes exactly once — under the session it joins —
//! and the very first frame it receives is already drawn at that geometry.
//!
//! Measured against the pinned tmux 3.7b (binary sha256 `eb05f981…`, the
//! `frozen_reference.yaml` build) on macOS 26.5.2 arm64. `alpha:0` is linked into
//! `beta:1`, `beta` shows the alias, one real 100x40 PTY client migrates from
//! `alpha` to `beta`, and the geometry is read from the window as soon as
//! `switch-client` returns:
//!
//! ```text
//! source  target  window-size  at switch  settled
//! 2       off     smallest     100x40     100x40
//! 2       off     largest      100x40     100x40
//! 2       off     latest       100x40     100x40
//! off     2       smallest     100x38     100x38
//! off     2       largest      100x38     100x38
//! off     2       latest       100x38     100x38
//! 2       2       smallest     100x38     100x38
//! 2       2       largest      100x38     100x38
//! 2       2       latest       100x38     100x38
//! off     off     smallest     100x40     100x40
//! off     off     largest      100x40     100x40
//! off     off     latest       100x40     100x40
//! ```
//!
//! The source status never reaches the result and the policy never changes it:
//! one client owns one vote, and it is cast under the target's status. Probe and
//! log: `.rmux-audit/m9-switch-frame/oracle_switch_frame.py`,
//! `oracle-switch-frame.log`.

use super::*;

const CLIENT_SIZE: TerminalSize = TerminalSize {
    cols: 100,
    rows: 40,
};
/// The in-flight request of a command whose registration has been replaced.
const STALE_CLIENT_SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };
/// The geometry the replacement legitimately owns. Both statuses are `off` in
/// the replacement race, so content rows and outer rows coincide.
const REPLACEMENT_CLIENT_SIZE: TerminalSize = TerminalSize {
    cols: 120,
    rows: 50,
};
const STATUS_TWO: &str = "2";
const STATUS_OFF: &str = "off";
const SWITCHING_PID: u32 = 93_101;
const SOURCE_WINDOW_INDEX: u32 = 0;
const TARGET_WINDOW_INDEX: u32 = 1;

/// `(source status, target status, geometry the joined session implies)`.
const SWITCH_FRAME_MATRIX: [(&str, &str, TerminalSize); 4] = [
    (
        STATUS_TWO,
        STATUS_OFF,
        TerminalSize {
            cols: 100,
            rows: 40,
        },
    ),
    (
        STATUS_OFF,
        STATUS_TWO,
        TerminalSize {
            cols: 100,
            rows: 38,
        },
    ),
    (
        STATUS_TWO,
        STATUS_TWO,
        TerminalSize {
            cols: 100,
            rows: 38,
        },
    ),
    (
        STATUS_OFF,
        STATUS_OFF,
        TerminalSize {
            cols: 100,
            rows: 40,
        },
    ),
];

/// The frame the switching client actually receives, not merely the size the
/// window settles on afterwards: a late reconcile can repair the stored geometry
/// while the client has already painted a wrong first screen.
#[tokio::test]
async fn switch_client_frames_the_target_with_the_joined_sessions_status() {
    let mut regressions = Vec::new();
    for (source_status, target_status, expected) in SWITCH_FRAME_MATRIX {
        for policy in ["smallest", "largest", "latest"] {
            let handler = RequestHandler::new();
            let (alpha, beta) = linked_alias_sessions(&handler, source_status, target_status).await;
            set_window_size_policy(&handler, &alpha, SOURCE_WINDOW_INDEX, policy).await;
            set_window_size_policy(&handler, &beta, TARGET_WINDOW_INDEX, policy).await;
            let mut control_rx =
                register_declared_attach(&handler, SWITCHING_PID, &alpha, CLIENT_SIZE).await;
            drain_attach_controls(&mut control_rx);

            let response = handler
                .dispatch(
                    SWITCHING_PID,
                    Request::SwitchClient(SwitchClientRequest {
                        target: beta.clone(),
                    }),
                )
                .await
                .response;
            assert!(
                matches!(response, Response::SwitchClient(_)),
                "switch-client must succeed, got {response:?}"
            );

            let framed = frame_geometry(
                recv_switch_target(&mut control_rx, "linked-alias switch frame").await,
            );
            if framed != expected {
                regressions.push(format!(
                    "source status={source_status} target status={target_status} \
                     window-size={policy}: switch frame is {framed:?}, expected {expected:?}"
                ));
            }
            let settled = window_content_size(&handler, &beta, TARGET_WINDOW_INDEX).await;
            if settled != expected {
                regressions.push(format!(
                    "source status={source_status} target status={target_status} \
                     window-size={policy}: settled window is {settled:?}, expected {expected:?}"
                ));
            }
        }
    }

    assert!(
        regressions.is_empty(),
        "a migrating client owns one vote, cast under the session it joins, but \
         {regressions:?}"
    );
}

/// The same seam reached by an already-attached client running `attach-session`,
/// which tmux also routes through `cmd_switch_client_exec`'s client move.
#[tokio::test]
async fn attach_session_frames_the_target_with_the_joined_sessions_status() {
    let mut regressions = Vec::new();
    for (source_status, target_status, expected) in SWITCH_FRAME_MATRIX {
        for policy in ["smallest", "largest"] {
            let handler = RequestHandler::new();
            let (alpha, beta) = linked_alias_sessions(&handler, source_status, target_status).await;
            set_window_size_policy(&handler, &alpha, SOURCE_WINDOW_INDEX, policy).await;
            set_window_size_policy(&handler, &beta, TARGET_WINDOW_INDEX, policy).await;
            let mut control_rx =
                register_declared_attach(&handler, SWITCHING_PID, &alpha, CLIENT_SIZE).await;
            drain_attach_controls(&mut control_rx);

            let response = handler
                .dispatch(
                    SWITCHING_PID,
                    Request::AttachSession(rmux_proto::AttachSessionRequest {
                        target: beta.clone(),
                    }),
                )
                .await
                .response;
            assert!(
                matches!(response, Response::SwitchClient(_)),
                "attach-session from an attached client must switch it, got {response:?}"
            );

            let framed = frame_geometry(
                recv_switch_target(&mut control_rx, "linked-alias attach frame").await,
            );
            if framed != expected {
                regressions.push(format!(
                    "source status={source_status} target status={target_status} \
                     window-size={policy}: attach frame is {framed:?}, expected {expected:?}"
                ));
            }
            let settled = window_content_size(&handler, &beta, TARGET_WINDOW_INDEX).await;
            if settled != expected {
                regressions.push(format!(
                    "source status={source_status} target status={target_status} \
                     window-size={policy}: settled window is {settled:?}, expected {expected:?}"
                ));
            }
        }
    }

    assert!(
        regressions.is_empty(),
        "attach-session moves a client the same way switch-client does, but \
         {regressions:?}"
    );
}

/// The selection itself, for the direction the rendered frame cannot show.
///
/// The renderer clips the active pane to `terminal rows - status(target)`, so a
/// stale vote that makes the selection too *tall* is masked in the frame while a
/// stale vote that makes it too *short* is not. The geometry the commit stores
/// is wrong either way, and a late reconcile repairs it, so this pins the rule at
/// the boundary it belongs to: the client migrating in owns exactly one vote,
/// cast under the session it is joining.
#[tokio::test]
async fn a_migrating_client_replaces_its_own_stale_registration_in_the_selection() {
    let mut regressions = Vec::new();
    for (source_status, target_status, expected) in SWITCH_FRAME_MATRIX {
        for policy in ["smallest", "largest", "latest"] {
            let handler = RequestHandler::new();
            let (alpha, beta) = linked_alias_sessions(&handler, source_status, target_status).await;
            set_window_size_policy(&handler, &alpha, SOURCE_WINDOW_INDEX, policy).await;
            set_window_size_policy(&handler, &beta, TARGET_WINDOW_INDEX, policy).await;
            let _control_rx =
                register_declared_attach(&handler, SWITCHING_PID, &alpha, CLIENT_SIZE).await;

            let selected = handler
                .selected_attached_session_size(
                    &beta,
                    Some(super::super::attach_support::IncomingSizeClient::joining(
                        Some(attach_generation(&handler, SWITCHING_PID).await),
                        CLIENT_SIZE,
                        super::super::attach_support::ClientFlags::default(),
                    )),
                )
                .await
                .expect("the target session resolves a size selection")
                .selected_size();
            if selected != Some(expected) {
                regressions.push(format!(
                    "source status={source_status} target status={target_status} \
                     window-size={policy}: selected {selected:?}, expected {expected:?}"
                ));
            }
        }
    }

    assert!(
        regressions.is_empty(),
        "the session a client is leaving must not vote for it, but {regressions:?}"
    );
}

/// A same-pid replacement is a different client, and the command that no longer
/// owns the pid must neither erase its vote nor move the window under it.
///
/// `register_attach_identity` replaces `by_pid[pid]` in place and hands the
/// replacement a fresh generation, so an `attach-session` still in flight holds a
/// registration it no longer owns. Displacing by pid number dropped the
/// replacement's legitimate `120x50` vote from the field and wrote the stale
/// `80x24` request over the shared linked window; the command only failed
/// identity validation afterwards, in `set_attached_client_flags`, with the
/// window already shrunk.
///
/// Both policies must hold, and they fail for different reasons. Under `largest`
/// a counted replacement wins outright, so only its removal from the field can
/// produce `80x24`. Under `smallest` the stale request would win a field it is
/// no longer entitled to enter, so the mutation must not happen at all.
#[tokio::test]
async fn a_stale_attach_session_must_not_displace_a_same_pid_replacement() {
    let mut regressions = Vec::new();
    for policy in ["largest", "smallest"] {
        let handler = RequestHandler::new();
        let (alpha, beta) = linked_alias_sessions(&handler, STATUS_OFF, STATUS_OFF).await;
        set_window_size_policy(&handler, &alpha, SOURCE_WINDOW_INDEX, policy).await;
        set_window_size_policy(&handler, &beta, TARGET_WINDOW_INDEX, policy).await;

        let mut stale_rx =
            register_declared_attach(&handler, SWITCHING_PID, &alpha, STALE_CLIENT_SIZE).await;
        drain_attach_controls(&mut stale_rx);
        let stale_identity = handler.active_attach_identity_for_test(SWITCHING_PID).await;

        // The pause lands after the stale command has selected its size and
        // released every lock, which is exactly the window a re-attach uses.
        let pause = handler.install_attached_size_selection_pause();
        // The binding an attached client's own command carries: attached key
        // dispatch and the command prompt run `attach-session` inside the
        // registration that issued it, so the command keeps speaking for that
        // exact generation and for no other.
        let stale_attach = super::super::with_expected_attach_and_session_identity(
            stale_identity,
            alpha.clone(),
            stale_identity.session_id(),
            // A sized `attach-session`: the request carries the client's own
            // geometry, so it reaches the session resize that selects and
            // applies the shared window's size.
            handler.dispatch(
                SWITCHING_PID,
                Request::AttachSessionExt2(Box::new(AttachSessionExt2Request {
                    target: Some(beta.clone()),
                    target_spec: Some(beta.to_string()),
                    detach_other_clients: false,
                    kill_other_clients: false,
                    read_only: false,
                    skip_environment_update: false,
                    flags: None,
                    working_directory: None,
                    client_terminal: rmux_proto::ClientTerminalContext::default(),
                    client_size: Some(STALE_CLIENT_SIZE),
                })),
            ),
        );
        let replace_the_registration = async {
            pause.reached.notified().await;
            let mut replacement_rx =
                register_declared_attach(&handler, SWITCHING_PID, &beta, REPLACEMENT_CLIENT_SIZE)
                    .await;
            drain_attach_controls(&mut replacement_rx);
            let replacement_generation = attach_generation_id(&handler, SWITCHING_PID).await;
            let staged = window_content_size(&handler, &beta, TARGET_WINDOW_INDEX).await;
            pause.release.notify_one();
            (replacement_rx, replacement_generation, staged)
        };
        let (stale, (mut replacement_rx, replacement_generation, staged)) =
            tokio::join!(stale_attach, replace_the_registration);

        assert_ne!(
            replacement_generation,
            stale_identity.attach_id(),
            "the re-attach must install a new generation under the same pid"
        );
        assert_eq!(
            staged, REPLACEMENT_CLIENT_SIZE,
            "window-size={policy}: the replacement must own the shared window \
             before the stale command is released"
        );

        if !matches!(stale.response, Response::Error(_)) {
            regressions.push(format!(
                "window-size={policy}: the stale attach-session must fail, got {:?}",
                stale.response
            ));
        }
        for (alias, window_index) in [(&beta, TARGET_WINDOW_INDEX), (&alpha, SOURCE_WINDOW_INDEX)] {
            let settled = window_content_size(&handler, alias, window_index).await;
            if settled != REPLACEMENT_CLIENT_SIZE {
                regressions.push(format!(
                    "window-size={policy}: alias {alias}:{window_index} is {settled:?}, \
                     expected the replacement's {REPLACEMENT_CLIENT_SIZE:?}"
                ));
            }
        }
        let held = attach_generation_id(&handler, SWITCHING_PID).await;
        if held != replacement_generation {
            regressions.push(format!(
                "window-size={policy}: the replacement must still hold the pid"
            ));
        }
        while let Ok(control) = replacement_rx.try_recv() {
            if matches!(control, AttachControl::Switch(_)) {
                regressions.push(format!(
                    "window-size={policy}: the stale command must not frame the replacement"
                ));
            }
        }
    }

    assert!(
        regressions.is_empty(),
        "a stale command must fail before it displaces or moves a same-pid \
         replacement, but {regressions:?}"
    );
}

/// The geometry the switch payload really carries: the active pane rectangle is
/// derived from the same rendered snapshot the frame bytes were painted from, so
/// it is the frame's own geometry rather than whatever the stored window later
/// reconciles to.
fn frame_geometry(target: crate::pane_io::AttachTarget) -> TerminalSize {
    TerminalSize {
        cols: target.active_pane_geometry.cols(),
        rows: target.active_pane_geometry.rows(),
    }
}

/// `alpha:0` linked into `beta:1`, with `beta` showing the alias. Both sessions
/// are 100x40 so the only geometry in play is the migrating client's.
async fn linked_alias_sessions(
    handler: &RequestHandler,
    source_status: &str,
    target_status: &str,
) -> (SessionName, SessionName) {
    let alpha = session_name("switch-frame-alpha");
    let beta = session_name("switch-frame-beta");
    create_session(handler, &alpha).await;
    create_session(handler, &beta).await;
    set_session_status(handler, &alpha, source_status).await;
    set_session_status(handler, &beta, target_status).await;
    let linked = handler
        .handle(Request::LinkWindow(LinkWindowRequest {
            source: WindowTarget::with_window(alpha.clone(), SOURCE_WINDOW_INDEX),
            target: WindowTarget::with_window(beta.clone(), TARGET_WINDOW_INDEX),
            after: false,
            before: false,
            kill_destination: false,
            detached: false,
        }))
        .await;
    assert!(
        matches!(linked, Response::LinkWindow(_)),
        "expected link-window success, got {linked:?}"
    );
    assert_eq!(
        active_window_index(handler, &beta).await,
        TARGET_WINDOW_INDEX,
        "the target session must be showing the linked alias"
    );
    (alpha, beta)
}

async fn create_session(handler: &RequestHandler, session: &SessionName) {
    let created = handler
        .handle(Request::NewSession(NewSessionRequest {
            session_name: session.clone(),
            detached: true,
            size: Some(CLIENT_SIZE),
            environment: None,
        }))
        .await;
    assert!(
        matches!(created, Response::NewSession(_)),
        "expected new-session success, got {created:?}"
    );
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

/// tmux keys `window-size` on the window itself, so a linked window carries one
/// policy through every alias. rmux keys window options per `(session, index)`,
/// so both aliases are set here to reproduce the oracle's single window option.
async fn set_window_size_policy(
    handler: &RequestHandler,
    session: &SessionName,
    window_index: u32,
    value: &str,
) {
    let response = handler
        .handle(Request::SetOption(SetOptionRequest {
            scope: ScopeSelector::Window(WindowTarget::with_window(session.clone(), window_index)),
            option: OptionName::WindowSize,
            value: value.to_owned(),
            mode: SetOptionMode::Replace,
        }))
        .await;
    assert!(matches!(response, Response::SetOption(_)), "{response:?}");
}

async fn register_declared_attach(
    handler: &RequestHandler,
    requester_pid: u32,
    session: &SessionName,
    size: TerminalSize,
) -> mpsc::UnboundedReceiver<AttachControl> {
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let uid = current_owner_uid();
    handler
        .register_attach_with_access(
            requester_pid,
            session.clone(),
            None,
            AttachRegistration {
                control_tx,
                control_backlog: Arc::new(AtomicUsize::new(0)),
                closing: Arc::new(AtomicBool::new(false)),
                persistent_overlay_epoch: Arc::new(AtomicU64::new(0)),
                terminal_context: OuterTerminalContext::default(),
                flags: super::super::attach_support::ClientFlags::default(),
                render_stream: false,
                uid,
                user: rmux_os::identity::UserIdentity::Uid(uid),
                can_write: true,
                client_size: Some(size),
            },
        )
        .await
        .expect("declared attach registration succeeds");
    handler
        .handle_attached_resize(requester_pid, size)
        .await
        .expect("declared client size is accepted");
    control_rx
}

/// The exact registration `requester_pid` holds right now.
async fn attach_generation(
    handler: &RequestHandler,
    requester_pid: u32,
) -> super::super::attach_support::AttachGeneration {
    super::super::attach_support::AttachGeneration::new(
        requester_pid,
        attach_generation_id(handler, requester_pid).await,
    )
}

/// The generation half of that registration, which a same-pid re-attach bumps.
async fn attach_generation_id(handler: &RequestHandler, requester_pid: u32) -> u64 {
    handler
        .active_attach_identity_for_test(requester_pid)
        .await
        .attach_id()
}

async fn active_window_index(handler: &RequestHandler, session: &SessionName) -> u32 {
    handler
        .state
        .lock()
        .await
        .sessions
        .session(session)
        .expect("session exists")
        .active_window_index()
}

async fn window_content_size(
    handler: &RequestHandler,
    session: &SessionName,
    window_index: u32,
) -> TerminalSize {
    handler
        .state
        .lock()
        .await
        .sessions
        .session(session)
        .expect("session exists")
        .window_at(window_index)
        .expect("window exists")
        .size()
}
