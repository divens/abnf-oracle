# abnf-oracle

[![CI](https://github.com/divens/abnf-oracle/actions/workflows/ci.yml/badge.svg)](https://github.com/divens/abnf-oracle/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/abnf-oracle.svg)](https://crates.io/crates/abnf-oracle)
[![docs.rs](https://docs.rs/abnf-oracle/badge.svg)](https://docs.rs/abnf-oracle)

Parse ABNF grammars (RFC 5234, RFC 7405), decide whether an input matches a rule, and generate
inputs that do.

Built as a **testing oracle** for projects that ship an ABNF grammar as the formalization of
their language: correctness over speed, accept/reject over parse trees, simple code over clever
code. The library has no required dependencies.

```rust
use abnf_oracle::{Generator, Grammar, Recognizer};

// A grammar, as it would appear in an RFC.
let source = concat!(
    "full-date     = date-fullyear \"-\" date-month \"-\" date-mday\r\n",
    "date-fullyear = 4DIGIT\r\n",
    "date-month    = 2DIGIT\r\n",
    "date-mday     = 2DIGIT\r\n",
);

// Parse, then check. `check` merges incremental definitions, resolves every name and runs
// the analyses; it is the only way to reach a recognizer or a generator.
let grammar = Grammar::parse(source)
    .expect("valid ABNF")
    .check()
    .expect("no structural errors");

// Recognize.
assert!(Recognizer::new(&grammar, "2026-09-20").accepts("full-date").unwrap());
assert!(!Recognizer::new(&grammar, "20260920").accepts("full-date").unwrap());

// Generate. The same seed always gives the same string.
let mut generator = Generator::new(&grammar, 42);
let produced = generator.generate("full-date").expect("generates");

// What the generator produces, the recognizer accepts.
assert!(Recognizer::new(&grammar, &produced).accepts("full-date").unwrap());
```

<sub>This is the crate-level doctest verbatim, so it is compiled and run on every CI build.</sub>

## What makes it an oracle

**Alternation is unordered and backtracking is full.** The recognizer computes the set of every
end position an element can reach rather than committing to the first branch that matches, so an
ambiguous grammar is decided correctly instead of arbitrarily. That is the opposite of a PEG,
and it is the point: an oracle must answer what the grammar says, not what a parsing strategy
finds convenient.

**"Could not decide" is never reported as "does not match."** A step limit, a recursion limit, a
prose value like `<see Section 4.1>`, a terminal with no Unicode scalar — each makes a question
unanswerable. Every one is an `Err`, never `Ok(false)`. An oracle that conflates the two is
worse than none.

**Generation is the inverse of recognition, and it is checked both ways.** Everything the
generator produces, the recognizer accepts — 83,200 round-trips across the RFC fixture set on
every CI run. A coverage mode takes a different alternation branch each call, with a *guarantee*
rather than a probability: while any reachable branch is untaken, each successful call takes at
least one.

**It reads its own specification.** RFC 5234 defines ABNF in ABNF, so the crate is pointed at
itself in both directions: the canonical self-grammar recognizes every fixture, and grammars
*generated* from it must parse. That closes the invariant "this parser accepts the language of
the RFC's own grammar" from both sides — with two documented exceptions, both tested rather
than asserted:

- **Line endings.** The RFC's grammar requires CRLF; this parser also accepts LF, because
  refusing a grammar file over its checkout settings helps nobody.
- **Numeric magnitude.** `repeat` is `1*DIGIT` with no ceiling, so `1*99999999999999999999"a"`
  is valid ABNF that this crate refuses, storing bounds as `u64`.

Everything else the RFC's grammar admits, this parser admits.

**It is checked against other implementations.** [`scripts/differential.py`](https://github.com/divens/abnf-oracle/blob/main/scripts/differential.py) compares verdicts
against [python-abnf](https://pypi.org/project/abnf/) and
[go-abnf](https://github.com/pandatix/go-abnf) in three directions, including a 269-file JSON
corpus built by neither. See [`scripts/DIFFERENTIAL.md`](https://github.com/divens/abnf-oracle/blob/main/scripts/DIFFERENTIAL.md) — the last run
found a case-sensitivity bug in one of them.

## Command line

```console
$ cargo install abnf-oracle --features cli

$ abnf-oracle check   grammar.abnf --start message   # parse, check, lint
$ abnf-oracle rules   grammar.abnf                   # nullable / min length / flags
$ abnf-oracle match   grammar.abnf --rule message --file input.txt
$ abnf-oracle gen     grammar.abnf --rule message --count 50 --coverage --out corpus/
```

`match` exits `0` for accepted, `1` for rejected, `2` for anything it could not decide — so a
script can tell a rejection from a failure. With `--dir` it reports one line per file and an
error anywhere dominates a rejection.

## What this is not

Deliberately out of scope, and not planned:

- **Parse trees.** v1 answers accept/reject. The recognizer design does not preclude a forest later.
- **Ambiguity detection.** Counting derivations is a different algorithm.
- **PEG or regex semantics.** Ordered choice, possessive repetition and lookahead do not exist here.
- **Byte-oriented matching.** Input is Unicode scalar values. Grammars over octets — HTTP's
  `obs-text = %x80-FF`, RFC 5322's `obs-*` rules — load fine, but those terminals mean the code
  points U+0080–U+00FF, not "any byte". A known limitation, not a bug.
- **Streaming**, **grammar transformations**, and **parser generation**.
- **Performance guarantees.** Pathological grammars are bounded by explicit step and depth
  limits, not by complexity promises. Inputs are test strings, not network traffic.

[`SCOPE.md`](https://github.com/divens/abnf-oracle/blob/main/SCOPE.md) section 3 has the full list with the reasoning.

## Changes

[`CHANGELOG.md`](https://github.com/divens/abnf-oracle/blob/main/CHANGELOG.md) records what
changed in each release and why.

## Requirements

Rust 1.88 or later, verified in CI by running the full suite on that toolchain. The library
builds with no dependencies; the `cli` feature adds `clap` and `anyhow`.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
