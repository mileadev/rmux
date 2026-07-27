use rmux_proto::{PaneStreamEvent, Response, RmuxError, TerminalSize};

use crate::pane_recovery::{
    MAX_RECOVERY_SURFACE_CELLS, MAX_SURFACE_FRAME_BYTES, MIN_SURFACE_CELL_ENCODED_BYTES,
};

use super::surface_test_support::*;
use super::{test_pane, CONNECTION_ID};

#[tokio::test]
async fn surface_prefilter_rejects_only_a_grid_impossible_with_empty_cells() {
    let cols = 1024_u16;
    let rows = u16::try_from(MAX_RECOVERY_SURFACE_CELLS / usize::from(cols) + 1)
        .expect("surface cap rows fit u16");
    let cells = usize::from(cols) * usize::from(rows);
    assert!(cells > MAX_RECOVERY_SURFACE_CELLS);
    assert!(cells * MIN_SURFACE_CELL_ENCODED_BYTES > MAX_SURFACE_FRAME_BYTES);

    let handler = super::RequestHandler::new();
    let (target, _, transcript) = test_pane(&handler).await;
    install_blank_screen(&transcript, TerminalSize { cols, rows });

    let response = subscribe_response(&handler, CONNECTION_ID, &target).await;
    assert!(
        matches!(
            response,
            Response::Error(ref error) if matches!(error.error, RmuxError::Server(_))
        ),
        "a grid that cannot fit even with empty cells must fail structurally: {response:?}"
    );
    assert_surface_state(&handler, 0, 0);
}

#[tokio::test]
async fn large_max_content_surface_reaches_exact_frame_validation() {
    let handler = super::RequestHandler::new();
    let (target, _, transcript) = test_pane(&handler).await;
    let size = TerminalSize {
        cols: 1024,
        rows: 256,
    };
    assert!(
        usize::from(size.cols) * usize::from(size.rows) <= MAX_RECOVERY_SURFACE_CELLS,
        "the structural prefilter must admit this geometry"
    );
    install_max_text_screen(&transcript, size);

    let response = subscribe_response(&handler, CONNECTION_ID, &target).await;
    assert_frame_too_large(&response);
    assert_surface_state(&handler, 0, 0);
}

#[tokio::test]
async fn ready_admission_uses_one_post_validation_capture_under_output() {
    let handler = std::sync::Arc::new(super::RequestHandler::new());
    let (target, output, transcript) = test_pane(handler.as_ref()).await;
    let active = expect_surface_subscription(
        subscribe_response(handler.as_ref(), CONNECTION_ID, &target).await,
    );

    let first_pause = handler.install_surface_admission_pause();
    let peer_handler = std::sync::Arc::clone(&handler);
    let peer_target = target.clone();
    let mut peer = tokio::spawn(async move {
        subscribe_response(peer_handler.as_ref(), SECOND_CONNECTION_ID, &peer_target).await
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        first_pause.reached.notified(),
    )
    .await
    .expect("first admission validation");
    let forbidden_retry_pause = handler.install_surface_admission_pause();
    publish_title(&transcript, &output, 32, b'a');
    first_pause.release.notify_one();

    let response = tokio::select! {
        result = &mut peer => result.expect("peer admission task"),
        _ = forbidden_retry_pause.reached.notified() => {
            forbidden_retry_pause.release.notify_one();
            let response = peer.await.expect("peer admission after forbidden retry");
            panic!("under-budget admission retried instead of validating its post-pause capture: {response:?}");
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
            panic!("under-budget admission did not converge");
        }
    };
    let peer = expect_surface_subscription(response);
    assert_surface_state(handler.as_ref(), 2, 1);
    assert_eq!(
        handler.surface_admission_materialization_count(),
        1,
        "the post-validation fingerprint must be materialized once without entering a retry"
    );

    unsubscribe(handler.as_ref(), SECOND_CONNECTION_ID, peer).await;
    unsubscribe(handler.as_ref(), CONNECTION_ID, active).await;
    assert_surface_state(handler.as_ref(), 0, 0);
}

#[tokio::test]
async fn ready_admission_stays_live_during_continuous_under_budget_output() {
    let handler = super::RequestHandler::new();
    let (target, output, transcript) = test_pane(&handler).await;
    let active =
        expect_surface_subscription(subscribe_response(&handler, CONNECTION_ID, &target).await);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let publisher_stop = std::sync::Arc::clone(&stop);
    let publisher = tokio::task::spawn_blocking(move || {
        let mut byte = b'a';
        while !publisher_stop.load(std::sync::atomic::Ordering::Relaxed) {
            publish_title(&transcript, &output, 48, byte);
            byte = if byte == b'z' { b'a' } else { byte + 1 };
            std::thread::yield_now();
        }
    });

    let admissions = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        for pass in 0..16_u64 {
            let connection_id = SECOND_CONNECTION_ID + 10 + pass;
            let response = subscribe_response(&handler, connection_id, &target).await;
            let peer = match response {
                Response::SubscribePaneStream(response)
                    if matches!(response.event, PaneStreamEvent::SurfaceReset(_)) =>
                {
                    response.subscription_id
                }
                response => {
                    return Err(format!(
                        "busy-pane admission {pass} did not return a Surface subscription: {response:?}"
                    ));
                }
            };
            unsubscribe(&handler, connection_id, peer).await;
        }
        Ok::<(), String>(())
    })
    .await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    publisher.await.expect("continuous title publisher");
    match admissions {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("{error}"),
        Err(_) => panic!("busy-pane admissions did not converge within ten seconds"),
    }

    unsubscribe(&handler, CONNECTION_ID, active).await;
    assert_surface_state(&handler, 0, 0);
}

#[tokio::test]
async fn ready_admission_reuses_cached_exact_validation() {
    let handler = super::RequestHandler::new();
    let (target, output, transcript) = test_pane(&handler).await;
    let active =
        expect_surface_subscription(subscribe_response(&handler, CONNECTION_ID, &target).await);
    assert_eq!(handler.surface_admission_materialization_count(), 0);

    let unchanged = expect_surface_subscription(
        subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await,
    );
    assert_eq!(
        handler.surface_admission_materialization_count(),
        0,
        "an unchanged fingerprint must reuse the driver's validated frame"
    );
    unsubscribe(&handler, SECOND_CONNECTION_ID, unchanged).await;

    publish_title(&transcript, &output, 24, b'c');
    let changed = expect_surface_subscription(
        subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await,
    );
    assert_eq!(
        handler.surface_admission_materialization_count(),
        1,
        "one changed fingerprint must be materialized exactly once"
    );
    unsubscribe(&handler, SECOND_CONNECTION_ID, changed).await;

    let cached = expect_surface_subscription(
        subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await,
    );
    assert_eq!(
        handler.surface_admission_materialization_count(),
        1,
        "the exact result for the changed fingerprint must be retained"
    );
    unsubscribe(&handler, SECOND_CONNECTION_ID, cached).await;
    unsubscribe(&handler, CONNECTION_ID, active).await;
}

#[tokio::test]
async fn oversized_surface_poll_is_recoverable_for_all_active_peers() {
    let handler = super::RequestHandler::new();
    let (target, output, transcript) = test_pane(&handler).await;
    let (_, title_length) = install_frame_at_size(&handler, &transcript, SURFACE_POLL_FRAME_LIMIT);
    let first =
        expect_surface_subscription(subscribe_response(&handler, CONNECTION_ID, &target).await);
    let second = expect_surface_subscription(
        subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await,
    );
    assert_surface_state(&handler, 2, 1);

    publish_title(&transcript, &output, title_length + 1, b'd');
    let first_error = cursor_response_for_connection(&handler, CONNECTION_ID, first).await;
    assert_surface_budget_error(&first_error);
    let second_error = cursor_response_for_connection(&handler, SECOND_CONNECTION_ID, second).await;
    assert_surface_budget_error(&second_error);
    assert_surface_state(&handler, 2, 1);
    {
        let subscriptions = handler
            .subscriptions
            .lock()
            .expect("subscription registry mutex");
        for subscription_id in [first, second] {
            assert_eq!(
                subscriptions
                    .streams
                    .get(&subscription_id)
                    .and_then(super::super::PaneStreamSubscription::end_reason),
                None,
                "oversized poll must not mark peer {subscription_id:?} as ending"
            );
        }
    }

    publish_title(&transcript, &output, title_length, b'e');
    for (connection_id, subscription_id) in [(CONNECTION_ID, first), (SECOND_CONNECTION_ID, second)]
    {
        let events = cursor_for_connection(&handler, connection_id, subscription_id).await;
        let frame = events
            .iter()
            .find_map(super::surface_frame)
            .expect("peer receives the recovered P frame");
        assert_eq!(
            bincode::serialized_size(frame).expect("recovered frame size"),
            SURFACE_POLL_FRAME_LIMIT as u64
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, PaneStreamEvent::End(_))),
            "peer must recover without End: {events:?}"
        );
    }

    unsubscribe(&handler, SECOND_CONNECTION_ID, second).await;
    unsubscribe(&handler, CONNECTION_ID, first).await;
    assert_surface_state(&handler, 0, 0);
}
