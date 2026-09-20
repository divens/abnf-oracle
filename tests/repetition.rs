//! The repetition regression table of SCOPE.md 6.3, which is mandatory (D3).
//!
//! Revision 1 of the spec had a fixpoint cutoff that was wrong for nullable elements with a
//! positive minimum — `3*3 ["a"]` on empty input must accept — so every row here exists
//! because some plausible implementation gets it wrong.
//!
//! Every row runs twice, with the memo table on and off. Memoization is supposed to change
//! only the work done, and a table that only ever ran one way could not tell.

use abnf_oracle::{Grammar, MatchOptions, Recognizer};

/// Builds a one-rule grammar around `elements` and asks whether it matches `input`.
fn accepts(elements: &str, input: &str) -> bool {
    let source = format!("start = {elements}\r\n");
    let grammar = Grammar::parse(&source)
        .unwrap_or_else(|e| panic!("{source:?}: {e}"))
        .check()
        .unwrap_or_else(|e| panic!("{source:?}: {e:?}"));

    let mut memoized = Recognizer::new(&grammar, input);
    let with = memoized.accepts("start").expect("recognizes");

    let mut plain = Recognizer::new(&grammar, input).without_memoization();
    let without = plain.accepts("start").expect("recognizes");

    assert_eq!(
        with, without,
        "{elements:?} on {input:?}: memoization changed the answer"
    );
    with
}

/// The steps taken on `input`, which is what the O(input) claims are asserted against (D31).
fn steps(elements: &str, input: &str) -> u64 {
    let source = format!("start = {elements}\r\n");
    let grammar = Grammar::parse(&source)
        .expect("parses")
        .check()
        .expect("checks");
    let mut recognizer = Recognizer::new(&grammar, input);
    recognizer.accepts("start").expect("recognizes");
    recognizer.steps()
}

#[test]
fn a_repeat_binds_tightly_to_its_element() {
    // `repetition = [repeat] element` admits nothing between the bounds and what they repeat,
    // so the readable spelling SCOPE.md 6.3 uses in its table — `3*3 ["a"]` — is not valid
    // ABNF. Every row below therefore uses the tight form.
    assert!(Grammar::parse("start = 3*3[\"a\"]\r\n").is_ok());
    assert!(
        Grammar::parse("start = 3*3 [\"a\"]\r\n").is_err(),
        "whitespace after a repeat would need `repetition = [repeat] *c-wsp element`"
    );
}

// -- the table -----------------------------------------------------------------------------

#[test]
fn nullable_body_with_a_positive_minimum() {
    // The case revision 1 got wrong. Three repetitions of an optional can match nothing at
    // all, because each repetition is allowed to match nothing.
    assert!(accepts("3*3[\"a\"]", ""));
    assert!(accepts("3*3[\"a\"]", "aa"));
    assert!(
        !accepts("3*3[\"a\"]", "aaaa"),
        "three repetitions cannot make four"
    );
}

#[test]
fn an_exact_count_is_a_minimum_too() {
    assert!(!accepts("2*2\"a\"", "a"));
    assert!(accepts("2*2\"a\"", "aa"));
}

#[test]
fn unbounded_repetition_of_a_nullable_body_terminates() {
    assert!(accepts("*[\"a\"]", ""));
    assert!(accepts("*[\"a\"]", "aaa"));
}

#[test]
fn a_nullable_alternative_inside_a_repetition() {
    assert!(accepts("1*(\"a\" / \"\")", ""));
    assert!(accepts("1*(\"a\" / \"\")", "aa"));
}

#[test]
fn bounded_repetition_of_a_multi_character_body() {
    assert!(accepts("2*4\"ab\"", "ababab"));
    assert!(
        !accepts("2*4\"ab\"", "ababababab"),
        "five is past the maximum"
    );
    assert!(!accepts("2*4\"ab\"", "ab"), "one is below the minimum");
}

#[test]
fn an_enormous_minimum_over_a_nullable_body_is_prompt() {
    // Phase 1 stops at the first exact fixpoint, so this costs one iteration rather than a
    // billion. A subset test in place of the equality test would loop here.
    assert!(accepts("1000000000*[\"a\"]", ""));
    let taken = steps("1000000000*[\"a\"]", "");
    assert!(taken <= 4, "took {taken} steps for an empty input");
}

#[test]
fn an_enormous_minimum_over_a_consuming_body_is_prompt() {
    assert!(!accepts("1000000000*\"a\"", "aaa"));
    let taken = steps("1000000000*\"a\"", "aaa");
    assert!(
        taken <= 4 * (3 + 1),
        "took {taken} steps for a three-character input"
    );
}

#[test]
fn work_stays_linear_in_the_input() {
    // The O(input) property stated directly, rather than as a timeout: every repetition
    // either consumes a scalar or reaches a fixpoint, so the iteration count is bounded by
    // the input length however large the bounds are (D19, D31).
    for length in [0, 1, 8, 64, 512] {
        let input = "a".repeat(length);
        let taken = steps("1000000000*\"a\"", &input);
        let bound = 4 * (length as u64 + 1);
        assert!(
            taken <= bound,
            "{length} characters took {taken} steps, over {bound}"
        );
    }
}

#[test]
fn a_shrinking_position_set_does_not_end_phase_one() {
    // Found by the property test in `recognize_property.rs`, which is how it earned its place
    // here: the twelve mandatory rows all pass with phase 1's equality test weakened to a
    // subset test, and this does not.
    //
    // `r0` needs three repetitions of `"a" r1`, each consuming one or two characters, so it
    // matches three to six. On "aa" the reachable set goes {0} -> {1,2} -> {2}: it *shrinks*,
    // and {2} is a subset of {1,2}. A subset test would stop there and accept after two
    // repetitions. Equality does not, because the two sets are not equal.
    let source = "r0 = 3*3((\"a\" r1))
r1 = 0*1(%x61)
";
    let grammar = Grammar::parse(source)
        .expect("parses")
        .check()
        .expect("checks");

    for (input, expected) in [
        ("aa", false),
        ("aaa", true),
        ("aaaaaa", true),
        ("aaaaaaa", false),
    ] {
        let matched = Recognizer::new(&grammar, input)
            .accepts("r0")
            .expect("recognizes");
        assert_eq!(matched, expected, "{input:?}");
    }
}

// -- the rows that are structural errors rather than matches ---------------------------------

#[test]
fn an_inverted_repeat_range_never_reaches_the_recognizer() {
    let errors = Grammar::parse("start = 5*2\"a\"\r\n")
        .expect("parses")
        .check()
        .expect_err("does not check");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, abnf_oracle::CheckError::InvalidRepeatRange { .. })),
        "{errors:?}"
    );
}

#[test]
fn an_inverted_numeric_range_never_reaches_the_recognizer() {
    let errors = Grammar::parse("start = %x5A-41\r\n")
        .expect("parses")
        .check()
        .expect_err("does not check");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, abnf_oracle::CheckError::InvalidNumericRange { .. })),
        "{errors:?}"
    );
}

// -- limits ----------------------------------------------------------------------------------

#[test]
fn the_step_limit_is_an_error_not_a_rejection() {
    // Conflating "could not decide" with "does not match" is what would make this useless as
    // an oracle (D16).
    let grammar = Grammar::parse("start = 1000000000*\"a\"\r\n")
        .expect("parses")
        .check()
        .expect("checks");
    let input = "a".repeat(64);
    let mut recognizer = Recognizer::new(&grammar, &input).with_options(MatchOptions {
        max_steps: Some(3),
        ..Default::default()
    });
    assert!(matches!(
        recognizer.accepts("start"),
        Err(abnf_oracle::MatchError::StepLimit)
    ));
}

#[test]
fn no_limit_is_the_default() {
    let grammar = Grammar::parse("start = *\"a\"\r\n")
        .expect("parses")
        .check()
        .expect("checks");
    let input = "a".repeat(2048);
    let mut recognizer = Recognizer::new(&grammar, &input);
    assert!(recognizer.accepts("start").expect("recognizes"));
    assert!(recognizer.steps() > 3, "and it really did do the work");
}
