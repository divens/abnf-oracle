//! Every fixture under `tests/grammars/` parses, round-trips, and is ASCII-clean.
//!
//! The fixtures are transcribed from RFC text, never from memory or a third-party copy, and
//! each carries a header comment naming its RFC, section and the errata applied (SCOPE.md 9).
//! That provenance is the point: a fixture that quietly drifts from the RFC it claims to be
//! would make every test that depends on it meaningless.
//!
//! Deliberately broken fixtures live under `invalid/`, each naming the `CheckError` it is
//! there to produce; `invalid/parse/` holds the ones that fail earlier, at parse time.

use std::fs;
use std::path::{Path, PathBuf};

use abnf_oracle::{CheckError, Grammar, ParseOptions};

/// Deliberately broken fixtures live here; other tests assert how they fail.
const INVALID: &str = "invalid";

fn grammars_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("grammars")
}

/// Every `.abnf` file that is supposed to be valid.
fn valid_fixtures() -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(&grammars_dir(), &mut found, true);
    found.sort();
    found
}

fn collect(dir: &Path, found: &mut Vec<PathBuf>, skip_invalid: bool) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if skip_invalid && name == INVALID {
                continue;
            }
            collect(&path, found, skip_invalid);
        } else if path.extension().and_then(|e| e.to_str()) == Some("abnf") {
            found.push(path);
        }
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn name(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
}

/// Renders a parse failure against its source: "expected an element" on its own is useless for
/// an 11KB fixture.
fn locate(src: &str, span: abnf_oracle::Span) -> String {
    let offset = span.start as usize;
    let before = &src[..offset.min(src.len())];
    let line_number = before.lines().count().max(1);
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = src[line_start..].lines().next().unwrap_or_default();
    let column = offset - line_start;
    format!(
        "line {line_number}, column {}:\n  {line}\n  {}^",
        column + 1,
        " ".repeat(column)
    )
}

/// A test that silently finds nothing passes, which would be worse than failing.
#[test]
fn the_fixture_set_is_present() {
    let fixtures = valid_fixtures();
    assert!(
        fixtures.len() >= 9,
        "expected the full fixture set, found {}: {:?}",
        fixtures.len(),
        fixtures.iter().map(|p| name(p)).collect::<Vec<_>>()
    );

    // The set M1 names explicitly (SCOPE.md 8).
    for required in [
        "rfc5234-core.abnf",
        "rfc5234-abnf-as-published.abnf",
        "rfc5234-abnf-errata.abnf",
        "abnf-canonical.abnf",
        "rfc8259-json.abnf",
        "rfc3986-uri.abnf",
        "rfc5322-message.abnf",
        "rfc3339-datetime.abnf",
        "rfc9110-http.abnf",
    ] {
        assert!(
            fixtures.iter().any(|p| name(p) == required),
            "missing fixture {required}"
        );
    }
}

#[test]
fn every_fixture_parses() {
    for path in valid_fixtures() {
        let src = read(&path);
        let grammar = Grammar::parse(&src).unwrap_or_else(|e| {
            panic!(
                "{} did not parse: {e}
{}",
                name(&path),
                locate(&src, e.span())
            )
        });
        assert!(
            !grammar.definitions().is_empty(),
            "{} parsed to no rules at all",
            name(&path)
        );
    }
}

#[test]
fn every_fixture_carries_its_provenance() {
    for path in valid_fixtures() {
        let src = read(&path);
        let header: Vec<&str> = src.lines().take_while(|l| l.starts_with(';')).collect();
        assert!(
            header
                .iter()
                .any(|l| l.contains("RFC") || l.contains("rfc")),
            "{} has no header comment naming its RFC",
            name(&path)
        );
        assert!(
            header.iter().any(|l| l.contains("Errata applied")),
            "{} does not say which errata it applies",
            name(&path)
        );
    }
}

#[test]
fn every_fixture_round_trips() {
    for path in valid_fixtures() {
        let original = Grammar::parse(&read(&path)).expect("parses");
        let printed = original.to_string();
        let reparsed = Grammar::parse(&printed)
            .unwrap_or_else(|e| panic!("canonical form of {} did not parse: {e}", name(&path)));

        assert_eq!(
            original,
            reparsed,
            "{} changed under round-trip",
            name(&path)
        );
        assert_eq!(
            printed,
            reparsed.to_string(),
            "{}: canonical form is not a fixpoint",
            name(&path)
        );
    }
}

#[test]
fn every_fixture_round_trips_under_strict_crlf() {
    // Fixtures are stored with LF (see .gitattributes), so feed strict mode CRLF text. The
    // canonical form is CRLF already, which is why the second parse needs no conversion.
    let strict = ParseOptions { strict_crlf: true };
    for path in valid_fixtures() {
        let crlf = read(&path).replace('\n', "\r\n");
        let original = Grammar::parse_with(&crlf, strict.clone())
            .unwrap_or_else(|e| panic!("{} did not parse under strict_crlf: {e}", name(&path)));
        let reparsed = Grammar::parse_with(&original.to_string(), strict.clone())
            .unwrap_or_else(|e| panic!("{}: canonical form is not CRLF: {e}", name(&path)));
        assert_eq!(
            original,
            reparsed,
            "{} changed under round-trip",
            name(&path)
        );
    }
}

#[test]
fn parse_options_do_not_change_what_is_parsed() {
    let strict = ParseOptions { strict_crlf: true };
    for path in valid_fixtures() {
        let lf = read(&path);
        let lenient = Grammar::parse(&lf).expect("parses");
        let strictly =
            Grammar::parse_with(&lf.replace('\n', "\r\n"), strict.clone()).expect("parses");
        assert_eq!(
            lenient,
            strictly,
            "{}: parse options are provenance and must not affect equality (D21)",
            name(&path)
        );
    }
}

#[test]
fn every_fixture_is_ascii_clean() {
    // SCOPE.md 4.2 requires it, and the parser enforces it (D35) — but a fixture that grew a
    // non-ASCII byte would fail with a parse error somewhere unhelpful, so say it plainly here.
    // `invalid/` is included: only `invalid/parse/` is allowed to hold non-ASCII, and this scan
    // is what keeps that exception narrow.
    let mut all = Vec::new();
    collect(&grammars_dir(), &mut all, false);
    for path in all {
        if path.components().any(|c| c.as_os_str() == "parse") {
            continue;
        }
        let bytes = fs::read(&path).expect("readable");
        if let Some(index) = bytes.iter().position(|b| *b >= 0x80) {
            panic!(
                "{} has a non-ASCII byte {:#04x} at offset {index}",
                name(&path),
                bytes[index]
            );
        }
    }
}

#[test]
fn every_fixture_checks() {
    for path in valid_fixtures() {
        let src = read(&path);
        let grammar = Grammar::parse(&src).expect("parses");
        if let Err(errors) = grammar.check() {
            panic!(
                "{} did not check: {}",
                name(&path),
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
    }
}

#[test]
fn checked_fixtures_round_trip() {
    // The semantic layer's contract: `Grammar::parse(cg.to_string()).check() == cg` (D12).
    for path in valid_fixtures() {
        let checked = Grammar::parse(&read(&path))
            .expect("parses")
            .check()
            .expect("checks");
        let reparsed = Grammar::parse(&checked.to_string())
            .unwrap_or_else(|e| panic!("{}: merged form did not parse: {e}", name(&path)))
            .check()
            .unwrap_or_else(|e| panic!("{}: merged form did not check: {e:?}", name(&path)));
        assert_eq!(
            checked,
            reparsed,
            "{} changed under round-trip",
            name(&path)
        );
        assert_eq!(checked.to_string(), reparsed.to_string());
    }
}

/// Names the `CheckError` variant a broken fixture is there to produce.
type Predicate = fn(&CheckError) -> bool;

#[test]
fn broken_fixtures_fail_the_way_they_say_they_do() {
    let dir = grammars_dir().join(INVALID);
    let expected: &[(&str, Predicate)] = &[
        ("undefined-rule.abnf", |e| {
            matches!(e, CheckError::UndefinedRule { .. })
        }),
        ("duplicate-definition.abnf", |e| {
            matches!(e, CheckError::DuplicateDefinition { .. })
        }),
        ("incremental-without-base.abnf", |e| {
            matches!(
                e,
                CheckError::IncrementalWithoutBase {
                    shadows_core: false,
                    ..
                }
            )
        }),
        ("incremental-on-core-rule.abnf", |e| {
            matches!(
                e,
                CheckError::IncrementalWithoutBase {
                    shadows_core: true,
                    ..
                }
            )
        }),
        ("invalid-repeat-range.abnf", |e| {
            matches!(e, CheckError::InvalidRepeatRange { min: 5, max: 2, .. })
        }),
        ("invalid-numeric-range.abnf", |e| {
            matches!(
                e,
                CheckError::InvalidNumericRange {
                    lo: 0x5A,
                    hi: 0x41,
                    ..
                }
            )
        }),
    ];

    for (file, matches_variant) in expected {
        let src = read(&dir.join(file));
        let grammar =
            Grammar::parse(&src).unwrap_or_else(|e| panic!("{file} should still parse: {e}"));
        let errors = grammar.check().expect_err("{file} should not check");
        assert!(
            errors.iter().any(matches_variant),
            "{file} produced the wrong error: {errors:?}"
        );
    }

    // Every file in the directory is accounted for, so adding one without a test is caught.
    let mut present: Vec<String> = Vec::new();
    let mut found = Vec::new();
    collect(&dir, &mut found, false);
    for path in found {
        if !path.components().any(|c| c.as_os_str() == "parse") {
            present.push(name(&path).to_owned());
        }
    }
    present.sort();
    let mut named: Vec<String> = expected.iter().map(|(f, _)| (*f).to_owned()).collect();
    named.sort();
    assert_eq!(present, named, "an invalid fixture has no test");
}
