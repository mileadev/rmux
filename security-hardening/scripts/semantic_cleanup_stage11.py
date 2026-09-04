#!/usr/bin/env python3
"""Remove dead remote/platform remnants exposed by strict macOS Clippy."""
from __future__ import annotations
import os
import re
import sys
from pathlib import Path


def atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + '.rmux-stage11-tmp')
    tmp.write_text(text, encoding='utf-8')
    os.replace(tmp, path)


def edit(path: Path, fn) -> None:
    if not path.exists():
        return
    old = path.read_text(encoding='utf-8')
    new = fn(old)
    if new != old:
        atomic_write(path, new)
        print(f'edited {path}')


def rx(text: str, pattern: str, repl: str = '') -> str:
    return re.sub(pattern, repl, text, flags=re.MULTILINE | re.DOTALL)


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()

    # Removed remote fanout left a no-op loop.
    edit(root/'crates/rmux-server/src/handler_pane/lifecycle.rs', lambda t: rx(
        t,
        r'\n\s*for \(destroyed_session, session_id\) in\n\s*&result\.destroyed_sessions\n\s*\{\n\s*\}\n',
        '\n'
    ))

    # Bounded recovery was only consumed by the removed remote renderer.
    edit(root/'crates/rmux-server/src/handler_attach.rs', lambda t: re.sub(
        r'(?m)^\s*bounded_recovery:\s*[^,\n]+,\n', '', t
    ))

    # Production-only wrapper became test-only after the web attach path was deleted.
    def registration(t: str) -> str:
        marker = '    pub(crate) async fn register_attach_identity_with_access(\n'
        if marker in t and '    #[cfg(test)]\n' + marker not in t:
            t = t.replace(marker, '    #[cfg(test)]\n' + marker, 1)
        return t
    edit(root/'crates/rmux-server/src/handler_attach/registration.rs', registration)

    # Async blocking observation entrypoint was only used by removed remote streaming.
    edit(root/'crates/rmux-server/src/pane_io/types.rs', lambda t: rx(
        t,
        r'\n    pub\(crate\) async fn recv_observed\(&mut self\) -> PaneObservationItem \{.*?\n    \}\n\n    pub\(crate\) fn try_recv_observed',
        '\n    pub(crate) fn try_recv_observed'
    ))

    def recovery(t: str) -> str:
        # Generalize comments away from the removed browser/share transport.
        t = t.replace(
            '/// A pane-scoped WebShare snapshot shares the two-MiB outbound budget with\n'
            '/// its opcode and sanitizer state. The sanitizer never expands its input, so\n'
            '/// 4 KiB leaves bounded framing headroom while retaining the largest common\n'
            '/// typed viewport.\n',
            '/// Recovery keyframes reserve 4 KiB of detached-frame headroom while\n'
            '/// retaining the largest common typed viewport.\n'
        )
        # The fork has one local owner-authenticated recovery policy; collapse the enum.
        t = rx(t, r'\n/// How much scrolled-off scrollback a recovery keyframe is allowed to replay\..*?\nimpl RecoveryHistoryPolicy \{.*?\n\}\n\nimpl PaneRecoveryDraft \{', '\nimpl PaneRecoveryDraft {')
        t = t.replace(
            '    pub(crate) fn materialize(self) -> Result<PaneRecoverySeed, RmuxError> {\n'
            '        self.materialize_with_history(RecoveryHistoryPolicy::RecentHistory)\n'
            '    }\n\n'
            '    /// Materialize a keyframe that replays the pane\'s visible state only.\n'
            '    ///\n'
            '    /// This is the WebShare pane-surface entry point: it must stay the only\n'
            '    /// materializer reachable from a shared link.\n'
            '    pub(crate) fn materialize_visible_only(self) -> Result<PaneRecoverySeed, RmuxError> {\n'
            '        self.materialize_with_history(RecoveryHistoryPolicy::VisibleOnly)\n'
            '    }\n\n'
            '    fn materialize_with_history(\n'
            '        self,\n'
            '        history_policy: RecoveryHistoryPolicy,\n'
            '    ) -> Result<PaneRecoverySeed, RmuxError> {',
            '    pub(crate) fn materialize(self) -> Result<PaneRecoverySeed, RmuxError> {'
        )
        # Both render attempts now use the sole local history policy.
        t = re.sub(r'(?m)^\s*history_policy,\n', '', t)
        # Remove the obsolete policy parameter from the renderer helper.
        t = re.sub(
            r'(?m)^(\s*metadata_complete: bool,)\n\s*history_policy: RecoveryHistoryPolicy,\n',
            r'\1\n',
            t,
        )
        t = t.replace(
            '    // Both fixes land here. The share-viewer policy bounds how much history a\n'
            '    // spectator may receive; the wrap fix needs `cols` and links the boundary\n'
            '    // row pair so a soft-wrapped row is not glued onto the next one.\n'
            '    let history_budget = history_policy.history_budget(MAX_RECOVERY_KEYFRAME_BYTES - mandatory_len);',
            '    // Preserve as much recent local history as fits while retaining keyframe headroom.\n'
            '    let history_budget = MAX_RECOVERY_KEYFRAME_BYTES - mandatory_len;'
        )
        # Method exposed only for the deleted remote projection.
        t = rx(t, r'\n    pub\(crate\) const fn alternate\(&self\) -> bool \{\n        self\.projection\.alternate\(\)\n    \}\n', '\n')
        return t
    edit(root/'crates/rmux-server/src/pane_recovery.rs', recovery)

    # Closure-based screen access was used by the removed remote surface only.
    edit(root/'crates/rmux-server/src/pane_terminals/pane_transcripts.rs', lambda t: rx(
        t,
        r'\n    pub\(crate\) fn with_pane_screen<R>\(.*?\n    \}\n\n    pub\(crate\) fn pane_screen',
        '\n    pub(crate) fn pane_screen'
    ))

    # Remove the no-longer-used top-level extension scanner and stale help entries.
    def top_level(t: str) -> str:
        t = t.replace('use crate::cli_args::{scan_top_level_command, Cli};', 'use crate::cli_args::Cli;')
        t = t.replace('  claude [install-skill|claude-args...]\\n', '')
        t = t.replace('  web-share [flags]\\n  web-share list|lookup|stop|disconnect|off|config\\n', '')
        return t
    edit(root/'src/cli/top_level.rs', top_level)

    edit(root/'src/cli_args.rs', lambda t: rx(
        t,
        r'\n/// Result of parsing only the clap-owned top-level prefix and opaque command\n.*?\nfn normalize_top_level_attached_short_values',
        '\nfn normalize_top_level_attached_short_values'
    ))

    # A macOS-only daemon must never advertise a Windows console-key capability.
    def capabilities(t: str) -> str:
        t = rx(t, r'\n/// Stable feature id for attach-stream Windows console key messages\.\npub const CAPABILITY_ATTACH_WINDOWS_CONSOLE_KEY: &str = "stream\.attach\.windows_console_key";', '')
        t = re.sub(r'(?m)^\s*CAPABILITY_ATTACH_WINDOWS_CONSOLE_KEY,\n', '', t)
        t = t.replace('        CAPABILITY_ATTACH_WINDOWS_CONSOLE_KEY, ', '')
        t = t.replace('        CAPABILITY_ATTACH_WINDOWS_CONSOLE_KEY,\n', '')
        # capabilities_for_features remains public API; only remove the stale test import.
        t = t.replace('        capabilities_for_features, HandshakeRequest, HandshakeResponse, ', '        HandshakeRequest, HandshakeResponse, ')
        # Remove the test assertion for the deleted platform capability.
        t = rx(t, r'\n        assert!\(response\n            \.capabilities\n            \.iter\(\)\n            \.any\(\|capability\| capability == CAPABILITY_ATTACH_WINDOWS_CONSOLE_KEY\)\);', '')
        return t
    edit(root/'crates/rmux-proto/src/capabilities.rs', capabilities)

    print('stage11 clippy/platform cleanup complete')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
