use super::*;
use crate::pane_io::PaneExitEvent;
use rmux_proto::{OptionScopeSelector, PaneKillRequest, SetHookRequest, SetOptionByNameRequest};

const RENUMBER_MARKER: &str = "@renumber-metadata";

struct RenumberMetadataFixture {
    session_name: SessionName,
    removed_pane: PaneTarget,
    removed_pane_id: rmux_core::PaneId,
    surviving_window_id: rmux_core::WindowId,
}

async fn set_renumber_metadata(
    handler: &RequestHandler,
    target: WindowTarget,
    marker: &str,
    hook_command: &str,
) {
    let option = handler
        .handle(Request::SetOptionByName(Box::new(SetOptionByNameRequest {
            scope: OptionScopeSelector::Window(target.clone()),
            name: RENUMBER_MARKER.to_owned(),
            value: Some(marker.to_owned()),
            mode: SetOptionMode::Replace,
            only_if_unset: false,
            unset: false,
            unset_pane_overrides: false,
            format: false,
            format_target: None,
        })))
        .await;
    assert!(matches!(option, Response::SetOptionByName(_)), "{option:?}");

    let hook = handler
        .handle(Request::SetHook(SetHookRequest {
            scope: ScopeSelector::Window(target),
            hook: HookName::WindowLayoutChanged,
            command: hook_command.to_owned(),
            lifecycle: HookLifecycle::Persistent,
        }))
        .await;
    assert!(matches!(hook, Response::SetHook(_)), "{hook:?}");
}

async fn renumber_metadata_fixture(
    handler: &RequestHandler,
    label: &str,
) -> RenumberMetadataFixture {
    let session_name = session_name(label);
    create_session(handler, session_name.as_str()).await;
    insert_window(handler, &session_name, 1).await;
    insert_window(handler, &session_name, 2).await;

    let set_renumber = handler
        .handle(Request::SetOption(SetOptionRequest {
            scope: ScopeSelector::Session(session_name.clone()),
            option: OptionName::RenumberWindows,
            value: "on".to_owned(),
            mode: SetOptionMode::Replace,
        }))
        .await;
    assert!(matches!(set_renumber, Response::SetOption(_)));

    set_renumber_metadata(
        handler,
        WindowTarget::with_window(session_name.clone(), 1),
        "discarded",
        "display-message discarded-hook",
    )
    .await;
    set_renumber_metadata(
        handler,
        WindowTarget::with_window(session_name.clone(), 2),
        "survivor",
        "display-message survivor-hook",
    )
    .await;
    let renamed = handler
        .handle(Request::RenameWindow(RenameWindowRequest {
            target: WindowTarget::with_window(session_name.clone(), 2),
            name: "surviving-name".to_owned(),
        }))
        .await;
    assert!(matches!(renamed, Response::RenameWindow(_)), "{renamed:?}");

    let state = handler.state.lock().await;
    let session = state
        .sessions
        .session(&session_name)
        .expect("session exists");
    let removed_pane_id = session
        .window_at(1)
        .and_then(|window| window.pane(0))
        .map(rmux_core::Pane::id)
        .expect("removed pane exists");
    let surviving_window_id = session
        .window_at(2)
        .map(rmux_core::Window::id)
        .expect("surviving window exists");
    drop(state);

    RenumberMetadataFixture {
        removed_pane: PaneTarget::with_window(session_name.clone(), 1, 0),
        session_name,
        removed_pane_id,
        surviving_window_id,
    }
}

async fn assert_surviving_renumber_metadata(
    handler: &RequestHandler,
    fixture: &RenumberMetadataFixture,
) {
    let state = handler.state.lock().await;
    let session = state
        .sessions
        .session(&fixture.session_name)
        .expect("session survives");
    assert_eq!(
        session.windows().keys().copied().collect::<Vec<_>>(),
        vec![0, 1]
    );
    let survivor = session.window_at(1).expect("surviving window is reindexed");
    assert_eq!(survivor.id(), fixture.surviving_window_id);
    assert_eq!(survivor.name(), Some("surviving-name"));
    assert!(!survivor.automatic_rename());

    let target = WindowTarget::with_window(fixture.session_name.clone(), 1);
    assert_eq!(
        state
            .options
            .explicit_value_by_name(
                &OptionScopeSelector::Window(target.clone()),
                RENUMBER_MARKER,
            )
            .expect("valid user option")
            .1
            .as_deref(),
        Some("survivor")
    );
    assert_eq!(
        state
            .options
            .resolve_for_window(&fixture.session_name, 1, OptionName::AutomaticRename),
        Some("off")
    );
    assert_eq!(
        state
            .hooks
            .window_bindings_view(&target, Some(HookName::WindowLayoutChanged))
            .iter()
            .map(|binding| binding.command())
            .collect::<Vec<_>>(),
        vec!["display-message survivor-hook"]
    );
}

#[tokio::test]
async fn kill_window_renumbers_when_session_option_is_enabled() {
    let handler = RequestHandler::new();
    let alpha = session_name("alpha");
    create_session(&handler, "alpha").await;
    insert_window(&handler, &alpha, 1).await;
    insert_window(&handler, &alpha, 2).await;

    let set_renumber = handler
        .handle(Request::SetOption(SetOptionRequest {
            scope: ScopeSelector::Session(alpha.clone()),
            option: OptionName::RenumberWindows,
            value: "on".to_owned(),
            mode: SetOptionMode::Replace,
        }))
        .await;
    assert!(matches!(set_renumber, Response::SetOption(_)));

    let response = handler
        .handle(Request::KillWindow(KillWindowRequest {
            target: WindowTarget::with_window(alpha.clone(), 1),
            kill_all_others: false,
        }))
        .await;

    assert_eq!(
        response,
        Response::KillWindow(rmux_proto::KillWindowResponse {
            target: WindowTarget::with_window(alpha.clone(), 0),
        })
    );

    let state = handler.state.lock().await;
    let session = state
        .sessions
        .session(&alpha)
        .expect("session should exist");
    assert_eq!(
        session.windows().keys().copied().collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(session.active_window_index(), 0);
}

#[tokio::test]
async fn kill_last_pane_renumbers_when_session_option_is_enabled() {
    // tmux 3.7b, measured on 2026-07-26: killing the only pane in window 1
    // closes that window and renumbers the surviving 0/2 slots to 0/1.
    let handler = RequestHandler::new();
    let alpha = session_name("kill-pane-renumber");
    create_session(&handler, alpha.as_str()).await;
    insert_window(&handler, &alpha, 1).await;
    insert_window(&handler, &alpha, 2).await;

    let set_renumber = handler
        .handle(Request::SetOption(SetOptionRequest {
            scope: ScopeSelector::Session(alpha.clone()),
            option: OptionName::RenumberWindows,
            value: "on".to_owned(),
            mode: SetOptionMode::Replace,
        }))
        .await;
    assert!(matches!(set_renumber, Response::SetOption(_)));

    let response = handler
        .handle(Request::KillPane(KillPaneRequest {
            target: PaneTarget::with_window(alpha.clone(), 1, 0),
            kill_all_except: false,
        }))
        .await;
    assert!(matches!(response, Response::KillPane(_)), "{response:?}");

    let state = handler.state.lock().await;
    let session = state.sessions.session(&alpha).expect("session survives");
    assert_eq!(
        session.windows().keys().copied().collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[tokio::test]
async fn kill_last_pane_discards_removed_metadata_before_renumbering_survivor() {
    let handler = RequestHandler::new();
    let fixture = renumber_metadata_fixture(&handler, "kill-pane-renumber-metadata").await;

    let response = handler
        .handle(Request::KillPane(KillPaneRequest {
            target: fixture.removed_pane.clone(),
            kill_all_except: false,
        }))
        .await;
    assert!(matches!(response, Response::KillPane(_)), "{response:?}");

    assert_surviving_renumber_metadata(&handler, &fixture).await;
}

#[tokio::test]
async fn pane_kill_by_id_preserves_surviving_metadata_when_window_is_renumbered() {
    let handler = RequestHandler::new();
    let fixture = renumber_metadata_fixture(&handler, "pane-id-renumber-metadata").await;

    let response = handler
        .handle(Request::PaneKill(PaneKillRequest {
            target: PaneTargetRef::by_id(fixture.session_name.clone(), fixture.removed_pane_id),
            kill_all_except: false,
        }))
        .await;
    assert!(matches!(response, Response::KillPane(_)), "{response:?}");

    assert_surviving_renumber_metadata(&handler, &fixture).await;
}

#[tokio::test]
async fn natural_last_pane_exit_preserves_surviving_metadata_when_window_is_renumbered() {
    let handler = RequestHandler::new();
    let fixture = renumber_metadata_fixture(&handler, "natural-renumber-metadata").await;
    {
        let mut state = handler.state.lock().await;
        state
            .mark_pane_dead_without_exit_details(&fixture.removed_pane)
            .expect("mark pane exited");
    }

    handler
        .handle_pane_exit_event(PaneExitEvent::eof_published(
            fixture.session_name.clone(),
            fixture.removed_pane_id,
            None,
        ))
        .await;

    assert_surviving_renumber_metadata(&handler, &fixture).await;
}

#[tokio::test]
async fn kill_last_linked_pane_renumbers_each_surviving_session() {
    let handler = RequestHandler::new();
    let owner = session_name("kill-linked-pane-renumber-owner");
    let alias = session_name("kill-linked-pane-renumber-alias");
    create_session(&handler, owner.as_str()).await;
    insert_window(&handler, &owner, 1).await;
    insert_window(&handler, &owner, 2).await;
    create_session(&handler, alias.as_str()).await;

    let linked = handler
        .handle(Request::LinkWindow(LinkWindowRequest {
            source: WindowTarget::with_window(owner.clone(), 1),
            target: WindowTarget::with_window(alias.clone(), 9),
            after: false,
            before: false,
            kill_destination: false,
            detached: true,
        }))
        .await;
    assert!(matches!(linked, Response::LinkWindow(_)), "{linked:?}");
    for session_name in [&owner, &alias] {
        let response = handler
            .handle(Request::SetOption(SetOptionRequest {
                scope: ScopeSelector::Session(session_name.clone()),
                option: OptionName::RenumberWindows,
                value: "on".to_owned(),
                mode: SetOptionMode::Replace,
            }))
            .await;
        assert!(matches!(response, Response::SetOption(_)), "{response:?}");
    }

    let response = handler
        .handle(Request::KillPane(KillPaneRequest {
            target: PaneTarget::with_window(owner.clone(), 1, 0),
            kill_all_except: false,
        }))
        .await;
    assert!(matches!(response, Response::KillPane(_)), "{response:?}");

    let state = handler.state.lock().await;
    assert_eq!(
        state
            .sessions
            .session(&owner)
            .expect("owner survives")
            .windows()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        state
            .sessions
            .session(&alias)
            .expect("alias survives")
            .windows()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![0]
    );
}
