# Differential testing results

```
cargo build --release --features cli
cd scripts/goharness && go build -o goharness.exe . && cd ../..
python scripts/differential.py --redefine-core
```

Every other test in this repository was written by the same person who wrote the code, from the
same reading of RFC 5234. A misreading would be invisible to all of them, and consistently so.
An independent implementation has different blind spots, which is the only reason this is worth
running.

## Latest run

    date            2026-09-20
    abnf-oracle     0.1.0
    python-abnf     2.9.0        1 grammar it will not load
    go-abnf         v0.5.1       with --redefine-core
    verdicts        1631 compared, 1631 agreed
    disagreements   0 new, 1 known (below)

The JSON corpus is included: **269 files with known-correct verdicts, decided identically** by
go-abnf and this crate -- 95 that must be accepted and 174 that must be rejected. That corpus was
built by neither implementation, which is what makes it worth running.

## Implementations

| tool | version | role | notes |
|---|---|---|---|
| [python-abnf](https://pypi.org/project/abnf/) | 2.9.0 | recognizer | recursive descent; cannot generate |
| [go-abnf](https://github.com/pandatix/go-abnf) | v0.5.1 | recognizer **and generator** | GLL/BSR parser, driven through `scripts/goharness` |
| [abnfgen](https://www.quut.com/abnfgen/) | — | not run | C, no Windows package |

go-abnf generating is what makes the comparison two-sided. python-abnf can only judge strings
this crate produces; go-abnf also produces strings this crate must judge, which is the direction
that would catch an over-permissive recognizer here.

`scripts/goharness` is a thin Go program exposing go-abnf through the same command shape and the
same exit codes as `abnf-oracle` (0 yes, 1 no, 2 could not decide), so the Python driver treats
both the same way. It parses each grammar once per batch: doing it per input made a run take
minutes, because parsing RFC 5322 dominates everything else.

## A real bug found in go-abnf v0.5.1

**An alternation whose first branch carries `%s` is treated as case-sensitive throughout.**

```abnf
r = %s"x" / %i"x"      ; go-abnf rejects "X"   -- wrong
r = %i"x" / %s"x"      ; go-abnf accepts "X"   -- right
r = %s"x" / "x"        ; go-abnf rejects "X"   -- wrong
r = "x" / %s"x"        ; go-abnf accepts "X"   -- right
```

RFC 7405 §2.2 makes `%i` and unmarked strings case-insensitive regardless of what precedes them
in an alternation, so the first and third lines should accept `"X"`. The order dependence points
at the case-sensitivity flag from the leading branch leaking across the rest of the alternation.

Confidence that the bug is theirs and not ours: **this crate and python-abnf agree on all four
orderings**, and agree with each other on every other case in the fixture — `%s"aBc"` rejecting
`ABC`, `%i"aBc"` accepting it, a bare `"aBc"` accepting it. Only go-abnf differs, only on the two
orderings above.

Found by generating `"X"` from `both = %s"x" / %i"x" / "x"` in
`tests/grammars/rfc7405-case-sensitivity.abnf` — the fixture written in M3.3 because no RFC
grammar in the set uses `%s` or `%i`. It exists to exercise a corner, and the corner had a bug in
it.

Recorded in `ATTRIBUTED` in `scripts/differential.py`, so a run stays green on this while any
*new* disagreement still fails. Nothing goes in there that has not been reduced to a minimal case
and checked against a third implementation.

## Core-rule shadowing: three implementations, three positions

RFC 8259 defines `char`, shadowing the core rule `CHAR`. What each tool does with that says
something about its resolution model.

| | shadowing | loads RFC 8259 |
|---|---|---|
| python-abnf | **forbidden** | no |
| go-abnf | opt-in, with a warning | only with `WithRedefineCoreRules(true)` |
| abnf-oracle | **always allowed**, lint only (D6) | yes |

Both others are *leaky*: a redefinition replaces the rule everywhere, not just in the grammar
that wrote it. Their own words:

> python-abnf: "a grammar cannot define it: **the definition would replace the rule everywhere,
> not just here**."

> go-abnf: "we left to the user the responsibility to ensure the redefinition **keeps the ABNF
> grammar coherent** (i.e. there is an isomorphism between the core rule and the redefinition)."

That precondition is exactly what a leaky model forces on you, and RFC 8259 violates it: JSON's
`char` is a string-body character, nothing like core `CHAR = %x01-7F`. Enabling the flag for JSON
happens to be harmless only because JSON uses `DIGIT` and `HEXDIG`, neither of which references
`CHAR` — an accident of that grammar, not a property of the option.

This crate resolves core rules **hygienically** (D33): a reference inside a core rule body always
resolves within the core environment, so a user's `char` changes only their own references and
`HEXDIG` is unaffected. Shadowing is therefore safe by construction, which is what lets D6 make
it a lint and D40 cite RFC 8259 as the evidence. Two independent implementations now demonstrate
the cost of the alternative.

The run passes `--redefine-core` so go-abnf loads RFC 8259 anyway, because the JSON corpus is the
most valuable comparison available and the accident above makes it sound for this grammar.

## What this does not cover

- **No generator-first implementation.** `abnfgen`, which SCOPE.md M4 names, is a C program with
  no Windows package. go-abnf's generator covers the same direction, so this is a redundancy gap
  rather than a hole.
- **Rules are sampled, not exhaustive.** `--max-rules` defaults to 20 per grammar, spread evenly
  so the sample reaches RFC 5322's obsolete-syntax rules as well as its current ones. Raise it
  with `--max-rules 0` for a full sweep.
- **No octet-range disagreement was observed**, though SCOPE.md M4 expected to document one in
  advance. RFC 9110's `obs-text = %x80-FF` round-trips through both implementations, so all three
  read those terminals as code points rather than bytes. The §3 limitation is real; it would take
  a genuinely byte-oriented implementation to expose it.

## Two harness bugs worth remembering

Both produced *silent* wrong results rather than errors, which is the failure mode this kind of
testing invites.

**Universal newline translation.** The first run reported about forty disagreements across RFC
5322's `obs-*` rules. All were the harness's: Python's `read_text()` turns CRLF into LF on
Windows, and every one of those rules ends in `CRLF`. Fixed with `newline=""` everywhere.

**A deferred flush that never ran.** `goharness matchdir` buffered its verdicts and flushed them
with `defer` — but `os.Exit` does not run deferred functions, so it printed nothing and exited 0.
The driver read the empty output as "could not decide" and skipped every comparison, while the
totals still said 100% agreement. It was caught only because the per-grammar line read `0 of ours
accepted` next to a clean summary.

The script now prints an `undecided` count per grammar for exactly this reason: a batch that
silently returns nothing is indistinguishable from perfect agreement in a total, and silence must
not look like success.
