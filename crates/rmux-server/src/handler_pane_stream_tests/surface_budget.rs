use rmux_core::{input::InputParser, PaneId, Screen};
use rmux_proto::{
    KillSessionRequest, PaneOutputSubscriptionId, PaneSnapshotCell, PaneStreamEndReason,
    PaneStreamEvent, PaneStreamMode, PaneTarget, PaneTargetRef, Request, ResizeWindowRequest,
    Response, RmuxError, SubscribePaneStreamRequest, TerminalSize, UnsubscribePaneStreamRequest,
    WindowTarget, DEFAULT_MAX_DETACHED_FRAME_LENGTH,
};

use crate::pane_recovery::{PaneProjectionSeed, MAX_RECOVERY_STRING_BYTES};
use crate::pane_transcript::SharedPaneTranscript;

use super::{cursor, subscribe, test_pane, CONNECTION_ID};

const SECOND_CONNECTION_ID: u64 = CONNECTION_ID + 1;
const SURFACE_RESPONSE_RESERVE: usize = 64 * 1024;
const SURFACE_EVENT_DISCRIMINANT: usize = std::mem::size_of::<u32>();
const SURFACE_POLL_FRAME_LIMIT: usize =
    DEFAULT_MAX_DETACHED_FRAME_LENGTH - SURFACE_RESPONSE_RESERVE - SURFACE_EVENT_DISCRIMINANT;
const EXPECTED_MAX_SURFACE_CELL_ENCODED_BYTES: u64 = 49;
const MAX_CELL_TEXT: &str =
    "a\u{0301}\u{0301}\u{0301}\u{0301}\u{0301}\u{0301}\u{0301}\u{0301}\u{0301}\u{0301}";

#[test]
fn maximum_surface_cell_encoding_matches_geometry_budget() {
    let cell = PaneSnapshotCell {
        text: MAX_CELL_TEXT.to_owned(),
        width: 1,
        padding: false,
        attributes: u16::MAX,
        fg: i32::MAX,
        bg: i32::MAX,
        us: i32::MAX,
        link: u32::MAX,
    };

    assert_eq!(MAX_CELL_TEXT.len(), 21);
    assert_eq!(
        bincode::serialized_size(&cell).expect("surface cell size"),
        EXPECTED_MAX_SURFACE_CELL_ENCODED_BYTES
    );
}

#[tokio::test]
async fn surface_subscription_rejects_geometry_that_cannot_fit_max_encoded_cells() {
    let handler = super::RequestHandler::new();
    let (target, _, transcript) = test_pane(&handler).await;
    install_blank_screen(
        &transcript,
        TerminalSize {
            cols: 1024,
            rows: 256,
        },
    );

    let response = subscribe_response(&handler, CONNECTION_ID, &target).await;
    assert!(
        matches!(
            response,
            Response::Error(ref error) if matches!(error.error, RmuxError::Server(_))
        ),
        "unsafe Surface geometry must be rejected before creating a driver"
    );
    assert_surface_state(&handler, 0, 0);
}

#[tokio::test]
async fn new_surface_driver_boundary_is_stable_on_first_application_and_repetition() {
    for pass in 0..2 {
        for (target_size, accepted) in [
            (SURFACE_POLL_FRAME_LIMIT - 1, true),
            (SURFACE_POLL_FRAME_LIMIT, true),
            (SURFACE_POLL_FRAME_LIMIT + 1, false),
        ] {
            let handler = super::RequestHandler::new();
            let (target, _, transcript) = test_pane(&handler).await;
            let (frame, _) = install_frame_at_size(&handler, &transcript, target_size);
            assert_eq!(
                bincode::serialized_size(frame.as_ref()).expect("surface frame size"),
                target_size as u64,
                "pass {pass} must prepare the exact requested boundary"
            );

            let response = subscribe_response(&handler, CONNECTION_ID, &target).await;
            if accepted {
                let subscription_id = expect_surface_subscription(response);
                assert_surface_state(&handler, 1, 1);
                unsubscribe(&handler, CONNECTION_ID, subscription_id).await;
                assert_surface_state(&handler, 0, 0);
            } else {
                assert_surface_budget_error(&response);
                assert_surface_state(&handler, 0, 0);
            }
        }
    }
}

#[tokio::test]
async fn shared_surface_admission_rejects_current_p_plus_one_without_ending_active_stream() {
    let handler = super::RequestHandler::new();
    let (target, output, transcript) = test_pane(&handler).await;
    let (at_limit, title_length) =
        install_frame_at_size(&handler, &transcript, SURFACE_POLL_FRAME_LIMIT);
    assert_eq!(
        bincode::serialized_size(at_limit.as_ref()).expect("surface frame size"),
        SURFACE_POLL_FRAME_LIMIT as u64
    );
    let existing = subscribe(&handler, &target, PaneStreamMode::Surface).await;
    assert_surface_state(&handler, 1, 1);

    publish_title(&transcript, &output, title_length + 1, b'u');
    let oversized = materialize_frame(&handler, &transcript);
    assert_eq!(
        bincode::serialized_size(oversized.as_ref()).expect("surface frame size"),
        (SURFACE_POLL_FRAME_LIMIT + 1) as u64
    );

    let response = subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await;
    assert_surface_budget_error(&response);
    assert_surface_state(&handler, 1, 1);
    {
        let subscriptions = handler
            .subscriptions
            .lock()
            .expect("subscription registry mutex");
        assert_eq!(
            subscriptions
                .streams
                .get(&existing.subscription_id)
                .and_then(super::super::PaneStreamSubscription::end_reason),
            None,
            "failed peer admission must not mark the active stream as ending"
        );
    }

    publish_title(&transcript, &output, title_length, b'v');
    let events = cursor(&handler, existing.subscription_id).await;
    let frame = events
        .iter()
        .find_map(super::surface_frame)
        .expect("active subscriber receives the live return to P");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, rmux_proto::PaneStreamEvent::End(_))),
        "rejected peer subscription must not terminate the existing subscriber: {events:?}"
    );
    assert_eq!(
        bincode::serialized_size(frame).expect("returned surface frame size"),
        SURFACE_POLL_FRAME_LIMIT as u64
    );
    assert!(
        frame.snapshot.title.bytes().all(|byte| byte == b'v'),
        "the delivered frame must be the real P+1 to P title mutation"
    );
    assert_surface_state(&handler, 1, 1);
}

#[tokio::test]
async fn ready_surface_peer_boundaries_match_for_same_and_distinct_connections() {
    for (target_size, accepted) in [
        (SURFACE_POLL_FRAME_LIMIT - 1, true),
        (SURFACE_POLL_FRAME_LIMIT, true),
        (SURFACE_POLL_FRAME_LIMIT + 1, false),
    ] {
        let handler = super::RequestHandler::new();
        let (target, output, transcript) = test_pane(&handler).await;
        let initial_size = target_size.min(SURFACE_POLL_FRAME_LIMIT);
        let (_, title_length) = install_frame_at_size(&handler, &transcript, initial_size);
        let active = subscribe(&handler, &target, PaneStreamMode::Surface).await;
        if !accepted {
            publish_title(&transcript, &output, title_length + 1, b'y');
            assert_eq!(
                bincode::serialized_size(materialize_frame(&handler, &transcript).as_ref())
                    .expect("surface frame size"),
                target_size as u64
            );
        }

        for (connection_id, relation) in [
            (CONNECTION_ID, "same connection"),
            (SECOND_CONNECTION_ID, "distinct connection"),
        ] {
            let response = subscribe_response(&handler, connection_id, &target).await;
            if accepted {
                let peer = expect_surface_subscription(response);
                assert_surface_state(&handler, 2, 1);
                unsubscribe(&handler, connection_id, peer).await;
            } else {
                assert_surface_budget_error(&response);
            }
            assert_surface_state(&handler, 1, 1);
            let subscriptions = handler
                .subscriptions
                .lock()
                .expect("subscription registry mutex");
            assert_eq!(
                subscriptions
                    .streams
                    .get(&active.subscription_id)
                    .and_then(super::super::PaneStreamSubscription::end_reason),
                None,
                "{relation} P+1 admission must not end the active peer"
            );
        }
    }
}

#[tokio::test]
async fn distinct_peer_cannot_join_when_surface_crosses_p_after_ready_validation() {
    let handler = std::sync::Arc::new(super::RequestHandler::new());
    let (target, output, transcript) = test_pane(handler.as_ref()).await;
    let (_, title_length) =
        install_frame_at_size(handler.as_ref(), &transcript, SURFACE_POLL_FRAME_LIMIT);
    let existing = subscribe(handler.as_ref(), &target, PaneStreamMode::Surface).await;
    assert_surface_state(handler.as_ref(), 1, 1);

    let pause = handler.install_surface_admission_pause();
    let peer_handler = std::sync::Arc::clone(&handler);
    let peer_target = target.clone();
    let peer = tokio::spawn(async move {
        subscribe_response(peer_handler.as_ref(), SECOND_CONNECTION_ID, &peer_target).await
    });

    pause.reached.notified().await;
    publish_title(&transcript, &output, title_length + 1, b'w');
    let oversized = materialize_frame(handler.as_ref(), &transcript);
    assert_eq!(
        bincode::serialized_size(oversized.as_ref()).expect("surface frame size"),
        (SURFACE_POLL_FRAME_LIMIT + 1) as u64,
        "the distinct peer must be released only after current state crosses P"
    );
    pause.release.notify_one();

    let response = peer.await.expect("distinct peer subscribe task");
    assert_surface_budget_error(&response);
    assert_surface_state(handler.as_ref(), 1, 1);

    publish_title(&transcript, &output, title_length, b'x');
    let events = cursor(handler.as_ref(), existing.subscription_id).await;
    let frame = events
        .iter()
        .find_map(super::surface_frame)
        .expect("active peer receives the return to P");
    assert_eq!(
        bincode::serialized_size(frame).expect("returned surface frame size"),
        SURFACE_POLL_FRAME_LIMIT as u64
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, PaneStreamEvent::End(_))),
        "a rejected concurrent peer must leave the P-sized peer live: {events:?}"
    );
}

#[tokio::test]
async fn waiting_surface_peer_rechecks_p_plus_one_after_driver_becomes_ready() {
    let handler = std::sync::Arc::new(super::RequestHandler::new());
    let (target, output, transcript) = test_pane(handler.as_ref()).await;
    let (_, title_length) =
        install_frame_at_size(handler.as_ref(), &transcript, SURFACE_POLL_FRAME_LIMIT);
    let source = {
        let state = handler.state.lock().await;
        super::super::stream_source_for_target(&state, target.clone()).expect("stream source")
    };
    let initialization_token = {
        let mut subscriptions = handler.subscriptions.lock().expect("subscription registry");
        let super::super::SurfaceDriverRoute::Initialize { token } =
            subscriptions.surface_driver_route(&source.key)
        else {
            panic!("test must own the first Surface initialization");
        };
        token
    };

    let peer_handler = std::sync::Arc::clone(&handler);
    let peer_target = target.clone();
    let peer = tokio::spawn(async move {
        subscribe_response(peer_handler.as_ref(), SECOND_CONNECTION_ID, &peer_target).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while handler
            .subscriptions
            .lock()
            .expect("subscription registry")
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("waiting Surface peer must reserve before the driver becomes Ready");
    {
        let subscriptions = handler.subscriptions.lock().expect("subscription registry");
        assert_eq!(subscriptions.registry.len(), 1);
        assert!(subscriptions.surface_drivers.is_empty());
        assert!(matches!(
            subscriptions.streams.values().next(),
            Some(super::super::PaneStreamSubscription::Reserved {
                mode: PaneStreamMode::Surface,
                ..
            })
        ));
    }

    let active_id = {
        let mut subscriptions = handler.subscriptions.lock().expect("subscription registry");
        let id = subscriptions
            .registry
            .subscribe(CONNECTION_ID, source.key.clone(), std::time::Instant::now())
            .expect("active Surface reservation")
            .id();
        subscriptions.streams.insert(
            id,
            super::super::PaneStreamSubscription::reserved(PaneStreamMode::Surface),
        );
        id
    };
    let (source, captured) = handler
        .capture_current_surface_stream_source(source)
        .await
        .expect("capture initial Surface source");
    let seed = captured
        .seed
        .as_ref()
        .expect("forced Surface capture must include a projection");
    let driver = super::super::SurfaceDriver::new(
        initialization_token,
        captured.receiver,
        super::super::materialize_surface_frame(
            handler.as_ref(),
            source.key.pane_id(),
            1,
            1,
            1,
            captured.boundary.next_output_sequence,
            seed,
        )
        .expect("materialize active Surface frame"),
        captured.fingerprint,
    );
    let response =
        handler.finish_new_surface_subscription(CONNECTION_ID, active_id, source, driver);
    assert_eq!(expect_surface_subscription(response), active_id);
    assert_surface_state(handler.as_ref(), 2, 1);

    publish_title(&transcript, &output, title_length + 1, b'q');
    handler
        .subscriptions
        .lock()
        .expect("subscription registry")
        .finish_surface_initialization(initialization_token);

    let response = peer.await.expect("waiting Surface peer task");
    assert_surface_budget_error(&response);
    assert_surface_state(handler.as_ref(), 1, 1);

    publish_title(&transcript, &output, title_length, b'r');
    let events = cursor(handler.as_ref(), active_id).await;
    assert!(events.iter().any(|event| {
        super::surface_frame(event).is_some_and(|frame| {
            bincode::serialized_size(frame).expect("returned Surface frame size")
                == SURFACE_POLL_FRAME_LIMIT as u64
        })
    }));
    unsubscribe(handler.as_ref(), CONNECTION_ID, active_id).await;
    assert_surface_state(handler.as_ref(), 0, 0);
}

#[tokio::test]
async fn distinct_peer_rejects_real_window_resize_beyond_surface_geometry_budget() {
    let handler = super::RequestHandler::new();
    let (target, _, transcript) = test_pane(&handler).await;
    resize_window(&handler, &target, 512, 331).await;
    assert_eq!(
        transcript
            .lock()
            .expect("pane transcript mutex")
            .screen()
            .size(),
        TerminalSize {
            cols: 512,
            rows: 331,
        }
    );
    let active = subscribe(&handler, &target, PaneStreamMode::Surface).await;

    resize_window(&handler, &target, 512, 332).await;
    assert_eq!(
        transcript
            .lock()
            .expect("pane transcript mutex")
            .screen()
            .size(),
        TerminalSize {
            cols: 512,
            rows: 332,
        }
    );
    let response = subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await;
    assert!(
        matches!(
            response,
            Response::Error(ref error) if matches!(error.error, RmuxError::Server(_))
        ),
        "the window resize route must reject a distinct P+1 peer: {response:?}"
    );
    assert_surface_state(&handler, 1, 1);

    resize_window(&handler, &target, 512, 331).await;
    let events = cursor(&handler, active.subscription_id).await;
    let frame = events
        .iter()
        .find_map(super::surface_frame)
        .expect("active peer receives the real window resize back to P");
    assert_eq!((frame.snapshot.cols, frame.snapshot.rows), (512, 331));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, PaneStreamEvent::End(_))),
        "rejected window-resize peer must leave the active Surface stream live: {events:?}"
    );
}

#[tokio::test]
async fn shared_surface_one_two_three_peers_close_and_resubscribe_in_isolation() {
    let handler = super::RequestHandler::new();
    let (target, _, _) = test_pane(&handler).await;
    let first =
        expect_surface_subscription(subscribe_response(&handler, CONNECTION_ID, &target).await);
    assert_surface_state(&handler, 1, 1);

    for pass in 0..2 {
        let second = expect_surface_subscription(
            subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await,
        );
        assert_surface_state(&handler, 2, 1);
        let third = expect_surface_subscription(
            subscribe_response(&handler, SECOND_CONNECTION_ID + 1, &target).await,
        );
        assert_surface_state(&handler, 3, 1);

        unsubscribe(&handler, SECOND_CONNECTION_ID, second).await;
        assert_surface_state(&handler, 2, 1);
        assert!(
            cursor_for_connection(&handler, CONNECTION_ID, first)
                .await
                .is_empty(),
            "pass {pass}: closing one peer must leave the first peer live"
        );

        let resubscribed = expect_surface_subscription(
            subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await,
        );
        assert_surface_state(&handler, 3, 1);
        unsubscribe(&handler, SECOND_CONNECTION_ID, resubscribed).await;
        unsubscribe(&handler, SECOND_CONNECTION_ID + 1, third).await;
        assert_surface_state(&handler, 1, 1);
    }

    unsubscribe(&handler, CONNECTION_ID, first).await;
    assert_surface_state(&handler, 0, 0);
}

#[tokio::test]
async fn short_raw_sibling_destruction_releases_both_streams_and_surface_driver() {
    for pass in 0..2 {
        let handler = super::RequestHandler::new();
        let (target, _, _) = test_pane(&handler).await;
        let surface =
            expect_surface_subscription(subscribe_response(&handler, CONNECTION_ID, &target).await);
        let raw = expect_subscription(
            subscribe_mode_response(&handler, SECOND_CONNECTION_ID, &target, PaneStreamMode::Raw)
                .await,
            PaneStreamMode::Raw,
        );

        let response = handler
            .handle(Request::KillSession(KillSessionRequest {
                target: target.session_name().clone(),
                kill_all_except_target: false,
                clear_alerts: false,
                kill_group: false,
            }))
            .await;
        assert!(
            matches!(response, Response::KillSession(_)),
            "pass {pass}: {response:?}"
        );

        assert_eq!(
            end_reason(
                cursor_until_end(&handler, CONNECTION_ID, surface).await,
                "Surface",
            ),
            PaneStreamEndReason::PaneRemoved
        );
        assert_eq!(
            end_reason(
                cursor_until_end(&handler, SECOND_CONNECTION_ID, raw).await,
                "Raw",
            ),
            PaneStreamEndReason::PaneRemoved
        );
        assert_surface_state(&handler, 0, 0);
    }
}

fn install_blank_screen(transcript: &SharedPaneTranscript, size: TerminalSize) {
    let history_limit = transcript
        .lock()
        .expect("pane transcript mutex")
        .history_limit();
    transcript
        .lock()
        .expect("pane transcript mutex")
        .set_screen_for_test(Screen::new(size, history_limit));
}

fn install_frame_at_size(
    handler: &super::RequestHandler,
    transcript: &SharedPaneTranscript,
    target_size: usize,
) -> (std::sync::Arc<rmux_proto::PaneSurfaceFrame>, usize) {
    assert_eq!(MAX_CELL_TEXT.len(), 21);
    let size = TerminalSize {
        cols: 512,
        rows: 330,
    };
    let cells = usize::from(size.cols) * usize::from(size.rows);
    let history_limit = transcript
        .lock()
        .expect("pane transcript mutex")
        .history_limit();
    let mut screen = Screen::new(size, history_limit);
    let mut parser = InputParser::new();
    let content = MAX_CELL_TEXT.repeat(cells);
    parser.parse(content.as_bytes(), &mut screen);
    transcript
        .lock()
        .expect("pane transcript mutex")
        .set_screen_for_test(screen);

    let base = materialize_frame(handler, transcript);
    let base_size =
        usize::try_from(bincode::serialized_size(base.as_ref()).expect("base frame size"))
            .expect("base frame size fits usize");
    let title_len = target_size
        .checked_sub(base_size)
        .expect("chosen cell grid must leave room for title calibration");
    assert!(
        title_len <= MAX_RECOVERY_STRING_BYTES,
        "title calibration {title_len} exceeds the production metadata bound"
    );
    transcript
        .lock()
        .expect("pane transcript mutex")
        .set_title("t".repeat(title_len));

    (materialize_frame(handler, transcript), title_len)
}

fn publish_title(
    transcript: &SharedPaneTranscript,
    output: &crate::pane_io::PaneOutputSender,
    title_length: usize,
    byte: u8,
) {
    let mut payload = Vec::with_capacity(title_length + 5);
    payload.extend_from_slice(b"\x1b]0;");
    payload.resize(payload.len() + title_length, byte);
    payload.push(b'\x07');
    crate::pane_io::publish_pane_bytes_for_test(transcript, output, payload);
}

async fn resize_window(handler: &super::RequestHandler, target: &PaneTarget, cols: u16, rows: u16) {
    let response = handler
        .handle(Request::ResizeWindow(ResizeWindowRequest {
            target: WindowTarget::with_window(target.session_name().clone(), target.window_index()),
            width: Some(cols),
            height: Some(rows),
            adjustment: None,
        }))
        .await;
    assert!(
        matches!(response, Response::ResizeWindow(_)),
        "window resize failed: {response:?}"
    );
}

fn materialize_frame(
    handler: &super::RequestHandler,
    transcript: &SharedPaneTranscript,
) -> std::sync::Arc<rmux_proto::PaneSurfaceFrame> {
    let seed = {
        let transcript = transcript.lock().expect("pane transcript mutex");
        PaneProjectionSeed::capture(&transcript).expect("test Surface projection")
    };
    super::super::materialize_surface_frame(handler, PaneId::new(1), 1, 1, 1, 0, &seed)
        .expect("materialize test Surface frame")
}

async fn subscribe_response(
    handler: &super::RequestHandler,
    connection_id: u64,
    target: &PaneTarget,
) -> Response {
    subscribe_mode_response(handler, connection_id, target, PaneStreamMode::Surface).await
}

async fn subscribe_mode_response(
    handler: &super::RequestHandler,
    connection_id: u64,
    target: &PaneTarget,
    mode: PaneStreamMode,
) -> Response {
    handler
        .handle_subscribe_pane_stream(
            connection_id,
            SubscribePaneStreamRequest {
                target: PaneTargetRef::slot(target.clone()),
                mode,
                include_snapshot: false,
            },
        )
        .await
}

fn expect_surface_subscription(response: Response) -> PaneOutputSubscriptionId {
    expect_subscription(response, PaneStreamMode::Surface)
}

fn expect_subscription(
    response: Response,
    expected_mode: PaneStreamMode,
) -> PaneOutputSubscriptionId {
    let Response::SubscribePaneStream(response) = response else {
        panic!("{expected_mode:?} subscription failed: {response:?}");
    };
    match (&response.event, expected_mode) {
        (PaneStreamEvent::SurfaceReset(_), PaneStreamMode::Surface)
        | (PaneStreamEvent::RawRebase(_), PaneStreamMode::Raw) => response.subscription_id,
        _ => panic!(
            "{expected_mode:?} subscription returned the wrong initial event: {:?}",
            response.event
        ),
    }
}

async fn unsubscribe(
    handler: &super::RequestHandler,
    connection_id: u64,
    subscription_id: PaneOutputSubscriptionId,
) {
    let response = handler
        .handle_unsubscribe_pane_stream(
            connection_id,
            UnsubscribePaneStreamRequest { subscription_id },
        )
        .await;
    assert!(
        matches!(
            response,
            Response::UnsubscribePaneStream(ref response) if response.removed
        ),
        "unsubscribe failed: {response:?}"
    );
}

async fn cursor_for_connection(
    handler: &super::RequestHandler,
    connection_id: u64,
    subscription_id: PaneOutputSubscriptionId,
) -> Vec<PaneStreamEvent> {
    let response = handler
        .handle_pane_stream_cursor(
            connection_id,
            rmux_proto::PaneStreamCursorRequest {
                subscription_id,
                max_events: Some(32),
            },
        )
        .await;
    let Response::PaneStreamCursor(response) = response else {
        panic!("cursor failed: {response:?}");
    };
    response.events
}

async fn cursor_until_end(
    handler: &super::RequestHandler,
    connection_id: u64,
    subscription_id: PaneOutputSubscriptionId,
) -> Vec<PaneStreamEvent> {
    let mut events = Vec::new();
    for _ in 0..8 {
        let next = cursor_for_connection(handler, connection_id, subscription_id).await;
        let ended = next
            .iter()
            .any(|event| matches!(event, PaneStreamEvent::End(_)));
        events.extend(next);
        if ended {
            return events;
        }
    }
    panic!("stream did not end after short destruction: {events:?}");
}

fn end_reason(events: Vec<PaneStreamEvent>, mode: &str) -> PaneStreamEndReason {
    events
        .into_iter()
        .find_map(|event| match event {
            PaneStreamEvent::End(reason) => Some(reason),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{mode} stream did not deliver a typed end"))
}

fn assert_surface_budget_error(response: &Response) {
    match response {
        Response::Error(error) => assert_eq!(
            error.error,
            RmuxError::FrameTooLarge {
                length: DEFAULT_MAX_DETACHED_FRAME_LENGTH + 1,
                maximum: DEFAULT_MAX_DETACHED_FRAME_LENGTH,
            }
        ),
        Response::SubscribePaneStream(_) => {
            panic!("Surface subscribe accepted a frame outside the cursor envelope")
        }
        _ => panic!("Surface subscribe returned an unexpected response kind"),
    }
}

fn assert_surface_state(
    handler: &super::RequestHandler,
    subscriptions_expected: usize,
    drivers_expected: usize,
) {
    let subscriptions = handler
        .subscriptions
        .lock()
        .expect("subscription registry mutex");
    assert_eq!(subscriptions.registry.len(), subscriptions_expected);
    assert_eq!(subscriptions.streams.len(), subscriptions_expected);
    assert_eq!(subscriptions.surface_drivers.len(), drivers_expected);
}
