use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use rmux_core::LifecycleEvent;
use rmux_proto::{
    ControlMode, PaneKillRequest, PaneResizeRequest, PaneTargetRef, Request, ResizePaneAdjustment,
    Response, SessionName,
};
use tokio::sync::mpsc;

use super::RequestHandler;
use crate::control::{ControlModeUpgrade, ControlServerEvent, CONTROL_SERVER_EVENT_CAPACITY};

const CONTROL_NOTIFICATION_SETTLE: Duration = Duration::from_millis(100);
const CONTROL_NOTIFICATION_POLL: Duration = Duration::from_millis(10);

struct LayoutCommandCase {
    label: &'static str,
    setup: &'static [&'static str],
    warmup: Option<&'static str>,
    command: &'static str,
    expected: usize,
}

#[tokio::test]
async fn layout_command_sites_keep_one_canonical_notification_per_mutation_product_divergence() {
    // tmux 3.7b oracle, measured 2026-07-26 in control mode:
    // select-layout (same and different), next-layout and previous-layout
    // publish twice; move-pane publishes three times; resize-pane, swap-pane
    // and kill-pane publish once. RMUX intentionally has one canonical
    // WindowLayoutChanged producer for each committed mutation.
    let cases = [
        LayoutCommandCase {
            label: "select-layout-identical",
            setup: &["split-window -d -h -t {session}"],
            warmup: Some("select-layout -t {session} even-horizontal"),
            command: "select-layout -t {session} even-horizontal",
            expected: 1,
        },
        LayoutCommandCase {
            label: "select-layout-different",
            setup: &["split-window -d -h -t {session}"],
            warmup: None,
            command: "select-layout -t {session} even-vertical",
            expected: 1,
        },
        LayoutCommandCase {
            label: "next-layout",
            setup: &["split-window -d -h -t {session}"],
            warmup: None,
            command: "next-layout -t {session}",
            expected: 1,
        },
        LayoutCommandCase {
            label: "previous-layout",
            setup: &["split-window -d -h -t {session}"],
            warmup: None,
            command: "previous-layout -t {session}",
            expected: 1,
        },
        LayoutCommandCase {
            label: "resize-pane",
            setup: &["split-window -d -h -t {session}"],
            warmup: None,
            command: "resize-pane -t {session}:.0 -R 5",
            expected: 1,
        },
        LayoutCommandCase {
            label: "swap-pane",
            setup: &["split-window -d -h -t {session}"],
            warmup: None,
            command: "swap-pane -s {session}:.0 -t {session}:.1",
            expected: 1,
        },
        LayoutCommandCase {
            label: "move-pane",
            setup: &[
                "split-window -d -h -t {session}",
                "split-window -d -v -t {session}:.0",
            ],
            warmup: None,
            command: "move-pane -s {session}:.2 -t {session}:.1 -h",
            expected: 1,
        },
        LayoutCommandCase {
            label: "kill-pane",
            setup: &["split-window -d -h -t {session}"],
            warmup: None,
            command: "kill-pane -t {session}:.1",
            expected: 1,
        },
    ];

    let mut actual = Vec::with_capacity(cases.len());
    let mut expected = Vec::with_capacity(cases.len());
    for case in cases {
        let session =
            SessionName::new(format!("layout-notify-{}", case.label)).expect("test session name");
        let handler = RequestHandler::new();
        create_session(&handler, &session).await;
        for command in case.setup {
            run_detached_command(&handler, &render_command(command, &session)).await;
        }
        if let Some(command) = case.warmup {
            run_detached_command(&handler, &render_command(command, &session)).await;
        }
        let (control_pid, mut notifications) = register_control_client(&handler, &session).await;
        let _ = settle_control_notifications(&mut notifications).await;

        run_control_command(
            &handler,
            control_pid,
            &render_command(case.command, &session),
        )
        .await;
        let count = layout_change_count(&mut notifications).await;
        actual.push((case.label, count));
        expected.push((case.label, case.expected));
    }

    assert_eq!(actual, expected);
}

#[tokio::test]
async fn custom_layout_resize_has_one_layout_notification_not_two() {
    let handler = RequestHandler::new();
    let session = SessionName::new("layout-notify-custom-resize").expect("test session name");
    create_session(&handler, &session).await;
    run_detached_command(&handler, &format!("split-window -d -h -t {session}")).await;
    run_detached_command(&handler, &format!("resize-window -t {session} -x 60 -y 20")).await;
    let small_layout = {
        let state = handler.state.lock().await;
        state
            .sessions
            .session(&session)
            .expect("test session")
            .window()
            .layout_dump()
    };
    run_detached_command(&handler, &format!("resize-window -t {session} -x 80 -y 24")).await;

    let (control_pid, mut notifications) = register_control_client(&handler, &session).await;
    let _ = settle_control_notifications(&mut notifications).await;
    let mut lifecycle_events = handler.subscribe_lifecycle_events();
    run_control_command(
        &handler,
        control_pid,
        &format!("select-layout -t {session} \"{small_layout}\""),
    )
    .await;

    assert_eq!(layout_change_count(&mut notifications).await, 1);
    let events = std::iter::from_fn(|| lifecycle_events.try_recv().ok())
        .map(|event| event.event)
        .collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LifecycleEvent::WindowLayoutChanged { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LifecycleEvent::WindowResized { .. }))
            .count(),
        1,
        "deduplicating the layout producer must keep the resize hook"
    );
}

#[tokio::test]
async fn layout_no_ops_are_silent_on_cli_and_stable_id_paths() {
    // tmux 3.7b emits no %layout-change for a directional resize in a
    // one-pane window, resize-pane -T, or a pane swapped with itself.
    let handler = RequestHandler::new();
    let session = SessionName::new("layout-notify-no-op").expect("test session name");
    create_session(&handler, &session).await;
    let (control_pid, mut notifications) = register_control_client(&handler, &session).await;
    let _ = settle_control_notifications(&mut notifications).await;

    for command in [
        format!("resize-pane -t {session}:.0 -R 5"),
        format!("resize-pane -t {session}:.0 -T"),
    ] {
        run_control_command(&handler, control_pid, &command).await;
        assert_eq!(
            layout_change_count(&mut notifications).await,
            0,
            "{command}"
        );
    }

    run_detached_command(&handler, &format!("split-window -d -h -t {session}")).await;
    let _ = settle_control_notifications(&mut notifications).await;
    let swap_self = format!("swap-pane -s {session}:.0 -t {session}:.0");
    run_control_command(&handler, control_pid, &swap_self).await;
    assert_eq!(
        layout_change_count(&mut notifications).await,
        0,
        "{swap_self}"
    );

    let pane_id = {
        let state = handler.state.lock().await;
        state
            .sessions
            .session(&session)
            .expect("test session")
            .window()
            .pane(0)
            .expect("test pane")
            .id()
    };
    let response = handler
        .handle(Request::PaneResize(PaneResizeRequest {
            target: PaneTargetRef::by_id(session.clone(), pane_id),
            adjustment: ResizePaneAdjustment::Left { cells: 250 },
        }))
        .await;
    assert!(matches!(response, Response::ResizePane(_)), "{response:?}");
    let _ = settle_control_notifications(&mut notifications).await;

    let response = handler
        .handle(Request::PaneResize(PaneResizeRequest {
            target: PaneTargetRef::by_id(session.clone(), pane_id),
            adjustment: ResizePaneAdjustment::Left { cells: 1 },
        }))
        .await;
    assert!(matches!(response, Response::ResizePane(_)), "{response:?}");
    assert_eq!(layout_change_count(&mut notifications).await, 0);
}

#[tokio::test]
async fn stable_id_resize_and_kill_keep_required_layout_notifications() {
    let handler = RequestHandler::new();
    let session = SessionName::new("layout-notify-stable-id").expect("test session name");
    create_session(&handler, &session).await;
    run_detached_command(&handler, &format!("split-window -d -h -t {session}")).await;
    let (first_pane_id, second_pane_id) = {
        let state = handler.state.lock().await;
        let window = state
            .sessions
            .session(&session)
            .expect("test session")
            .window();
        (
            window.pane(0).expect("first test pane").id(),
            window.pane(1).expect("second test pane").id(),
        )
    };
    let (_control_pid, mut notifications) = register_control_client(&handler, &session).await;
    let _ = settle_control_notifications(&mut notifications).await;

    let response = handler
        .handle(Request::PaneResize(PaneResizeRequest {
            target: PaneTargetRef::by_id(session.clone(), first_pane_id),
            adjustment: ResizePaneAdjustment::Right { cells: 2 },
        }))
        .await;
    assert!(matches!(response, Response::ResizePane(_)), "{response:?}");
    assert_eq!(layout_change_count(&mut notifications).await, 1);

    let response = handler
        .handle(Request::PaneKill(PaneKillRequest {
            target: PaneTargetRef::by_id(session, second_pane_id),
            kill_all_except: false,
        }))
        .await;
    assert!(matches!(response, Response::KillPane(_)), "{response:?}");
    assert_eq!(layout_change_count(&mut notifications).await, 1);
}

async fn create_session(handler: &RequestHandler, session: &SessionName) {
    run_detached_command(handler, &format!("new-session -d -s {session} -x 80 -y 24")).await;
}

async fn run_detached_command(handler: &RequestHandler, command: &str) {
    let parsed = handler
        .parse_control_commands(command)
        .await
        .unwrap_or_else(|error| panic!("failed to parse {command:?}: {error}"));
    handler
        .execute_parsed_commands_for_test(std::process::id(), parsed)
        .await
        .unwrap_or_else(|error| panic!("failed to execute {command:?}: {error}"));
}

async fn run_control_command(handler: &RequestHandler, control_pid: u32, command: &str) {
    let parsed = handler
        .parse_control_commands(command)
        .await
        .unwrap_or_else(|error| panic!("failed to parse {command:?}: {error}"));
    let result = handler.execute_control_commands(control_pid, parsed).await;
    assert!(
        result.error.is_none(),
        "failed to execute {command:?}: {:?}",
        result.error
    );
}

async fn register_control_client(
    handler: &RequestHandler,
    session: &SessionName,
) -> (u32, mpsc::Receiver<ControlServerEvent>) {
    let control_pid = std::process::id();
    let (event_tx, event_rx) = mpsc::channel(CONTROL_SERVER_EVENT_CAPACITY);
    handler
        .register_control_with_closing(
            control_pid,
            ControlModeUpgrade {
                initial_command_count: 0,
                mode: ControlMode::Plain,
                terminal_context: crate::outer_terminal::OuterTerminalContext::default(),
            },
            event_tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await;
    handler
        .set_control_session(control_pid, Some(session.clone()))
        .await
        .expect("set test control session");
    (control_pid, event_rx)
}

async fn layout_change_count(notifications: &mut mpsc::Receiver<ControlServerEvent>) -> usize {
    settle_control_notifications(notifications)
        .await
        .iter()
        .filter(|line| line.starts_with("%layout-change "))
        .count()
}

async fn settle_control_notifications(
    notifications: &mut mpsc::Receiver<ControlServerEvent>,
) -> Vec<String> {
    let mut deadline = tokio::time::Instant::now() + CONTROL_NOTIFICATION_SETTLE;
    let mut lines = Vec::new();
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(CONTROL_NOTIFICATION_POLL, notifications.recv()).await {
            Ok(Some(ControlServerEvent::Notification(line))) => {
                deadline = tokio::time::Instant::now() + CONTROL_NOTIFICATION_SETTLE;
                lines.push(line);
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break,
        }
    }
    lines
}

fn render_command(command: &str, session: &SessionName) -> String {
    command.replace("{session}", session.as_str())
}
