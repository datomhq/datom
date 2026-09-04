//! End-to-end tests of `datom parse`: spawns the real binary against source
//! files in a temp dir and checks the printed tree, the diagnostics, and the
//! exit codes.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::tempdir;

/// Write `source` to `dir/input.datom` and run `datom parse` on it.
fn parse(dir: &Path, source: &str) -> Output {
    let file = dir.join("input.datom");
    std::fs::write(&file, source).expect("failed to write source file");

    Command::new(env!("CARGO_BIN_EXE_datom"))
        .args(["parse", file.to_str().expect("path should be utf-8")])
        .output()
        .expect("failed to spawn datom binary")
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).expect("stdout should be utf-8")
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("stderr should be utf-8")
}

#[test]
fn a_valid_file_prints_its_tree_and_succeeds() {
    let dir = tempdir().unwrap();
    let output = parse(
        dir.path(),
        "type Person(name: string, tags: list<string>)\n",
    );

    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "\
program
└─ single type `Person`
   ├─ field `name`: string
   └─ field `tags`: list<string>
"
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn an_empty_file_is_a_valid_empty_program() {
    let dir = tempdir().unwrap();
    let output = parse(dir.path(), "");

    assert!(output.status.success());
    assert_eq!(stdout(&output), "program\n");
}

#[test]
fn a_parse_error_fails_and_names_what_it_wanted() {
    let dir = tempdir().unwrap();
    let output = parse(dir.path(), "type Person(name string)\n");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("[1:18] error: Expected `:`, found `string`"));
}

#[test]
fn a_scan_error_reports_the_offending_character_and_its_position() {
    let dir = tempdir().unwrap();
    let output = parse(
        dir.path(),
        "type Person(id: number)\ntype Rob#t(id: number)\n",
    );

    assert!(!output.status.success());

    // the `#` is the ninth character of the second line
    assert!(stderr(&output).contains("[2:9] error: Unexpected character '#'"));
}

/// Diagnostics belong on stderr so the tree stays pipeable, and a failed
/// parse should not emit a partial tree at all.
#[test]
fn a_failed_parse_writes_nothing_to_stdout() {
    let dir = tempdir().unwrap();
    let output = parse(dir.path(), "type Person(name string)\n");

    assert!(!output.status.success());
    assert_eq!(stdout(&output), "");
}

#[test]
fn a_missing_file_fails_without_mentioning_syntax() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nope.datom");

    let output = Command::new(env!("CARGO_BIN_EXE_datom"))
        .args(["parse", missing.to_str().unwrap()])
        .output()
        .expect("failed to spawn datom binary");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("could not read"));
}
