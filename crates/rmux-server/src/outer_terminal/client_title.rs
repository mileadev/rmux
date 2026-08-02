//! Per-client outer-terminal identity: the OSC 0 title and OSC 7 path a client
//! has already been told, and what a fresh render still owes it.

/// What one client's outer terminal was last told about its identity.
///
/// tmux keeps the same pair per client in `c->title` / `c->path` and writes
/// only when a fresh expansion differs, so a refresh that resolves the same
/// values stays silent. It keeps them while `set-titles` is off too, which is
/// why toggling the option back on does not rewrite an unchanged title.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClientTitleState {
    pub(super) title: Option<String>,
    pub(super) path: Option<String>,
}

impl ClientTitleState {
    /// The expanded title this client's outer terminal currently shows.
    ///
    /// Production code compares through [`ClientTitleUpdate::pending_title`];
    /// this is the read-back the regression tests assert on.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
}

/// What one render actually did to a client's outer terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RenderedClientTitle {
    state: ClientTitleState,
    /// The OSC 0 / OSC 7 bytes this render put in the frame.
    ///
    /// They are kept apart from the frame because they are a property of the
    /// outer terminal rather than of the drawn screen — tmux writes the title
    /// from its server loop, not from a redraw. A frame the attach loop must
    /// not draw still owes the client these bytes (issue #182).
    bytes: Vec<u8>,
}

impl RenderedClientTitle {
    /// What the outer terminal shows once this render reaches it.
    pub(crate) const fn state(&self) -> &ClientTitleState {
        &self.state
    }

    /// This render put OSC 0 / OSC 7 bytes in the frame. A later render
    /// deduplicates against them, so the frame carrying them must reach the
    /// client rather than be replaced in the control queue by that successor.
    pub(crate) const fn wrote(&self) -> bool {
        !self.bytes.is_empty()
    }

    /// The outer-terminal sequences this render produced, which any path that
    /// drops the carrying frame must still deliver.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// What this render actually committed to the client's remembered
    /// identity, or `None` when it put no bytes in the frame.
    ///
    /// A render that resolved a title but wrote nothing changed nothing, and
    /// it deduplicated against a snapshot taken before the frame was built. If
    /// another path delivered a different title in between, adopting this
    /// value would claim the outer terminal shows something it does not — and
    /// the next expansion back to this value would then be skipped, stranding
    /// the stale title that issue #182 is about.
    pub(crate) fn committed(&self) -> Option<&ClientTitleState> {
        self.wrote().then_some(&self.state)
    }
}

/// What one client's outer terminal should now be told.
///
/// `resolved` is the expanded `set-titles-string`, or `None` when the session
/// has `set-titles off` — tmux's gate for touching the outer terminal at all.
/// Measured on tmux 3.7b: with `set-titles off` an attached client emits
/// neither OSC 0 nor OSC 7 even when the terminal advertises both.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ClientTitleUpdate<'a> {
    pub(crate) resolved: Option<&'a str>,
    pub(crate) path: Option<&'a str>,
    pub(crate) previous: Option<&'a ClientTitleState>,
}

impl<'a> ClientTitleUpdate<'a> {
    /// The title to write, or `None` when titles are off or already current.
    ///
    /// tmux's `server_client_set_title()` compares the fresh expansion against
    /// `c->title` and calls `tty_set_title()` only when they differ, so a
    /// client that already shows this exact title is left alone.
    pub(super) fn pending_title(&self) -> Option<&'a str> {
        let resolved = self.resolved?;
        (self.previous.and_then(|previous| previous.title.as_deref()) != Some(resolved))
            .then_some(resolved)
    }

    /// The OSC 7 path to write, under the same `set-titles` gate and the same
    /// per-client comparison tmux makes against `c->path`.
    ///
    pub(super) fn pending_path(&self) -> Option<&'a str> {
        self.resolved?;
        let path = self.path.filter(|path| !path.is_empty())?;
        (self.previous.and_then(|previous| previous.path.as_deref()) != Some(path)).then_some(path)
    }

    /// What this render leaves behind, or `None` while `set-titles` is off and
    /// the outer terminal is left alone.
    ///
    /// `bytes` is what the outer terminal actually rendered for this update: it
    /// is empty when the terminal advertised no template for the sequence. The
    /// values are remembered even then, exactly as tmux assigns `c->title`
    /// before `tty_set_title()` decides it has no TSL/FSL to write with — but a
    /// render that emitted nothing is still a replaceable refresh.
    /// What a real outer terminal makes of this update, for tests that assert
    /// on the record rather than on the frame it came from.
    #[cfg(test)]
    pub(super) fn rendered_by(
        &self,
        terminal: &super::OuterTerminal,
    ) -> Option<RenderedClientTitle> {
        terminal.rendered_client_title(*self)
    }

    pub(super) fn rendered(&self, bytes: Vec<u8>) -> Option<RenderedClientTitle> {
        let resolved = self.resolved?;
        Some(RenderedClientTitle {
            bytes,
            state: ClientTitleState {
                title: Some(resolved.to_owned()),
                path: self
                    .path
                    .filter(|path| !path.is_empty())
                    .map(str::to_owned)
                    .or_else(|| {
                        self.previous
                            .and_then(|previous| previous.path.as_ref())
                            .cloned()
                    }),
            },
        })
    }
}
