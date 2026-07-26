#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const ARCHITECTURES: [&str; 2] = ["amd64", "arm64"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    repo_root()
        .join(".rmux-audit/test-runs")
        .join(format!("apt-safety-{label}-{}-{nonce}", std::process::id()))
}

fn make_executable(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("read tool metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make tool executable");
}

fn install_tools(root: &Path) {
    fs::create_dir_all(root).expect("create tool directory");
    let dpkg_deb = root.join("dpkg-deb");
    fs::write(
        &dpkg_deb,
        r#"#!/bin/sh
set -eu
test "$1" = -f
case "$2" in
  *_amd64.deb) architecture=amd64 ;;
  *_arm64.deb) architecture=arm64 ;;
  *) exit 64 ;;
esac
printf 'Package: rmux\nVersion: 0.10.0\nArchitecture: %s\n' "$architecture"
"#,
    )
    .expect("write dpkg-deb fixture");
    make_executable(&dpkg_deb);

    let gpg = root.join("gpg");
    fs::write(
        &gpg,
        r#"#!/bin/sh
set -eu
case " $* " in
  *" --list-secret-keys "*) exit 0 ;;
esac
output=
kind=signature
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) output=$2; shift 2 ;;
    --clearsign) kind=in-release; shift ;;
    *) shift ;;
  esac
done
test -n "$output"
printf 'deterministic-%s\n' "$kind" > "$output"
"#,
    )
    .expect("write gpg fixture");
    make_executable(&gpg);
}

fn write_package(input: &Path, architecture: &str, generation: &str) {
    fs::create_dir_all(input).expect("create input directory");
    fs::write(
        input.join(format!("rmux_0.10.0_{architecture}.deb")),
        format!("{architecture}:{generation}\n"),
    )
    .expect("write package fixture");
}

fn write_packages(input: &Path, generation: &str) {
    for architecture in ARCHITECTURES {
        write_package(input, architecture, generation);
    }
}

fn generate(
    input: &Path,
    output: &Path,
    tools: &Path,
    previous: Option<&Path>,
    signed: bool,
) -> Output {
    let path = std::env::join_paths([tools, Path::new("/usr/bin"), Path::new("/bin")])
        .expect("compose pinned PATH");
    let mut command = Command::new(repo_root().join("scripts/generate-apt-repository.sh"));
    command
        .args(["--input-dir"])
        .arg(input)
        .args(["--output-dir"])
        .arg(output)
        .args(["--suite", "stable", "--component", "main"]);
    for architecture in ARCHITECTURES {
        command.args(["--architecture", architecture]);
    }
    if let Some(previous) = previous {
        command.args(["--previous-repository-dir"]).arg(previous);
    }
    if signed {
        command.args(["--signing-key", "fixture-signing-key"]);
    }
    command
        .env("PATH", path)
        .env_remove("RMUX_APT_GPG_KEY")
        .current_dir(repo_root())
        .output()
        .expect("run APT generator")
}

#[derive(Debug, Eq, PartialEq)]
enum TreeEntry {
    Directory(u32),
    File(u32, Vec<u8>),
    Symlink(PathBuf),
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, TreeEntry> {
    fn visit(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, TreeEntry>) {
        let mut entries: Vec<_> = fs::read_dir(directory)
            .expect("read snapshot directory")
            .map(|entry| entry.expect("read snapshot entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path).expect("read snapshot metadata");
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path is below root")
                .to_path_buf();
            if metadata.file_type().is_symlink() {
                snapshot.insert(
                    relative,
                    TreeEntry::Symlink(fs::read_link(&path).expect("read snapshot symlink")),
                );
            } else if metadata.is_dir() {
                snapshot.insert(
                    relative,
                    TreeEntry::Directory(metadata.permissions().mode()),
                );
                visit(root, &path, snapshot);
            } else {
                snapshot.insert(
                    relative,
                    TreeEntry::File(
                        metadata.permissions().mode(),
                        fs::read(&path).expect("read snapshot file"),
                    ),
                );
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn validates_every_input_architecture_before_replacing_owned_output() {
    let root = fixture_root("preflight");
    let input = root.join("input");
    let output = root.join("output");
    let tools = root.join("tools");
    install_tools(&tools);
    write_packages(&input, "initial");
    let created = generate(&input, &output, &tools, None, false);
    assert!(created.status.success(), "{}", stderr(&created));
    let before = snapshot_tree(&output);

    fs::remove_dir_all(&input).expect("remove populated input");
    fs::create_dir_all(&input).expect("recreate empty input");
    let empty = generate(&input, &output, &tools, None, false);
    assert!(!empty.status.success(), "empty input was accepted");
    assert!(stderr(&empty).contains("no rmux_*_amd64.deb files found"));
    assert_eq!(snapshot_tree(&output), before, "empty input mutated output");

    write_package(&input, "amd64", "arm64-missing");
    let missing = generate(&input, &output, &tools, None, false);
    assert!(
        !missing.status.success(),
        "missing architecture was accepted"
    );
    assert!(stderr(&missing).contains("no rmux_*_arm64.deb files found"));
    assert_eq!(
        snapshot_tree(&output),
        before,
        "partial architecture input mutated output"
    );

    fs::remove_dir_all(root).expect("remove preflight fixture");
}

#[test]
fn rejects_foreign_and_symlinked_outputs_without_mutating_any_bytes() {
    let root = fixture_root("foreign");
    let input = root.join("input");
    let tools = root.join("tools");
    install_tools(&tools);
    write_packages(&input, "valid");

    let foreign = root.join("foreign");
    fs::create_dir_all(foreign.join("dists/stable")).expect("create foreign dists");
    fs::create_dir_all(foreign.join("pool/main/r/rmux")).expect("create foreign pool");
    fs::write(
        foreign.join("dists/stable/Release"),
        b"FOREIGN-RELEASE-SENTINEL\n",
    )
    .expect("write foreign Release");
    fs::write(
        foreign.join("pool/main/r/rmux/foreign.deb"),
        b"FOREIGN-POOL-SENTINEL\n",
    )
    .expect("write foreign package");
    fs::write(foreign.join("foreign-root"), b"FOREIGN-ROOT-SENTINEL\n")
        .expect("write foreign root sentinel");
    let foreign_before = snapshot_tree(&foreign);
    let rejected = generate(&input, &foreign, &tools, None, false);
    assert!(!rejected.status.success(), "foreign output was accepted");
    assert_eq!(
        snapshot_tree(&foreign),
        foreign_before,
        "foreign output changed during refusal"
    );

    let target = root.join("symlink-target");
    fs::create_dir_all(target.join("dists/stable")).expect("create symlink target");
    fs::write(
        target.join("dists/stable/sentinel"),
        b"SYMLINK-TARGET-SENTINEL\n",
    )
    .expect("write symlink target sentinel");
    let target_before = snapshot_tree(&target);
    let output_link = root.join("output-link");
    symlink(&target, &output_link).expect("create output symlink");
    let rejected = generate(&input, &output_link, &tools, None, false);
    assert!(!rejected.status.success(), "symlinked output was accepted");
    assert!(stderr(&rejected).contains("must not traverse symbolic links"));
    assert_eq!(
        snapshot_tree(&target),
        target_before,
        "symlink target changed during refusal"
    );

    fs::remove_dir_all(root).expect("remove foreign-output fixture");
}

#[test]
fn rejects_input_output_overlap_in_both_directions_without_mutation() {
    let root = fixture_root("overlap");
    let tools = root.join("tools");
    install_tools(&tools);

    let outer_output = root.join("outer-output");
    let nested_input = outer_output.join("pool/main/r/rmux");
    write_packages(&nested_input, "input-inside-output");
    fs::create_dir_all(outer_output.join("dists/stable")).expect("create output sentinel path");
    fs::write(
        outer_output.join("dists/stable/sentinel"),
        b"INPUT-INSIDE-OUTPUT-SENTINEL\n",
    )
    .expect("write overlap sentinel");
    let before = snapshot_tree(&outer_output);
    let rejected = generate(&nested_input, &outer_output, &tools, None, false);
    assert!(!rejected.status.success(), "nested input was accepted");
    assert!(stderr(&rejected).contains("cannot overlap"));
    assert_eq!(
        snapshot_tree(&outer_output),
        before,
        "input-inside-output refusal changed bytes"
    );

    let outer_input = root.join("outer-input");
    let nested_output = outer_input.join("generated-repository");
    write_packages(&outer_input, "output-inside-input");
    fs::create_dir_all(&nested_output).expect("create nested output");
    fs::write(
        nested_output.join("sentinel"),
        b"OUTPUT-INSIDE-INPUT-SENTINEL\n",
    )
    .expect("write nested output sentinel");
    let before = snapshot_tree(&outer_input);
    let rejected = generate(&outer_input, &nested_output, &tools, None, false);
    assert!(!rejected.status.success(), "nested output was accepted");
    assert!(stderr(&rejected).contains("cannot overlap"));
    assert_eq!(
        snapshot_tree(&outer_input),
        before,
        "output-inside-input refusal changed bytes"
    );

    fs::remove_dir_all(root).expect("remove overlap fixture");
}

#[test]
fn regenerates_an_exact_signed_rmux_output_and_preserves_the_caller_key() {
    let root = fixture_root("owned");
    let input = root.join("input");
    let output = root.join("debian");
    let tools = root.join("tools");
    install_tools(&tools);
    write_packages(&input, "first");

    let first = generate(&input, &output, &tools, None, true);
    assert!(first.status.success(), "{}", stderr(&first));
    let public_key =
        b"-----BEGIN PGP PUBLIC KEY BLOCK-----\nRMUX\n-----END PGP PUBLIC KEY BLOCK-----\n";
    fs::write(output.join("rmux.asc"), public_key).expect("write caller-owned public key");
    write_packages(&input, "second");

    let second = generate(&input, &output, &tools, None, true);
    assert!(second.status.success(), "{}", stderr(&second));
    assert_eq!(
        fs::read(output.join("rmux.asc")).expect("read preserved key"),
        public_key
    );
    assert_eq!(
        fs::read(output.join("pool/main/r/rmux/rmux_0.10.0_amd64.deb"))
            .expect("read regenerated package"),
        b"amd64:second\n"
    );
    assert!(output.join("dists/stable/InRelease").is_file());
    assert!(output.join("dists/stable/Release.gpg").is_file());

    fs::remove_dir_all(root).expect("remove owned-output fixture");
}

#[test]
fn supports_a_new_ci_leaf_and_keeps_previous_repository_bytes_unchanged() {
    let root = fixture_root("ci-previous");
    let input = root.join("input");
    let previous = root.join("history/debian");
    let ci_parent = root.join("output");
    let current = ci_parent.join("debian");
    let tools = root.join("tools");
    install_tools(&tools);
    write_packages(&input, "previous");
    let first = generate(&input, &previous, &tools, None, false);
    assert!(first.status.success(), "{}", stderr(&first));
    let previous_before = snapshot_tree(&previous);

    write_packages(&input, "current");
    fs::create_dir_all(&ci_parent).expect("create CI output parent");
    assert!(!current.exists(), "CI leaf must begin absent");
    let second = generate(&input, &current, &tools, Some(&previous), false);
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(String::from_utf8_lossy(&second.stdout).contains("by_hash_retention=retained\n"));
    assert_eq!(
        snapshot_tree(&previous),
        previous_before,
        "previous repository was mutated"
    );
    for architecture in ARCHITECTURES {
        let by_hash = current.join(format!(
            "dists/stable/main/binary-{architecture}/by-hash/SHA256"
        ));
        assert_eq!(
            fs::read_dir(by_hash)
                .expect("read retained by-hash directory")
                .count(),
            4
        );
    }

    let current_before_rejection = snapshot_tree(&current);
    let previous_by_hash = previous.join("dists/stable/main/binary-amd64/by-hash/SHA256");
    let retained = fs::read_dir(&previous_by_hash)
        .expect("read previous by-hash")
        .next()
        .expect("find previous by-hash entry")
        .expect("read previous by-hash entry")
        .path();
    fs::write(retained, b"CORRUPTED-PREVIOUS-BY-HASH\n")
        .expect("corrupt previous repository fixture");
    let rejected = generate(&input, &current, &tools, Some(&previous), false);
    assert!(!rejected.status.success(), "invalid previous was accepted");
    assert_eq!(
        snapshot_tree(&current),
        current_before_rejection,
        "invalid previous repository caused output mutation"
    );

    fs::remove_dir_all(root).expect("remove CI/previous fixture");
}
