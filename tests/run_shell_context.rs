#![cfg(unix)]

mod common;

use std::error::Error;
use std::fs;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use common::{assert_success, stderr, stdout, AttachedSession, CliHarness, DaemonGuard};
use rmux_pty::TerminalSize;

const FORMAT: &str = "#{session_name}:#{pane_id}";

#[test]
fn detached_cli_run_shell_uses_the_canonical_current_target() -> Result<(), Box<dyn Error>> {
    let (harness, _daemon) = current_target_harness("run-shell-detached-current")?;
    let expected = display_format(&harness, None)?;
    assert!(
        expected.starts_with("beta:%"),
        "the detached-current fixture should select beta, got {expected:?}"
    );

    let if_shell = harness.run(&[
        "if-shell",
        "-F",
        "1",
        &format!("display-message -p '{FORMAT}'"),
    ])?;
    assert_output_success(&if_shell);
    assert_eq!(stdout(&if_shell), format!("{expected}\n"));
    assert!(stderr(&if_shell).is_empty());

    let run_shell = harness.run(&["run-shell", &format_print_command()])?;
    assert_output_success(&run_shell);
    assert_eq!(stdout(&run_shell), format!("{expected}\n"));
    assert!(stderr(&run_shell).is_empty());

    let explicit_expected = display_format(&harness, Some("alpha:0.0"))?;
    let explicit_output = harness.tmpdir().join("explicit-context.txt");
    let explicit = harness.run(&[
        "run-shell",
        "-t",
        "alpha:0.0",
        &format_write_command(&explicit_output),
    ])?;
    assert_success(&explicit);
    wait_for_file_text(&explicit_output, &format!("{explicit_expected}\n"))?;
    Ok(())
}

#[test]
fn sourced_bound_and_hook_run_shell_keep_their_entry_target() -> Result<(), Box<dyn Error>> {
    let (harness, _daemon) = current_target_harness("run-shell-entry-targets")?;
    let expected = display_format(&harness, None)?;

    let source_file = harness.tmpdir().join("run-shell-context.conf");
    fs::write(
        &source_file,
        format!("run-shell {}\n", shell_quote_str(&format_print_command())),
    )?;
    let sourced = harness.run(&[
        "source-file",
        source_file.to_str().expect("utf-8 source path"),
    ])?;
    assert_output_success(&sourced);
    assert_eq!(stdout(&sourced), format!("{expected}\n"));
    assert!(stderr(&sourced).is_empty());

    let binding_output = harness.tmpdir().join("binding-context.txt");
    let binding_command = format_write_command(&binding_output);
    assert_success(&harness.run(&[
        "bind-key",
        "-T",
        "prefix",
        "X",
        "run-shell",
        &binding_command,
    ])?);
    let mut attach = AttachedSession::spawn(&harness, "beta", TerminalSize::new(80, 12))?;
    attach.wait_for_raw_mode(Duration::from_secs(5))?;
    attach.send_bytes(b"\x02X")?;
    wait_for_file_text(&binding_output, &format!("{expected}\n"))?;
    attach.send_bytes(b"\x02d")?;
    assert!(attach.wait_for_exit(Duration::from_secs(5))?.success());

    let hook_output = harness.tmpdir().join("hook-context.txt");
    let hook_command = format!(
        "run-shell {}",
        shell_quote_str(&format_write_command(&hook_output))
    );
    assert_success(&harness.run(&["set-hook", "-g", "after-new-window", &hook_command])?);
    let created = harness.run(&["new-window", "-dP", "-F", FORMAT, "-t", "beta", "sleep 30"])?;
    assert_output_success(&created);
    let created_target = stdout(&created);
    wait_for_file_text(&hook_output, &created_target)?;
    Ok(())
}

#[test]
fn delayed_run_shell_keeps_the_dispatch_context_after_session_rename() -> Result<(), Box<dyn Error>>
{
    let (harness, _daemon) = current_target_harness("run-shell-context-rename")?;
    let expected = display_format(&harness, None)?;
    let mut run_shell = spawn_delayed_run_shell(&harness)?;
    wait_until_delayed(&mut run_shell)?;

    assert_success(&harness.run(&["rename-session", "-t", "beta", "renamed"])?);
    let output = run_shell.wait_with_output()?;

    assert_output_success(&output);
    assert_eq!(stdout(&output), format!("{expected}\n"));
    assert!(stderr(&output).is_empty());
    Ok(())
}

#[test]
fn delayed_run_shell_does_not_rebind_to_a_replacement_session() -> Result<(), Box<dyn Error>> {
    let (harness, _daemon) = current_target_harness("run-shell-context-replacement")?;
    let expected = display_format(&harness, None)?;
    let mut run_shell = spawn_delayed_run_shell(&harness)?;
    wait_until_delayed(&mut run_shell)?;

    assert_success(&harness.run(&["kill-session", "-t", "beta"])?);
    assert_success(&harness.run(&["new-session", "-d", "-s", "beta", "sleep 30"])?);
    let replacement = display_format(&harness, Some("beta:0.0"))?;
    assert_ne!(
        replacement, expected,
        "the replacement must have a distinct stable pane identity"
    );

    let output = run_shell.wait_with_output()?;
    assert_output_success(&output);
    assert_eq!(stdout(&output), format!("{expected}\n"));
    assert!(stderr(&output).is_empty());
    Ok(())
}

#[test]
fn delayed_command_run_shell_follows_stable_session_identity_after_rename(
) -> Result<(), Box<dyn Error>> {
    let (harness, _daemon) = current_target_harness("run-shell-command-context-rename")?;
    let mut run_shell = spawn_delayed_command_run_shell(&harness, "new-window -d -n delayed")?;
    wait_until_delayed(&mut run_shell)?;

    assert_success(&harness.run(&["rename-session", "-t", "beta", "renamed"])?);
    assert_success(&harness.run(&["new-session", "-d", "-s", "gamma", "sleep 30"])?);
    let output = run_shell.wait_with_output()?;

    assert_output_success(&output);
    assert_eq!(window_names(&harness, "alpha")?, vec!["sleep"]);
    assert_eq!(window_names(&harness, "gamma")?, vec!["sleep"]);
    assert_eq!(window_names(&harness, "renamed")?, vec!["sleep", "delayed"]);
    Ok(())
}

#[test]
fn delayed_command_run_shell_rejects_replacement_session_product_divergence(
) -> Result<(), Box<dyn Error>> {
    // tmux 3.7b oracle, measured 2026-07-26: after the original target is
    // killed, the delayed command resolves a replacement with the same name
    // and creates the requested window there. RMUX pins the stable session
    // identity and fails closed instead of mutating the replacement.
    let (harness, _daemon) = current_target_harness("run-shell-command-context-replacement")?;
    let mut run_shell = spawn_delayed_command_run_shell(&harness, "new-window -d -n rebound")?;
    wait_until_delayed(&mut run_shell)?;

    assert_success(&harness.run(&["kill-session", "-t", "beta"])?);
    assert_success(&harness.run(&["new-session", "-d", "-s", "beta", "sleep 30"])?);
    let output = run_shell.wait_with_output()?;

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "queued pinned target was replaced before execution\n"
    );
    assert_eq!(window_names(&harness, "alpha")?, vec!["sleep"]);
    assert_eq!(window_names(&harness, "beta")?, vec!["sleep"]);
    Ok(())
}

fn current_target_harness(label: &str) -> Result<(CliHarness, DaemonGuard), Box<dyn Error>> {
    let harness = CliHarness::new(label)?;
    let daemon = harness.start_hidden_daemon()?;
    assert_success(&harness.run(&["new-session", "-d", "-s", "alpha", "sleep 30"])?);
    assert_success(&harness.run(&["new-session", "-d", "-s", "beta", "sleep 30"])?);
    Ok((harness, daemon))
}

fn display_format(harness: &CliHarness, target: Option<&str>) -> Result<String, Box<dyn Error>> {
    let mut arguments = vec!["display-message", "-p"];
    if let Some(target) = target {
        arguments.extend(["-t", target]);
    }
    arguments.push(FORMAT);
    let output = harness.run(&arguments)?;
    assert_output_success(&output);
    assert!(stderr(&output).is_empty());
    Ok(stdout(&output).trim_end().to_owned())
}

fn spawn_delayed_run_shell(harness: &CliHarness) -> Result<Child, Box<dyn Error>> {
    let mut command = harness.base_command();
    command
        .args(["run-shell", "-d", "0.5", &format_print_command()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command.spawn()?)
}

fn spawn_delayed_command_run_shell(
    harness: &CliHarness,
    nested_command: &str,
) -> Result<Child, Box<dyn Error>> {
    let mut command = harness.base_command();
    command
        .args(["run-shell", "-C", "-d", "0.5", nested_command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command.spawn()?)
}

fn window_names(harness: &CliHarness, session: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let output = harness.run(&["list-windows", "-t", session, "-F", "#{window_name}"])?;
    assert_output_success(&output);
    Ok(stdout(&output).lines().map(ToOwned::to_owned).collect())
}

fn wait_until_delayed(child: &mut Child) -> Result<(), Box<dyn Error>> {
    std::thread::sleep(Duration::from_millis(100));
    if let Some(status) = child.try_wait()? {
        return Err(format!("delayed run-shell exited before mutation: {status}").into());
    }
    Ok(())
}

fn assert_output_success(output: &std::process::Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "command failed: stdout={:?} stderr={:?}",
        stdout(output),
        stderr(output)
    );
    assert!(stderr(output).is_empty(), "stderr={:?}", stderr(output));
}

fn wait_for_file_text(path: &std::path::Path, expected: &str) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = None;
    while Instant::now() < deadline {
        match fs::read_to_string(path) {
            Ok(contents) if contents == expected => return Ok(()),
            Ok(contents) => last = Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "timed out waiting for {} to contain {expected:?}; last={last:?}",
        path.display()
    )
    .into())
}

fn format_print_command() -> String {
    format!("printf '%s\\n' '{FORMAT}'")
}

fn format_write_command(path: &std::path::Path) -> String {
    format!("{} > {}", format_print_command(), shell_quote(path))
}

fn shell_quote(path: &std::path::Path) -> String {
    shell_quote_str(&path.display().to_string())
}

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
