//! The recognizer, checked against an independently derived reference (SCOPE.md 6.3, §9).
//!
//! A property test is only worth the independence of its oracle. A second set-of-positions
//! implementation would share any conceptual error with the first — the same misreading of
//! §6.3, the same wrong fixpoint cutoff — and agree with it enthusiastically. So the reference
//! here works a different way entirely: it **enumerates the language**.
//!
//! For a tiny grammar over `{a, b}` and a length bound, `language` computes, for every rule,
//! the set of strings that rule derives, by a least fixpoint over the rule table. Membership is
//! then a set lookup, and the property is `accepts(r, s) ⟺ s ∈ lang(r)` for every string up to
//! the bound — every acceptance *and* every rejection, not a sample of either.
//!
//! The enumerator is meant to be obviously correct by inspection rather than clever. It shares
//! no code with `recognize.rs`, which is the point.

use std::collections::BTreeSet;

use abnf_oracle::{
    CheckedGrammar, Grammar, MatchOptions, Node, NodeId, NumVal, Recognizer, RuleId,
};
use proptest::prelude::*;

/// Longest string considered. `{a, b}` up to length 6 is 127 strings — small enough to test
/// exhaustively for each generated grammar, which is what makes the rejections meaningful.
const MAX_LEN: usize = 6;

/// Every string over `{a, b}` of length at most [`MAX_LEN`].
fn universe() -> Vec<String> {
    let mut all = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..MAX_LEN {
        let mut next = Vec::new();
        for prefix in &frontier {
            for letter in ['a', 'b'] {
                let mut word = prefix.clone();
                word.push(letter);
                next.push(word);
            }
        }
        all.extend(next.iter().cloned());
        frontier = next;
    }
    all
}

/// The language of every user rule, up to [`MAX_LEN`].
///
/// A least fixpoint: each rule starts at the empty set and is recomputed from the current
/// estimate of the others until nothing changes. Sets only grow and are bounded by the finite
/// universe, so this terminates — including on recursive rules, though `check` has already
/// refused the left-recursive ones.
fn language(grammar: &CheckedGrammar) -> Vec<BTreeSet<String>> {
    let mut lang = vec![BTreeSet::new(); grammar.rules().len()];
    loop {
        let mut changed = false;
        for (index, rule) in grammar.rules().iter().enumerate() {
            let derived = strings_of(grammar, rule.body, &lang);
            if derived != lang[index] {
                lang[index] = derived;
                changed = true;
            }
        }
        if !changed {
            return lang;
        }
    }
}

/// The strings a node derives, given the current estimate of each rule's language.
fn strings_of(
    grammar: &CheckedGrammar,
    node: NodeId,
    lang: &[BTreeSet<String>],
) -> BTreeSet<String> {
    match grammar.node(node) {
        Node::Alt { branches } => branches
            .iter()
            .flat_map(|branch| strings_of(grammar, *branch, lang))
            .collect(),
        Node::Concat { items } => items.iter().fold(once(String::new()), |acc, item| {
            concat(&acc, &strings_of(grammar, *item, lang))
        }),
        Node::Repeat { repeat, body, .. } => {
            let inner = strings_of(grammar, *body, lang);
            // Beyond MAX_LEN copies nothing new can appear: a string of length <= MAX_LEN uses
            // at most that many non-empty pieces, and empty ones are free however many there
            // are. So the sweep can stop there whatever the declared maximum is.
            let ceiling = repeat.max.unwrap_or(u64::MAX).min(MAX_LEN as u64 + 1);
            let floor = repeat.min.min(ceiling);
            let mut derived = BTreeSet::new();
            for count in floor..=ceiling {
                let mut product = once(String::new());
                for _ in 0..count {
                    product = concat(&product, &inner);
                }
                derived.extend(product);
            }
            derived
        }
        Node::Optional { body } => {
            let mut derived = strings_of(grammar, *body, lang);
            derived.insert(String::new());
            derived
        }
        Node::RuleRef { .. } => match grammar.target(node) {
            Some(RuleId::User(index)) => lang[index as usize].clone(),
            // Generated grammars name only their own rules, so this cannot arise.
            _ => BTreeSet::new(),
        },
        // Terminals are lowercase by construction, so ASCII folding never applies here; it has
        // its own tests in `recognize.rs`.
        Node::CharVal(value) => once(value.value.clone()),
        Node::NumVal { value, .. } => match value {
            NumVal::Scalar(scalar) => char::from_u32(u32::try_from(*scalar).unwrap_or(0))
                .map(|c| once(c.to_string()))
                .unwrap_or_default(),
            NumVal::Range { lo, hi } => (*lo..=*hi)
                .filter_map(|v| char::from_u32(u32::try_from(v).ok()?))
                .map(|c| c.to_string())
                .collect(),
            NumVal::Concat(values) => once(
                values
                    .iter()
                    .filter_map(|v| char::from_u32(u32::try_from(*v).ok()?))
                    .collect(),
            ),
        },
        Node::ProseVal(_) => BTreeSet::new(),
    }
}

fn once(word: String) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    set.insert(word);
    set
}

/// Every `left + right` short enough to matter.
fn concat(left: &BTreeSet<String>, right: &BTreeSet<String>) -> BTreeSet<String> {
    let mut joined = BTreeSet::new();
    for prefix in left {
        for suffix in right {
            if prefix.len() + suffix.len() <= MAX_LEN {
                joined.insert(format!("{prefix}{suffix}"));
            }
        }
    }
    joined
}

// -- generating tiny grammars ------------------------------------------------------------

/// One element of a rule body, as text, so the parser is exercised too.
///
/// Every composite is parenthesized, which keeps the generated text unambiguous and means a
/// failing case can be pasted straight into a grammar file.
fn element(rule_count: usize) -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        Just("\"a\"".to_owned()),
        Just("\"b\"".to_owned()),
        Just("\"ab\"".to_owned()),
        Just("\"\"".to_owned()),
        Just("%x61".to_owned()),
        Just("%x61-62".to_owned()),
        (0..rule_count).prop_map(|index| format!("r{index}")),
    ];
    leaf.prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 2..=3)
                .prop_map(|parts| format!("({})", parts.join(" / "))),
            prop::collection::vec(inner.clone(), 2..=3)
                .prop_map(|parts| format!("({})", parts.join(" "))),
            inner.clone().prop_map(|body| format!("[{body}]")),
            (0_u32..=3, 0_u32..=3, inner).prop_map(|(a, b, body)| {
                // Ordered, so the generator never produces the inverted range `check` rejects.
                format!("{}*{}({body})", a.min(b), a.max(b))
            }),
        ]
    })
}

/// A grammar of one to four rules named `r0`..`r3`.
fn grammar_text() -> impl Strategy<Value = String> {
    (1_usize..=4).prop_flat_map(|count| {
        prop::collection::vec(element(count), count).prop_map(|bodies| {
            bodies
                .iter()
                .enumerate()
                .map(|(index, body)| format!("r{index} = {body}\r\n"))
                .collect()
        })
    })
}

proptest! {
    /// The recognizer agrees with the enumerator on every string up to the bound.
    #[test]
    fn the_recognizer_agrees_with_an_enumerated_language(source in grammar_text()) {
        let Ok(parsed) = Grammar::parse(&source) else {
            // The generator only emits valid ABNF; a parse failure is a bug in one of them.
            panic!("generated text did not parse:\n{source}");
        };
        // Left recursion is the one thing the generator can produce that `check` refuses, and
        // the enumerator would not agree about it anyway: the recognizer cannot decide it.
        let Ok(grammar) = parsed.check() else { return Ok(()) };

        let lang = language(&grammar);
        for word in universe() {
            let expected = lang[0].contains(&word);
            let actual = Recognizer::new(&grammar, &word)
                .accepts("r0")
                .expect("a generated grammar reaches no prose and no unrepresentable terminal");
            prop_assert_eq!(
                actual, expected,
                "{:?} on {:?}: recognizer said {}, the enumerated language says {}\n{}",
                "r0", word, actual, expected, source
            );
        }
    }

    /// Memoization changes the work and not the answer (invariant 4).
    #[test]
    fn memoization_is_invisible(source in grammar_text()) {
        let Ok(parsed) = Grammar::parse(&source) else { return Ok(()) };
        let Ok(grammar) = parsed.check() else { return Ok(()) };

        for word in universe() {
            let with = Recognizer::new(&grammar, &word).accepts("r0");
            let without = Recognizer::new(&grammar, &word)
                .without_memoization()
                .accepts("r0");
            prop_assert_eq!(with.is_ok(), without.is_ok(), "{}", source);
            if let (Ok(with), Ok(without)) = (with, without) {
                prop_assert_eq!(with, without, "on {:?}\n{}", word, source);
            }
        }
    }

    /// Every rule of a generated grammar is usable as a start rule, and agrees too.
    ///
    /// Not just `r0`: a bug reachable only from a rule nothing else references would hide from
    /// a test that always started at the same place.
    #[test]
    fn every_rule_agrees_not_only_the_first(source in grammar_text()) {
        let Ok(parsed) = Grammar::parse(&source) else { return Ok(()) };
        let Ok(grammar) = parsed.check() else { return Ok(()) };

        let lang = language(&grammar);
        for (index, rule) in grammar.rules().iter().enumerate() {
            let name = rule.name.as_str().to_owned();
            for word in universe() {
                let expected = lang[index].contains(&word);
                let actual = Recognizer::new(&grammar, &word)
                    .accepts(&name)
                    .expect("usable start rule");
                prop_assert_eq!(actual, expected, "{} on {:?}\n{}", name, word, source);
            }
        }
    }

    /// A step limit turns into an error, never into a different verdict (D16).
    #[test]
    fn a_step_limit_never_changes_an_answer_into_a_rejection(source in grammar_text()) {
        let Ok(parsed) = Grammar::parse(&source) else { return Ok(()) };
        let Ok(grammar) = parsed.check() else { return Ok(()) };

        for word in universe().into_iter().take(16) {
            let unlimited = Recognizer::new(&grammar, &word).accepts("r0").expect("decides");
            let mut limited = Recognizer::new(&grammar, &word)
                .with_options(MatchOptions { max_steps: Some(4), ..MatchOptions::default() });
            match limited.accepts("r0") {
                Ok(answer) => prop_assert_eq!(answer, unlimited, "{}", source),
                Err(error) => prop_assert!(
                    matches!(error, abnf_oracle::MatchError::StepLimit),
                    "a limit must report itself, not guess: {error}"
                ),
            }
        }
    }
}

#[test]
fn the_enumerator_is_right_about_grammars_we_can_check_by_hand() {
    // The oracle needs an oracle. These are small enough to verify by reading.
    for (source, expected) in [
        ("r0 = \"a\"\r\n", vec![""; 0]),
        ("r0 = \"\"\r\n", vec![""]),
        ("r0 = (\"a\" / \"b\")\r\n", vec!["a", "b"]),
        ("r0 = (\"a\" \"b\")\r\n", vec!["ab"]),
        ("r0 = [\"a\"]\r\n", vec!["", "a"]),
        ("r0 = 0*2(\"a\")\r\n", vec!["", "a", "aa"]),
        ("r0 = %x61-62\r\n", vec!["a", "b"]),
        ("r0 = r1\r\nr1 = \"b\"\r\n", vec!["b"]),
    ] {
        let grammar = Grammar::parse(source)
            .expect("parses")
            .check()
            .expect("checks");
        let lang = language(&grammar);
        let mut found: Vec<String> = lang[0].iter().cloned().collect();
        found.sort();
        let mut want: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
        want.sort();
        if source == "r0 = \"a\"\r\n" {
            want = vec!["a".to_owned()];
        }
        assert_eq!(found, want, "{source}");
    }
}

#[test]
fn unbounded_repetition_is_bounded_by_the_length_limit() {
    // `*("a")` derives infinitely many strings; the enumerator must stop at MAX_LEN rather
    // than iterating forever, and must not stop early either.
    let grammar = Grammar::parse("r0 = *(\"a\")\r\n")
        .expect("parses")
        .check()
        .expect("checks");
    let lang = language(&grammar);
    assert_eq!(
        lang[0].len(),
        MAX_LEN + 1,
        "the empty string plus one per length"
    );
    assert!(lang[0].contains("aaaaaa"));
    assert!(!lang[0].contains("aaaaaaa"));
}
