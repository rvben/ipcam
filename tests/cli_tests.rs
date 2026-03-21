use assert_cmd::Command;
use predicates::prelude::*;

fn ipcam() -> Command {
    Command::cargo_bin("ipcam").expect("binary 'ipcam' should exist")
}

// ── help ────────────────────────────────────────────────────────────

#[test]
fn test_help_exits_zero_and_shows_description() {
    ipcam()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Manage IP cameras"));
}

#[test]
fn test_help_lists_subcommands() {
    ipcam()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("snapshot"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("discover"))
        .stdout(predicate::str::contains("completions"));
}

// ── config ──────────────────────────────────────────────────────────

#[test]
fn test_config_json_has_path_and_exists_keys() {
    ipcam()
        .args(["--json", "config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"path\""))
        .stdout(predicate::str::contains("\"exists\""));
}

#[test]
fn test_config_json_output_is_valid_json() {
    let output = ipcam()
        .args(["--json", "config"])
        .output()
        .expect("failed to run ipcam");

    assert!(output.status.success());

    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("config --json should produce valid JSON");
    assert!(parsed.get("path").is_some(), "JSON should contain 'path'");
    assert!(
        parsed.get("exists").is_some(),
        "JSON should contain 'exists'"
    );
}

#[test]
fn test_config_plain_shows_path() {
    ipcam()
        .args(["config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Config path:"));
}

// ── discover ────────────────────────────────────────────────────────

#[test]
fn test_discover_json_returns_valid_json_array() {
    let output = ipcam()
        .args(["--json", "discover", "--timeout", "1"])
        .output()
        .expect("failed to run ipcam");

    assert!(output.status.success());

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("discover --json should produce valid JSON");
    assert!(parsed.is_array(), "discover --json should return an array");
}

// ── no subcommand ───────────────────────────────────────────────────

#[test]
fn test_no_subcommand_fails() {
    ipcam().assert().failure();
}

// ── unknown subcommand ──────────────────────────────────────────────

#[test]
fn test_unknown_subcommand_fails() {
    ipcam()
        .arg("nonexistent-subcommand")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

// ── snapshot without camera argument ────────────────────────────────

#[test]
fn test_snapshot_no_args_requires_camera_or_flag() {
    // `snapshot` with no camera name and no --all/--grid should either
    // fail or list cameras (depends on whether config exists), but it
    // should not panic.
    let assert = ipcam().arg("snapshot").assert();
    // We just verify it doesn't panic (exit code 101 with signal)
    let code = assert.get_output().status.code().unwrap_or(-1);
    assert!(code == 0 || code == 1, "should exit cleanly, got {code}");
}
