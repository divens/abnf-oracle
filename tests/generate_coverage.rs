//! Coverage mode: the guarantee, and the cases that would break it (SCOPE.md 6.8, M3).
//!
//! The claim under test is strong and deliberately so: **while any reachable coverage unit is
//! uncovered, every successful call covers at least one new one**. So full coverage takes at
//! most `uncovered(rule)` calls — a bound, asserted exactly, not a probability. Three things
//! make it hold, and each has a test here that fails without it:
//!
//! * an untaken branch is preferred outright (rule 1);
//! * otherwise the walk *chases* the nearest uncovered unit by a strictly decreasing distance,
//!   which terminates where "any branch that reaches one" would not (rule 2, D38);
//! * a repetition that can reach something uncovered runs at least once, since a zero count
//!   would skip past the units being counted (D25).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use abnf_oracle::{CheckedGrammar, GenOptions, Generator, Grammar, MatchOptions, Recognizer};

fn checked(source: &str) -> CheckedGrammar {
    Grammar::parse(source)
        .unwrap_or_else(|e| panic!("{source:?}: {e}"))
        .check()
        .unwrap_or_else(|e| panic!("{source:?}: {e:?}"))
}

fn fixture(name: &str) -> CheckedGrammar {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("grammars")
        .join(name);
    checked(&fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}")))
}

fn covering(grammar: &CheckedGrammar, seed: u64) -> Generator<'_> {
    Generator::new(grammar, seed).with_options(GenOptions {
        coverage: true,
        ..GenOptions::default()
    })
}

/// Drives a generator to full coverage, asserting the bound holds at every step.
///
/// The bound is *at most* `N` calls, not exactly `N`: one call walks a whole derivation and
/// routinely covers several units on the way. What must hold is that no call covers nothing
/// while something reachable is still uncovered — that is the step the guarantee rests on, and
/// the total falls out of it.
///
/// Returns what it produced, so a caller can check *what* was generated as well as how much.
fn drive_to_coverage(grammar: &CheckedGrammar, rule: &str, seed: u64) -> Vec<String> {
    let mut generator = covering(grammar, seed);
    let total = generator.uncovered(rule).expect("countable");
    let mut produced = Vec::new();

    for call in 0..total {
        let before = generator.uncovered(rule).expect("countable");
        if before == 0 {
            return produced;
        }

        produced.push(generator.generate(rule).expect("generates"));

        let after = generator.uncovered(rule).expect("countable");
        assert!(
            after < before,
            "{rule}: call {call} covered nothing — {before} of {total} units still uncovered"
        );
    }

    assert_eq!(
        generator.uncovered(rule).expect("countable"),
        0,
        "{rule}: {total} calls did not cover all {total} units"
    );
    produced
}

#[test]
fn the_bound_holds_on_the_json_grammar() {
    // M3's headline case: a real grammar, and the bound asserted exactly.
    let grammar = fixture("rfc8259-json.abnf");
    let produced = drive_to_coverage(&grammar, "JSON-text", 0);

    // Everything it produced is still valid JSON — steering must not cost correctness.
    for text in &produced {
        let accepted = Recognizer::new(&grammar, text)
            .with_options(MatchOptions {
                max_depth: Some(4_096),
                ..MatchOptions::default()
            })
            .accepts("JSON-text")
            .expect("recognizes");
        assert!(
            accepted,
            "coverage mode produced {text:?}, which is not JSON"
        );
    }
}

#[test]
fn the_bound_holds_on_every_fixture() {
    // Not only JSON: a grammar whose shape defeats the chase would show up here.
    for name in [
        "rfc3339-datetime.abnf",
        "rfc7405-case-sensitivity.abnf",
        "rfc5234-core.abnf",
        "abnf-canonical.abnf",
    ] {
        let grammar = fixture(name);
        let rule = grammar.rules()[0].name.as_str().to_owned();
        if grammar.can_generate(&rule).is_err() {
            continue;
        }
        drive_to_coverage(&grammar, &rule, 0);
    }
}

#[test]
fn an_unproductive_alternative_does_not_break_the_bound() {
    // M3 asks for this specifically: the dead branch is not a unit, so it is not counted, and
    // the bound still holds over what remains.
    let grammar = checked("start = \"ok\" / \"alt\" / bad\r\nbad = \"x\" bad\r\n");
    let produced = drive_to_coverage(&grammar, "start", 0);

    for text in &produced {
        assert_ne!(
            text.to_ascii_lowercase(),
            "x",
            "the dead branch was emitted"
        );
    }
}

#[test]
fn units_inside_a_dead_branch_are_not_counted() {
    // The nested case from SCOPE.md 6.8: `"p"` and `"q"` have finite `min_len` of their own but
    // sit inside an unproductive branch, so they are not generatable and must not be counted.
    let grammar = checked("start = \"ok\" / \"alt\" / bad\r\nbad = (\"p\" / \"q\") bad\r\n");
    let mut generator = covering(&grammar, 0);

    assert_eq!(
        generator.uncovered("start").expect("countable"),
        2,
        "only the two productive branches of `start` are units"
    );
    drive_to_coverage(&grammar, "start", 0);
}

#[test]
fn units_inside_a_zero_repetition_are_not_counted() {
    // `*0(x)` can never run its body, so nothing inside it is reachable (D36).
    let grammar = checked("start = *0(\"a\" / \"b\") (\"c\" / \"d\")\r\n");
    let mut generator = covering(&grammar, 0);

    assert_eq!(
        generator.uncovered("start").expect("countable"),
        2,
        "only `\"c\" / \"d\"` contributes units"
    );
    drive_to_coverage(&grammar, "start", 0);
}

#[test]
fn a_repetition_runs_at_least_once_to_reach_what_is_inside_it() {
    // D25. `*("a" / "b")` can legally produce the empty string every time, which would cover
    // nothing at all; in coverage mode it must run.
    let grammar = checked("start = *(\"a\" / \"b\")\r\n");
    let mut generator = covering(&grammar, 0);

    let total = generator.uncovered("start").expect("countable");
    assert_eq!(total, 2, "one unit per branch");

    let first = generator.generate("start").expect("generates");
    let second = generator.generate("start").expect("generates");
    assert_eq!(
        generator.uncovered("start").expect("countable"),
        0,
        "both branches covered in at most two calls, got {first:?} then {second:?}"
    );
}

#[test]
fn the_chase_is_deterministic_and_does_not_wander() {
    // SCOPE.md 6.8's worked case. Both branches of `a` reach the uncovered unit, so "prefer any
    // branch that reaches one, ties at random" could loop through `b` emitting "z" until the
    // output limit trips. The distance measure picks `c` outright.
    //
    // `preserve_case` so the output is exact; the grammar is all case-insensitive strings.
    let grammar = checked("a = b / c\r\nb = \"z\" a\r\nc = \"x\" / \"y\"\r\n");

    // Only the calls that actually chase: once everything is covered the walk falls back to
    // random choice, which is seed-dependent by design and says nothing about the chase.
    let run = |seed: u64| -> Vec<String> {
        let mut generator = Generator::new(&grammar, seed).with_options(GenOptions {
            coverage: true,
            preserve_case: true,
            ..GenOptions::default()
        });
        let mut produced = Vec::new();
        while generator.uncovered("a").expect("countable") > 0 {
            produced.push(generator.generate("a").expect("generates"));
        }
        produced
    };

    let first = run(0);
    assert_eq!(first, run(12_345), "the chase consults no randomness");

    // `b` is a unit of `a` in its own right, so it is taken once — but only once, and never
    // as a lap on the way to something else. A wandering chase would emit "z" repeatedly.
    let laps: usize = first.iter().map(|text| text.matches('z').count()).sum();
    assert!(laps <= 1, "the chase wandered through `b`: {first:?}");
    assert!(
        first.iter().any(|text| text.ends_with('x'))
            && first.iter().any(|text| text.ends_with('y')),
        "both branches of `c` should have been covered: {first:?}"
    );
}

#[test]
fn coverage_is_independent_of_the_depth_budget() {
    // D38, and the case JSON is too shallow to catch: the only uncovered units sit behind a
    // chain of five rule references, with a budget of two. Without suspending the budget along
    // a chase, witness mode would cut the walk short and the bound would fail.
    let grammar = checked(
        "start = one\r\none = two\r\ntwo = three\r\nthree = four\r\nfour = five\r\n\
         five = \"x\" / \"y\"\r\n",
    );

    let mut generator = Generator::new(&grammar, 0).with_options(GenOptions {
        coverage: true,
        max_depth: 2,
        preserve_case: true,
        ..GenOptions::default()
    });

    let total = generator.uncovered("start").expect("countable");
    assert_eq!(total, 2, "the two branches of `five`");

    let mut produced = BTreeSet::new();
    for call in 0..total {
        let before = generator.uncovered("start").expect("countable");
        produced.insert(generator.generate("start").expect("generates"));
        assert!(
            generator.uncovered("start").expect("countable") < before,
            "call {call} covered nothing with max_depth = 2"
        );
    }
    assert_eq!(produced, BTreeSet::from(["x".to_owned(), "y".to_owned()]));
}

#[test]
fn coverage_only_counts_what_the_start_rule_can_reach() {
    // Units are start-rule-relative (D10): a rule nothing reachable references contributes
    // nothing to this rule's bound.
    let grammar = checked("start = \"a\" / \"b\"\r\nelsewhere = \"c\" / \"d\"\r\n");
    let mut generator = covering(&grammar, 0);

    assert_eq!(generator.uncovered("start").expect("countable"), 2);
    assert_eq!(generator.uncovered("elsewhere").expect("countable"), 2);

    drive_to_coverage(&grammar, "start", 0);

    let mut after = covering(&grammar, 0);
    after.generate("start").expect("generates");
    assert!(
        after.uncovered("elsewhere").expect("countable") > 0,
        "covering `start` says nothing about `elsewhere`"
    );
}

#[test]
fn a_failed_call_covers_nothing() {
    // Coverage commits on success. A call that ends in `OutputLimit` produced no output, so
    // counting what it touched would let the bound be satisfied by strings nobody ever saw.
    let grammar = checked("start = (\"a\" / \"b\") 1000000000\"c\"\r\n");
    let mut generator = Generator::new(&grammar, 0).with_options(GenOptions {
        coverage: true,
        max_output_len: Some(8),
        ..GenOptions::default()
    });

    let before = generator.uncovered("start").expect("countable");
    assert!(
        generator.generate("start").is_err(),
        "the output limit should trip"
    );
    assert_eq!(
        generator.uncovered("start").expect("countable"),
        before,
        "a call that produced nothing covered nothing"
    );
}

#[test]
fn an_optional_contributes_two_units() {
    // `[x]` is `x / empty` for coverage purposes, and `*1x` canonicalizes to it, so the two
    // spellings are indistinguishable here (SCOPE.md 6.8).
    for source in ["start = [\"a\"]\r\n", "start = *1\"a\"\r\n"] {
        let grammar = checked(source);
        let mut generator = covering(&grammar, 0);
        assert_eq!(
            generator.uncovered("start").expect("countable"),
            2,
            "{source:?}: taking the body and skipping it are both units"
        );

        let produced: BTreeSet<String> = (0..2)
            .map(|_| {
                generator
                    .generate("start")
                    .expect("generates")
                    .to_ascii_lowercase()
            })
            .collect();
        assert_eq!(produced, BTreeSet::from([String::new(), "a".to_owned()]));
    }
}

#[test]
fn coverage_mode_still_produces_strings_the_grammar_accepts() {
    // Steering changes which strings appear, never whether they are valid.
    let grammar = fixture("rfc3339-datetime.abnf");
    let mut generator = covering(&grammar, 7);

    for _ in 0..40 {
        let text = generator.generate("date-time").expect("generates");
        let accepted = Recognizer::new(&grammar, &text)
            .accepts("date-time")
            .expect("recognizes");
        assert!(accepted, "{text:?}");
    }
}

#[test]
fn coverage_mode_is_deterministic() {
    let grammar = fixture("rfc8259-json.abnf");
    let first: Vec<String> = (0..20)
        .scan(covering(&grammar, 99), |generator, _| {
            Some(generator.generate("JSON-text").expect("generates"))
        })
        .collect();
    let second: Vec<String> = (0..20)
        .scan(covering(&grammar, 99), |generator, _| {
            Some(generator.generate("JSON-text").expect("generates"))
        })
        .collect();
    assert_eq!(first, second);
}

#[test]
fn core_rules_report_their_own_coverage() {
    // The reachability cache used to be keyed by an index into the user-rule slice. Implicit
    // core rules have no such index, so they all collapsed onto one entry and the second one
    // asked returned the first one's answer. Keyed by body `NodeId` they are distinct (D46).
    let grammar = checked("start = ALPHA HEXDIG\r\n");

    // ALPHA is `%x41-5A / %x61-7A`: two branches. HEXDIG is DIGIT plus six letters: seven.
    let mut forwards = covering(&grammar, 0);
    assert_eq!(forwards.uncovered("ALPHA").expect("countable"), 2);
    assert_eq!(forwards.uncovered("HEXDIG").expect("countable"), 7);

    // The same two answers when asked the other way round, which is the whole point.
    let mut backwards = covering(&grammar, 0);
    assert_eq!(backwards.uncovered("HEXDIG").expect("countable"), 7);
    assert_eq!(backwards.uncovered("ALPHA").expect("countable"), 2);
}

#[test]
fn user_and_core_rules_share_one_generator_without_interfering() {
    let grammar = checked("start = ALPHA HEXDIG\r\nother = \"p\" / \"q\" / \"r\"\r\n");
    let mut generator = covering(&grammar, 0);

    // Asked repeatedly and interleaved: every answer must be the rule's own, every time.
    let expected = [
        ("ALPHA", 2),
        ("other", 3),
        ("HEXDIG", 7),
        ("ALPHA", 2),
        ("other", 3),
    ];
    for (name, count) in expected {
        assert_eq!(
            generator.uncovered(name).expect("countable"),
            count,
            "{name} reported the wrong count"
        );
    }
}

#[test]
fn a_rule_with_no_alternation_has_nothing_to_cover() {
    // `DIGIT = %x30-39` is a single range: one way to write it, so no choice to exercise.
    // Worth pinning because zero is also what a *broken* cache lookup would plausibly return.
    let grammar = checked("start = DIGIT\r\n");
    let mut generator = covering(&grammar, 0);
    assert_eq!(generator.uncovered("DIGIT").expect("countable"), 0);

    // And `start`, which only references it, inherits that: still nothing to cover.
    assert_eq!(generator.uncovered("start").expect("countable"), 0);
}

#[test]
fn the_bound_holds_when_generating_from_a_core_rule() {
    // Coverage mode was never broken by the cache — the chase reads distances, not this cache —
    // but the guarantee is worth asserting on core rules too, since `uncovered` is how a caller
    // would check it and that is what was wrong.
    let grammar = checked("start = HEXDIG\r\n");
    let produced = drive_to_coverage(&grammar, "HEXDIG", 0);
    assert!(
        !produced.is_empty(),
        "driving a core rule to full coverage should generate something"
    );
}
