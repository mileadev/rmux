#[path = "support/python3.rs"]
mod python3;

use std::path::PathBuf;
use std::process::Output;

const CHOCOLATEY: &str = include_str!("../.github/workflows/release-chocolatey-retry.yml");
const PREPARE_ACTION: &str =
    include_str!("../.github/actions/release-channel-retry-prepare/action.yml");
const SNAP: &str = include_str!("../.github/workflows/release-snap-retry.yml");

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn assert_python(source: &str) {
    let output = python3::command()
        .args(["-c", source])
        .current_dir(repo_root())
        .output()
        .expect("run release retry input fixture");
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repeated(character: char, count: usize) -> String {
    std::iter::repeat_n(character, count).collect()
}

fn identity_args(channel: &str) -> Vec<(String, String)> {
    vec![
        ("channel".into(), channel.into()),
        ("source-sha".into(), repeated('a', 40)),
        ("release-id".into(), "7".into()),
        ("release-ref".into(), "v1.2.3-rc.4".into()),
        (
            "idempotency-key".into(),
            format!("rmux-downstream-v1:{}", repeated('b', 64)),
        ),
    ]
}

fn prepare_args() -> Vec<(String, String)> {
    let mut args = identity_args("chocolatey");
    args.extend([
        ("receipt-run-id".into(), "101".into()),
        ("receipt-run-workflow-id".into(), "202".into()),
        ("receipt-workflow-id".into(), "202".into()),
        ("receipt-artifact-id".into(), "301".into()),
        (
            "receipt-artifact-digest".into(),
            format!("sha256:{}", repeated('c', 64)),
        ),
        ("receipt-envelope-artifact-id".into(), "302".into()),
        (
            "receipt-envelope-artifact-digest".into(),
            format!("sha256:{}", repeated('d', 64)),
        ),
        ("receipt-predicate-sha256".into(), repeated('e', 64)),
        ("receipt-envelope-sha256".into(), repeated('f', 64)),
        ("prior-result-run-id".into(), "101".into()),
        ("prior-result-run-workflow-id".into(), "202".into()),
        ("prior-result-producer-workflow-id".into(), "202".into()),
        (
            "prior-result-producer-workflow-path".into(),
            ".github/workflows/release-chocolatey-channel.yml".into(),
        ),
        ("prior-result-artifact-id".into(), "401".into()),
        (
            "prior-result-artifact-digest".into(),
            format!("sha256:{}", repeated('0', 64)),
        ),
        ("prior-result-predicate-sha256".into(), repeated('1', 64)),
        ("prior-result-envelope-artifact-id".into(), "402".into()),
        (
            "prior-result-envelope-artifact-digest".into(),
            format!("sha256:{}", repeated('2', 64)),
        ),
        ("prior-result-envelope-sha256".into(), repeated('3', 64)),
    ]);
    args
}

fn run_validator(command: &str, args: &[(String, String)]) -> Output {
    let mut process = python3::command();
    process
        .arg("scripts/release/prepare-channel-retry.py")
        .arg(command)
        .current_dir(repo_root());
    for (name, value) in args {
        process.arg(format!("--{name}")).arg(value);
    }
    process.output().expect("run retry input validator")
}

fn replace_arg(
    args: &[(String, String)],
    target: &str,
    replacement: &str,
) -> Vec<(String, String)> {
    args.iter()
        .map(|(name, value)| {
            (
                name.clone(),
                if name == target {
                    replacement.to_owned()
                } else {
                    value.clone()
                },
            )
        })
        .collect()
}

fn assert_rejected(command: &str, args: &[(String, String)], label: &str) {
    let output = run_validator(command, args);
    assert!(
        !output.status.success(),
        "{label} was accepted\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_environment_identity_validator(idempotency_key: &str, omitted: Option<&str>) -> Output {
    let mut process = python3::command();
    process
        .args([
            "scripts/release/prepare-channel-retry.py",
            "validate-identity-inputs",
            "--from-env",
        ])
        .current_dir(repo_root());
    for (name, value) in [
        ("RMUX_CHANNEL", "chocolatey"),
        (
            "RMUX_SOURCE_SHA",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        ("RMUX_RELEASE_ID", "17"),
        ("RMUX_RELEASE_REF", "v1.2.3"),
        ("RMUX_REQUEST_IDEMPOTENCY_KEY", idempotency_key),
    ] {
        if omitted != Some(name) {
            process.env(name, value);
        } else {
            process.env_remove(name);
        }
    }
    process
        .output()
        .expect("run retry environment input validator")
}

#[test]
fn retry_workflows_never_embed_dispatch_inputs_in_run() {
    assert_python(
        r#"
import pathlib
import sys

sys.path.insert(0, "scripts/release")
from workflow_run_input_boundary import (
    find_direct_input_expressions,
    validate_no_direct_input_expressions,
)

invalid = (
    """
steps:
  - name: structural marker
    run: |
      echo "${{ inputs.harmless_marker }}"
""",
    """
steps:
  - {name: structural marker, run: "echo ${{github.event.inputs.harmless_marker}}"}
""",
    """
steps:
  - run: echo "${{ format('{0}', inputs.harmless_marker) }}"
""",
)
for fixture in invalid:
    if len(find_direct_input_expressions(fixture)) != 1:
        raise SystemExit("direct input fixture was not detected")

safe = """
jobs:
  marker:
    name: ${{ inputs.harmless_marker }}
    if: inputs.harmless_marker != ''
    steps:
      - env:
          RMUX_MARKER: ${{ inputs.harmless_marker }}
        run: echo "$RMUX_MARKER"
      - uses: example/action@0000000000000000000000000000000000000000
        with:
          marker: ${{ inputs.harmless_marker }}
      - run: echo "${{ steps.marker.outputs.value }}"
"""
if find_direct_input_expressions(safe):
    raise SystemExit("Actions-engine fixture was mistaken for shell interpolation")

root = pathlib.Path.cwd()
validate_no_direct_input_expressions(
    (
        root / ".github/workflows/release-channel-retry.yml",
        root / ".github/workflows/release-chocolatey-retry.yml",
        root / ".github/workflows/release-snap-retry.yml",
        root / ".github/actions/release-channel-retry-prepare/action.yml",
    )
)
"#,
    );
}

#[test]
fn retry_boundary_scanner_covers_twelve_yaml_scalar_and_context_forms() {
    assert_python(
        r###"
import pathlib
import sys

sys.path.insert(0, "scripts/release")
from workflow_run_input_boundary import find_direct_input_expressions

safe = "${{ steps.marker.outputs.value }}"
active = "${{ inputs.marker }}"
active_bare_context = "${{ format('{0}', inputs) }}"
forms = (
    ("plain", """steps:
  - run: echo __EXPR__
""", active),
    ("single_quoted", """steps:
  - run: 'echo __EXPR__'
""", active),
    ("double_quoted", """steps:
  - run: "echo __EXPR__"
""", active),
    ("literal", """steps:
  - run: |-
      echo __EXPR__
""", active),
    ("folded", """steps:
  - run: >+
      echo __EXPR__
""", active),
    ("flow_quoted", """steps:
  - {name: flow quoted, run: "echo __EXPR__"}
""", active),
    ("flow_plain", """steps:
  - [{name: flow plain, run: echo-__EXPR__}]
""", active),
    ("multiline_plain", """steps:
  - run: echo first
      __EXPR__
""", active),
    ("multiline_single", """steps:
  - run: 'echo first
      __EXPR__'
""", active),
    ("multiline_double", """steps:
  - run: "echo first
      __EXPR__"
""", active),
    ("quoted_flow_key", """steps:
  - {"run": "echo __EXPR__", name: quoted key}
""", active),
    ("bare_inputs_context", """steps:
  - run: "echo __EXPR__"
""", active_bare_context),
)

missed = []
false_positives = []
for name, template, mutant in forms:
    inert = template.replace("__EXPR__", safe)
    if find_direct_input_expressions(inert):
        false_positives.append(name)
    active_fixture = template.replace("__EXPR__", mutant)
    if len(find_direct_input_expressions(active_fixture)) != 1:
        missed.append(name)

if false_positives:
    raise SystemExit(f"safe YAML forms were rejected: {false_positives}")
if missed:
    raise SystemExit(f"active YAML forms were missed: {missed}")

root = pathlib.Path.cwd()
for relative in (
    ".github/workflows/release-chocolatey-retry.yml",
    ".github/workflows/release-snap-retry.yml",
):
    findings = find_direct_input_expressions(
        (root / relative).read_text(encoding="utf-8")
    )
    if findings:
        raise SystemExit(f"clean workflow rejected: {relative}: {findings}")
"###,
    );
}

#[test]
fn retry_validators_read_untrusted_values_from_fixed_environment_names() {
    let canonical_key = format!("rmux-downstream-v1:{}", repeated('b', 64));
    let output = run_environment_identity_validator(&canonical_key, None);
    assert!(
        output.status.success(),
        "canonical environment validation failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for (value, label) in [
        (
            format!(
                "rmux-downstream-v1:{}\"{}",
                repeated('b', 32),
                repeated('b', 32)
            ),
            "embedded double quote",
        ),
        (String::new(), "explicit empty value"),
    ] {
        let output = run_environment_identity_validator(&value, None);
        assert!(
            !output.status.success(),
            "{label} was accepted after direct environment transport"
        );
    }
    let output = run_environment_identity_validator(&canonical_key, Some("RMUX_RELEASE_ID"));
    assert!(
        !output.status.success(),
        "missing environment value was mistaken for an explicit value"
    );

    assert!(
        PREPARE_ACTION.contains("validate-prepare-inputs --from-env"),
        "prepare validator must read the fixed environment projection"
    );
    assert!(
        PREPARE_ACTION.contains("prepare --from-env"),
        "prepare must revalidate the fixed environment projection before I/O"
    );
    for (name, workflow) in [("Chocolatey", CHOCOLATEY), ("Snap", SNAP)] {
        assert!(
            workflow.contains("validate-identity-inputs --from-env"),
            "{name} identity validator still receives raw argv values"
        );
        assert!(
            workflow.contains("verify-prepared --from-env"),
            "{name} prepared verifier still receives raw argv values"
        );
    }
    for (source, forbidden) in [
        (
            CHOCOLATEY,
            "validate-identity-inputs `\n            --channel",
        ),
        (CHOCOLATEY, "verify-prepared `\n            --prepared"),
        (SNAP, "validate-identity-inputs \\\n            --channel"),
        (SNAP, "verify-prepared \\\n            --prepared"),
        (
            PREPARE_ACTION,
            "validate-prepare-inputs \\\n          --channel",
        ),
        (PREPARE_ACTION, "prepare \\\n          --root"),
    ] {
        assert!(
            !source.contains(forbidden),
            "untrusted retry value remains on a shell argv boundary: {forbidden}"
        );
    }
}

#[test]
fn retry_input_validators_accept_canonical_and_reject_invalid_forms() {
    let identity = identity_args("snap_candidate");
    let output = run_validator("validate-identity-inputs", &identity);
    assert!(
        output.status.success(),
        "canonical identity failed\nstderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let prepare = prepare_args();
    let output = run_validator("validate-prepare-inputs", &prepare);
    assert!(
        output.status.success(),
        "canonical prepare inputs failed\nstderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    for (field, value, label) in [
        ("channel", "harmless_marker", "unknown channel"),
        (
            "source-sha",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "uppercase source SHA",
        ),
        ("release-id", "01", "noncanonical release ID"),
        ("release-ref", "v1.2", "incomplete release ref"),
        (
            "idempotency-key",
            "rmux-downstream-v1:harmless-marker",
            "malformed idempotency key",
        ),
    ] {
        assert_rejected(
            "validate-identity-inputs",
            &replace_arg(&identity, field, value),
            label,
        );
    }

    for (field, value, label) in [
        ("receipt-artifact-id", "0", "zero artifact ID"),
        (
            "receipt-artifact-digest",
            "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "uppercase artifact digest",
        ),
        (
            "receipt-predicate-sha256",
            "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
            "nonhex predicate digest",
        ),
        (
            "prior-result-producer-workflow-path",
            ".github/workflows/release-snap-channel.yml",
            "cross-channel producer path",
        ),
        ("prior-result-run-id", "102", "cross-run mismatch"),
        (
            "prior-result-run-workflow-id",
            "203",
            "workflow identity mismatch",
        ),
    ] {
        assert_rejected(
            "validate-prepare-inputs",
            &replace_arg(&prepare, field, value),
            label,
        );
    }
}
