#[cfg(any(unix, windows))]
use std::path::Path;

#[cfg(any(unix, windows))]
pub(crate) fn command_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(unix)]
pub(crate) fn sh_quote(value: &str) -> String {
    command_quote(value)
}

#[cfg(unix)]
pub(crate) fn sh_quote_path(path: &Path) -> String {
    sh_quote(&path.display().to_string())
}

#[cfg(any(unix, windows))]
pub(crate) fn stdin_discard_command() -> String {
    platform_stdin_discard_command()
}

#[cfg(unix)]
fn platform_stdin_discard_command() -> String {
    "cat >/dev/null".to_owned()
}

#[cfg(windows)]
fn platform_stdin_discard_command() -> String {
    powershell_encoded_command(
        "$inputStream=[Console]::OpenStandardInput(); $inputStream.CopyTo([System.IO.Stream]::Null)",
    )
}

#[cfg(windows)]
pub(crate) fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
pub(crate) fn powershell_quote_path(path: &Path) -> String {
    powershell_quote(&path.display().to_string())
}

#[cfg(windows)]
pub(crate) fn powershell_encoded_command(script: &str) -> String {
    format!(
        "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
        encode_powershell_script(script)
    )
}

#[cfg(windows)]
fn encode_powershell_script(script: &str) -> String {
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    base64_encode(&bytes)
}

#[cfg(windows)]
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let value = ((first as u32) << 16) | ((second as u32) << 8) | third as u32;
        encoded.push(TABLE[((value >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((value >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[((value >> 6) & 0x3f) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(value & 0x3f) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

/// A real pane child that persists the bytes its application actually read.
///
/// Every other bracketed-paste suite in this crate observes
/// `RawPaneInputProbe`, which short-circuits
/// `prepare_pane_input_write_with_encoding` to `PaneInputSink::CapturedForTest`
/// *before* the starting-pane queue and *before* the Windows
/// passthrough/legacy sink selection. Those probes stay green when the legacy
/// console sink is bypassed, so they cannot close the `AttachInput::bytes ->
/// ... -> real child application bytes` obligation.
///
/// A slot is a directory the harness and the child exchange files through:
///
/// | File    | Written by | Meaning                                     |
/// |---------|------------|---------------------------------------------|
/// | `ready` | child      | terminal is raw and the read is starting     |
/// | `out`   | child      | the complete application-side capture        |
/// | `error` | child      | the child failed before capturing            |
/// | `stop`  | harness    | the child may leave its keep-alive park      |
///
/// The child must read from a *raw* terminal: a cooked console or PTY line
/// discipline treats the paste's leading `ESC` as an editing command and
/// rewrites CR/LF, so the captured bytes would say nothing about the sink. The
/// crate is `#![forbid(unsafe_code)]`, so the child cannot be this test binary
/// re-executed — the Windows console-mode change needs FFI. Each platform
/// therefore uses its native interpreter, which is exactly what this module
/// already exists to build.
#[cfg(any(unix, windows))]
pub(crate) mod final_sink {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    const READY_FILE: &str = "ready";
    const OUT_FILE: &str = "out";
    const OUT_PARTIAL_FILE: &str = "out.part";
    const ERROR_FILE: &str = "error";
    const STOP_FILE: &str = "stop";

    /// DECSET 2004: the pane advertises bracketed-paste support for real, so
    /// per-destination mode is decided from production transcript state rather
    /// than from a stamped test transcript.
    pub(crate) const ENABLE_BRACKETED_PASTE: &[u8] = b"\x1b[?2004h";

    /// How long the child stays alive after capturing, so the harness can still
    /// resolve it as a live synchronized destination while it asserts.
    const CHILD_PARK_SECONDS: u64 = 120;
    /// Generous: the Windows child compiles a small P/Invoke shim on start.
    const CAPTURE_TIMEOUT: Duration = Duration::from_secs(60);

    pub(crate) struct FinalSinkSlot {
        directory: PathBuf,
        expected: Vec<u8>,
        bracket_aware: bool,
    }

    impl FinalSinkSlot {
        /// `expected` is the exact byte sequence the child must read. It must be
        /// valid UTF-8: the Windows child reads UTF-16 console characters, and
        /// arbitrary non-UTF-8 bytes are a separate policy that this harness
        /// deliberately does not exercise.
        pub(crate) fn new(label: &str, expected: &[u8], bracket_aware: bool) -> Self {
            assert!(
                std::str::from_utf8(expected).is_ok(),
                "the final-sink harness only covers the valid-UTF-8 #92 path"
            );
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let directory = std::env::temp_dir().join(format!(
                "rmux-final-sink-{}-{}-{label}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).expect("final-sink slot directory");
            Self {
                directory,
                expected: expected.to_vec(),
                bracket_aware,
            }
        }

        fn path(&self, file: &str) -> String {
            self.directory.join(file).display().to_string()
        }

        /// Number of UTF-16 code units the Windows child must accumulate.
        fn expected_code_units(&self) -> usize {
            std::str::from_utf8(&self.expected)
                .expect("checked in `new`")
                .encode_utf16()
                .count()
        }

        pub(crate) fn wait_until_ready(&self) {
            self.wait_for(READY_FILE, "child never signalled readiness");
        }

        /// Blocks until the child persisted its complete capture and returns
        /// the exact application-side bytes.
        pub(crate) fn application_bytes(&self) -> Vec<u8> {
            self.wait_for(OUT_FILE, "child never persisted its capture");
            fs::read(self.directory.join(OUT_FILE)).expect("final-sink capture")
        }

        fn wait_for(&self, file: &str, message: &str) {
            let deadline = Instant::now() + CAPTURE_TIMEOUT;
            let target = self.directory.join(file);
            while Instant::now() < deadline {
                if target.is_file() {
                    return;
                }
                if let Ok(error) = fs::read_to_string(self.directory.join(ERROR_FILE)) {
                    panic!("{message}: child reported {error}");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            panic!(
                "{message} within {}s (slot {}, expected {} bytes)",
                CAPTURE_TIMEOUT.as_secs(),
                self.directory.display(),
                self.expected.len()
            );
        }

        #[cfg(unix)]
        pub(crate) fn pane_command(&self) -> Vec<String> {
            let announce = if self.bracket_aware {
                r"printf '\033[?2004h'"
            } else {
                ":"
            };
            let script = format!(
                "stty raw -echo\n\
                 {announce}\n\
                 : > {ready}\n\
                 dd bs=1 count={count} of={partial} 2>/dev/null\n\
                 mv {partial} {out}\n\
                 i=0\n\
                 while [ \"$i\" -lt {ticks} ] && [ ! -e {stop} ]; do\n\
                 sleep 0.05\n\
                 i=$((i+1))\n\
                 done\n",
                ready = super::sh_quote(&self.path(READY_FILE)),
                count = self.expected.len(),
                partial = super::sh_quote(&self.path(OUT_PARTIAL_FILE)),
                out = super::sh_quote(&self.path(OUT_FILE)),
                stop = super::sh_quote(&self.path(STOP_FILE)),
                ticks = CHILD_PARK_SECONDS * 20,
            );
            vec!["/bin/sh".to_owned(), "-c".to_owned(), script]
        }

        #[cfg(windows)]
        pub(crate) fn pane_command(&self) -> Vec<String> {
            vec![
                "powershell.exe".to_owned(),
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-EncodedCommand".to_owned(),
                super::encode_powershell_script(&self.windows_script()),
            ]
        }

        /// Puts the pane console into the mode a bracketed-paste-aware
        /// application uses, then reads exactly the expected number of UTF-16
        /// console characters and persists them as UTF-8.
        #[cfg(windows)]
        fn windows_script(&self) -> String {
            let announce = if self.bracket_aware {
                "[Console]::Out.Write([char]27 + '[?2004h'); [Console]::Out.Flush()"
            } else {
                "$null = $null"
            };
            format!(
                r#"
$ErrorActionPreference = 'Stop'
$ready = {ready}
$partial = {partial}
$out = {out}
$stop = {stop}
$errorFile = {error_file}
try {{
    Add-Type -Namespace RmuxFinalSink -Name Con -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true)]
public static extern IntPtr GetStdHandle(int nStdHandle);
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool GetConsoleMode(IntPtr h, out uint mode);
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool SetConsoleMode(IntPtr h, uint mode);
[DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern bool ReadConsoleW(IntPtr h, [Out] char[] buffer, uint toRead, out uint read, IntPtr control);
'@
    $handle = [RmuxFinalSink.Con]::GetStdHandle(-10)
    $mode = [uint32]0
    if (-not [RmuxFinalSink.Con]::GetConsoleMode($handle, [ref]$mode)) {{ throw 'GetConsoleMode failed' }}
    # Clear PROCESSED|LINE|ECHO, set VIRTUAL_TERMINAL_INPUT.
    $raw = ($mode -band (-bnot 0x7)) -bor 0x200
    if (-not [RmuxFinalSink.Con]::SetConsoleMode($handle, $raw)) {{ throw 'SetConsoleMode failed' }}
    {announce}
    [IO.File]::WriteAllText($ready, '1')
    $want = {units}
    $builder = New-Object System.Text.StringBuilder
    $buffer = New-Object char[] 4096
    while ($builder.Length -lt $want) {{
        $read = [uint32]0
        if (-not [RmuxFinalSink.Con]::ReadConsoleW($handle, $buffer, [uint32]$buffer.Length, [ref]$read, [IntPtr]::Zero)) {{
            throw 'ReadConsoleW failed'
        }}
        if ($read -eq 0) {{ throw "console input ended after $($builder.Length) of $want units" }}
        $null = $builder.Append($buffer, 0, $read)
    }}
    [IO.File]::WriteAllBytes($partial, [Text.Encoding]::UTF8.GetBytes($builder.ToString()))
    Move-Item -LiteralPath $partial -Destination $out
    $deadline = (Get-Date).AddSeconds({park})
    while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath $stop)) {{
        Start-Sleep -Milliseconds 50
    }}
}} catch {{
    [IO.File]::WriteAllText($errorFile, $_.Exception.Message)
    exit 1
}}
"#,
                ready = super::powershell_quote(&self.path(READY_FILE)),
                partial = super::powershell_quote(&self.path(OUT_PARTIAL_FILE)),
                out = super::powershell_quote(&self.path(OUT_FILE)),
                stop = super::powershell_quote(&self.path(STOP_FILE)),
                error_file = super::powershell_quote(&self.path(ERROR_FILE)),
                units = self.expected_code_units(),
                park = CHILD_PARK_SECONDS,
            )
        }
    }

    impl Drop for FinalSinkSlot {
        fn drop(&mut self) {
            let _ = fs::write(self.directory.join(STOP_FILE), b"1");
        }
    }

    /// Creates a detached session whose only pane is a real final-sink child.
    ///
    /// Lives here rather than in either test module because the active-path
    /// proofs sit under the private `handler::tests` tree and the deferred
    /// proof sits under `handler::session_support`; neither can see the other.
    pub(crate) async fn create_final_sink_session(
        handler: &crate::handler::RequestHandler,
        session: &rmux_proto::SessionName,
        slot: &FinalSinkSlot,
    ) {
        let created = handler
            .handle(rmux_proto::Request::NewSessionExt(Box::new(
                rmux_proto::NewSessionExtRequest {
                    session_name: Some(session.clone()),
                    // The child script carries absolute slot paths, so it needs
                    // neither a working directory nor environment plumbing.
                    working_directory: None,
                    detached: true,
                    size: Some(rmux_proto::TerminalSize { cols: 80, rows: 24 }),
                    environment: None,
                    group_target: None,
                    attach_if_exists: false,
                    detach_other_clients: false,
                    kill_other_clients: false,
                    flags: None,
                    window_name: None,
                    print_session_info: false,
                    print_format: None,
                    command: Some(slot.pane_command()),
                    process_command: None,
                    client_environment: None,
                    skip_environment_update: false,
                },
            )))
            .await;
        assert!(
            matches!(created, rmux_proto::Response::NewSession(_)),
            "unexpected new-session response: {created:?}"
        );
    }

    /// Adds a second final-sink pane to window 0 and returns its target.
    pub(crate) async fn split_final_sink_pane(
        handler: &crate::handler::RequestHandler,
        session: &rmux_proto::SessionName,
        slot: &FinalSinkSlot,
    ) -> rmux_proto::PaneTarget {
        let split = handler
            .handle(rmux_proto::Request::SplitWindowExt(Box::new(
                rmux_proto::SplitWindowExtRequest {
                    target: rmux_proto::SplitWindowTarget::Pane(rmux_proto::PaneTarget::new(
                        session.clone(),
                        0,
                    )),
                    direction: rmux_proto::SplitDirection::Horizontal,
                    before: false,
                    environment: None,
                    command: Some(slot.pane_command()),
                    process_command: None,
                    start_directory: None,
                    keep_alive_on_exit: None,
                    detached: false,
                    size: None,
                    preserve_zoom: false,
                    full_size: false,
                    stdin_payload: None,
                },
            )))
            .await;
        let rmux_proto::Response::SplitWindow(split) = split else {
            panic!("expected split-window response: {split:?}");
        };
        split.pane
    }
}
