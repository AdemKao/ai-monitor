use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_exposes_provider_commands() {
    Command::cargo_bin("ai-monitor")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("overview"))
        .stdout(predicate::str::contains("codex"))
        .stdout(predicate::str::contains("opencode"));
}

#[test]
fn lowercase_version_flag_prints_version() {
    Command::cargo_bin("ai-monitor")
        .unwrap()
        .arg("-v")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("ai-monitor "));
}

#[test]
fn optimize_requires_explicit_confirmation() {
    Command::cargo_bin("ai-monitor")
        .unwrap()
        .args(["opencode", "optimize", "create"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("rerun with --yes"));
}

#[test]
fn generates_shell_completion() {
    Command::cargo_bin("ai-monitor")
        .unwrap()
        .args(["completion", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef ai-monitor"))
        .stdout(predicate::str::contains("--no-private-api"))
        .stdout(predicate::str::contains("dashboard"));
}

#[test]
fn codex_usage_help_exposes_dashboard_options() {
    Command::cargo_bin("ai-monitor")
        .unwrap()
        .args(["codex", "usage", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-private-api"));
}

#[test]
fn opencode_dashboard_help_exposes_breakdown_options() {
    Command::cargo_bin("ai-monitor")
        .unwrap()
        .args(["opencode", "dashboard", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--all-projects"))
        .stdout(predicate::str::contains("--top-projects"))
        .stdout(predicate::str::contains("--top-agents"))
        .stdout(predicate::str::contains("--top-relationships"));
}

#[test]
fn usage_help_exposes_time_range_presets() {
    Command::cargo_bin("ai-monitor")
        .unwrap()
        .args(["opencode", "usage", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--range"))
        .stdout(predicate::str::contains("today"))
        .stdout(predicate::str::contains("yesterday"))
        .stdout(predicate::str::contains("30-days"));
}
