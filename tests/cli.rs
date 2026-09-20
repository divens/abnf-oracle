//! The `cli` feature's behaviour, exercised through the built binary.
//!
//! Exit codes are part of the contract (SCOPE.md 11) and are what a script depends on, so they
//! are asserted rather than smoke-tested by hand. The whole file compiles away without the
//! feature, since the binary does not exist then.
#![cfg(feature = "cli")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Deliberately broken fixtures.
const INVALID: &str = "invalid";

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
fn the_unimplemented_subcommand_says_so_rather_than_pretending() {
    let file = grammars().join("rfc3339-datetime.abnf");
    let output = run(&[
        "gen",
        file.to_str().expect("utf-8 path"),
        "--rule",
        "date-time",
    ]);
    assert_eq!(code(&output), ERROR);
    assert!(stderr(&output).contains("M3.4"), "{}", stderr(&output));
}

// -- `match` and the exit codes of SCOPE.md 11 -----------------------------------------------

/// The question was answered, and the answer was no.
const NO: i32 = 1;

fn json() -> String {
    grammars()
        .join("rfc8259-json.abnf")
        .to_str()
        .expect("utf-8 path")
        .to_owned()
}

fn dates() -> String {
    grammars()
        .join("rfc3339-datetime.abnf")
        .to_str()
        .expect("utf-8 path")
        .to_owned()
}

fn corpus(bucket: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
        .join("rfc8259")
        .join(bucket)
        .to_str()
        .expect("utf-8 path")
        .to_owned()
}

#[test]
fn a_single_input_answers_yes_or_no() {
    let accepted = run(&[
        "match",
        &dates(),
        "--rule",
        "date-time",
        "--input",
        "2026-09-20T12:00:00Z",
    ]);
    assert_eq!(code(&accepted), OK);

    let rejected = run(&["match", &dates(), "--rule", "date-time", "--input", "nope"]);
    assert_eq!(code(&rejected), NO);
}

#[test]
fn a_rejection_is_silent_on_stderr() {
    // Exit 1 is an answer, not a failure: nothing went wrong, the input simply does not match.
    let output = run(&["match", &dates(), "--rule", "date-time", "--input", "nope"]);
    assert_eq!(code(&output), NO);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
}

#[test]
fn an_unknown_rule_is_an_error_not_a_rejection() {
    let output = run(&["match", &dates(), "--rule", "nope", "--input", "x"]);
    assert_eq!(code(&output), ERROR);
    assert!(
        stderr(&output).contains("unknown rule"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_compatibility_limit_is_reported_before_the_input_is_read() {
    // RFC 9110 gives its URI rules as prose, so `Location` has no matching semantics at all.
    // Answering "does not match" would be a lie about a rule that cannot be matched (D11).
    let http = grammars().join("rfc9110-http.abnf");
    let output = run(&[
        "match",
        http.to_str().expect("utf-8 path"),
        "--rule",
        "Location",
        "--input",
        "http://example.test/",
    ]);
    assert_eq!(code(&output), ERROR);
    assert!(
        stderr(&output).contains("prose value"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn limits_are_errors_not_verdicts() {
    for (flag, value, needle) in [
        ("--max-steps", "3", "step limit"),
        ("--max-depth", "4", "depth limit"),
    ] {
        let output = run(&[
            "match",
            &json(),
            "--rule",
            "JSON-text",
            "--input",
            "[[[[[1]]]]]",
            flag,
            value,
        ]);
        assert_eq!(code(&output), ERROR, "{flag}: {}", stderr(&output));
        assert!(stderr(&output).contains(needle), "{}", stderr(&output));
    }
}

#[test]
fn the_depth_limit_applies_by_default() {
    // Without a finite default the process would abort rather than answer, and an abort is not
    // an exit code a caller can act on (D42).
    let deep = format!("{}1{}", "[".repeat(5_000), "]".repeat(5_000));
    let output = run(&["match", &json(), "--rule", "JSON-text", "--input", &deep]);
    assert_eq!(code(&output), ERROR);
    assert!(
        stderr(&output).contains("depth limit"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn input_can_come_from_a_file() {
    let dir = std::env::temp_dir().join("abnf-oracle-cli-file");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("date.txt");
    std::fs::write(&path, "2026-09-20T12:00:00Z").expect("write");

    let output = run(&[
        "match",
        &dates(),
        "--rule",
        "date-time",
        "--file",
        path.to_str().expect("utf-8 path"),
    ]);
    assert_eq!(code(&output), OK, "{}", stderr(&output));
}

#[test]
fn invalid_utf8_input_is_an_error_never_a_rejection() {
    // D14, and the reason the corpus keeps such files out of `reject/`: the recognizer never
    // sees them, so "does not match" would be answering a different question.
    let dir = std::env::temp_dir().join("abnf-oracle-cli-utf8");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("bad.txt");
    std::fs::write(&path, [0x22, 0xFF, 0x22]).expect("write");

    let output = run(&[
        "match",
        &json(),
        "--rule",
        "JSON-text",
        "--file",
        path.to_str().expect("utf-8 path"),
    ]);
    assert_eq!(code(&output), ERROR);
    assert!(stderr(&output).contains("UTF-8"), "{}", stderr(&output));
}

#[test]
fn a_directory_of_accepted_inputs_exits_zero() {
    let output = run(&[
        "match",
        &json(),
        "--rule",
        "JSON-text",
        "--dir",
        &corpus("accept"),
    ]);
    assert_eq!(code(&output), OK, "{}", stderr(&output));

    let listing = stdout(&output);
    let lines: Vec<&str> = listing.lines().collect();
    assert_eq!(lines.len(), 95, "one line per file");
    assert!(
        lines.iter().all(|line| line.ends_with("ACCEPT")),
        "every line should be ACCEPT"
    );
}

#[test]
fn a_directory_with_a_rejection_exits_one() {
    let output = run(&[
        "match",
        &json(),
        "--rule",
        "JSON-text",
        "--dir",
        &corpus("reject"),
    ]);
    assert_eq!(code(&output), NO);
    assert!(stdout(&output).lines().all(|line| line.ends_with("REJECT")));
}

#[test]
fn an_error_anywhere_in_a_directory_dominates() {
    // `indeterminate/` holds undecodable files, files that nest past the depth limit, and
    // ordinary acceptances and rejections. One unanswerable question makes the run
    // unanswered, whatever else it found (SCOPE.md 11).
    let output = run(&[
        "match",
        &json(),
        "--rule",
        "JSON-text",
        "--dir",
        &corpus("indeterminate"),
    ]);
    assert_eq!(code(&output), ERROR);

    let listing = stdout(&output);
    assert!(
        listing.contains("ACCEPT"),
        "and it kept going after the errors"
    );
    assert!(listing.contains("ERROR not valid UTF-8"));
    assert!(listing.contains("ERROR recursion depth limit exceeded"));
}

#[test]
fn a_directory_listing_is_one_line_per_file_and_sorted() {
    let output = run(&[
        "match",
        &json(),
        "--rule",
        "JSON-text",
        "--dir",
        &corpus("accept"),
    ]);
    let listing = stdout(&output);
    let names: Vec<&str> = listing
        .lines()
        .map(|line| line.split_whitespace().next().unwrap_or_default())
        .collect();

    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "sorted, so two runs can be diffed");
}

#[test]
fn exactly_one_input_source_is_required() {
    let none = run(&["match", &dates(), "--rule", "date-time"]);
    assert_eq!(code(&none), ERROR);

    let both = run(&[
        "match",
        &dates(),
        "--rule",
        "date-time",
        "--input",
        "x",
        "--file",
        "y",
    ]);
    assert_eq!(code(&both), ERROR, "clap rejects two sources");
}

#[test]
fn a_grammar_that_does_not_check_is_an_error() {
    let broken = grammars().join(INVALID).join("undefined-rule.abnf");
    let output = run(&[
        "match",
        broken.to_str().expect("utf-8 path"),
        "--rule",
        "start",
        "--input",
        "a",
    ]);
    assert_eq!(code(&output), ERROR);
    assert!(
        stderr(&output).contains("undefined rule"),
        "{}",
        stderr(&output)
    );
}
