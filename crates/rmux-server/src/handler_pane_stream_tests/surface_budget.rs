use rmux_core::{input::InputParser, PaneId, Screen};
use rmux_proto::{
    PaneSnapshotCell, PaneStreamMode, PaneTarget, PaneTargetRef, Response, RmuxError,
    SubscribePaneStreamRequest, TerminalSize, DEFAULT_MAX_DETACHED_FRAME_LENGTH,
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
async fn new_surface_driver_rejects_frame_reserved_for_cursor_envelope() {
    let handler = super::RequestHandler::new();
    let (target, _, transcript) = test_pane(&handler).await;
    let frame = install_frame_in_subscribe_poll_gap(&handler, &transcript);
    assert_eq!(
        bincode::serialized_size(frame.as_ref()).expect("surface frame size"),
        (SURFACE_POLL_FRAME_LIMIT + 1) as u64
    );

    let response = subscribe_response(&handler, CONNECTION_ID, &target).await;
    assert_surface_budget_error(&response);
    assert_surface_state(&handler, 0, 0);
}

#[tokio::test]
async fn existing_surface_driver_rejection_preserves_its_subscriber() {
    let handler = super::RequestHandler::new();
    let (target, _, transcript) = test_pane(&handler).await;
    let existing = subscribe(&handler, &target, PaneStreamMode::Surface).await;
    let oversized = install_frame_in_subscribe_poll_gap(&handler, &transcript);
    {
        let mut subscriptions = handler
            .subscriptions
            .lock()
            .expect("subscription registry mutex");
        let driver = subscriptions
            .surface_drivers
            .values_mut()
            .next()
            .expect("existing Surface driver");
        driver.latest = oversized;
    }

    let response = subscribe_response(&handler, SECOND_CONNECTION_ID, &target).await;
    assert_surface_budget_error(&response);
    assert_surface_state(&handler, 1, 1);

    let events = cursor(&handler, existing.subscription_id).await;
    assert!(
        events.is_empty(),
        "rejected peer subscription must not terminate the existing subscriber: {events:?}"
    );
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

fn install_frame_in_subscribe_poll_gap(
    handler: &super::RequestHandler,
    transcript: &SharedPaneTranscript,
) -> std::sync::Arc<rmux_proto::PaneSurfaceFrame> {
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
    let target_size = SURFACE_POLL_FRAME_LIMIT + 1;
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

    materialize_frame(handler, transcript)
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
    handler
        .handle_subscribe_pane_stream(
            connection_id,
            SubscribePaneStreamRequest {
                target: PaneTargetRef::slot(target.clone()),
                mode: PaneStreamMode::Surface,
                include_snapshot: false,
            },
        )
        .await
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
