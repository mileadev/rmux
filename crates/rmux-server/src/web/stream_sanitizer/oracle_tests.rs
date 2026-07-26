use std::io::Write;
use std::process::{Command, Stdio};

use rmux_core::{GridRenderOptions, ScreenCaptureRange, TerminalScreen};
use rmux_proto::TerminalSize;
use serde_json::json;

use super::*;

type OracleCase = (&'static str, &'static [u8], &'static [u8]);

fn sanitize(input: &[u8]) -> Vec<u8> {
    let mut sanitizer = WebTerminalSanitizer::default();
    let mut output = Vec::new();
    sanitizer.push(input, &mut output);
    output
}

fn rmux_printable(input: &[u8]) -> Vec<u8> {
    let mut terminal = TerminalScreen::new(TerminalSize { cols: 120, rows: 1 }, 0);
    terminal.feed(input);
    let mut output = terminal
        .screen()
        .capture_transcript(ScreenCaptureRange::default(), GridRenderOptions::default());
    while output.last() == Some(&b'\n') {
        output.pop();
    }
    String::from_utf8(output)
        .expect("RMUX transcript is UTF-8")
        .chars()
        .filter(|character| !matches!(u32::from(*character), 0x80..=0x9f))
        .collect::<String>()
        .into_bytes()
}

fn oracle_cases() -> Vec<OracleCase> {
    vec![
        ("dcs-can", b"a\x1bPq\x18HIDDEN\x1b\\b", b"ab"),
        ("dcs-sub", b"a\x1bPq\x1aHIDDEN\x1b\\b", b"ab"),
        ("dcs-esc-csi", b"a\x1bPq\x1b[31mHIDDEN\x1b\\b", b"ab"),
        ("dcs-esc-final", b"a\x1bPq\x1bAHIDDEN\x1b\\b", b"ab"),
        ("dcs-bel", b"a\x1bPqpayload\x07HIDDEN\x1b\\b", b"ab"),
        ("dcs-entry-can", b"a\x1bP1\x18VISIBLEb", b"aVISIBLEb"),
        ("apc-can", b"a\x1b_payload\x18VISIBLEb", b"aVISIBLEb"),
        ("apc-sub", b"a\x1b_payload\x1aVISIBLEb", b"aVISIBLEb"),
        ("apc-bel", b"a\x1b_payload\x07HIDDEN\x1b\\b", b"ab"),
        ("pm-can", b"a\x1b^payload\x18VISIBLEb", b"aVISIBLEb"),
        ("pm-st", b"a\x1b^HIDDEN\x1b\\b", b"ab"),
        ("sos-sub", b"a\x1bXpayload\x1aVISIBLEb", b"aVISIBLEb"),
        ("sos-st", b"a\x1bXHIDDEN\x1b\\b", b"ab"),
        ("osc-can", b"a\x1b]2;title\x18VISIBLEb", b"aVISIBLEb"),
        ("osc-sub", b"a\x1b]2;title\x1aVISIBLEb", b"aVISIBLEb"),
        ("osc-bel", b"a\x1b]52;c;WA==\x07VISIBLEb", b"aVISIBLEb"),
        ("osc-st", b"a\x1b]52;c;WA==\x1b\\VISIBLEb", b"aVISIBLEb"),
        ("rename-st", b"a\x1bkWINDOWNAME\x1b\\b", b"ab"),
        ("rename-bel", b"a\x1bkWINDOWNAME\x07HIDDEN\x1b\\b", b"ab"),
        ("utf8-c1-dcs", b"a\xc2\x90VISIBLEb", b"aVISIBLEb"),
        ("utf8-c1-sos", b"a\xc2\x98VISIBLEb", b"aVISIBLEb"),
        ("utf8-c1-osc", b"a\xc2\x9dVISIBLEb", b"aVISIBLEb"),
        ("utf8-c1-pm", b"a\xc2\x9eVISIBLEb", b"aVISIBLEb"),
        ("utf8-c1-apc", b"a\xc2\x9fVISIBLEb", b"aVISIBLEb"),
        ("bare-c1", b"a\x9dVISIBLEb", "a\u{fffd}VISIBLEb".as_bytes()),
    ]
}

#[test]
fn sanitized_bytes_match_rmux_printable_text() {
    for (name, input, expected) in oracle_cases() {
        assert_eq!(rmux_printable(input), expected, "RMUX owner: {name}");
        assert_eq!(sanitize(input), expected, "sanitized bytes: {name}");
    }
}

#[test]
#[ignore = "requires the pinned xterm.js package installed from package-lock.json"]
fn sanitized_bytes_match_pinned_xterm_viewer() {
    let vectors = oracle_cases()
        .into_iter()
        .map(|(name, input, expected)| {
            assert_eq!(rmux_printable(input), expected, "RMUX owner: {name}");
            json!({
                "name": name,
                "cols": 120,
                "rows": 8,
                "scrollback": 0,
                "initial": expected,
                "keyframe": sanitize(input),
                "tail": []
            })
        })
        .collect::<Vec<_>>();
    let oracle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/xterm-oracle/recovery-oracle.mjs");
    let mut child = Command::new("node")
        .arg(oracle)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start pinned xterm.js sanitizer oracle");
    child
        .stdin
        .take()
        .expect("oracle stdin")
        .write_all(json!({ "vectors": vectors }).to_string().as_bytes())
        .expect("write sanitizer vectors");
    let output = child.wait_with_output().expect("wait for xterm.js oracle");
    assert!(
        output.status.success(),
        "xterm.js sanitizer oracle failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
