use std::fs;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn ghostbuster() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ghostbuster"))
}

#[test]
fn reports_dead_code_and_uses_exit_three() {
    let directory = TempDir::new().expect("create temporary directory");
    let source = directory.path().join("dead.py");
    fs::write(&source, "def abandoned():\n    pass\n").expect("write Python source");

    let output = ghostbuster()
        .arg(&source)
        .output()
        .expect("run ghostbuster");

    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(stdout.contains("V103 unused function 'abandoned' (60% confidence)"));
    assert!(output.stderr.is_empty());
}

#[test]
fn emits_machine_readable_json() {
    let directory = TempDir::new().expect("create temporary directory");
    let source = directory.path().join("dead.py");
    fs::write(&source, "import abandoned\n").expect("write Python source");

    let output = ghostbuster()
        .args(["--format", "json"])
        .arg(&source)
        .output()
        .expect("run ghostbuster");

    assert_eq!(output.status.code(), Some(3));
    let findings: Value = serde_json::from_slice(&output.stdout).expect("parse JSON output");
    assert_eq!(findings[0]["code"], "V104");
    assert_eq!(findings[0]["name"], "abandoned");
}

#[test]
fn clean_and_invalid_inputs_have_distinct_exit_codes() {
    let directory = TempDir::new().expect("create temporary directory");
    let clean = directory.path().join("clean.py");
    let broken = directory.path().join("broken.py");
    fs::write(&clean, "print('alive')\n").expect("write clean Python source");
    fs::write(&broken, "def broken(:\n").expect("write invalid Python source");

    let clean_output = ghostbuster().arg(&clean).output().expect("run ghostbuster");
    let broken_output = ghostbuster()
        .arg(&broken)
        .output()
        .expect("run ghostbuster");

    assert_eq!(clean_output.status.code(), Some(0));
    assert_eq!(broken_output.status.code(), Some(1));
    assert!(
        String::from_utf8(broken_output.stderr)
            .expect("stderr is UTF-8")
            .contains("broken.py")
    );
}

#[test]
fn invalid_patterns_use_the_argument_error_exit_code() {
    let directory = TempDir::new().expect("create temporary directory");
    let output = ghostbuster()
        .args(["--exclude", "["])
        .arg(directory.path())
        .output()
        .expect("run ghostbuster");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr is UTF-8")
            .contains("invalid exclude pattern")
    );
}
