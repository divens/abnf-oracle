//! Everything the generator produces, the recognizer accepts (SCOPE.md 8, M3).
//!
//! This is the property the generator exists for, and the two halves are genuinely independent:
//! one walks the grammar top-down choosing expansions, the other computes sets of end positions
//! bottom-up. A bug in either — a repetition count off by one, a case-folding rule applied to
//! the wrong kind of string — shows up as a string one produced and the other refuses.
//!
//! Coverage mode is not exercised here; that is M3.2.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use abnf_oracle::{CheckedGrammar, GenOptions, Generator, Grammar, MatchOptions, Recognizer};

/// Seeds per rule.
///
/// M3 asks for 200, which is 83,200 generate-and-recognize pairs across 416 generatable rules:
/// about a minute in release and several in debug. Nearly all of that is the recognizer
/// checking long generated strings, not the generator producing them.
///
/// So the default is a sample and CI runs the full sweep (see `.github/workflows/ci.yml`).
/// Override with `ABNF_ORACLE_SEEDS`:
///
/// ```text
/// ABNF_ORACLE_SEEDS=200 cargo test --release --test generate_roundtrip
/// ```
fn seeds() -> u64 {
    std::env::var("ABNF_ORACLE_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10)
}

/// Generated strings can nest more deeply than a default recognizer follows, so give it room
/// (D42); this is far below what the corpus needed.
fn match_options() -> MatchOptions {
    MatchOptions {
        max_depth: Some(4_096),
        ..MatchOptions::default()
    }
}

/// How long a generated string may get during the sweep.
///
/// Uncapped, `spread` 3 lets a repetition inside a repetition inside a recursive rule reach
/// several kilobytes — RFC 5322's `fields` hits 2,500 characters — and *verifying* those is
/// what costs: the recognizer clones a position set per memo hit (PLAN.md R9), so checking a
/// long string is superlinear in its length.
///
/// Capping the output rather than lowering `spread` keeps repetition properly exercised; it
/// only stops the sweep spending its time on a handful of enormous strings. CI runs the sweep
/// uncapped as well, so nothing is permanently out of reach.
fn sweep_limit() -> Option<usize> {
    match std::env::var("ABNF_ORACLE_MAX_LEN").ok().as_deref() {
        Some("none") => None,
        Some(value) => value.parse().ok(),
        None => Some(512),
    }
}

fn grammars_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("grammars")
}

fn fixtures() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(grammars_dir())
        .expect("readable")
        .filter_map(|entry| {
            let path = entry.expect("entry").path();
            (path.extension().and_then(|e| e.to_str()) == Some("abnf")).then_some(path)
        })
        .collect();
    found.sort();
    found
}

fn checked(path: &Path) -> CheckedGrammar {
    let source = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Grammar::parse(&source)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .check()
        .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()))
}

fn name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_owned()
}

/// Generates from `rule` with `seed` and checks the recognizer agrees.
fn round_trip(grammar: &CheckedGrammar, rule: &str, seed: u64, fixture: &str) {
    let mut generator = Generator::new(grammar, seed).with_options(GenOptions {
        max_output_len: sweep_limit(),
        ..GenOptions::default()
    });
    let produced = match generator.generate(rule) {
        Ok(text) => text,
        // A resource limit is a legitimate outcome, not a failure: some rules really do demand
        // more than the default budget allows (D29).
        // A resource limit is a legitimate outcome, not a failure: some rules really do demand
        // more than the default budget allows (D29). Reported so that a fixture quietly
        // producing nothing but limits would be visible.
        Err(error @ (abnf_oracle::GenError::OutputLimit | abnf_oracle::GenError::StepLimit)) => {
            eprintln!("{fixture} {rule} seed {seed}: {error}");
            return;
        }
        Err(error) => panic!("{fixture} {rule} seed {seed}: {error}"),
    };

    let accepted = Recognizer::new(grammar, &produced)
        .with_options(match_options())
        .accepts(rule)
        .unwrap_or_else(|e| panic!("{fixture} {rule} seed {seed}: recognizing {produced:?}: {e}"));

    assert!(
        accepted,
        "{fixture} {rule} seed {seed} produced {produced:?}, which the recognizer rejects"
    );
}

#[test]
fn every_fixture_generates_strings_its_own_grammar_accepts() {
    let fixtures = fixtures();
    assert!(fixtures.len() >= 10, "expected the fixture set");

    for path in fixtures {
        let fixture = name(&path);
        let grammar = checked(&path);

        // Every rule the grammar can generate from, not only a chosen entry point: a bug in a
        // rule nothing references would otherwise never be reached.
        for rule in grammar.rules() {
            let rule_name = rule.name.as_str();
            if grammar.can_generate(rule_name).is_err() || !grammar.min_len(rule.body).is_finite() {
                continue;
            }
            for seed in 0..seeds() {
                round_trip(&grammar, rule_name, seed, &fixture);
            }
        }
    }
}

#[test]
fn generation_is_deterministic() {
    // D15: the same seed, options and call sequence give the same strings. Asserted across a
    // sequence rather than one call, because the RNG advances as it goes.
    let grammar = checked(&grammars_dir().join("rfc3339-datetime.abnf"));

    let mut first = Generator::new(&grammar, 42);
    let mut second = Generator::new(&grammar, 42);
    for call in 0..100 {
        let left = first.generate("date-time").expect("generates");
        let right = second.generate("date-time").expect("generates");
        assert_eq!(left, right, "call {call}");
    }
}

#[test]
fn different_seeds_give_different_strings() {
    // Not a distribution test — just enough to catch a generator that ignores its seed, which
    // would pass every other test here.
    let grammar = checked(&grammars_dir().join("rfc8259-json.abnf"));
    let produced: BTreeSet<String> = (0..50)
        .map(|seed| {
            Generator::new(&grammar, seed)
                .generate("JSON-text")
                .expect("generates")
        })
        .collect();
    assert!(
        produced.len() > 10,
        "50 seeds produced only {} distinct strings",
        produced.len()
    );
}

#[test]
fn an_unproductive_start_rule_is_refused_immediately() {
    // Not discovered by walking into it: there is no string to produce, so there is nothing to
    // attempt (D9).
    let grammar = Grammar::parse("start = \"ok\" / bad\r\nbad = \"x\" bad\r\n")
        .expect("parses")
        .check()
        .expect("checks");

    assert!(matches!(
        Generator::new(&grammar, 0).generate("bad"),
        Err(abnf_oracle::GenError::NoFiniteExpansion { .. })
    ));
}

#[test]
fn a_dead_branch_is_never_emitted() {
    // The ban on unproductive branches is absolute, not a fallback for an exhausted budget
    // (D20). `start` has a dead branch; every seed must take the other one.
    let grammar = Grammar::parse("start = \"ok\" / bad\r\nbad = \"x\" bad\r\n")
        .expect("parses")
        .check()
        .expect("checks");

    for seed in 0..1_000 {
        let produced = Generator::new(&grammar, seed)
            .with_options(GenOptions {
                preserve_case: true,
                ..GenOptions::default()
            })
            .generate("start")
            .expect("generates");
        assert_eq!(produced, "ok", "seed {seed}");
    }
}

#[test]
fn prose_and_unrepresentable_terminals_are_refused() {
    let grammar = Grammar::parse("prosey = <anything>\r\nbad = %xD800-DFFF\r\nfine = \"x\"\r\n")
        .expect("parses")
        .check()
        .expect("checks");

    assert!(matches!(
        Generator::new(&grammar, 0).generate("prosey"),
        Err(abnf_oracle::GenError::ProseValueReachable { .. })
    ));
    assert!(matches!(
        Generator::new(&grammar, 0).generate("bad"),
        Err(abnf_oracle::GenError::UnrepresentableTerminal { .. })
    ));
    assert!(Generator::new(&grammar, 0).generate("fine").is_ok());
}

#[test]
fn an_unbounded_mandatory_repetition_reports_a_limit() {
    // `start = 1000000000*"a"` is productive and has no shorter expansion, so termination alone
    // does not save it. The output bound is what turns a gigabyte into an error (D29).
    let grammar = Grammar::parse("start = 1000000000*\"a\"\r\n")
        .expect("parses")
        .check()
        .expect("checks");

    assert!(matches!(
        Generator::new(&grammar, 0).generate("start"),
        Err(abnf_oracle::GenError::OutputLimit)
    ));
}

#[test]
fn witness_mode_terminates_where_choosing_would_not() {
    // With no depth budget at all, every choice is a witness, which is the case D28 exists for.
    // The grammar is SCOPE.md 6.4's tie case, in the spelling that actually checks (D41).
    let grammar = Grammar::parse("a = b / \"x\"\r\nb = \"z\" a / \"y\"\r\n")
        .expect("parses")
        .check()
        .expect("checks");

    for seed in 0..100 {
        let produced = Generator::new(&grammar, seed)
            .with_options(GenOptions {
                max_depth: 0,
                preserve_case: true,
                ..GenOptions::default()
            })
            .generate("a")
            .expect("terminates");
        assert_eq!(produced, "x", "seed {seed}: witness mode is deterministic");
    }
}

#[test]
fn recursive_grammars_terminate_on_every_seed() {
    // Random mode, deep recursion available, and a grammar that can always recurse further.
    // The depth budget is what stops it, by switching to witnesses rather than by failing.
    let grammar = Grammar::parse("start = \"(\" start \")\" / \"x\"\r\n")
        .expect("parses")
        .check()
        .expect("checks");

    for seed in 0..1_000 {
        let produced = Generator::new(&grammar, seed)
            .generate("start")
            .expect("terminates");
        let accepted = Recognizer::new(&grammar, &produced)
            .with_options(match_options())
            .accepts("start")
            .expect("recognizes");
        assert!(accepted, "seed {seed}: {produced:?}");
    }
}

#[test]
fn case_is_varied_unless_it_is_preserved() {
    let grammar = Grammar::parse("start = \"abc\"\r\n")
        .expect("parses")
        .check()
        .expect("checks");

    // A case-insensitive string matches either case, so a generator that always emitted one
    // would never exercise the other.
    let varied: BTreeSet<String> = (0..100)
        .map(|seed| {
            Generator::new(&grammar, seed)
                .generate("start")
                .expect("generates")
        })
        .collect();
    assert!(varied.len() > 1, "case was never varied: {varied:?}");
    for text in &varied {
        assert_eq!(text.to_ascii_lowercase(), "abc");
    }

    let preserved: BTreeSet<String> = (0..100)
        .map(|seed| {
            Generator::new(&grammar, seed)
                .with_options(GenOptions {
                    preserve_case: true,
                    ..GenOptions::default()
                })
                .generate("start")
                .expect("generates")
        })
        .collect();
    assert_eq!(preserved.len(), 1, "preserve_case should pin the output");
    assert!(preserved.contains("abc"));
}

#[test]
fn a_case_sensitive_string_is_never_varied() {
    // RFC 7405's `%s` means exactly these characters, so varying them would emit strings the
    // grammar does not accept — which the round-trip above would catch, but say it directly.
    let grammar = Grammar::parse("start = %s\"aBc\"\r\n")
        .expect("parses")
        .check()
        .expect("checks");

    for seed in 0..100 {
        let produced = Generator::new(&grammar, seed)
            .generate("start")
            .expect("generates");
        assert_eq!(produced, "aBc", "seed {seed}");
    }
}
