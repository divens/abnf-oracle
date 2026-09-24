# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Under `0.x`, the minor
version is the compatibility boundary: `0.1.z` releases are drop-in, `0.2.0` may not be.

## [Unreleased]

## [0.1.1] — 2026-09-24

No API changes; every fix is internal, so this is a drop-in replacement for 0.1.0.

All four defects came from an external review of 0.1.0. Each has a regression test that was
confirmed to fail without its fix.

### Fixed

- **The recognizer could report left recursion that does not exist.** A step or depth limit
  unwound past the point where a memo entry is completed, leaving an `InProgress` marker behind.
  Reusing the same `Recognizer` afterwards then read that marker and returned
  `MatchError::LeftRecursionDetected` — for a grammar `check()` had already proved was *not*
  left-recursive. Reuse is ordinary usage, since `end_positions` is parameterized on both the
  rule and the start position. Memo entries are now removed as the error unwinds.

  This was the most serious of the four: a wrong verdict rather than a crash or a refusal.

- **The recognizer panicked on the largest legal repetition bound.** `18446744073709551615*["a"]`
  is valid ABNF — `repeat` is `1*DIGIT` with no ceiling — and overflowed a counter in the
  repetition matcher: a panic in debug builds, a silent wrap in release. Since this crate is a
  testing oracle, it mostly runs inside other people's test suites, which are debug builds. The
  repetition matcher now counts down from the repetitions still allowed, so there is no counter
  at all when the maximum is unbounded.

- **`Generator::uncovered()` reported another rule's count for implicit core rules.** The
  reachability cache was keyed by an index into the user-rule table, which core rules do not
  have, so every core rule shared one entry: `uncovered("HEXDIG")` after `uncovered("ALPHA")`
  returned `ALPHA`'s answer. It is keyed by node id now. Generated strings were never affected —
  coverage steering reads a different structure — so this was confined to the reported count.

- **`lint_from()` treated rules behind a `*0(…)` repetition as reachable.** A repetition with a
  maximum of zero can never run, and the checker, the compatibility analysis and the generator
  all already treated its body as unreachable. `LintWarning::UnreachableRule` now agrees.
  `LintWarning::UnreferencedRule` deliberately does not: it asks whether any rule body *mentions*
  a name, and a name inside a body that cannot run is still written there.

### Documentation

- The README no longer claims the parser accepts *exactly* the language of the RFC's own
  grammar. It accepts that language with two documented exceptions — line-ending normalization
  and the `u64` bound on numeric magnitudes — and both are now stated where users will see them.
- `Generator::steps()` is documented as counting the most recent call, which is what it does;
  `Recognizer::steps()` is documented as counting over the recognizer's whole life, which is what
  *it* does. The two differ, and now say so.
- `SCOPE.md` describes the differential harness that was actually built, rather than the shell
  script and tool originally planned, and records that the octet-range disagreements it expected
  to find did not appear. The repository layout and the API and CLI sketches were also brought
  back in line with the code.
- `PLAN.md` is relabelled as a historical implementation record rather than a live plan.

## [0.1.0] — 2026-09-20

Initial release.

- Parses ABNF grammars per RFC 5234, as amended by RFC 7405 and Errata 2968 and 3076.
- Decides whether an input is in the language of a rule, with unordered alternation and full
  backtracking, so an ambiguous grammar is decided correctly rather than arbitrarily.
- Generates strings a rule accepts, deterministically from a seed, with a coverage mode that
  guarantees — rather than makes likely — that each successful call exercises a new alternation
  branch while any reachable one remains untaken.
- Reports every limit as an error and never as a rejection: "could not decide" and "does not
  match" are different answers.
- Optional `cli` feature: `check`, `rules`, `match` and `gen`, with exit codes that distinguish
  a rejection from a failure.
- No required dependencies.

[Unreleased]: https://github.com/divens/abnf-oracle/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/divens/abnf-oracle/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/divens/abnf-oracle/releases/tag/v0.1.0
