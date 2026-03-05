//! Integration test covering CLI surface.

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn help_output_includes_core_flags() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("whisper_input"));
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("--model-size"))
        .stdout(contains("--model-dir"))
        .stdout(contains("--hotkey-max-tap-ms"))
        .stdout(contains("--no-gpu"))
        .stdout(contains("--no-flash-attn"))
        .stdout(contains("--no-auto-paste"))
        .stdout(contains("--tool-transcribe-once"))
        .stdout(contains("--tool-record-seconds"))
        .stdout(contains("--tool-initial-prompt"))
        .stdout(contains("--tool-initial-prompt-file"));
}
