//! Targetless session ranking when public whole-second timestamps collide.
//!
//! `session_created` and `session_activity` are whole Unix seconds, so they
//! cannot order two events inside the same second. tmux 3.7b compares full
//! `timeval` values instead: three sessions created back to back resolve
//! targetlessly to the last one, not to the alphabetically first. Every fixture
//! here pins the public seconds so the collision is a property of the test
//! rather than a race against the clock.

use super::{TargetFindContext, TargetFindFlags, TargetFindType, UnresolvedTarget};
use crate::{SessionStore, WindowId};
use rmux_proto::{SessionName, Target, TerminalSize, WindowTarget};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// One arbitrary but fixed second shared by every session in a fixture.
const PINNED_SECOND: i64 = 1_785_500_000;

fn session_name(value: &str) -> SessionName {
    SessionName::new(value).expect("valid session name")
}

/// Builds a store whose sessions were created in `creation_order`, then used in
/// `use_order`, and whose public timestamps all report `PINNED_SECOND`.
fn same_second_store(creation_order: &[&str], use_order: &[&str]) -> SessionStore {
    let mut store = SessionStore::new();
    for name in creation_order {
        store
            .create_session(session_name(name), SIZE)
            .expect("session create succeeds");
    }
    for name in use_order {
        store
            .session_mut(&session_name(name))
            .expect("used session exists")
            .touch_attached();
    }
    pin_public_seconds(&mut store);
    store
}

fn pin_public_seconds(store: &mut SessionStore) {
    let names = store
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    for name in names {
        store
            .session_mut(&name)
            .expect("listed session exists")
            .pin_public_times_for_tests(PINNED_SECOND);
    }
    assert!(
        store
            .iter()
            .all(|(_, session)| session.created_at() == PINNED_SECOND
                && session.activity_at() == PINNED_SECOND),
        "the fixture must leave no public second able to order the sessions"
    );
}

fn default_session(store: &SessionStore) -> SessionName {
    let target = store
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

#[test]
fn default_session_is_the_last_used_one_not_the_first_by_name_or_id() {
    // m03 is deliberately neither the alphabetically first name nor the
    // highest creation id, so neither legacy tiebreak can produce it.
    for creation_order in [
        ["m03", "z99", "a01"],
        ["a01", "z99", "m03"],
        ["z99", "a01", "m03"],
    ] {
        let store = same_second_store(&creation_order, &["z99", "m03"]);
        assert_eq!(
            default_session(&store),
            session_name("m03"),
            "creation order {creation_order:?}"
        );
    }
}

#[test]
fn default_session_is_the_last_created_one_when_none_was_ever_used() {
    let store = same_second_store(&["z99", "a01", "m03"], &[]);
    // tmux 3.7b measured: three same-second sessions resolve to the last one
    // created, because creation seeds the activity timeval it ranks by.
    assert_eq!(default_session(&store), session_name("m03"));
}

#[test]
fn empty_window_index_resolves_through_the_same_last_used_session() {
    let mut store = same_second_store(&["m03", "z99", "a01"], &["z99", "m03"]);
    store
        .session_mut(&session_name("m03"))
        .expect("m03 exists")
        .insert_window_with_initial_pane(1, SIZE)
        .expect("second window insert succeeds");

    let target = store
        .resolve_unresolved_target(
            &UnresolvedTarget::new(":"),
            TargetFindType::Window,
            TargetFindFlags::WINDOW_INDEX,
            &TargetFindContext::new(None),
        )
        .expect("empty window index resolves");

    // Only m03 owns a second window, so the free index proves which session
    // the empty target resolved through, not just which name came back.
    assert_eq!(
        target,
        Target::Window(WindowTarget::with_window(session_name("m03"), 2))
    );
}

#[test]
fn an_explicit_target_still_wins_over_the_most_recent_session() {
    let store = same_second_store(&["m03", "z99", "a01"], &["z99", "m03"]);
    let target = store
        .resolve_unresolved_target(
            &UnresolvedTarget::new("a01"),
            TargetFindType::Session,
            TargetFindFlags::NONE,
            &TargetFindContext::new(None),
        )
        .expect("explicit session target resolves");

    assert_eq!(target, Target::Session(session_name("a01")));
}

#[test]
fn a_valid_current_context_still_wins_over_the_most_recent_session() {
    let store = same_second_store(&["m03", "z99", "a01"], &["z99", "m03"]);
    let context = TargetFindContext::from_target(Target::Session(session_name("a01")));
    let target = store
        .resolve_unresolved_target(
            &UnresolvedTarget::none(),
            TargetFindType::Session,
            TargetFindFlags::NONE,
            &context,
        )
        .expect("current session context resolves");

    assert_eq!(target, Target::Session(session_name("a01")));
}

#[test]
fn renaming_a_session_creates_no_activity() {
    // Renaming the *older* session must not promote it, and renaming the
    // latest one must not demote it either: a rename is not use.
    let mut store = same_second_store(&["m03", "z99", "a01"], &["z99", "m03"]);
    store
        .rename_session(&session_name("a01"), session_name("zzz"))
        .expect("older session rename succeeds");
    assert_eq!(default_session(&store), session_name("m03"));

    store
        .rename_session(&session_name("m03"), session_name("y77"))
        .expect("latest session rename succeeds");
    assert_eq!(default_session(&store), session_name("y77"));
}

#[test]
fn recreating_a_same_name_session_starts_a_newer_lifetime() {
    let mut store = same_second_store(&["m03", "z99", "a01"], &["z99", "m03"]);
    let removed = store
        .remove_session(&session_name("a01"))
        .expect("session removal succeeds");
    store
        .create_session(session_name("a01"), SIZE)
        .expect("session recreation succeeds");
    store
        .session_mut(&session_name("a01"))
        .expect("recreated session exists")
        .pin_public_times_for_tests(PINNED_SECOND);

    assert_ne!(
        store
            .session(&session_name("a01"))
            .expect("recreated session exists")
            .id(),
        removed.id(),
        "a recreated session must be a new identity, not the destroyed one"
    );
    assert_eq!(default_session(&store), session_name("a01"));
}

#[test]
fn a_removed_session_cannot_keep_winning_through_a_stale_recency() {
    let mut store = same_second_store(&["a01", "z99", "m03"], &["m03"]);
    store
        .remove_session(&session_name("m03"))
        .expect("latest session removal succeeds");

    // z99 was created after a01 and is now the newest surviving lifetime.
    assert_eq!(default_session(&store), session_name("z99"));
}

#[test]
fn a_grouped_peer_gets_its_own_lifetime() {
    let mut store = same_second_store(&["m03", "z99", "a01"], &["z99", "m03"]);
    store
        .create_grouped_session_with_base_index(session_name("peer"), SIZE, 0, session_name("a01"))
        .expect("grouped session create succeeds");
    store
        .session_mut(&session_name("peer"))
        .expect("grouped peer exists")
        .pin_public_times_for_tests(PINNED_SECOND);

    // tmux 3.7b measured: a grouped peer is a new session whose own creation
    // seeds its activity, so it outranks the group source it cloned.
    assert_eq!(default_session(&store), session_name("peer"));
}

#[test]
fn synchronizing_grouped_windows_creates_no_activity() {
    let mut store = same_second_store(&["m03", "z99", "a01"], &["z99", "m03"]);
    store
        .create_grouped_session_with_base_index(session_name("peer"), SIZE, 0, session_name("a01"))
        .expect("grouped session create succeeds");
    store
        .session_mut(&session_name("peer"))
        .expect("grouped peer exists")
        .pin_public_times_for_tests(PINNED_SECOND);
    store
        .session_mut(&session_name("a01"))
        .expect("group source exists")
        .insert_window_with_initial_pane(1, SIZE)
        .expect("group source window insert succeeds");

    let source = store
        .session(&session_name("a01"))
        .expect("group source exists")
        .clone();
    store
        .session_mut(&session_name("peer"))
        .expect("grouped peer exists")
        .synchronize_group_from(&source);

    // Propagating the source's new window into the peer is bookkeeping, not
    // use: neither session may overtake the peer's own creation.
    assert_eq!(default_session(&store), session_name("peer"));
}

#[test]
fn a_window_id_only_change_creates_no_activity() {
    let mut store = same_second_store(&["a01", "m03", "z99"], &[]);
    let session = store.session_mut(&session_name("a01")).expect("a01 exists");
    session
        .insert_window_with_initial_pane(1, SIZE)
        .expect("window insert succeeds");
    session.select_window(1).expect("window select succeeds");
    assert_ne!(
        session.window().id(),
        WindowId::new(u32::MAX),
        "the fixture must have a real window identity"
    );

    // Window and pane bookkeeping is not session use; z99 was created last.
    assert_eq!(default_session(&store), session_name("z99"));
}

#[test]
fn concurrent_interactions_never_share_a_recency_position() {
    // Concurrent clients linearize wherever they acquire the server locks, but
    // whichever order that produces has to be a *total* one: two sessions must
    // never end up tied, or the readers fall back to a name or an id again.
    const THREADS: usize = 8;
    const PER_THREAD: usize = 256;

    let mut tokens = std::thread::scope(|scope| {
        let handles = (0..THREADS)
            .map(|_| {
                scope.spawn(|| {
                    (0..PER_THREAD)
                        .map(|_| {
                            let mut session = crate::Session::new(session_name("racer"), SIZE);
                            session.touch_activity();
                            session.recency()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("recency thread joins"))
            .collect::<Vec<_>>()
    });

    let minted = tokens.len();
    tokens.sort_unstable();
    tokens.dedup();
    assert_eq!(
        tokens.len(),
        minted,
        "every lifetime and interaction event must occupy its own recency position"
    );
}
