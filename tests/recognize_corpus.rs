//! Every `accept/` input matches its grammar and every `reject/` input does not.
//!
//! The corpus is the source of truth (SCOPE.md 9): one input per file, stored as raw bytes, so
//! that a checkout cannot quietly change what is being tested. Files that cannot be decoded as
//! UTF-8 are a *test error* rather than a rejection — the matching domain is Unicode scalar
//! values, so an undecodable file is asking a question the recognizer was never given.
//!
//! `indeterminate/` is recorded and not asserted. See each corpus directory's `NOTES.md` for
//! why a file is there; every one of them is a property of this crate's input domain rather
//! than of the grammar.
//!
//! Everything runs on a large stack. The recognizer descends recursively at roughly 10 KB per
//! level of input nesting, and the 2 MB a test thread gets by default overflows at around 200 —
//! well inside what this corpus contains (PLAN.md R1).

use std::fs;
use std::path::{Path, PathBuf};

use abnf_oracle::{CheckedGrammar, Grammar, MatchError, Recognizer};

/// Reaches about 5,000 levels of nesting, which covers every file here.
const STACK: usize = 64 * 1024 * 1024;

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
}

fn grammars_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("grammars")
}

/// Runs `body` with room to recurse.
fn with_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(body)
        .expect("spawns")
        .join()
        .expect("the corpus fits in the stack it was given");
}

fn checked(fixture: &str) -> CheckedGrammar {
    let source = fs::read_to_string(grammars_root().join(fixture))
        .unwrap_or_else(|e| panic!("reading {fixture}: {e}"));
    Grammar::parse(&source)
        .unwrap_or_else(|e| panic!("{fixture}: {e}"))
        .check()
        .unwrap_or_else(|e| panic!("{fixture}: {e:?}"))
}

fn inputs(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("entry").path();
            path.is_file()
                .then_some(path)
                .filter(|p| p.extension().is_some_and(|e| e != "md"))
        })
        .collect();
    found.sort();
    found
}

/// Reads one corpus input. A decoding failure is a test error, not a verdict (SCOPE.md 9).
fn read_input(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    String::from_utf8(bytes).unwrap_or_else(|_| {
        panic!(
            "{} is not valid UTF-8, so it cannot be recognized at all; it belongs in \
             indeterminate/ with a note",
            path.display()
        )
    })
}

fn name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_owned()
}

/// Checks one corpus directory against one start rule.
fn run_corpus(corpus: &str, fixture: &'static str, rule: &'static str) {
    let dir = corpus_root().join(corpus);
    let accept = inputs(&dir.join("accept"));
    let reject = inputs(&dir.join("reject"));

    assert!(!accept.is_empty(), "{corpus}/accept is empty");
    assert!(!reject.is_empty(), "{corpus}/reject is empty");

    with_stack(move || {
        let grammar = checked(fixture);
        for path in accept {
            let input = read_input(&path);
            let matched = Recognizer::new(&grammar, &input)
                .accepts(rule)
                .unwrap_or_else(|e| panic!("{}: {e}", name(&path)));
            assert!(matched, "{} should match {rule}", name(&path));
        }
        for path in reject {
            let input = read_input(&path);
            let matched = Recognizer::new(&grammar, &input)
                .accepts(rule)
                .unwrap_or_else(|e| panic!("{}: {e}", name(&path)));
            assert!(!matched, "{} should not match {rule}", name(&path));
        }
    });
}

#[test]
fn rfc8259_json() {
    run_corpus("rfc8259", "rfc8259-json.abnf", "JSON-text");
}

#[test]
fn the_json_corpus_is_the_whole_suite() {
    // 318 files upstream, split three ways. A corpus that silently lost half its files would
    // still pass every assertion above.
    let dir = corpus_root().join("rfc8259");
    let accept = inputs(&dir.join("accept")).len();
    let reject = inputs(&dir.join("reject")).len();
    let indeterminate = inputs(&dir.join("indeterminate")).len();
    assert_eq!(
        (accept, reject, indeterminate),
        (95, 174, 49),
        "the corpus changed; update NOTES.md and this count together"
    );
    assert_eq!(accept + reject + indeterminate, 318);
}

#[test]
fn indeterminate_inputs_are_recorded_not_asserted() {
    // The suite leaves these to the implementation, so asserting either way would invent a
    // requirement. What is asserted is that they do not *crash*: whatever answer comes back,
    // or an error, is fine.
    let dir = corpus_root().join("rfc8259").join("indeterminate");
    let files = inputs(&dir);
    assert!(!files.is_empty());

    with_stack(move || {
        let grammar = checked("rfc8259-json.abnf");
        let mut decided = 0;
        for path in files {
            // Undecodable files live here precisely because they cannot be recognized; skip
            // them rather than failing, which is the whole reason they were moved.
            let Ok(input) = String::from_utf8(fs::read(&path).expect("readable")) else {
                continue;
            };
            if Recognizer::new(&grammar, &input)
                .accepts("JSON-text")
                .is_ok()
            {
                decided += 1;
            }
        }
        // 24 of the 49 files here are valid UTF-8: 22 of the 35 `i_` cases, plus the two
        // 100,000-deep `n_` files. Three of those nest past the default depth limit — the two
        // giants and `i_structure_500_nested_arrays.json` — and report a limit rather than a
        // verdict. That leaves 21 decided either way.
        assert_eq!(decided, 21, "decidable inputs in indeterminate/");
    });
}

#[test]
fn the_corpus_carries_its_licence_and_provenance() {
    let dir = corpus_root().join("rfc8259");
    for required in ["LICENSE", "PROVENANCE.md", "NOTES.md"] {
        assert!(dir.join(required).is_file(), "{required} is missing");
    }
    let provenance = fs::read_to_string(dir.join("PROVENANCE.md")).expect("readable");
    assert!(
        provenance.contains("JSONTestSuite") && provenance.contains("commit "),
        "provenance must name the source and the commit it was taken from"
    );
}

#[test]
fn the_deepest_inputs_report_a_limit_instead_of_crashing() {
    // The two 100,000-deep files are why `MatchOptions::max_depth` has a finite default. On
    // an ordinary thread, with ordinary options, they now answer "could not decide" rather
    // than aborting the process — which is what every other limit in this crate does.
    let dir = corpus_root().join("rfc8259").join("indeterminate");
    let grammar = checked("rfc8259-json.abnf");

    for file in [
        "n_structure_100000_opening_arrays.json",
        "n_structure_open_array_object.json",
    ] {
        let input = read_input(&dir.join(file));
        assert!(
            matches!(
                Recognizer::new(&grammar, &input).accepts("JSON-text"),
                Err(MatchError::DepthLimit)
            ),
            "{file} should hit the depth limit"
        );
    }
}
