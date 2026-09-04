#!/usr/bin/env python3
"""Restore the generic top-level CLI scanner removed by an earlier broad cleanup."""
from __future__ import annotations
import os
import re
import sys
from pathlib import Path

SCANNER = r'''
/// Result of parsing only the clap-owned top-level prefix and opaque command
/// tail. Internal dispatch uses this before command-queue parsing so it shares
/// the exact same short-option cluster and value-boundary rules as the main CLI.
#[derive(Debug)]
pub(crate) struct TopLevelCommandScan {
    pub(crate) assume_256_colors: bool,
    pub(crate) control_mode: u8,
    pub(crate) no_fork: bool,
    pub(crate) shell_command: Option<String>,
    pub(crate) config_files: Vec<PathBuf>,
    pub(crate) login_shell: bool,
    pub(crate) socket_name: Option<OsString>,
    pub(crate) no_start_server: bool,
    pub(crate) socket_path: Option<OsString>,
    pub(crate) terminal_features: Vec<String>,
    pub(crate) utf8: bool,
    pub(crate) verbose: u8,
    pub(crate) command: Vec<OsString>,
}

pub(crate) fn scan_top_level_command(
    arguments: &[OsString],
) -> Result<TopLevelCommandScan, clap::Error> {
    let args = std::iter::once(OsString::from("rmux"))
        .chain(arguments.iter().cloned())
        .collect::<Vec<_>>();
    let args = normalize_top_level_attached_short_values(args);
    let matches = RawCli::command().try_get_matches_from(args)?;
    let raw = RawCli::from_arg_matches(&matches)?;
    Ok(TopLevelCommandScan {
        assume_256_colors: raw.assume_256_colors,
        control_mode: raw.control_mode,
        no_fork: raw.no_fork,
        shell_command: raw.shell_command,
        config_files: raw.config_files,
        login_shell: raw.login_shell,
        socket_name: raw.socket_name,
        no_start_server: raw.no_start_server,
        socket_path: raw.socket_path,
        terminal_features: raw.terminal_features,
        utf8: raw.utf8,
        verbose: raw.verbose,
        command: raw.command,
    })
}

'''


def atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + '.rmux-stage18-tmp')
    tmp.write_text(text, encoding='utf-8')
    os.replace(tmp, path)


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()
    cli_args = root / 'src/cli_args.rs'
    text = cli_args.read_text(encoding='utf-8')
    if 'pub(crate) struct TopLevelCommandScan' not in text:
        marker = 'fn normalize_top_level_attached_short_values<I>(args: I) -> Vec<OsString>\n'
        if marker not in text:
            raise SystemExit('normalize_top_level_attached_short_values marker not found')
        text = text.replace(marker, SCANNER + marker, 1)
        atomic_write(cli_args, text)
        print(f'restored generic CLI scanner in {cli_args}')

    auto_start = root / 'crates/rmux-client/src/auto_start.rs'
    text = auto_start.read_text(encoding='utf-8')
    new = re.sub(
        r'(fn hidden_daemon_binary_path_for_config\(\n    current_exe: &Path,\n    )config: &AutoStartConfig,',
        r'\1_config: &AutoStartConfig,',
        text,
        count=1,
    )
    if new != text:
        atomic_write(auto_start, new)
        print(f'normalized unused local-only config parameter in {auto_start}')

    print('stage18 generic CLI scanner restoration complete')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
