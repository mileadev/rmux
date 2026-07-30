//! Builds the Windows final-sink pane child from pinned Rust source.
//!
//! The child cannot be this test binary re-executed: putting the pane console
//! into raw mode needs FFI and the crate is `#![forbid(unsafe_code)]`. The
//! previous child was therefore a PowerShell script that re-implemented the
//! historical probe's read boundary by hand — which is precisely what left the
//! R1 diagnostic ambiguous, because a hand-written `ReadConsoleW` loop has no
//! demonstrated equivalent for the standard library's request sizing,
//! incomplete-UTF-8 buffering, partial returns or unpaired-surrogate rejection.
//!
//! So the child is the real thing instead: [`CHILD_MAIN_SOURCE`] hands
//! `std::io::stdin().lock()` to [`super::byte_observer`], and both files are
//! written out and compiled with the workspace-pinned toolchain the first time a
//! slot needs a child. `OBSERVER_SOURCE` is `include_str!` of the very module
//! this test binary compiles and drives, so the observer the harness asserts on
//! and the observer the child runs are the same bytes by construction.
//!
//! The build is cached under the OS temporary directory, keyed by a digest of
//! those exact sources, and the sources plus the `rustc` identity are kept
//! beside the program so the later Windows 10 build-19045 A/B can prove it
//! rebuilt this observer and not another one.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const OBSERVER_FILE: &str = "byte_observer.rs";
const MAIN_FILE: &str = "main.rs";
const PROGRAM_FILE: &str = "rmux-final-sink-child.exe";
const TOOLCHAIN_FILE: &str = "toolchain.txt";

/// The observer module this test binary itself compiles.
pub(super) const OBSERVER_SOURCE: &str = include_str!("byte_observer.rs");

/// The child's entry point: console mode, the capability announcement, the
/// readiness signal, the historical read boundary, and the park/`done`
/// teardown handshake. Everything about the capture itself is delegated to the
/// observer module, which is why nothing here re-implements a console read.
pub(super) const CHILD_MAIN_SOURCE: &str = r##"#![allow(dead_code)]
//! The Windows final-sink pane child.
//!
//! Generated verbatim from
//! `crates/rmux-server/src/test_shell/final_sink/windows_byte_child.rs`; edit it
//! there. Its only read boundary is the historical probe's
//! `std::io::stdin().lock()` handed to the byte observer that the harness
//! asserting on this child compiles from the same source.

mod byte_observer;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

type Handle = isize;

#[link(name = "kernel32")]
extern "system" {
    fn GetStdHandle(which: i32) -> Handle;
    fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: Handle, mode: u32) -> i32;
}

const STD_INPUT_HANDLE: i32 = -10;
/// `ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT`: a cooked
/// console treats the paste's leading ESC as an editing command and rewrites
/// CR/LF, so the captured bytes would say nothing about the sink.
const COOKED_INPUT_FLAGS: u32 = 0x7;
/// `ENABLE_VIRTUAL_TERMINAL_INPUT`.
const VIRTUAL_TERMINAL_INPUT: u32 = 0x200;
const POLL_INTERVAL: Duration = Duration::from_millis(50);

struct Slot {
    ready: PathBuf,
    partial: PathBuf,
    out: PathBuf,
    error: PathBuf,
    stop: PathBuf,
    done: PathBuf,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [ready, partial, out, error, stop, done, want, awareness, park] = arguments.as_slice()
    else {
        eprintln!(
            "usage: rmux-final-sink-child <ready> <out.part> <out> <error> <stop> <done> \
             <bytes> <aware|unaware> <park-seconds>"
        );
        std::process::exit(2);
    };
    let slot = Slot {
        ready: PathBuf::from(ready),
        partial: PathBuf::from(partial),
        out: PathBuf::from(out),
        error: PathBuf::from(error),
        stop: PathBuf::from(stop),
        done: PathBuf::from(done),
    };
    let (Ok(want), Ok(park)) = (want.parse::<usize>(), park.parse::<u64>()) else {
        eprintln!("the expected byte count and park duration must both be numbers");
        std::process::exit(2);
    };

    let failure = capture(&slot, want, awareness == "aware").err();
    if let Some(message) = &failure {
        // Written before the park, so the harness reports the exact reason
        // instead of waiting out a capture boundary.
        let _ = std::fs::write(&slot.error, message.as_bytes());
    }
    park_until_stopped(&slot, park);
    // `done` is the only teardown acknowledgement and is written whether or not
    // the capture succeeded: teardown is signalled separately from success.
    let _ = std::fs::write(&slot.done, b"1");
    if failure.is_some() {
        std::process::exit(1);
    }
}

fn capture(slot: &Slot, want: usize, aware: bool) -> Result<(), String> {
    // Readiness is published only after raw mode is established. A child that
    // signalled first could be read in cooked mode, which would corrupt the
    // capture without ever reporting a setup failure.
    set_raw_console_input()?;
    if aware {
        print!("\u{1b}[?2004h");
        std::io::stdout()
            .flush()
            .map_err(|error| format!("the capability announcement failed: {error}"))?;
    }
    std::fs::write(&slot.ready, b"1")
        .map_err(|error| format!("readiness could not be signalled: {error}"))?;

    let mut stdin = std::io::stdin().lock();
    byte_observer::capture_to_slot(&mut stdin, want, &slot.partial, &slot.out)
        .map(|_| ())
        .map_err(|failure| failure.to_string())
}

fn set_raw_console_input() -> Result<(), String> {
    let mode = unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0_u32;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return Err("GetConsoleMode failed: standard input is not a console".to_owned());
        }
        let raw = (mode & !COOKED_INPUT_FLAGS) | VIRTUAL_TERMINAL_INPUT;
        if SetConsoleMode(handle, raw) == 0 {
            return Err(format!("SetConsoleMode({raw:#x}) failed"));
        }
        raw
    };
    let _ = mode;
    Ok(())
}

/// Stays alive after capturing so the harness can still resolve this pane as a
/// live destination while it asserts.
fn park_until_stopped(slot: &Slot, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline && !Path::new(&slot.stop).exists() {
        std::thread::sleep(POLL_INTERVAL);
    }
}
"##;

/// The compiled child, built once per process and shared by every slot.
pub(super) fn child_program() -> Result<PathBuf, String> {
    static PROGRAM: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    PROGRAM.get_or_init(compile_child_program).clone()
}

/// A stable digest of the exact child sources.
///
/// `RandomState` is reseeded per process and `DefaultHasher` is an
/// implementation detail, so this is an explicit FNV-1a. Carriage returns are
/// excluded so a checkout's line endings cannot change the identity a later
/// Windows 10 A/B compares against.
pub(super) fn source_digest() -> u64 {
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for byte in OBSERVER_SOURCE
        .bytes()
        .chain(CHILD_MAIN_SOURCE.bytes())
        .filter(|byte| *byte != b'\r')
    {
        digest ^= u64::from(byte);
        digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
    digest
}

fn compile_child_program() -> Result<PathBuf, String> {
    let directory =
        std::env::temp_dir().join(format!("rmux-final-sink-child-{:016x}", source_digest()));
    let program = directory.join(PROGRAM_FILE);
    if program.is_file() {
        return Ok(program);
    }

    // Compiled in a process-private staging directory: several test processes
    // can reach this at once, and only the final install is allowed to race.
    let staging = directory.join(format!("build-{}", std::process::id()));
    write_source_tree(&staging)?;
    let compiler = compiler_program();
    let built = staging.join(PROGRAM_FILE);
    let compiled = Command::new(&compiler)
        .arg("--edition")
        .arg("2021")
        .arg("--crate-name")
        .arg("rmux_final_sink_child")
        .arg("-o")
        .arg(&built)
        .arg(staging.join(MAIN_FILE))
        .output()
        .map_err(|error| {
            format!(
                "{} could not be run to build the final-sink child: {error}",
                compiler.to_string_lossy()
            )
        })?;
    if !compiled.status.success() {
        return Err(format!(
            "the pinned final-sink child source did not compile ({}):\n{}",
            compiled.status,
            String::from_utf8_lossy(&compiled.stderr)
        ));
    }

    // Kept beside the program: the Windows 10 A/B has to be able to show it
    // rebuilt this observer, with this toolchain, and not another one.
    let _ = fs::write(directory.join(OBSERVER_FILE), OBSERVER_SOURCE);
    let _ = fs::write(directory.join(MAIN_FILE), CHILD_MAIN_SOURCE);
    let _ = fs::write(
        directory.join(TOOLCHAIN_FILE),
        toolchain_identity(&compiler),
    );

    match fs::rename(&built, &program) {
        Ok(()) => {}
        // Another process installed the byte-identical program first.
        Err(_) if program.is_file() => {}
        Err(error) => {
            return Err(format!(
                "the compiled final-sink child could not be installed at {}: {error}",
                program.display()
            ))
        }
    }
    let _ = fs::remove_dir_all(&staging);
    Ok(program)
}

fn write_source_tree(staging: &std::path::Path) -> Result<(), String> {
    fs::create_dir_all(staging).map_err(|error| {
        format!(
            "the final-sink child build directory {} could not be created: {error}",
            staging.display()
        )
    })?;
    for (name, source) in [
        (OBSERVER_FILE, OBSERVER_SOURCE),
        (MAIN_FILE, CHILD_MAIN_SOURCE),
    ] {
        fs::write(staging.join(name), source).map_err(|error| {
            format!(
                "the final-sink child source {} could not be written: {error}",
                staging.join(name).display()
            )
        })?;
    }
    Ok(())
}

/// Cargo does not export `RUSTC` to a test binary, but the test's working
/// directory is inside the workspace, so the `rustup` shim resolves the
/// `rust-toolchain.toml` channel. An explicit `RUSTC` still wins.
fn compiler_program() -> OsString {
    std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"))
}

fn toolchain_identity(compiler: &OsStr) -> String {
    Command::new(compiler)
        .arg("--version")
        .arg("--verbose")
        .output()
        .map(|identity| String::from_utf8_lossy(&identity.stdout).into_owned())
        .unwrap_or_else(|error| format!("the compiler identity is unavailable: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the correction: the child must read through the
    /// historical standard-input byte boundary, not a hand-written console
    /// loop. This turns red the moment either half is replaced.
    #[test]
    fn the_child_reads_through_the_historical_standard_input_byte_boundary() {
        assert!(
            CHILD_MAIN_SOURCE.contains("let mut stdin = std::io::stdin().lock();"),
            "the child must lock standard input exactly as the historical probe did"
        );
        assert!(
            CHILD_MAIN_SOURCE.contains("byte_observer::capture_to_slot(&mut stdin, want,"),
            "the locked standard input must be the observer's reader"
        );
        assert!(
            OBSERVER_SOURCE.contains("let mut buffer = [0_u8; READ_BUFFER_BYTES];"),
            "the observer must read into the historical byte buffer"
        );
        assert!(
            OBSERVER_SOURCE.contains("pub(crate) const READ_BUFFER_BYTES: usize = 4096;"),
            "the historical buffer is 4096 bytes"
        );
        assert!(
            !CHILD_MAIN_SOURCE.contains("ReadConsoleW"),
            "re-emulating the console read is the construction this correction removes"
        );
    }

    /// The child's console handling is the only thing that may not be shared
    /// with this binary, so pin the mode it establishes and the order it
    /// establishes it in.
    #[test]
    fn the_child_establishes_raw_input_before_it_signals_readiness() {
        let raw_mode = CHILD_MAIN_SOURCE
            .find("set_raw_console_input()?")
            .expect("the child establishes raw console input");
        let readiness = CHILD_MAIN_SOURCE
            .find("std::fs::write(&slot.ready")
            .expect("the child signals readiness");
        assert!(
            raw_mode < readiness,
            "readiness must never be announced before raw mode succeeds"
        );
        assert!(CHILD_MAIN_SOURCE.contains("const COOKED_INPUT_FLAGS: u32 = 0x7;"));
        assert!(CHILD_MAIN_SOURCE.contains("const VIRTUAL_TERMINAL_INPUT: u32 = 0x200;"));
    }

    /// A digest that is not stable is not identity. This also fails loudly if
    /// the sources are ever loaded through a path that rewrites line endings.
    #[test]
    fn the_child_source_digest_is_stable_within_a_run() {
        assert_eq!(source_digest(), source_digest());
        assert!(!OBSERVER_SOURCE.contains('\r'));
        assert!(!CHILD_MAIN_SOURCE.contains('\r'));
    }

    /// The pinned source must actually build with the workspace toolchain;
    /// otherwise every final-sink proof fails for a reason unrelated to the
    /// sink under test.
    #[test]
    fn the_pinned_child_source_compiles_with_the_workspace_toolchain() {
        let program = child_program().unwrap_or_else(|failure| panic!("{failure}"));
        assert!(
            program.is_file(),
            "the compiled child is missing at {}",
            program.display()
        );
    }

    /// The child's own failure path: a standard input that is not a console
    /// must be reported through `error`, must not publish `out`, and must still
    /// acknowledge teardown through `done`.
    #[test]
    fn the_child_reports_a_setup_failure_and_still_acknowledges_teardown() {
        let program = child_program().unwrap_or_else(|failure| panic!("{failure}"));
        let directory = std::env::temp_dir().join(format!(
            "rmux-final-sink-child-setup-{}-{:016x}",
            std::process::id(),
            source_digest()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("create the scratch slot");
        let path = |file: &str| directory.join(file).display().to_string();
        // Pre-signalled, so the child leaves its park immediately.
        fs::write(directory.join("stop"), b"1").expect("stage the stop signal");

        let status = Command::new(&program)
            .args([
                path("ready"),
                path("out.part"),
                path("out"),
                path("error"),
                path("stop"),
                path("done"),
                "16".to_owned(),
                "aware".to_owned(),
                "5".to_owned(),
            ])
            // Never inherit this process's console: the child would put the
            // test runner's own input into raw mode.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run the compiled final-sink child");

        let reported = fs::read_to_string(directory.join("error")).unwrap_or_default();
        assert!(
            reported.contains("GetConsoleMode failed"),
            "the child must attribute its setup failure: {reported:?}"
        );
        assert!(
            !directory.join("ready").exists(),
            "readiness must not be signalled when raw mode was never established"
        );
        assert!(
            !directory.join("out").exists(),
            "a failed child must never publish a capture"
        );
        assert!(
            directory.join("done").is_file(),
            "teardown must be acknowledged even when the capture failed"
        );
        assert_eq!(
            status.code(),
            Some(1),
            "a failed capture must exit non-zero"
        );
        let _ = fs::remove_dir_all(&directory);
    }
}
