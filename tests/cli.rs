//! The `cli` feature's behaviour, exercised through the built binary.
//!
//! Exit codes are part of the contract (SCOPE.md 11) and are what a script depends on, so they
//! are asserted rather than smoke-tested by hand. The whole file compiles away without the
//! feature, since the binary does not exist then.
#![cfg(feature = "cli")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The question was answered; the grammar is usable.
const OK: i32 = 0;
/// The question could not be answered at all.
const ERROR: i32 = 2;

fn grammars() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("grammars")
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_abnf-oracle"))
        .args(args)
        .output()
        .expect("the binary runs")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exited normally")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

fn fixtures(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("readable")
        .filter_map(|entry| {
            let path = entry.expect("entry").path();
            (path.extension().and_then(|e| e.to_str()) == Some("abnf")).then_some(path)
        })
        .collect();
    found.sort();
    found
}

#[test]
fn every_valid_fixture_checks_and_lists() {
    let files = fixtures(&grammars());
    assert!(
        files.len() >= 9,
        "expected the fixture set, found {}",
        files.len()
    );

    for path in files {
        let file = path.to_str().expect("utf-8 path");

        let checked = run(&["check", file]);
        assert_eq!(code(&checked), OK, "check {file}:\n{}", stderr(&checked));

        let listed = run(&["rules", file]);
        assert_eq!(code(&listed), OK, "rules {file}:\n{}", stderr(&listed));

        // A header plus one row per rule, and the counts the two subcommands report agree.
        let rows = stdout(&listed).lines().count() - 1;
        let summary = stdout(&checked);
        let reported: usize = summary
            .lines()
            .find_map(|line| {
                let (count, rest) = line.split_once(' ')?;
                rest.starts_with("rule").then(|| count.parse().ok())?
            })
            .unwrap_or_else(|| panic!("no rule count in:\n{summary}"));
        assert_eq!(
            rows, reported,
            "{file} lists {rows} rules but reports {reported}"
        );
    }
}

#[test]
fn every_broken_fixture_is_an_error_not_a_rejection() {
    for path in fixtures(&grammars().join("invalid")) {
        let file = path.to_str().expect("utf-8 path");
        let output = run(&["check", file]);
        assert_eq!(
            code(&output),
            ERROR,
            "{file} should exit {ERROR}:\n{}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("structural error"),
            "{file} should say what was wrong:\n{}",
            stderr(&output)
        );
    }
}

#[test]
fn errors_are_reported_with_their_location() {
    let file = grammars()
        .join("invalid")
        .join("invalid-numeric-range.abnf");
    let output = run(&["check", file.to_str().expect("utf-8 path")]);
    let stderr = stderr(&output);

    assert!(stderr.contains("line 7, column 19"), "{stderr}");
    assert!(
        stderr.contains("%x5A-41"),
        "the offending text is quoted back: {stderr}"
    );
    assert!(stderr.contains('^'), "with a caret under it: {stderr}");
}

#[test]
fn naming_entry_points_changes_which_warnings_appear() {
    let file = grammars().join("rfc8259-json.abnf");
    let file = file.to_str().expect("utf-8 path");

    let bare = stdout(&run(&["check", file]));
    assert!(
        bare.contains("--start"),
        "without entry points, say how to get the rest: {bare}"
    );

    let with_start = stdout(&run(&["check", file, "--start", "JSON-text"]));
    assert!(!with_start.contains("--start"), "{with_start}");
    assert!(
        !with_start.contains("is not reachable"),
        "every JSON rule is reachable from JSON-text: {with_start}"
    );
}

#[test]
fn warnings_do_not_fail_the_check() {
    // Advisory only (D5). The entry rule of any grammar is unreferenced, so exiting non-zero
    // on warnings would fail on every grammar there is.
    let file = grammars().join("rfc8259-json.abnf");
    let output = run(&["check", file.to_str().expect("utf-8 path")]);
    assert!(stdout(&output).contains("warning:"));
    assert_eq!(code(&output), OK);
}

#[test]
fn the_rules_table_carries_the_analyses() {
    let file = grammars().join("rfc9110-http.abnf");
    let listed = stdout(&run(&["rules", file.to_str().expect("utf-8 path")]));

    assert!(
        listed.starts_with("rule "),
        "a header row: {}",
        &listed[..40.min(listed.len())]
    );
    assert!(
        listed
            .lines()
            .any(|line| line.starts_with("Location ") && line.contains("not a usable start rule")),
        "a rule reaching prose is flagged"
    );
    assert!(
        listed
            .lines()
            .any(|line| line.starts_with("Allow ") && line.contains("yes")),
        "and a nullable one is marked nullable"
    );
}

#[test]
fn a_missing_file_is_an_error() {
    let output = run(&["check", "no/such/grammar.abnf"]);
    assert_eq!(code(&output), ERROR);
    assert!(stderr(&output).contains("reading"), "{}", stderr(&output));
}

#[test]
fn the_unimplemented_subcommands_say_so_rather_than_pretending() {
    for (subcommand, milestone) in [("match", "M2.7"), ("gen", "M3.4")] {
        let file = grammars().join("rfc3339-datetime.abnf");
        let output = run(&[
            subcommand,
            file.to_str().expect("utf-8 path"),
            "--rule",
            "date-time",
        ]);
        assert_eq!(code(&output), ERROR);
        assert!(stderr(&output).contains(milestone), "{}", stderr(&output));
    }
}
