//! The parser accepts what the canonical self-grammar generates (D35, M3).
//!
//! This is the other half of `self_definition.rs`. There the self-grammar judged the fixtures;
//! here it *writes* grammars and the hand-written parser has to accept every one. Together the
//! two close SCOPE.md 4.2's invariant in both directions:
//!
//! ```text
//! L(grammar) ⊆ L(parser)   here: text generated from the grammar, which the parser must accept
//! L(parser) ⊆ L(grammar)   self_definition.rs: text the parser accepts, which the grammar must
//!                          recognize — sampled over the fixtures, since the parser's language
//!                          cannot be enumerated
//! ```
//!
//! Neither half is worth much alone. A parser that accepted everything would pass this file and
//! fail `self_definition.rs`'s negative controls; a self-grammar that generated nothing
//! interesting would pass both. What makes this one bite is that the generated text is far
//! stranger than anything in the fixture set — tabs, comments mid-rule, continuation lines,
//! `=/`, uppercase `%X`/`%B`/`%S`/`%I` markers, empty strings, prose values — while remaining
//! valid ABNF by construction.

use std::fs;
use std::path::Path;

use abnf_oracle::{CheckedGrammar, GenOptions, Generator, Grammar};

/// Generated grammars per run. SCOPE.md M3 asks for 500.
const SAMPLES: usize = 500;

/// The production a whole grammar file is an instance of.
const START: &str = "rulelist";

fn self_grammar() -> CheckedGrammar {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("grammars")
        .join("abnf-canonical.abnf");
    let source = fs::read_to_string(&path).expect("the canonical self-grammar is a fixture");
    Grammar::parse(&source)
        .expect("it parses")
        .check()
        .expect("it checks")
}

/// A generator over the self-grammar.
///
/// `spread` is pinned rather than left to the default. Not because the default is wrong — at 3
/// the widest number generated is four digits — but because the test's meaning depends on it:
/// `num-val` draws its digits from `1*HEXDIG`, and 17 hex digits overflow a `u64`, so a spread
/// of 16 or more starts producing `ParseError::NumberTooLarge`. That is a real and documented
/// divergence (D17, and `self_definition.rs` pins it directly), but it is not what this test is
/// about, and a future change to the default should not quietly turn this into a test of it.
fn generator(grammar: &CheckedGrammar, seed: u64) -> Generator<'_> {
    Generator::new(grammar, seed).with_options(GenOptions {
        coverage: true,
        spread: 3,
        ..GenOptions::default()
    })
}

#[test]
fn everything_the_self_grammar_writes_the_parser_accepts() {
    let grammar = self_grammar();
    let mut generator = generator(&grammar, 0);

    let mut parsed = 0;
    for call in 0..SAMPLES {
        let text = generator
            .generate(START)
            .unwrap_or_else(|e| panic!("call {call}: {e}"));

        Grammar::parse(&text).unwrap_or_else(|error| {
            panic!(
                "call {call}: the self-grammar wrote a grammar the parser refuses.\n\
                 {}\n\
                 text: {text:?}",
                error.render(&text)
            )
        });
        parsed += 1;
    }
    assert_eq!(parsed, SAMPLES);
}

#[test]
fn the_same_holds_across_many_seeds() {
    // One seed explores one path through the grammar. Coverage mode makes that path a good one,
    // but a shape reachable only under a different seed would still hide from a single run.
    let grammar = self_grammar();

    for seed in 0..40 {
        let mut generator = generator(&grammar, seed);
        for call in 0..25 {
            let text = generator.generate(START).expect("generates");
            Grammar::parse(&text).unwrap_or_else(|error| {
                panic!(
                    "seed {seed} call {call}:\n{}\ntext: {text:?}",
                    error.render(&text)
                )
            });
        }
    }
}

#[test]
fn generated_grammars_round_trip() {
    // The M1 round-trip contract, exercised on input far more adversarial than the fixtures:
    // reparsing the canonical form gives an equal grammar, and printing it again is a fixpoint.
    let grammar = self_grammar();
    let mut generator = generator(&grammar, 7);

    for call in 0..SAMPLES {
        let text = generator.generate(START).expect("generates");
        let original = Grammar::parse(&text).expect("parses");

        let printed = original.to_string();
        let reparsed = Grammar::parse(&printed).unwrap_or_else(|error| {
            panic!(
                "call {call}: canonical form does not parse.\n{}\nfrom: {text:?}",
                error.render(&printed)
            )
        });

        assert_eq!(
            original, reparsed,
            "call {call}: changed under round-trip\n{text:?}"
        );
        assert_eq!(
            printed,
            reparsed.to_string(),
            "call {call}: canonical form is not a fixpoint\n{text:?}"
        );
    }
}

#[test]
fn generated_grammars_are_ascii_by_construction() {
    // Every terminal in the self-grammar is an ASCII range, so this cannot fail without the
    // generator or the fixture being wrong. Asserted rather than assumed, because D35's whole
    // invariant rests on the parser and the grammar agreeing about the character set.
    let grammar = self_grammar();
    let mut generator = generator(&grammar, 3);

    for _ in 0..SAMPLES {
        let text = generator.generate(START).expect("generates");
        assert!(
            text.is_ascii(),
            "the self-grammar produced a non-ASCII byte: {text:?}"
        );
    }
}

#[test]
fn generated_grammars_exercise_more_than_the_fixtures_do() {
    // A guard against the test passing for the wrong reason. If coverage mode ever stopped
    // steering — or the self-grammar fixture lost a production — this would still pass while
    // testing almost nothing, so check that the output really is varied.
    let grammar = self_grammar();
    let mut generator = generator(&grammar, 11);

    let mut seen_comment = false;
    let mut seen_incremental = false;
    let mut seen_prose = false;
    let mut seen_case_sensitive = false;
    let mut seen_continuation = false;
    let mut seen_uppercase_radix = false;

    for _ in 0..SAMPLES {
        let text = generator.generate(START).expect("generates");
        seen_comment |= text.contains(';');
        seen_incremental |= text.contains("=/");
        seen_prose |= text.contains('<');
        seen_case_sensitive |= text.contains("%s") || text.contains("%S");
        seen_continuation |= text.contains("\r\n ") || text.contains("\r\n\t");
        seen_uppercase_radix |= text.contains("%X") || text.contains("%B") || text.contains("%D");
    }

    assert!(seen_comment, "no comment was generated");
    assert!(seen_incremental, "no `=/` was generated");
    assert!(seen_prose, "no prose value was generated");
    assert!(seen_case_sensitive, "no %s string was generated");
    assert!(seen_continuation, "no line continuation was generated");
    assert!(
        seen_uppercase_radix,
        "no uppercase radix marker was generated; \
         the self-grammar spells these case-insensitively, so they are part of the language"
    );
}

#[test]
fn a_generated_grammar_is_syntax_only() {
    // Generated grammars reference rule names nothing defines, so nearly all of them fail
    // `check`. That is expected and is the point of the two-layer split: the self-grammar
    // describes ABNF's *syntax*, and says nothing about whether a name resolves (D32).
    let grammar = self_grammar();
    let mut generator = generator(&grammar, 5);

    let mut checked = 0;
    for _ in 0..100 {
        let text = generator.generate(START).expect("generates");
        if Grammar::parse(&text).expect("parses").check().is_ok() {
            checked += 1;
        }
    }
    assert!(
        checked < 100,
        "every generated grammar checked, which suggests they are all trivial"
    );
}
