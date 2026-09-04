#!/usr/bin/env python3
"""Idempotent semantic cleanup for the macOS local-only RMUX fork."""
from __future__ import annotations
import os, re, shutil, sys
from pathlib import Path


def atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + '.rmux-semantic-tmp')
    tmp.write_text(text, encoding='utf-8')
    os.replace(tmp, path)


def edit(path: Path, fn) -> bool:
    if not path.exists():
        return False
    old = path.read_text(encoding='utf-8')
    new = fn(old)
    if old != new:
        atomic_write(path, new)
        print(f'edited {path}')
        return True
    return False


def drop(path: Path) -> bool:
    if not path.exists() and not path.is_symlink():
        return False
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()
    print(f'deleted {path}')
    return True


def rx(text: str, pattern: str, repl: str = '') -> str:
    return re.sub(pattern, repl, text, flags=re.MULTILINE | re.DOTALL)


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()

    for rel in [
        'crates/rmux-sdk/src/bootstrap/startup_windows',
        'src/cli/web_commands.rs',
        'src/cli_args/web.rs',
        'crates/rmux-client/src/commands/web.rs',
        'crates/rmux-proto/src/request/web.rs',
        'crates/rmux-proto/src/response/web.rs',
        'crates/rmux-server/src/handler_web.rs',
        'crates/rmux-server/src/handler_web_disabled.rs',
        'crates/rmux-server/src/handler/web_request_identity.rs',
    ]:
        drop(root / rel)

    edit(root / 'crates/rmux-client/src/lib.rs', lambda t: (
        t.replace('#[cfg(windows)]\n#[path = "attach_windows.rs"]\npub mod attach;\n', '')
         .replace('#[cfg(windows)]\npub use attach::attach_terminal_with_initial_bytes_and_windows_console_key;\n', '')
         .replace('#[cfg(windows)]\npub use commands::server::connect_for_server_shutdown;\n', '')
    ))
    edit(root / 'crates/rmux-client/src/commands/mod.rs', lambda t: t.replace('pub(crate) mod web;\n', ''))

    edit(root / 'crates/rmux-ipc/src/stream.rs', lambda t: (
        t.replace('#[cfg(windows)]\n#[path = "stream_windows.rs"]\nmod windows;\n\n', '')
         .replace('#[cfg(windows)]\npub use windows::{\n    connect_blocking, connect_windows_pipe, BlockingLocalStream, LocalStream, WindowsPipeClient,\n};\n\n', '')
    ))
    def clean_ipc_lib(t: str) -> str:
        t = rx(t, r'\n#\[cfg\(any\(windows, test\)\)\]\nmod windows_endpoint_components;', '')
        t = rx(t, r'\n#\[cfg\(windows\)\]\nmod windows_[a-z_]+;', '')
        t = rx(t, r'\n#\[cfg\(windows\)\]\npub use stream::\{.*?\};', '')
        t = rx(t, r'\n#\[cfg\(windows\)\]\npub use windows_[a-z_]+::.*?;', '')
        t = rx(t, r'\n#\[cfg\(windows\)\]\npub use windows_mutex::\{.*?\n\};', '')
        return t
    edit(root / 'crates/rmux-ipc/src/lib.rs', clean_ipc_lib)

    def clean_process(t: str) -> str:
        t = t.replace('#[cfg(any(unix, windows))]\nuse std::ffi::OsString;\n', '#[cfg(unix)]\nuse std::ffi::OsString;\n')
        t = t.replace('#[cfg(windows)]\n#[path = "process_windows.rs"]\nmod windows_process;\n', '')
        t = t.replace('#[cfg(windows)]\npub use windows_process::ProcessJob;\n', '')
        t = t.replace('    #[cfg(any(unix, windows))]\n    pub fn raw_environment', '    #[cfg(unix)]\n    pub fn raw_environment')
        t = rx(t, r'\n    /// Returns executable names for live descendants of `pid`, when available\.\n    #\[cfg\(windows\)\]\n    pub fn descendant_command_names\(&self, pid: u32\) -> io::Result<Vec<String>> \{.*?\n    \}\n', '\n')
        return t
    edit(root / 'crates/rmux-os/src/process.rs', clean_process)
    edit(root / 'crates/rmux-os/src/process_tree.rs', lambda t: (
        t.replace('#[cfg(windows)]\nmod windows;\n#[cfg(windows)]\nuse windows::resume_suspended_process;\n', '')
    ))

    edit(root / 'crates/rmux-pty/src/lib.rs', lambda t: rx(
        t.replace('#[cfg(windows)]\nmod windows_console_input;\n', ''),
        r'\n#\[cfg\(windows\)\]\npub use windows_console_input::\{.*?\n\};\n', '\n'
    ))

    edit(root / 'crates/rmux-sdk/src/bootstrap/mod.rs', lambda t: t.replace('#[cfg(windows)]\npub mod startup_windows;\n', ''))
    edit(root / 'crates/rmux-sdk/tests/common/mod.rs', lambda t: t.replace('\n#[cfg(windows)]\npub mod windows_smoke;', '').rstrip() + '\n')

    def clean_buffer(t: str) -> str:
        t = rx(t, r'\n#\[cfg\(windows\)\]\nuse std::fs::\{self, OpenOptions\};.*?use std::path::Path;\n', '\n')
        t = t.replace('#[cfg(windows)]\n#[path = "buffer_file_io/windows.rs"]\nmod platform;\n\n', '')
        t = rx(t, r'\n#\[cfg\(windows\)\]\npub\(crate\) async fn read\(path: PathBuf\).*?\n\}\n\n', '\n')
        t = rx(t, r'\n#\[cfg\(windows\)\]\npub\(crate\) async fn write\(path: PathBuf, content: Vec<u8>, append: bool\).*?\n\}\n\n#\[cfg\(windows\)\]\nfn write_regular_file.*?\n\}\n?$', '\n')
        return t
    edit(root / 'crates/rmux-server/src/buffer_file_io.rs', clean_buffer)
    edit(root / 'crates/rmux-server/src/daemon.rs', lambda t: t.replace('\n#[cfg(all(test, windows))]\n#[path = "daemon_tests/windows.rs"]\nmod tests;', ''))

    def clean_cli(t: str) -> str:
        for block in [
            '#[path = "cli/claude_launcher.rs"]\nmod claude_launcher;\n',
            '#[path = "cli/claude_skill.rs"]\nmod claude_skill;\n',
            '#[path = "cli/web_commands.rs"]\nmod web_commands;\n',
            '#[path = "cli/web_share_display.rs"]\nmod web_share_display;\n',
        ]:
            t = t.replace(block, '')
        t = t.replace(
            'use top_level::{\n    accept_compatibility_options, infer_client_utf8_from_env, scan_claude_top_level_invocation,\n    top_level_parse_failure, top_level_version_output, top_level_version_requested,\n    validate_claude_top_level_invocation, validate_top_level_invocation,\n};',
            'use top_level::{\n    accept_compatibility_options, infer_client_utf8_from_env, top_level_parse_failure,\n    top_level_version_output, top_level_version_requested, validate_top_level_invocation,\n};'
        )
        t = rx(t, r'\n    let claude_invocation = scan_claude_top_level_invocation\(args\.get\(1\.\.\)\.unwrap_or\(&\[\]\)\);\n    validate_claude_top_level_invocation\(claude_invocation\.as_ref\(\)\)\?;\n    if let Some\(invocation\) = claude_launcher::parse_internal_runner\(args\.get\(1\.\.\)\.unwrap_or\(&\[\]\)\) \{\n        return claude_launcher::run_internal_runner\(invocation\);\n    \}\n', '\n')
        t = rx(t, r'\n    if let Some\(claude_invocation\) = claude_invocation \{.*?\n    \}\n    if let Some\(invocation\) = capabilities::parse_invocation', '\n    if let Some(invocation) = capabilities::parse_invocation')
        return t
    edit(root / 'src/cli.rs', clean_cli)
    edit(root / 'src/cli/diagnose.rs', lambda t: t.replace('#[cfg(windows)]\n#[path = "diagnose_windows.rs"]\nmod diagnose_windows;\n\n', ''))

    def clean_cli_args(t: str) -> str:
        t = t.replace('#[path = "cli_args/web.rs"]\nmod web;\npub(crate) use web::{WebShareArgs, WebShareTerminalThemeArg, WEB_SHARE_TUNNEL_PROVIDERS};\n', '')
        t = t.replace('    WebShare(WebShareArgs),\n', '')
        t = rx(t, r'pub\(crate\) struct StartServerArgs \{\n    #\[arg\(long = "web-port".*?\n\}\n', 'pub(crate) struct StartServerArgs {}\n')
        return t
    edit(root / 'src/cli_args.rs', clean_cli_args)

    def clean_inventory(t: str) -> str:
        t = rx(t, r'\n    CommandEntry \{\n        name: "claude",\n        alias: None,\n    \},', '')
        t = rx(t, r'\n    CommandEntry \{\n        name: "web-share",\n        alias: None,\n    \},', '')
        return t
    edit(root / 'crates/rmux-core/src/command_inventory.rs', clean_inventory)
    edit(root / 'crates/rmux-sdk/src/lib.rs', lambda t: t.replace('#[cfg(feature = "web")]\npub mod web_share;\n', ''))

    def clean_request(t: str) -> str:
        t = rx(t, r'\n#\[path = "request/web\.rs"\]\nmod web;\npub use web::\{.*?\n\};\n', '\n')
        t = t.replace('    /// Browser-visible pane sharing command family.\n    WebShare(Box<WebShareRequest>),\n', '')
        return t
    edit(root / 'crates/rmux-proto/src/request.rs', clean_request)

    def clean_response(t: str) -> str:
        t = rx(t, r'\n#\[path = "response/web\.rs"\]\nmod web;\npub use web::\{.*?\n\};\n', '\n')
        t = t.replace('    /// Success payload for browser-visible pane sharing.\n    WebShare(Box<WebShareResponse>),\n', '')
        t = re.sub(r'(?m)^\s*Self::WebShare\(_\) => "web-share",\n', '', t)
        return t
    edit(root / 'crates/rmux-proto/src/response.rs', clean_response)

    print('semantic cleanup complete')
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
