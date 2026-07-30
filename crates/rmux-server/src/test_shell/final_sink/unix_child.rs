//! The Unix final-sink pane child, and the protocol it owes the harness.
//!
//! The previous script was a straight line with no checked status:
//!
//! ```sh
//! stty raw -echo
//! : > ready
//! dd bs=1 count=N of=out.part 2>/dev/null
//! mv out.part out
//! ```
//!
//! `dd bs=1 count=N` treats end of input before `N` as normal end of input, not
//! as an error, so a short capture left a short `out.part` and the
//! unconditional `mv` published it as `out` — the slot protocol's word for a
//! *complete* capture. The harness then reported "wrong bytes" with
//! `child error: none`, which reads like a sink that delivered the wrong thing
//! rather than an input that ended early. A failed `stty` was equally invisible:
//! readiness was announced anyway, so the capture could be taken from a cooked
//! terminal that rewrites CR/LF and eats the paste's leading `ESC`. A failed
//! `mv` simply left no `out`, and the run ended at a timeout boundary that
//! never named the command that failed.
//!
//! The corrected script checks every step separately — raw setup, the
//! announcement, readiness, the read, the exact size and the publication — and
//! each failure writes an attributable `error`, keeps whatever `out.part`
//! holds, and still parks and acknowledges teardown through `done`. `out` is
//! created only by renaming a partial file that already holds exactly `N`
//! bytes.
//!
//! The generator is platform-neutral so the shape of that protocol is checked
//! wherever this crate's tests run. Executing the child is a separate matter:
//! the `#[cfg(unix)]` cases at the bottom run `/bin/sh` for real, and they are
//! the ones that carry the authority.

/// The real child's raw-mode setup. A regression pins that the shipped script
/// uses exactly this, so the substitutable seam below cannot leak into it.
const RAW_SETUP_COMMAND: &str = "stty raw -echo 2>/dev/null";

/// Runs the corrected capture protocol in the platform's own shell.
#[cfg(unix)]
pub(super) fn pane_command(slot: &super::FinalSinkSlot) -> Vec<String> {
    vec!["/bin/sh".to_owned(), "-c".to_owned(), script(slot)]
}

/// The script the real child runs.
pub(super) fn script(slot: &super::FinalSinkSlot) -> String {
    script_with_raw_setup(slot, RAW_SETUP_COMMAND)
}

/// The same script with a substitutable raw-mode step.
///
/// `stty raw -echo` needs a terminal, so a regression that drives the child
/// through a pipe cannot reach the read, size and publication boundaries
/// without replacing it. Only the tests below pass anything but
/// [`RAW_SETUP_COMMAND`].
pub(super) fn script_with_raw_setup(slot: &super::FinalSinkSlot, raw_setup: &str) -> String {
    let quote = crate::test_shell::command_quote;
    let announce = if slot.bracket_aware {
        r"printf '\033[?2004h'"
    } else {
        ":"
    };
    format!(
        "set -u\n\
         captured() {{\n\
         if [ -e {partial} ]; then\n\
         printf '%s' \"$(($(wc -c < {partial})))\"\n\
         else\n\
         printf '0'\n\
         fi\n\
         }}\n\
         park() {{\n\
         i=0\n\
         while [ \"$i\" -lt {ticks} ] && [ ! -e {stop} ]; do\n\
         sleep 0.05\n\
         i=$((i+1))\n\
         done\n\
         }}\n\
         fail() {{\n\
         printf '%s' \"$1\" > {error}\n\
         park\n\
         printf '' > {done}\n\
         exit 1\n\
         }}\n\
         {raw_setup} || fail 'raw mode could not be established: stty raw -echo failed'\n\
         {announce} || fail 'the capability announcement could not be written'\n\
         printf '' > {ready} || fail 'readiness could not be signalled'\n\
         dd bs=1 count={count} of={partial} 2>/dev/null || \
         fail \"the capture read failed after $(captured) of {count} bytes\"\n\
         [ \"$(captured)\" -eq {count} ] || \
         fail \"standard input ended after $(captured) of {count} bytes\"\n\
         mv {partial} {out} || fail 'the exact capture could not be published'\n\
         park\n\
         printf '' > {done}\n",
        ready = quote(&slot.path(super::READY_FILE)),
        partial = quote(&slot.path(super::OUT_PARTIAL_FILE)),
        out = quote(&slot.path(super::OUT_FILE)),
        error = quote(&slot.path(super::ERROR_FILE)),
        stop = quote(&slot.path(super::STOP_FILE)),
        done = quote(&slot.path(super::DONE_FILE)),
        count = slot.expected.len(),
        ticks = super::CHILD_PARK_SECONDS * 20,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_shell::final_sink::{
        FinalSinkSlot, DONE_FILE, ERROR_FILE, OUT_FILE, OUT_PARTIAL_FILE, READY_FILE, STOP_FILE,
    };

    /// A slot file exactly as it appears in the script.
    ///
    /// The quoted form is what distinguishes `'…/out'` from `'…/out.part'`, of
    /// which the first is a prefix.
    fn quoted(slot: &FinalSinkSlot, file: &str) -> String {
        crate::test_shell::command_quote(&slot.path(file))
    }

    /// The one line that runs `needle`, so an assertion cannot accidentally
    /// match the same text inside the failure path.
    fn line_running<'script>(script: &'script str, needle: &str) -> &'script str {
        let mut matches = script.lines().filter(|line| line.contains(needle));
        let line = matches
            .next()
            .unwrap_or_else(|| panic!("the script must run {needle:?}:\n{script}"));
        assert!(
            matches.next().is_none(),
            "{needle:?} must appear on exactly one line:\n{script}"
        );
        line
    }

    fn assert_guarded(script: &str, needle: &str, reason: &str) {
        let line = line_running(script, needle);
        assert!(line.contains("|| fail"), "{needle:?} is unchecked:\n{line}");
        assert!(
            line.contains(reason),
            "the {needle:?} guard must name {reason:?}:\n{line}"
        );
    }

    /// The finding itself: the old script checked nothing, so any of these
    /// steps could fail while the child carried on to `done`.
    #[test]
    fn raw_setup_the_read_the_exact_size_and_publication_are_each_checked_separately() {
        let slot = FinalSinkSlot::new("unix-guards", b"payload", true);
        let script = script(&slot);

        assert_guarded(
            &script,
            "stty raw -echo",
            "raw mode could not be established",
        );
        assert_guarded(
            &script,
            r"printf '\033[?2004h'",
            "the capability announcement could not be written",
        );
        assert_guarded(
            &script,
            &quoted(&slot, READY_FILE),
            "readiness could not be signalled",
        );
        assert_guarded(&script, "dd bs=1 count=7", "the capture read failed after");
        assert_guarded(
            &script,
            r#"[ "$(captured)" -eq 7 ]"#,
            "standard input ended after",
        );
        assert_guarded(
            &script,
            &quoted(&slot, OUT_FILE),
            "the exact capture could not be published",
        );
    }

    /// Order is the protocol. Readiness before raw mode lets the capture be
    /// taken from a cooked terminal; publication before the size check
    /// publishes a short capture as a complete one.
    #[test]
    fn setup_precedes_readiness_and_the_size_check_precedes_publication() {
        let slot = FinalSinkSlot::new("unix-order", b"0123456789", false);
        let script = script(&slot);
        let at = |needle: &str| {
            script
                .find(line_running(&script, needle))
                .expect("the line came from this script")
        };

        let raw_setup = at("stty raw -echo");
        let readiness = at(&quoted(&slot, READY_FILE));
        let read = at("dd bs=1 count=10");
        let size_check = at(r#"[ "$(captured)" -eq 10 ]"#);
        let publication = at(&quoted(&slot, OUT_FILE));

        assert!(
            raw_setup < readiness,
            "raw mode precedes readiness:\n{script}"
        );
        assert!(readiness < read, "readiness precedes the read:\n{script}");
        assert!(
            read < size_check,
            "the read precedes its size check:\n{script}"
        );
        assert!(
            size_check < publication,
            "`out` must never be published before the exact size is proved:\n{script}"
        );
    }

    /// `out` is the harness's word for a complete capture, so exactly one
    /// command may create it and it must be the rename of an exact partial.
    #[test]
    fn out_is_created_only_by_renaming_the_partial() {
        let slot = FinalSinkSlot::new("unix-publication", b"abc", true);
        let script = script(&slot);

        let publication = line_running(&script, &quoted(&slot, OUT_FILE));
        assert!(
            publication.starts_with(&format!(
                "mv {} {}",
                quoted(&slot, OUT_PARTIAL_FILE),
                quoted(&slot, OUT_FILE)
            )),
            "`out` may only be created by renaming the partial:\n{publication}"
        );
    }

    /// A failure must keep the evidence and still finish teardown: the harness
    /// reads `out.part` to say what did arrive, and waits for `done`.
    #[test]
    fn every_failure_keeps_the_partial_capture_and_still_acknowledges_teardown() {
        let slot = FinalSinkSlot::new("unix-failure-path", b"abc", true);
        let script = script(&slot);

        let failure_path = script
            .split_once("fail() {\n")
            .expect("the script defines a failure path")
            .1
            .split_once("\n}\n")
            .expect("the failure path is a shell function")
            .0;

        assert!(
            failure_path.contains(&quoted(&slot, ERROR_FILE)),
            "a failure must write `error`:\n{failure_path}"
        );
        assert!(
            failure_path.contains("\npark\n"),
            "a failure must still park so the pane stays resolvable:\n{failure_path}"
        );
        assert!(
            failure_path.contains(&quoted(&slot, DONE_FILE)),
            "teardown must be acknowledged even when the capture failed:\n{failure_path}"
        );
        assert!(
            !failure_path.contains(&quoted(&slot, OUT_FILE)),
            "a failure must never publish `out`:\n{failure_path}"
        );
        assert!(
            !script.contains(&format!("rm {}", quoted(&slot, OUT_PARTIAL_FILE))),
            "the partial capture is the evidence and is never removed:\n{script}"
        );
    }

    /// The substitutable raw-mode step is a regression seam and must never be
    /// what the real child runs.
    #[test]
    fn the_real_child_always_establishes_raw_mode_with_stty() {
        let slot = FinalSinkSlot::new("unix-raw-setup", b"abc", true);
        assert_eq!(RAW_SETUP_COMMAND, "stty raw -echo 2>/dev/null");
        assert!(script(&slot).starts_with("set -u\n"));
        assert!(script(&slot).contains(&format!("\n{RAW_SETUP_COMMAND} || fail ")));
    }

    /// Teardown is signalled separately from capture success: both paths park
    /// on the same `stop` file and both acknowledge with `done`.
    #[test]
    fn teardown_is_signalled_by_stop_on_both_paths() {
        let slot = FinalSinkSlot::new("unix-teardown", b"abc", false);
        let script = script(&slot);

        assert!(line_running(&script, &quoted(&slot, STOP_FILE)).contains("! -e"));
        assert_eq!(
            script.matches("\npark\n").count(),
            2,
            "both the failure path and the success path park:\n{script}"
        );
        assert_eq!(
            script.matches(&quoted(&slot, DONE_FILE)).count(),
            2,
            "both paths acknowledge teardown:\n{script}"
        );
    }

    /// Slot paths reach the shell as one argument whatever they contain.
    #[test]
    fn slot_paths_are_quoted_for_the_shell() {
        assert_eq!(crate::test_shell::command_quote("a'b"), r"'a'\''b'");
    }

    // -----------------------------------------------------------------------
    // Real `/bin/sh` execution.
    //
    // These carry the authority for this correction and cannot run on Windows,
    // so this attempt does not claim them; the targeted Unix job does.
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    mod execution {
        use super::*;
        use std::io::Write;
        use std::process::{Command, Stdio};

        /// Runs the script with `input` on standard input and waits for it.
        ///
        /// `stop` is pre-signalled so the park is immediate. A pipe is not a
        /// terminal, so cases that need to reach the read replace the raw-mode
        /// step; the case that exercises raw-mode failure keeps the real one.
        fn run(slot: &FinalSinkSlot, input: &[u8], raw_setup: &str) -> std::process::ExitStatus {
            std::fs::write(slot.directory.join(STOP_FILE), b"1").expect("pre-signal `stop`");
            let mut child = Command::new("/bin/sh")
                .arg("-c")
                .arg(script_with_raw_setup(slot, raw_setup))
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn the final-sink child script");
            child
                .stdin
                .take()
                .expect("the child has a standard input")
                .write_all(input)
                .expect("write the payload");
            child.wait().expect("wait for the child script")
        }

        fn slot_file(slot: &FinalSinkSlot, file: &str) -> Option<Vec<u8>> {
            std::fs::read(slot.directory.join(file)).ok()
        }

        #[test]
        fn an_exact_capture_is_published_and_teardown_is_acknowledged() {
            let payload = "\u{1b}[200~alpha\r\nβ 😀\u{1b}[201~".as_bytes();
            let slot = FinalSinkSlot::new("unix-exact", payload, true);

            let status = run(&slot, payload, "true");

            assert!(status.success(), "an exact capture must succeed");
            assert_eq!(
                slot_file(&slot, OUT_FILE).as_deref(),
                Some(payload),
                "the published capture must be byte-exact"
            );
            assert!(
                slot_file(&slot, OUT_PARTIAL_FILE).is_none(),
                "publication renames the partial rather than copying it"
            );
            assert!(slot_file(&slot, ERROR_FILE).is_none());
            assert!(slot_file(&slot, DONE_FILE).is_some());
        }

        /// The exact defect: `dd` exits successfully on a short read, so only
        /// the size check can stop a short capture from becoming `out`.
        #[test]
        fn end_of_input_before_the_expected_count_never_publishes_out() {
            let slot = FinalSinkSlot::new("unix-short-eof", b"0123456789", false);

            let status = run(&slot, b"01234", "true");

            assert_eq!(status.code(), Some(1), "a short capture must fail");
            assert_eq!(
                slot_file(&slot, ERROR_FILE)
                    .map(|reason| String::from_utf8(reason).expect("the reason is text")),
                Some("standard input ended after 5 of 10 bytes".to_owned())
            );
            assert!(
                slot_file(&slot, OUT_FILE).is_none(),
                "a short capture is not a complete capture"
            );
            assert_eq!(
                slot_file(&slot, OUT_PARTIAL_FILE).as_deref(),
                Some(&b"01234"[..]),
                "the bytes that did arrive are the evidence"
            );
            assert!(slot_file(&slot, DONE_FILE).is_some());
        }

        #[test]
        fn a_failed_raw_mode_never_signals_readiness() {
            let slot = FinalSinkSlot::new("unix-raw-failure", b"abc", true);

            // The real `stty raw -echo`, against a pipe: it cannot succeed.
            let status = run(&slot, b"abc", RAW_SETUP_COMMAND);

            assert_eq!(status.code(), Some(1), "a failed setup must fail the child");
            let reported = String::from_utf8(
                slot_file(&slot, ERROR_FILE).expect("a failed setup must write `error`"),
            )
            .expect("the reason is text");
            assert!(
                reported.contains("raw mode could not be established"),
                "unexpected reason: {reported}"
            );
            assert!(
                slot_file(&slot, READY_FILE).is_none(),
                "readiness must never be announced from a cooked terminal"
            );
            assert!(slot_file(&slot, OUT_FILE).is_none());
            assert!(slot_file(&slot, DONE_FILE).is_some());
        }

        /// A slot the child cannot write is a setup failure, not a silent
        /// readiness signal followed by a capture timeout.
        #[test]
        fn a_slot_it_cannot_write_is_reported_rather_than_silently_skipped() {
            use std::os::unix::fs::PermissionsExt;

            let slot = FinalSinkSlot::new("unix-unwritable", b"abc", false);
            let mut permissions = std::fs::metadata(&slot.directory)
                .expect("the slot exists")
                .permissions();
            permissions.set_mode(0o555);
            std::fs::set_permissions(&slot.directory, permissions.clone())
                .expect("make the slot read-only");

            let status = run(&slot, b"abc", "true");

            permissions.set_mode(0o755);
            std::fs::set_permissions(&slot.directory, permissions)
                .expect("restore the slot permissions");

            assert_eq!(status.code(), Some(1), "an unwritable slot must fail");
            assert!(
                slot_file(&slot, READY_FILE).is_none(),
                "readiness must not be claimed when it could not be written"
            );
            assert!(slot_file(&slot, OUT_FILE).is_none());
        }
    }
}
