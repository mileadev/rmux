//! Shared fixtures for the issue #182 outer-terminal title/path regressions.
//!
//! Everything here observes the same seams the daemon uses: the control queue a
//! client really drains, the per-client memory the server really keeps, and the
//! production overlay-barrier rules that decide which queued frame is drawn.

use super::*;

use crate::pane_io::replay_client_visible_payloads;

pub(super) const TITLE_OPEN: &str = "\u{1b}]0;";
pub(super) const TITLE_CLOSE: char = '\u{7}';

/// A terminal family that advertises the `title` capability (TSL/FSL).
pub(super) fn title_capable_context() -> OuterTerminalContext {
    OuterTerminalContext::from_pairs(&[("TERM", "xterm-256color")])
}

pub(super) async fn new_detached_session(handler: &RequestHandler, name: &rmux_proto::SessionName) {
    let created = handler
        .handle(Request::NewSession(NewSessionRequest {
            session_name: name.clone(),
            detached: true,
            size: Some(TerminalSize { cols: 80, rows: 24 }),
            environment: None,
        }))
        .await;
    assert!(matches!(created, Response::NewSession(_)));
}

pub(super) async fn set_global(handler: &RequestHandler, option: OptionName, value: &str) {
    let set = handler
        .handle(Request::SetOption(SetOptionRequest {
            scope: ScopeSelector::Global,
            option,
            value: value.to_owned(),
            mode: SetOptionMode::Replace,
        }))
        .await;
    assert!(matches!(set, Response::SetOption(_)), "set {option:?}");
}

/// What the render told this client's outer terminal to show.
pub(super) fn client_title_of(target: &crate::pane_io::AttachTarget) -> Option<&str> {
    target
        .client_title
        .as_ref()
        .and_then(|rendered| rendered.state().title())
}

/// The OSC 0 payloads carried by one render frame, in order.
pub(super) fn titles_in(frame: &[u8]) -> Vec<String> {
    payloads_in(frame, TITLE_OPEN, TITLE_CLOSE)
}

fn payloads_in(frame: &[u8], open: &str, close: char) -> Vec<String> {
    let text = String::from_utf8_lossy(frame);
    text.split(open)
        .skip(1)
        .filter_map(|rest| rest.split_once(close))
        .map(|(payload, _)| payload.to_owned())
        .collect()
}

/// Every OSC 0 payload this client's outer terminal really receives, replayed
/// through the production overlay-barrier rules.
pub(super) fn delivered_titles(
    control_rx: &mut mpsc::UnboundedReceiver<AttachControl>,
) -> Vec<String> {
    replay_client_visible_payloads(control_rx)
        .iter()
        .flat_map(|payload| titles_in(payload))
        .collect()
}

/// What the server believes this client's outer terminal currently shows.
pub(super) async fn remembered_title(handler: &RequestHandler, attach_pid: u32) -> Option<String> {
    let active_attach = handler.active_attach.lock().await;
    active_attach
        .by_pid
        .get(&attach_pid)
        .expect("attach is active")
        .client_title
        .title()
        .map(str::to_owned)
}
