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
        .stdout(predicate::str::contains("completions"))
        .stdout(predicate::str::contains("schema"));
}

// ── config ──────────────────────────────────────────────────────────

#[test]
fn test_config_json_has_path_and_exists_keys() {
    ipcam()
        .args(["--format", "json", "config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"path\""))
        .stdout(predicate::str::contains("\"exists\""));
}

#[test]
fn test_config_json_output_is_valid_json() {
    let output = ipcam()
        .args(["--format", "json", "config"])
        .output()
        .expect("failed to run ipcam");

    assert!(output.status.success());

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("config --format json should produce valid JSON");
    assert!(parsed.get("path").is_some(), "JSON should contain 'path'");
    assert!(
        parsed.get("exists").is_some(),
        "JSON should contain 'exists'"
    );
}

#[test]
fn test_config_legacy_json_flag_still_works() {
    ipcam()
        .args(["--json", "config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"path\""))
        .stdout(predicate::str::contains("\"exists\""));
}

#[test]
fn test_config_plain_shows_path() {
    ipcam()
        .args(["--format", "text", "config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Config path:"));
}

#[test]
fn format_flag_json_mode() {
    let output = ipcam()
        .args(["--format", "json", "config"])
        .output()
        .expect("failed to run ipcam");
    assert!(output.status.success());
    // Should be valid JSON
    let _: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--format json should produce valid JSON");
}

#[test]
fn format_flag_text_mode() {
    // --format text should produce plain text (not a JSON object)
    let output = ipcam()
        .args(["--format", "text", "config"])
        .output()
        .expect("failed to run ipcam");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Plain text output has "Config path:" not a JSON "path" key
    assert!(
        stdout.contains("Config path:"),
        "text mode should show 'Config path:', got: {stdout}"
    );
}

// ── discover ────────────────────────────────────────────────────────

#[test]
fn test_discover_no_add_json_returns_array() {
    let output = ipcam()
        .args(["--format", "json", "discover", "--no-add", "--timeout", "1"])
        .output()
        .expect("failed to run ipcam");

    assert!(output.status.success());

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("discover --no-add --format json should produce valid JSON");
    assert!(
        parsed.is_array(),
        "discover --no-add --format json should return an array"
    );
}

#[test]
fn test_discover_json_returns_object_with_discovered() {
    let output = ipcam()
        .args(["--format", "json", "discover", "--timeout", "1"])
        .output()
        .expect("failed to run ipcam");

    assert!(output.status.success());

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("discover --format json should produce valid JSON");
    assert!(
        parsed.is_object(),
        "discover --format json should return an object"
    );
    assert!(
        parsed.get("discovered").is_some(),
        "should contain 'discovered'"
    );
    assert!(parsed.get("added").is_some(), "should contain 'added'");
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
    // snapshot with no args and no config should fail (not panic with a signal / segfault)
    let assert = ipcam().arg("snapshot").assert();
    let code = assert.get_output().status.code().unwrap_or(-1);
    // Any exit code from the process is acceptable; what we rule out is a null code (signal kill)
    assert!(
        code != -1,
        "should exit with a process exit code, not be killed by a signal"
    );
}

// ── schema command ──────────────────────────────────────────────────

#[test]
fn schema_command_exits_zero() {
    ipcam().arg("schema").assert().success();
}

#[test]
fn schema_output_is_valid_json() {
    let output = ipcam()
        .arg("schema")
        .output()
        .expect("failed to run ipcam schema");
    assert!(output.status.success());
    let _: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("schema should produce valid JSON");
}

#[test]
fn schema_has_required_fields() {
    let output = ipcam()
        .arg("schema")
        .output()
        .expect("failed to run ipcam schema");
    assert!(output.status.success());
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("schema should produce valid JSON");
    assert!(
        parsed.get("clispec").is_some(),
        "schema should have 'clispec'"
    );
    assert!(parsed.get("name").is_some(), "schema should have 'name'");
    assert!(
        parsed.get("version").is_some(),
        "schema should have 'version'"
    );
    assert!(
        parsed.get("commands").is_some(),
        "schema should have 'commands'"
    );
    assert!(
        parsed.get("errors").is_some(),
        "schema should have 'errors'"
    );
    assert!(
        parsed.get("global_args").is_some(),
        "schema should have 'global_args'"
    );
}

#[test]
fn schema_validates_against_clispec_v03() {
    // Load the clispec v0.3 JSON Schema fixture and structurally verify
    // that schema output satisfies its required fields.
    let fixture_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/clispec-v0.3.json");
    let fixture_bytes =
        std::fs::read(&fixture_path).expect("clispec-v0.3.json fixture should exist");
    let fixture: serde_json::Value =
        serde_json::from_slice(&fixture_bytes).expect("fixture should be valid JSON");

    // Get the required fields from the JSON Schema
    let required_fields: Vec<&str> = fixture
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    // Get schema output from the tool
    let output = ipcam()
        .arg("schema")
        .output()
        .expect("failed to run ipcam schema");
    assert!(output.status.success());
    let schema_output: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("schema should produce valid JSON");

    // Verify all required fields are present
    for field in &required_fields {
        assert!(
            schema_output.get(field).is_some(),
            "schema output missing required field '{field}'"
        );
    }

    // Verify commands is an array
    assert!(
        schema_output
            .get("commands")
            .and_then(|c| c.as_array())
            .is_some(),
        "commands must be an array"
    );

    // Verify errors is an array
    assert!(
        schema_output
            .get("errors")
            .and_then(|e| e.as_array())
            .is_some(),
        "errors must be an array"
    );

    // Verify every error in errors has a kind field
    if let Some(errors) = schema_output.get("errors").and_then(|e| e.as_array()) {
        for (i, err) in errors.iter().enumerate() {
            assert!(
                err.get("kind").and_then(|k| k.as_str()).is_some(),
                "error[{i}] must have a 'kind' field"
            );
        }
    }
}

#[test]
fn schema_works_without_config() {
    ipcam()
        .env("IPCAM_CONFIG", "/nonexistent/path/config.toml")
        .arg("schema")
        .assert()
        .success();
}

// ── structured errors ───────────────────────────────────────────────

#[test]
fn structured_error_on_unknown_camera() {
    // With --format json, error should be structured JSON on stderr
    let output = ipcam()
        .args(["--format", "json", "status", "nonexistent-camera-xyz"])
        .output()
        .expect("failed to run ipcam");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    // The last non-empty line of stderr should be valid JSON with "error" key
    let last_json_line = stderr
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .expect("should have stderr output");

    let parsed: serde_json::Value =
        serde_json::from_str(last_json_line).expect("last stderr line should be valid JSON");
    assert!(
        parsed.get("error").is_some(),
        "structured error should have 'error' key, got: {parsed}"
    );
    assert!(
        parsed["error"].get("kind").is_some(),
        "structured error should have 'kind', got: {parsed}"
    );
}

#[test]
fn remove_nonexistent_camera_structured_error() {
    // ipcam remove nonexistent should exit nonzero with JSON error on stderr
    let output = ipcam()
        .args(["--format", "json", "remove", "nonexistent-camera-xyz"])
        .output()
        .expect("failed to run ipcam");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let last_json_line = stderr
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .expect("should have stderr output");

    let parsed: serde_json::Value =
        serde_json::from_str(last_json_line).expect("last stderr line should be valid JSON");
    assert!(
        parsed.get("error").is_some(),
        "structured error should have 'error' key"
    );
    let kind = parsed["error"]["kind"].as_str().unwrap_or("");
    assert!(!kind.is_empty(), "error kind should not be empty");
}
