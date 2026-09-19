//! The canonical self-grammar reads every fixture in the suite (D23).
//!
//! This is one direction of the invariant that SCOPE.md 4.2 rests on: *the hand-written parser
//! accepts exactly the language of the canonical self-grammar*. Here the grammar judges the
//! files; in M3 the generator runs it the other way, producing grammar text that the parser
//! must accept. Neither half means much alone — a parser that accepted everything would pass
//! this, and a self-grammar that accepted everything would too, which is why the negative
//! controls below matter as much as the fixtures.
//!
//! The canonical self-grammar is RFC 5234 §4 plus both verified errata plus RFC 7405 §2.2. It
//! recognizes itself, which is the point of the exercise.
//!
//! Line endings are normalized here rather than relied upon: the self-grammar demands CRLF,
//! fixtures are stored with LF (see `.gitattributes`), and a test whose result depended on
//! checkout settings would be worse than no test.

use std::fs;
use std::path::{Path, PathBuf};

use abnf_oracle::{CheckedGrammar, Grammar, Recognizer};

/// The production a whole grammar file is an instance of.
const START: &str = "rulelist";

fn grammars_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("grammars")
}

/// The grammar this crate implements, as a grammar.
fn self_grammar() -> CheckedGrammar {
    let source = fs::read_to_string(grammars_dir().join("abnf-canonical.abnf"))
        .expect("the canonical self-grammar is a fixture");
    Grammar::parse(&source)
        .expect("it parses")
        .check()
        .expect("it checks")
}

/// Every `.abnf` under `dir`, not recursing.
fn fixtures_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("entry").path();
            (path.extension().and_then(|e| e.to_str()) == Some("abnf")).then_some(path)
        })
        .collect();
    found.sort();
    found
}

/// A fixture's text with CRLF endings, whatever the checkout did to it.
fn crlf(path: &Path) -> String {
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

fn name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_owned()
}

fn recognizes(grammar: &CheckedGrammar, text: &str) -> bool {
    Recognizer::new(grammar, text)
        .accepts(START)
        .unwrap_or_else(|e| panic!("recognizing: {e}"))
}

#[test]
fn the_self_grammar_reads_every_valid_fixture() {
    let grammar = self_grammar();
    let fixtures = fixtures_in(&grammars_dir());
    assert!(
        fixtures.len() >= 10,
        "expected the fixture set, found {}",
        fixtures.len()
    );

    for path in fixtures {
        assert!(
            recognizes(&grammar, &crlf(&path)),
            "the canonical self-grammar does not accept {}",
            name(&path)
        );
    }
}

#[test]
fn the_self_grammar_reads_itself() {
    // The property that makes the fixture worth having: if it could not describe itself, it
    // would not be a description of ABNF.
    let grammar = self_grammar();
    let text = crlf(&grammars_dir().join("abnf-canonical.abnf"));
    assert!(recognizes(&grammar, &text));
}

#[test]
fn the_self_grammar_reads_case_sensitive_strings() {
    // D23 asks for this specifically. RFC 7405 is recent enough that no RFC grammar in the
    // fixture set uses `%s` or `%i`, so one exists to exercise them.
    let grammar = self_grammar();
    let text = crlf(&grammars_dir().join("rfc7405-case-sensitivity.abnf"));
    assert!(
        text.contains("%s\"aBc\"") && text.contains("%i\"aBc\""),
        "the fixture drifted"
    );
    assert!(recognizes(&grammar, &text));
}

#[test]
fn the_self_grammar_reads_grammars_that_fail_to_check() {
    // `invalid/` holds grammars that are syntactically fine and semantically broken — an
    // undefined reference, a duplicate definition, left recursion. Syntax is all the
    // self-grammar judges, so it must accept every one of them. A self-grammar that rejected
    // them would be conflating the two layers this crate keeps apart (D32).
    let grammar = self_grammar();
    for path in fixtures_in(&grammars_dir().join("invalid")) {
        assert!(
            recognizes(&grammar, &crlf(&path)),
            "{} is valid ABNF that fails `check`; the self-grammar should still read it",
            name(&path)
        );
    }
}

#[test]
fn the_self_grammar_rejects_what_the_parser_rejects() {
    // `invalid/parse/` holds text that is not ABNF at all. Both halves of the invariant should
    // refuse it — except for the one documented divergence, below.
    let grammar = self_grammar();
    let text = crlf(
        &grammars_dir()
            .join("invalid")
            .join("parse")
            .join("non-ascii-comment.abnf"),
    );

    assert!(Grammar::parse(&text).is_err(), "the parser rejects it");
    assert!(
        !recognizes(&grammar, &text),
        "and so does the self-grammar: RFC 5234 restricts comments to WSP / VCHAR"
    );
}

#[test]
fn the_one_documented_divergence_is_the_numeric_limit() {
    // D35 says the parser accepts exactly the self-grammar's language, "subject only to the
    // documented line-ending normalization and the u64 numeric-magnitude restriction". This is
    // that restriction, pinned: `repeat` is `1*DIGIT` with no bound, so a 25-digit repetition
    // is valid ABNF that this crate refuses.
    let grammar = self_grammar();
    let text = crlf(
        &grammars_dir()
            .join("invalid")
            .join("parse")
            .join("number-too-large.abnf"),
    );

    assert!(
        recognizes(&grammar, &text),
        "the self-grammar has no numeric bound, so it reads this happily"
    );
    assert!(
        matches!(
            Grammar::parse(&text),
            Err(abnf_oracle::ParseError::NumberTooLarge { .. })
        ),
        "while this crate stores bounds as u64 and says so"
    );
}

#[test]
fn the_self_grammar_is_not_simply_permissive() {
    // Without these, a self-grammar that accepted any text at all would pass every test above.
    let grammar = self_grammar();
    for (text, why) in [
        ("", "the empty string is not a rulelist"),
        ("start\r\n", "a rule needs a definition"),
        ("start = \r\n", "and something to define it as"),
        ("= \"a\"\r\n", "a definition needs a name"),
        ("start = \"a\"", "a rule ends with a line ending"),
        ("start = (\"a\"\r\n", "an unclosed group"),
        ("start = \"a\r\n", "an unclosed string"),
        ("start = %q41\r\n", "an unknown numeric radix"),
        (
            "start = 3*3 [\"a\"]\r\n",
            "whitespace between a repeat and its element",
        ),
        (" start = \"a\"\r\n", "a rule may not be indented"),
    ] {
        assert!(
            !recognizes(&grammar, text),
            "the self-grammar accepted {text:?}, but {why}"
        );
    }
}

#[test]
fn the_parser_and_the_self_grammar_agree_on_the_negative_cases() {
    // The same list, from the other side: whatever the self-grammar refuses, the parser
    // refuses too. This is the direction that would catch a parser gone lenient.
    for text in [
        "",
        "start\r\n",
        "start = \r\n",
        "= \"a\"\r\n",
        "start = \"a\"",
        "start = (\"a\"\r\n",
        "start = \"a\r\n",
        "start = %q41\r\n",
        "start = 3*3 [\"a\"]\r\n",
        " start = \"a\"\r\n",
    ] {
        assert!(
            Grammar::parse(text).is_err(),
            "the parser accepted {text:?}"
        );
    }
}

#[test]
fn line_endings_are_normalized_by_the_test_and_not_by_luck() {
    // The self-grammar demands CRLF. The parser is the lenient one (SCOPE.md 4.2), so a
    // fixture checked out with LF would fail here if this test did not convert it — and would
    // pass on a machine that checked out CRLF, which is the worst kind of test.
    let grammar = self_grammar();
    let lf = "start = \"a\"\n";
    let crlf = "start = \"a\"\r\n";

    assert!(Grammar::parse(lf).is_ok(), "the parser accepts LF");
    assert!(!recognizes(&grammar, lf), "the self-grammar does not");
    assert!(recognizes(&grammar, crlf));
}
