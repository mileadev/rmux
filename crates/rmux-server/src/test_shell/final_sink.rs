//! A real pane child that persists the bytes its application actually read.
//!
//! Every other bracketed-paste suite in this crate observes
//! `RawPaneInputProbe`, which short-circuits
//! `prepare_pane_input_write_with_encoding` to `PaneInputSink::CapturedForTest`
//! *before* the starting-pane queue and *before* the Windows passthrough/legacy
//! sink selection. Those probes stay green when the legacy console sink is
//! bypassed, so they cannot close the `AttachInput::bytes -> ... -> real child
//! application bytes` obligation.
//!
//! A slot is a directory the harness and the child exchange files through:
//!
//! | File       | Written by | Meaning                                        |
//! |------------|------------|------------------------------------------------|
//! | `ready`    | child      | terminal is raw and the read is starting        |
//! | `out.part` | child      | the capture so far, grown as bytes arrive       |
//! | `out`      | child      | the complete application-side capture           |
//! | `error`    | child      | a step failed, with the reason that step gives  |
//! | `stop`     | harness    | the child may leave its keep-alive park         |
//! | `done`     | child      | the child left its park, so teardown is over    |
//!
//! `out` means *complete*: both children create it only by renaming an
//! `out.part` that already holds exactly the expected byte count, and any
//! failure keeps `out.part` and writes `error` instead. A reported `error`
//! therefore outranks everything the harness might otherwise infer from a
//! file's presence.
//!
//! `out.part` is what makes a mutation red readable. A mutation that routes a
//! bracketed paste back through the delimiter-consuming legacy path leaves the
//! child twelve bytes short forever; without a growing partial file the only
//! observable would be a timeout indistinguishable from reader starvation.
//! With one, the failure names the exact bytes that arrived and the first
//! offset at which they diverge.
//!
//! The child must read from a *raw* terminal: a cooked console or PTY line
//! discipline treats the paste's leading `ESC` as an editing command and
//! rewrites CR/LF, so the captured bytes would say nothing about the sink. The
//! crate is `#![forbid(unsafe_code)]`, so the child cannot be this test binary
//! re-executed — the Windows console-mode change needs FFI.
//!
//! Unix therefore uses `/bin/sh`, which [`super`] already exists to build, and
//! Windows uses a small pinned Rust program: its read boundary is the whole
//! point of the R1 diagnostic, so it is the historical
//! `stdin().lock().read(&mut [0_u8; 4096])` itself rather than an emulation of
//! it. See [`byte_observer`] and [`windows_byte_child`].

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The historical standard-input byte boundary the Windows child reads through,
/// compiled both into this binary and into that child.
///
/// Explicitly pathed, like this module itself: a `#[path]`-loaded module owns
/// the directory its declaration names, not one named after itself.
#[path = "final_sink/byte_observer.rs"]
mod byte_observer;
/// The Unix child's capture protocol. Generated on every platform so its shape
/// is checked wherever this crate's tests run, executed only on Unix.
#[path = "final_sink/unix_child.rs"]
mod unix_child;
#[cfg(windows)]
#[path = "final_sink/windows_byte_child.rs"]
mod windows_byte_child;

const READY_FILE: &str = "ready";
const OUT_FILE: &str = "out";
const OUT_PARTIAL_FILE: &str = "out.part";
const ERROR_FILE: &str = "error";
const STOP_FILE: &str = "stop";
const DONE_FILE: &str = "done";

/// Every artifact a child or the harness may leave in a slot. Finding one at
/// construction time means the directory belongs to an earlier run.
const SLOT_ARTIFACTS: [&str; 6] = [
    READY_FILE,
    OUT_PARTIAL_FILE,
    OUT_FILE,
    ERROR_FILE,
    STOP_FILE,
    DONE_FILE,
];

/// How long the child stays alive after capturing, so the harness can still
/// resolve it as a live synchronized destination while it asserts.
const CHILD_PARK_SECONDS: u64 = 120;
/// Generous: a pane child starts a process and a terminal before it can read.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(60);
/// Once at least one byte has arrived, a gap this long with no further byte
/// means the sink has stopped delivering.
///
/// This never decides a pass. A complete capture is reported by the child
/// through `out`, so the success oracle is always the byte count, never elapsed
/// time. The gap only bounds how long an already-failing capture is waited on,
/// which is what lets a delimiter-consuming mutation report its short body
/// promptly instead of after the full timeout.
const CAPTURE_IDLE_GAP: Duration = Duration::from_secs(8);
/// Teardown is bounded: a child that will not leave its park must not hang the
/// suite, and its slot is kept for inspection instead.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Escaped byte dumps stay readable; elision is always stated, never silent.
const MAX_ESCAPED_BYTES: usize = 512;

pub(crate) struct FinalSinkSlot {
    directory: PathBuf,
    expected: Vec<u8>,
    bracket_aware: bool,
    capture_timeout: Duration,
    capture_idle_gap: Duration,
}

impl FinalSinkSlot {
    /// `expected` is the exact byte sequence the child must read. It must be
    /// valid UTF-8: the Windows child converts the console's UTF-16 to UTF-8
    /// itself, and arbitrary non-UTF-8 bytes are a separate policy that this
    /// harness deliberately does not exercise.
    pub(crate) fn new(label: &str, expected: &[u8], bracket_aware: bool) -> Self {
        Self::create(fresh_slot_directory(label), expected, bracket_aware)
            .unwrap_or_else(|failure| panic!("{failure}"))
    }

    /// Fallible so the stale-slot regression can assert the exact rejection.
    fn create(directory: PathBuf, expected: &[u8], bracket_aware: bool) -> Result<Self, String> {
        assert!(
            std::str::from_utf8(expected).is_ok(),
            "the final-sink harness only covers the valid-UTF-8 #92 path"
        );
        // Exclusive, not `create_dir_all`: a slot must never inherit another
        // child's `ready` or `out`. Readiness and output are file existence
        // checks, so an adopted directory would let an abandoned child satisfy
        // this run before its own child captured anything.
        if let Err(error) = fs::create_dir(&directory) {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                let stale = SLOT_ARTIFACTS
                    .iter()
                    .filter(|artifact| directory.join(artifact).exists())
                    .copied()
                    .collect::<Vec<_>>();
                return Err(format!(
                    "final-sink slot {} already exists and cannot be adopted; \
                     evidence left by an earlier child: {stale:?}",
                    directory.display()
                ));
            }
            return Err(format!(
                "final-sink slot {} could not be created: {error}",
                directory.display()
            ));
        }
        Ok(Self {
            directory,
            expected: expected.to_vec(),
            bracket_aware,
            capture_timeout: CAPTURE_TIMEOUT,
            capture_idle_gap: CAPTURE_IDLE_GAP,
        })
    }

    /// Shortens the diagnostic bounds so a harness regression can reach them
    /// without waiting out the real ones.
    fn with_capture_bounds(mut self, timeout: Duration, idle_gap: Duration) -> Self {
        self.capture_timeout = timeout;
        self.capture_idle_gap = idle_gap;
        self
    }

    fn path(&self, file: &str) -> String {
        self.directory.join(file).display().to_string()
    }

    pub(crate) fn wait_until_ready(&self) {
        self.try_wait_until_ready()
            .unwrap_or_else(|failure| panic!("{failure}"));
    }

    fn try_wait_until_ready(&self) -> Result<(), String> {
        let deadline = Instant::now() + self.capture_timeout;
        loop {
            if self.directory.join(READY_FILE).is_file() {
                return Ok(());
            }
            if let Some(error) = self.child_error() {
                return Err(
                    self.describe("the child failed before signalling readiness", Some(error))
                );
            }
            if Instant::now() >= deadline {
                return Err(self.describe(
                    &format!(
                        "the child never signalled readiness within {:?}",
                        self.capture_timeout
                    ),
                    None,
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Asserts that the child read exactly the bytes this slot was built for.
    ///
    /// Every final-sink proof compares against its own `expected`, so routing
    /// the comparison through the slot gives every red — short capture, wrong
    /// delimiters, timeout — the same unambiguous representation.
    pub(crate) fn assert_application_bytes(&self, context: &str) {
        match self.try_application_bytes() {
            Ok(received) if received == self.expected => {}
            Ok(received) => panic!(
                "{}",
                self.describe_against(
                    &format!("{context}: the child read the wrong bytes"),
                    &received,
                    None
                )
            ),
            Err(failure) => panic!("{context}: {failure}"),
        }
    }

    /// Blocks until the child persisted its complete capture, or until the sink
    /// demonstrably stopped delivering, and reports exact bytes either way.
    fn try_application_bytes(&self) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + self.capture_timeout;
        let mut partial_len = self.partial_bytes().len();
        let mut partial_grew_at = Instant::now();
        loop {
            // A reported failure outranks a published capture. Both children
            // now publish `out` only after an exact capture, so the two are
            // mutually exclusive; checking `out` first is what let a script
            // that renamed a short partial be read as a completed publication
            // and reported as merely the wrong bytes with `child error: none`.
            if let Some(error) = self.child_error() {
                return Err(self.describe(
                    "the child failed before persisting its capture",
                    Some(error),
                ));
            }
            if self.directory.join(OUT_FILE).is_file() {
                return fs::read(self.directory.join(OUT_FILE)).map_err(|error| {
                    self.describe(
                        &format!("the child's capture could not be read: {error}"),
                        None,
                    )
                });
            }
            let now = Instant::now();
            let current = self.partial_bytes().len();
            if current != partial_len {
                partial_len = current;
                partial_grew_at = now;
            }
            if partial_len > 0 && now.duration_since(partial_grew_at) >= self.capture_idle_gap {
                return Err(self.describe(
                    &format!(
                        "the child received no further byte for {:?} while its capture was still short",
                        self.capture_idle_gap
                    ),
                    None,
                ));
            }
            if now >= deadline {
                return Err(self.describe(
                    &format!(
                        "the child never persisted its capture within {:?}",
                        self.capture_timeout
                    ),
                    None,
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// The bytes the child has received so far. `dd` on Unix and the Windows
    /// child both grow `out.part` as input arrives, so this is the real
    /// application-side prefix, not a reconstruction.
    fn partial_bytes(&self) -> Vec<u8> {
        fs::read(self.directory.join(OUT_PARTIAL_FILE)).unwrap_or_default()
    }

    fn child_error(&self) -> Option<String> {
        fs::read_to_string(self.directory.join(ERROR_FILE)).ok()
    }

    fn describe(&self, headline: &str, child_error: Option<String>) -> String {
        let received = self.partial_bytes();
        self.describe_against(headline, &received, child_error)
    }

    /// One representation of every boundary a final-sink failure can hit, so a
    /// red names the delimiter or body difference rather than only a deadline.
    fn describe_against(
        &self,
        headline: &str,
        received: &[u8],
        child_error: Option<String>,
    ) -> String {
        let mut report = format!(
            "final-sink capture failed: {headline}\n  \
             slot: {}\n  \
             expected {} bytes: {}\n  \
             received {} bytes: {}",
            self.directory.display(),
            self.expected.len(),
            escape_bytes(&self.expected),
            received.len(),
            escape_bytes(received),
        );
        match first_difference(&self.expected, received) {
            Some(offset) => report.push_str(&format!(
                "\n  first difference at byte {offset}: expected {}, received {}",
                describe_byte(self.expected.get(offset).copied()),
                describe_byte(received.get(offset).copied()),
            )),
            None if received.len() < self.expected.len() => report.push_str(&format!(
                "\n  the received bytes are an exact prefix; {} byte(s) never arrived",
                self.expected.len() - received.len()
            )),
            None if received.len() > self.expected.len() => report.push_str(&format!(
                "\n  the expected bytes are an exact prefix; {} extra byte(s) arrived",
                received.len() - self.expected.len()
            )),
            None => report.push_str("\n  the bytes match"),
        }
        report.push_str(&format!(
            "\n  child error: {}\n  ready: {}",
            child_error.as_deref().unwrap_or("none"),
            self.directory.join(READY_FILE).is_file(),
        ));
        report
    }

    /// Runs the checked `/bin/sh` capture protocol. `dd bs=1` writes each byte
    /// to `out.part` as it arrives, so a capture that never completes still
    /// exposes exactly what reached the child.
    #[cfg(unix)]
    pub(crate) fn pane_command(&self) -> Vec<String> {
        unix_child::pane_command(self)
    }

    /// Runs the pinned Rust child, whose read boundary is the historical
    /// `stdin().lock().read(&mut [0_u8; 4096])`.
    ///
    /// The slot files, expected byte count, awareness and park duration are
    /// arguments rather than an interpolated script, so nothing about the
    /// child's source varies between runs — which is what lets the Windows 10
    /// A/B rebuild exactly this observer.
    #[cfg(windows)]
    pub(crate) fn pane_command(&self) -> Vec<String> {
        let program = windows_byte_child::child_program().unwrap_or_else(|failure| {
            panic!("the final-sink Windows child is unavailable: {failure}")
        });
        vec![
            program.display().to_string(),
            self.path(READY_FILE),
            self.path(OUT_PARTIAL_FILE),
            self.path(OUT_FILE),
            self.path(ERROR_FILE),
            self.path(STOP_FILE),
            self.path(DONE_FILE),
            self.expected.len().to_string(),
            if self.bracket_aware {
                "aware"
            } else {
                "unaware"
            }
            .to_owned(),
            CHILD_PARK_SECONDS.to_string(),
        ]
    }
}

impl Drop for FinalSinkSlot {
    fn drop(&mut self) {
        let _ = fs::write(self.directory.join(STOP_FILE), b"1");
        self.await_bounded_teardown();
        if std::thread::panicking() {
            // The failing assertion's diagnosis needs these artifacts.
            eprintln!(
                "final-sink slot preserved for diagnosis: {}",
                self.directory.display()
            );
            return;
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

impl FinalSinkSlot {
    /// Waits, bounded, for the child to acknowledge that it left its park.
    ///
    /// A child that never started cannot acknowledge anything, so this only
    /// waits once readiness was observed. Slot paths are unique per instance,
    /// so a child that outlives this wait can still never satisfy a later run;
    /// the wait exists so teardown is observed rather than assumed.
    fn await_bounded_teardown(&self) {
        if !self.directory.join(READY_FILE).is_file() {
            return;
        }
        let deadline = Instant::now() + TEARDOWN_TIMEOUT;
        while Instant::now() < deadline {
            if self.directory.join(DONE_FILE).is_file() || self.child_error().is_some() {
                return;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

/// A slot path no earlier run can reproduce.
///
/// The process id and a process-local counter are not enough: after an
/// interrupted run the operating system can reuse the id while the counter
/// restarts at zero, so a later invocation with the same label would land on an
/// abandoned directory. `RandomState` is seeded by the standard library from
/// the operating system, so the token differs between processes that share an
/// id.
fn fresh_slot_directory(label: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.write_u32(NEXT.fetch_add(1, Ordering::Relaxed));
    std::env::temp_dir().join(format!(
        "rmux-final-sink-{}-{:016x}-{label}",
        std::process::id(),
        hasher.finish()
    ))
}

/// The first offset at which the two sequences disagree, or `None` when one is
/// a prefix of the other.
fn first_difference(expected: &[u8], received: &[u8]) -> Option<usize> {
    expected
        .iter()
        .zip(received)
        .position(|(expected, received)| expected != received)
}

fn describe_byte(byte: Option<u8>) -> String {
    byte.map_or_else(|| "nothing".to_owned(), |byte| format!("0x{byte:02x}"))
}

/// Renders bytes so `ESC[200~` and a raw `0x02` are both legible. Elision is
/// always announced with its exact size.
fn escape_bytes(bytes: &[u8]) -> String {
    let head = bytes.len().min(MAX_ESCAPED_BYTES);
    let mut escaped = String::with_capacity(head + 16);
    for &byte in &bytes[..head] {
        match byte {
            b'\\' => escaped.push_str(r"\\"),
            0x20..=0x7e => escaped.push(byte as char),
            _ => escaped.push_str(&format!("\\x{byte:02x}")),
        }
    }
    if bytes.len() > head {
        escaped.push_str(&format!("... [{} more byte(s)]", bytes.len() - head));
    }
    escaped
}

/// Creates a detached session whose only pane is a real final-sink child.
///
/// Lives here rather than in either test module because the active-path proofs
/// sit under the private `handler::tests` tree and the deferred proof sits under
/// `handler::session_support`; neither can see the other.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot must not adopt a directory an earlier child left behind: it would
    /// accept that child's `ready` and `out` as this run's evidence.
    #[test]
    fn a_slot_left_by_an_earlier_child_is_never_adopted() {
        let directory = fresh_slot_directory("stale-rejection");
        fs::create_dir(&directory).expect("stage the abandoned slot");
        fs::write(directory.join(READY_FILE), b"1").expect("stage stale readiness");
        fs::write(directory.join(OUT_FILE), b"stale").expect("stage stale capture");

        let Err(failure) = FinalSinkSlot::create(directory.clone(), b"fresh", true) else {
            panic!("an existing slot must be rejected, never adopted");
        };

        assert!(
            failure.contains("already exists"),
            "unexpected rejection: {failure}"
        );
        assert!(
            failure.contains(READY_FILE) && failure.contains(OUT_FILE),
            "the rejection must name the stale evidence: {failure}"
        );
        assert_eq!(
            fs::read(directory.join(OUT_FILE)).expect("stale capture survives"),
            b"stale",
            "rejecting a slot must not destroy the evidence in it"
        );
        fs::remove_dir_all(&directory).expect("clean up the staged slot");
    }

    /// A delimiter-consuming sink leaves the child short forever. The failure
    /// must name the bytes that did arrive, not only that time ran out.
    #[test]
    fn a_short_capture_reports_the_exact_bytes_the_child_received() {
        let expected = b"\x1b[200~body\x1b[201~";
        let slot = FinalSinkSlot::new("short-capture", expected, true)
            .with_capture_bounds(Duration::from_secs(30), Duration::from_millis(200));
        // Exactly what the legacy console path leaves behind: the body with
        // both six-byte delimiters consumed.
        fs::write(slot.directory.join(OUT_PARTIAL_FILE), b"body").expect("stage the partial");

        let failure = slot
            .try_application_bytes()
            .expect_err("a capture that stays short must fail");

        assert!(
            failure.contains("no further byte for 200ms"),
            "the idle boundary must be named: {failure}"
        );
        assert!(
            failure.contains(r"expected 16 bytes: \x1b[200~body\x1b[201~"),
            "the expected bytes must be shown: {failure}"
        );
        assert!(
            failure.contains("received 4 bytes: body"),
            "the received bytes must be shown: {failure}"
        );
        assert!(
            failure.contains("first difference at byte 0: expected 0x1b, received 0x62"),
            "the divergence must be located: {failure}"
        );
    }

    /// A capture that is merely truncated is a different diagnosis from one
    /// whose bytes disagree, and must read as such.
    #[test]
    fn a_truncated_capture_is_reported_as_an_exact_prefix() {
        let slot = FinalSinkSlot::new("truncated-capture", b"abcdef", false)
            .with_capture_bounds(Duration::from_secs(30), Duration::from_millis(200));
        fs::write(slot.directory.join(OUT_PARTIAL_FILE), b"abc").expect("stage the partial");

        let failure = slot
            .try_application_bytes()
            .expect_err("a capture that stays short must fail");

        assert!(
            failure.contains("the received bytes are an exact prefix; 3 byte(s) never arrived"),
            "a truncation must not be reported as a mismatch: {failure}"
        );
    }

    /// A child that reported a failure must be diagnosed by that reason, not by
    /// whichever slot file the harness happens to look at first. A script that
    /// renamed a short partial used to be read as a completed publication and
    /// reported as merely the wrong bytes with `child error: none`.
    #[test]
    fn a_reported_child_failure_outranks_a_published_capture() {
        let slot = FinalSinkSlot::new("error-precedence", b"abcdef", false);
        fs::write(slot.directory.join(OUT_PARTIAL_FILE), b"abc").expect("stage the partial");
        fs::write(slot.directory.join(OUT_FILE), b"abc").expect("stage a published capture");
        fs::write(
            slot.directory.join(ERROR_FILE),
            b"standard input ended after 3 of 6 bytes",
        )
        .expect("stage the child's own reason");

        let failure = slot
            .try_application_bytes()
            .expect_err("a reported failure must not be read as a completed capture");

        assert!(
            failure.contains("the child failed before persisting its capture"),
            "the reported failure must be the diagnosis: {failure}"
        );
        assert!(
            failure.contains("child error: standard input ended after 3 of 6 bytes"),
            "the child's own reason must be carried through: {failure}"
        );
    }

    #[test]
    fn escaped_bytes_stay_legible_and_announce_every_elision() {
        assert_eq!(escape_bytes(b"\x1b[200~a\\b\x02"), r"\x1b[200~a\\b\x02");
        let long = vec![b'a'; MAX_ESCAPED_BYTES + 3];
        assert!(escape_bytes(&long).ends_with("... [3 more byte(s)]"));
    }
}
