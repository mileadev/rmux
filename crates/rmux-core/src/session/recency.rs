use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SESSION_RECENCY: AtomicU64 = AtomicU64::new(0);

/// Opaque process-local total order over session recency events.
///
/// Public session times are whole Unix seconds, so `session_created` and
/// `session_activity` cannot order two events inside the same second. tmux
/// compares full `timeval` values instead, which is why it ranks the last of
/// three same-second sessions first. This token restores that total order
/// without changing any public format, option or wire value: it is an internal
/// workspace key that is never serialized, rendered or compared across
/// processes.
///
/// Only a lifetime or accepted-interaction event mints a new token. Renaming a
/// session, synchronizing grouped windows, pane output and detaching all leave
/// the existing token in place.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionRecency(u64);

impl SessionRecency {
    /// Mints the next token in the process-local total order.
    pub(super) fn next() -> Self {
        // `fetch_update` rather than `fetch_add` so exhaustion aborts instead
        // of wrapping a live ordering key back behind existing sessions.
        let sequence = NEXT_SESSION_RECENCY
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("session recency sequence exhausted");
        Self(sequence)
    }
}
