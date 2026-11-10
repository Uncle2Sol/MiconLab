use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_list_command() {
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .arg("list")
        .assert()
        .success();
}

#[test]
fn test_list_json_format() {
    // JSON 格式始终输出有效 JSON 数组（即使无端口也输出 []）
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .args(["list", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("["));
}

#[test]
fn test_help_command() {
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("miconlab"));
}

#[test]
fn test_version_command() {
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("miconlab"));
}

#[test]
fn test_connect_help() {
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .args(["connect", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("baud"));
}

#[test]
fn test_send_help() {
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .args(["send", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hex"));
}

#[test]
fn test_monitor_help() {
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .args(["monitor", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("timestamp"));
}

#[test]
fn test_agent_help() {
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .args(["agent", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list-ports"));
}

#[test]
fn test_agent_list_ports() {
    Command::cargo_bin("miconlab-cli")
        .unwrap()
        .args(["agent", "list-ports"])
        .assert()
        .success()
        .stdout(predicate::str::contains("success"));
}
