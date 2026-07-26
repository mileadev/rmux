use super::*;

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
