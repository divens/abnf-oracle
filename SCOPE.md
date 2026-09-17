# Scoping document: `abnf-oracle` (revision 4)

A small, correct, dependency-light Rust crate that parses ABNF grammars (RFC 5234, RFC 7405), recognizes whether an input matches a rule, and generates random inputs that match a rule. Built to be a **testing oracle**, not a production parser.

Working crate name: `abnf-oracle`.

Revisions 2–4 incorporate three rounds of external review. Every normative decision those reviews forced is collected in §13, "Decisions before M1"; the rest of the document is written to agree with it.

---

## 1. Why this exists

This project helps building other DSL projects that usually ship an ABNF grammar as a formalization of the language.

No maintained Rust crate does "given an ABNF grammar and a string, tell me whether it matches." The existing `abnf` crate only parses grammar *definitions* into a data structure and has been dormant for years. The Python `abnf` package and Go's `go-abnf` do the job but are not Rust.

This crate fills that gap. It is deliberately scoped as an oracle: correctness over speed, accept/reject over parse trees, simple code over clever code.

## 2. Goals

1. Parse any *syntactically valid* ABNF grammar written to RFC 5234 as amended by RFC 7405 and Errata 2968 and 3076, including the Appendix B core rules, with one documented restriction: numeric literals and repetition bounds must fit in 64 bits (§6.1). Parsing succeeds even when the grammar contains terminals this crate cannot match; those are reported separately.
2. Decide, for a checked grammar, a start rule and an input string, whether the input is in the language. The decision must be faithful to ABNF semantics over Unicode scalar values: **unordered alternation** (not PEG ordered choice), full backtracking, ambiguity tolerated.
3. Generate strings that match a rule, deterministically from a seed, with a coverage mode that guarantees progress toward exercising every *generatable* branch (§6.8).
4. Validate grammars in three distinct categories (§6.6): structural errors, recognizer-compatibility limits, and lint warnings.
5. Be trustworthy enough to serve as one side of a differential test against `go-abnf`, Python `abnf`, and `abnfgen`.
6. Be small: a few thousand lines, no proc macros, no unsafe, no required dependencies for the library target.

## 3. Non-goals

- **Performance.** Inputs are grammar-sized test strings, not network traffic. Polynomial is fine; exponential worst cases are acceptable if documented.
- **Parse trees.** v1 returns accept/reject only. A parse forest may come later; the recognizer design must not preclude it.
- **Ambiguity detection.** Counting or enumerating derivations is a different algorithm. Not in v1; noted as a v2 candidate (§14).
- **Error messages for the input.** "Does not match" is sufficient. Grammar *definition* errors should be good, since humans fix those.
- **Streaming or incremental matching.**
- **PEG or regex semantics.** Ordered choice, possessive repetition and lookahead do not exist here and must not creep in.
- **Byte-oriented matching.** Input is a sequence of Unicode scalar values. Grammars defined over octets (HTTP's `obs-text = %x80-FF`, RFC 5322's `obs-*` rules) are *loadable* but their high-range terminals mean "the code point U+0080–U+00FF", not "any byte", which differs from a byte-oriented oracle. Documented as a known limitation; an octet mode is a v2 candidate.
- **Grammar transformations** (to regex, to PEG, to another parser generator).
- **A parser generator.** No code generation, no build-script integration.

## 4. Supported ABNF

Everything in RFC 5234 §2–§4 and Appendix B, plus RFC 7405.

| Construct | Example | Notes |
|---|---|---|
| Rule definition | `name = elements` | Rule names are case-insensitive (ASCII) |
| Incremental alternative | `name =/ elements` | Appends alternatives; see §4.1 |
| Alternation | `a / b` | **Unordered** |
| Concatenation | `a b` | |
| Grouping | `(a b)` | |
| Optional | `[a]` | Semantically identical to `*1a`; canonicalization rewrites `*1a` to `[a]` and `1*1a` to `a` (§6.7); modeled as a two-branch alternation `a / empty` for coverage purposes |
| Repetition | `*a`, `3a`, `2*5a`, `*3a`, `3*a` | Bounds inclusive; `*` alone is 0..∞; `min > max` is a structural error (§6.3) |
| Quoted string, case-insensitive | `"abc"` | Default per RFC 5234; ASCII case folding only |
| Quoted string, case-sensitive | `%s"abc"` | RFC 7405 |
| Quoted string, explicit case-insensitive | `%i"abc"` | RFC 7405 |
| Numeric value | `%x41`, `%d65`, `%b1000001` | Representability: §6.1 |
| Numeric range | `%x41-5A` | Representability: §6.1; descending ranges are a structural error |
| Numeric concatenation | `%x41.42.43` | Erratum 3076 |
| Prose value | `<some description>` | Parsed; not matchable or generatable (§6.5) |
| Comments | `; text` to end of line | |
| Line continuation | Continuation lines begin with whitespace | |
| Core rules | `ALPHA`, `DIGIT`, `CRLF`, `WSP`, … | Appendix B, always implicit in v1 (not configurable); see §4.1 |

Errata:

- **2968** fixes an ambiguity in RFC 5234's own grammar for ABNF (the `rulelist` whitespace/comment handling). The grammar-text parser implements the corrected form. Both the corrected and uncorrected self-definitions are kept as fixtures (§8).
- **3076** clarifies §3.7 concatenation of numeric values. Applied as written.

Line endings: grammar text is normalized (`CRLF`, `LF`, `CR` → `LF`) before parsing unless `ParseOptions::strict_crlf` is set, in which case only `CRLF` is accepted.

`ParseOptions` contains **syntactic** options only (`strict_crlf` in v1). It never contains anything that changes rule resolution or grammar meaning; in particular the core-rule environment is fixed and cannot be disabled in v1 (§12, item 8).

### 4.1 Definition precedence

| Situation | Result |
|---|---|
| `foo = a` then `foo =/ b` | Valid; `foo = a / b` |
| `foo = a` then `Foo =/ b` | Valid; names are case-insensitive |
| `foo = a` then `foo = b` | **Structural error**: duplicate definition |
| `foo =/ b` with no prior `foo =` | **Structural error**: incremental definition without base |
| Several `foo =/ …` lines after one `foo =` | Valid; appended in order |
| Grammar defines a core-rule name (`DIGIT = …`) | Valid; the explicit definition **shadows** the implicit core rule. Lint warning `ShadowsCoreRule` (RFC 3986 and RFC 9110 restate core rules verbatim, so this must not be an error) |
| Grammar defines a core-rule name twice explicitly | Structural error, as for any duplicate |

The internal model is therefore three things: the **user grammar** (explicit rules, with `=/` merged into their base rule), the **core-rule environment** (implicit, consulted only for names the user grammar does not define), and **parse options**. `Display` serializes the user grammar only (§6.7).

## 5. Architecture

```
abnf-oracle/
├── Cargo.toml                 features: cli (opt-in, NOT default)
├── SCOPE.md                   ← this file
├── src/
│   ├── lib.rs                 public API re-exports
│   ├── ast.rs                 Grammar, Rule, Element, Repeat, StringLit, NumVal, node ids
│   ├── parse.rs               grammar text → Grammar (hand-written recursive descent)
│   ├── core_rules.rs          Appendix B rules as a Grammar constant
│   ├── check.rs               structural check → CheckedGrammar; per-rule analyses
│   ├── lint.rs                warnings: unreferenced, unreachable, unproductive, shadowing
│   ├── recognize.rs           set-of-positions recognizer, bound to one input
│   ├── generate.rs            deterministic generator with coverage mode
│   ├── rng.rs                 inline SplitMix64 (no external RNG dependency)
│   └── error.rs               ParseError, CheckError, LintWarning, MatchError, GenError
├── src/bin/abnf-oracle.rs     CLI (feature `cli`)
├── tests/
│   ├── grammars/              .abnf fixtures with header comments naming RFC, section, errata
│   ├── corpus/<grammar>/      accept/ and reject/ directories, one input per file
│   ├── parse_grammars.rs      every .abnf in tests/grammars parses and checks
│   ├── recognize_corpus.rs    every accept/ file matches, every reject/ file does not
│   ├── repetition.rs          nullable-repetition regression cases (§6.3)
│   ├── generate_roundtrip.rs  generated strings are accepted; coverage bound holds
│   └── self_definition.rs     the RFC 5234+7405 self-grammar recognizes every fixture's text
└── scripts/
    └── differential.sh        compare verdicts against go-abnf / python abnf / abnfgen
```

Module dependency order: `ast` ← `parse`, `core_rules` ← `check` ← `lint`, `recognize`, `generate`. Nothing depends on the CLI.

## 6. Semantics

### 6.1 Input model and representable terminals

Input is a sequence of Unicode scalar values (`&str` iterated by `char`). Positions are indices into that sequence. Case-insensitive strings fold ASCII letters only.

**Numeric magnitude.** RFC 5234's grammar places no bound on numeric literals (`1*DIGIT`), but this crate does: numeric values, range endpoints and repetition bounds are stored as `u64`. A literal that does not fit is `ParseError::NumberTooLarge { span }`. This is the one deliberate restriction on Goal 1; arbitrary-precision numerics would serve no real grammar and would complicate canonical `%x` output.

A numeric terminal is **representable** if:

- a single value is a Unicode scalar value (`0..=0x10FFFF` excluding `0xD800..=0xDFFF`);
- a range contains at least one Unicode scalar value (a range that merely *spans* the surrogate block is fine — surrogates cannot occur in input, so they are simply never matched);
- a numeric concatenation has every element representable.

Unrepresentable terminals parse successfully and are recorded. They are a **recognizer-compatibility** limit (§6.6): `check()` does not fail, but any rule that can reach one cannot be used as a start rule for recognition or generation, and attempting to do so returns `MatchError::UnrepresentableTerminal { rule, terminal }` / the `GenError` equivalent.

### 6.2 Recognizer: set-of-positions

The recognizer computes, for an element and a start position, the **set of all end positions** at which that element can finish matching. This makes unordered alternation and ambiguity correct by construction.

A `Recognizer` is constructed from a `CheckedGrammar` **and one input**. Its memo table is keyed `(rule_index, pos)` and is valid only for that input; there is no API for swapping inputs. Memoization is a pure optimization and must not change results.

```
match(elem, pos) -> BTreeSet<usize>

  Terminal (scalar, range, string):
      { pos + len }  if input[pos..] begins with the terminal, else {}

  Concatenation [e1, e2, …, en]:
      S = { pos }
      for e in elements: S = ∪ { match(e, p) | p in S }
      return S

  Alternation [e1 / e2 / … / en]:
      return ∪ match(ei, pos)

  Repetition:  see §6.3

  Rule reference:
      memo lookup on (rule_index, pos);
      if the entry is marked in-progress → MatchError::LeftRecursionDetected
      (belt-and-braces; §6.4 rejects this at check time)
      else compute match(rule.body, pos), store, return

accepts(rule, input) := input.len() ∈ match(rule, 0)
```

Memoization bounds each `(rule, input position)` evaluation to one computation per recognition run. Pathological grammars may still be expensive; no complexity guarantee is claimed (performance is a non-goal). `MatchOptions::max_steps` provides an explicit resource limit; the default is unlimited. Exceeding it returns `MatchError::StepLimit`, never a silent reject.

### 6.3 Repetition

Bounds are validated before any of this runs: `min > max` (e.g. `5*2"a"`) is `CheckError::InvalidRepeatRange { min, max }`, and a descending numeric range (`%x5A-41`) is `CheckError::InvalidNumericRange { lo, hi }`. Both are structural errors; the recognizer may assume `min <= max` and `lo <= hi`.

Revision 1's fixpoint cutoff was wrong for nullable elements with a positive minimum (`3*3 ["a"]` on empty input must accept). The corrected algorithm has two phases:

```
Repetition min..max of e  (max may be ∞):

  // Phase 1: positions reachable after EXACTLY k repetitions, k = 0..min.
  // Repetition count matters here even when positions don't change, so the
  // only early exits are emptiness and an EXACT fixpoint of the step function.
  cur = { pos }
  for k in 1..=min:
      next = ∪ { match(e, p) | p in cur }
      if next is empty: return {}
      if next == cur: break          // step is deterministic: further iterations are identical
      cur = next

  // Phase 2: from exactly-min, extend by 0..(max−min) more repetitions.
  // Breadth-first by count; a position first reached at count c is never
  // more useful when reached again at c' > c, so frontier − result is sound.
  result = cur
  frontier = cur
  k = min
  while k < max and frontier is nonempty:
      frontier = (∪ { match(e, p) | p in frontier }) − result
      result ∪= frontier
      k += 1
  return result
```

Termination and cost. Phase 1's early exit is *equality* of the step result, not the subset test that was wrong in revision 1: the step `f(S) = ∪ match(e, p)` is deterministic, so `f(cur) == cur` implies every later iteration returns `cur` too. This bounds phase 1 regardless of how large `min` is (`1000000000*["a"]` on empty input must return promptly): for nullable `e`, `cur ⊆ f(cur)` so the sequence is monotone and stabilizes within input-length steps; for non-nullable `e`, every repetition consumes at least one scalar, so `cur` empties within input-length + 1 steps. Phase 2 strictly grows `result`, which is bounded by the input length, so it terminates even for `max = ∞`.

Mandatory regression cases in `tests/repetition.rs`:

| Grammar | Input | Expected |
|---|---|---|
| `3*3 ["a"]` | `` (empty) | accept |
| `3*3 ["a"]` | `aa` | accept |
| `3*3 ["a"]` | `aaaa` | reject |
| `2*2 "a"` | `a` | reject |
| `*["a"]` | `` | accept, terminates |
| `1*("a" / "")` | `` | accept |
| `2*4 "ab"` | `ababab` | accept |
| `2*4 "ab"` | `ababababab` | reject |
| `1000000000*["a"]` | `` | accept, promptly (phase-1 early exit) |
| `1000000000*"a"` | `aaa` | reject, promptly |
| `5*2"a"` | — | `CheckError::InvalidRepeatRange` |
| `%x5A-41` | — | `CheckError::InvalidNumericRange` |

Additionally, a property test compares the recognizer against a brute-force enumerator on random tiny grammars (≤ 4 rules, ≤ 3 nesting) and inputs of length ≤ 6.

### 6.4 Nullability, left recursion, productivity

`check()` computes the following analyses by fixpoint iteration over the user grammar plus the core-rule environment. `nullable` and `min_len` are computed **per AST node**; the rest per rule.

- **nullable(n)**: can `n` match the empty string?
- **min_len(n)**: length of the shortest string `n` can match, as `MinLen::Finite(u64)` or `MinLen::Infinite`. Arithmetic on the finite variant **saturates** at `u64::MAX`: nested repetitions can produce a shortest expansion that overflows even though every literal fits (§6.1), and a saturated value is still *finite*, hence still productive. A node is **productive** iff `min_len` is finite.
- **witness(n)**: for every alternation node with finite `min_len`, the branch that *first* attained the node's final `min_len` during fixpoint iteration; for every repetition, the count `min`. Witnesses form a well-founded derivation: each was fixed at an earlier iteration than the node that refers to it, so following witnesses from any productive node reaches terminals in finitely many steps. This is the generator's termination device (§6.8) — comparing `min_len` magnitudes is *not*, since ties (`a = b / "x"`, `b = a / "y"`, both `min_len = 1`) can cycle, and saturated values compare meaninglessly.
- **first-graph**: edge `A → B` iff `B` can be the first thing matched by `A`, i.e. `B` occurs in `A`'s body preceded only by nullable elements.

**Prose values in analyses.** `<prose>` has no defined matching semantics, so its true nullability and length are unknown. For analysis purposes only, v1 assumes prose is **non-nullable with `min_len = 1`**. This is the safe assumption: it never creates a first-graph edge (so prose cannot cause a spurious global left-recursion failure), never makes a branch look unproductive (so no misleading `UnproductiveAlternative`), and the only rules whose analyses could be wrong under it are prose-reaching rules, which v1 refuses to recognize or generate from anyway (§6.5). Rules that do not reach prose have analyses independent of the assumption. Revisit when the resolver hook arrives.

**Left recursion** is any cycle in the first-graph. It is a **structural error** (`CheckError::LeftRecursion { cycle }`) because the recognizer cannot terminate on it. This is deliberately *global* — an unreachable left-recursive rule still fails `check()` — even though §6.6 treats prose values and unrepresentable terminals per start rule. The difference is that those are legitimate ABNF this crate happens not to support, whereas left recursion is always a grammar bug for an ABNF recognizer. Do not relax this to a compatibility limit without revisiting that argument. RFC grammars essentially never use it; Earley-style support is a v2 candidate.

**Unproductive rules** (`min_len = ∞`, e.g. `thing = "x" thing`) are a **lint warning** (`UnproductiveRule`), not an error: the recognizer handles them fine on finite input (it simply never accepts), but they are almost always a grammar bug. The generator refuses them as a start rule immediately with `GenError::NoFiniteExpansion { rule }`.

**Unproductive alternatives** — a branch of an alternation whose own `min_len` is `∞` inside an otherwise productive rule (`start = "ok" / bad` with `bad = "x" bad`) — get their own lint warning (`UnproductiveAlternative`), because they are more often a real mistake than a wholly dead rule is. `min_len` is therefore computed per AST node, not only per rule, so the generator can consult it at every branch (§6.8).

### 6.5 Prose values

`<…>` is a human-readable placeholder for something not expressible in ABNF. The parser accepts it and stores the text. `check()` computes `reaches_prose(r)` for every rule, and treats prose as non-nullable, `min_len = 1` in its analyses (§6.4).

Policy for v1 — no three-valued logic: a rule that can reach a prose value cannot be used as a start rule. `Recognizer::accepts` and `Generator::generate` return `ProseValueReachable { rule, prose }` immediately, before examining any input. Rules that do not reach a prose value remain fully usable, so a grammar with prose in an obscure branch is still useful from other entry points.

A resolver hook that resolves prose values externally is deferred to v1.1. If added, it must resolve to a `BTreeSet<usize>` like any other element, so the set-of-positions model is unchanged.

### 6.6 Validation model

Three categories, deliberately kept separate:

| Category | Where | Effect | Examples |
|---|---|---|---|
| **Structural error** | `Grammar::check() -> Result<CheckedGrammar, Vec<CheckError>>` | Grammar cannot be used at all | undefined rule reference, duplicate definition, `=/` without base, left recursion |
| **Compatibility limit** | recorded per rule on `CheckedGrammar`; enforced by `Recognizer`/`Generator` per start rule | Specific start rules unusable; others fine | reaches a prose value, reaches an unrepresentable terminal |
| **Lint warning** | `CheckedGrammar::lint()` and `lint_from(&[start_rules])` | Advisory only | unreferenced rule, unreachable from given starts, unproductive rule, unproductive alternative, shadows a core rule |

"Unreferenced" (no rule mentions it) and "unreachable" (not reachable from the given start rules) are different warnings. The first needs no start rule; the second requires the caller to name entry points, since a grammar may legitimately expose several.

### 6.7 Canonical form and equality

`Display` for `Grammar` and `CheckedGrammar` emits the **user grammar only**, in canonical form: one rule per line, `=/` merged into the base definition, single spaces, `%x` for all numeric values, `*1a` rewritten to `[a]` and `1*1a` to `a`, CRLF line endings. Implicit core rules are never printed. Comments are not preserved (semantic-only model). These rewrites happen on the AST during normalization, so coverage units (§6.8) and node ids are computed on the same shape `Display` prints.

`PartialEq` compares user rules only (order-sensitive). It ignores **parse options** (they are syntactic provenance, not grammar semantics, and `Display` does not serialize them — which is exactly why `ParseOptions` must never hold semantic configuration such as a core-rule switch) and **node ids** (internal). Round-trip requirement: `Grammar::parse(g.to_string()) == g`, for a `g` parsed with any options.

**Node ids** are assigned deterministically by pre-order traversal of the *canonical* AST — after `=/` merging and normalization, in rule definition order — not during parsing. Two grammars with the same canonical form therefore have identical node ids, which is what makes the generator's determinism contract (§6.8) meaningful across equal grammars.

### 6.8 Generator

A recursive walk over the AST with a depth budget. Every choice point in the AST has a stable **node id** assigned from the canonical AST (§6.7).

- Terminal: emit it. Ranges pick uniformly among representable scalars. Case-insensitive strings vary case randomly unless `preserve_case` is set.
- Alternation (including `[a]` as `a / empty`): choose a branch per the selection rule below, **never a branch whose `min_len` is `∞`**. This ban is absolute and applies in every mode, not only under an exhausted depth budget: once inside an unproductive branch (`bad = "x" bad`) there may be no alternation left to steer by, and the walk would never end.
- Repetition `min..max`: choose a count uniformly in `[min, min(max, min + spread)]`, `spread` default 3 — except in coverage mode, where if the body can reach an uncovered coverage unit and `max >= 1`, the count is at least `max(min, 1)`. A repetition with `max == 0` makes its body unreachable; units inside it are not coverage units.
- Rule reference: recurse. When the depth budget is exhausted, the walk switches to **witness mode**: every subsequent alternation takes its `witness` branch and every repetition takes count `min` (§6.4). Because witnesses form a well-founded derivation, this terminates regardless of `min_len` ties or saturation. Choosing "the branch with the smallest `min_len`" is *not* an acceptable substitute.
- Resource bound: `GenOptions::max_output_len` (default `1 << 20` scalars) and `GenOptions::max_steps` (default `1 << 24` node visits). Exceeding either returns `GenError::OutputLimit` / `GenError::StepLimit`. Mandatory work is not bounded by depth — `start = 1000000000*"a"` is productive and has no shorter expansion — so termination alone does not make generation practical; the bound does.
- Prose value or unrepresentable terminal: cannot be reached, since the start rule was rejected at `generate()` entry if it could reach one.

**Coverage unit**: `(alternation node id, branch index)` **where the branch's `min_len` is finite and the branch is not inside a repetition with `max == 0`**. Syntactically reachable branches with infinite `min_len` are not coverage units — they are reported by the `UnproductiveAlternative` lint instead (§6.4). Repetition counts are not coverage units; `[a]` contributes two units via its `a / empty` model, and since `*1a` canonicalizes to `[a]` (§6.7) the two spellings are indistinguishable here.

**Selection rule in coverage mode** (deterministic-first):

1. Among the branches of the current alternation, prefer any branch that is itself uncovered.
2. Otherwise, prefer any branch whose subtree can reach an uncovered coverage unit (precomputed reachability, updated as coverage grows).
3. Otherwise, choose randomly.

Consequence, which M3 asserts: while any coverage unit reachable from the start rule remains uncovered, each successful `generate()` call covers at least one new unit. The coverage-aware repetition count above is what makes this hold through `*(...)`, where a zero count would otherwise cover nothing. Therefore, subject to configured resource limits, full coverage requires at most `N` successful calls, where `N` is the number of reachable (and by definition generatable) coverage units. This is a guarantee, not a probability.

**Determinism contract**: given the same canonical grammar, the same `GenOptions`, the same seed, and the same sequence of API calls on a fresh `Generator`, the sequence of generated strings is identical. This is guaranteed within a crate version. Cross-version stability is *not* promised: corpora are committed as files (§9), so the seed is not the artifact. The RNG is an inline SplitMix64 to avoid a dependency and to make cross-version stability likely in practice.

## 7. Public API sketch

```rust
pub struct Grammar { /* user rules with node ids; parse options */ }

impl Grammar {
    pub fn parse(src: &str) -> Result<Grammar, ParseError>;               // core rules implicit
    pub fn parse_with(src: &str, opts: ParseOptions) -> Result<Grammar, ParseError>;
    pub fn parse_options(&self) -> &ParseOptions;                         // provenance; not part of PartialEq
    pub fn check(self) -> Result<CheckedGrammar, Vec<CheckError>>;        // consumes; structural only
}

/// A grammar that passed structural validation. The only route to a Recognizer or Generator.
pub struct CheckedGrammar { /* Grammar + analyses: nullable, min_len (saturating) and witness per node; reaches_prose and reaches_unrepresentable per rule */ }

pub enum MinLen { Finite(u64), Infinite }   // Finite saturates at u64::MAX

impl CheckedGrammar {
    pub fn rule(&self, name: &str) -> Option<&Rule>;                     // case-insensitive
    pub fn lint(&self) -> Vec<LintWarning>;                              // unreferenced, unproductive rule/alternative, shadowing
    pub fn lint_from(&self, start_rules: &[&str]) -> Vec<LintWarning>;   // adds unreachable
    pub fn can_recognize(&self, rule: &str) -> Result<(), MatchError>;   // compatibility check without input
}

pub struct MatchOptions { pub max_steps: Option<u64> }   // default: None (unlimited)

/// Bound to exactly one input for its lifetime; the memo table is valid only for that input.
pub struct Recognizer<'g, 'i> { /* &CheckedGrammar, &[char] input, memo, options */ }

impl<'g, 'i> Recognizer<'g, 'i> {
    pub fn new(grammar: &'g CheckedGrammar, input: &'i str) -> Self;
    pub fn with_options(self, opts: MatchOptions) -> Self;
    pub fn accepts(&mut self, rule: &str) -> Result<bool, MatchError>;
    pub fn end_positions(&mut self, rule: &str, start: usize) -> Result<BTreeSet<usize>, MatchError>;
    pub fn steps(&self) -> u64;                       // repetition/rule step evaluations so far; the counter max_steps limits
}

pub struct GenOptions { pub max_depth: usize, pub spread: usize, pub coverage: bool, pub preserve_case: bool,
                        pub max_output_len: Option<usize>, pub max_steps: Option<u64> }

pub struct Generator<'g> { /* &CheckedGrammar, SplitMix64, coverage state, options */ }

impl<'g> Generator<'g> {
    pub fn new(grammar: &'g CheckedGrammar, seed: u64) -> Self;
    pub fn with_options(self, opts: GenOptions) -> Self;
    pub fn generate(&mut self, rule: &str) -> Result<String, GenError>;
    pub fn uncovered(&self, rule: &str) -> usize;     // reachable coverage units not yet covered
}

pub enum MatchError { ProseValueReachable {..}, UnrepresentableTerminal {..}, UnknownRule(String), LeftRecursionDetected {..}, StepLimit }
pub enum GenError   { ProseValueReachable {..}, UnrepresentableTerminal {..}, UnknownRule(String), NoFiniteExpansion {..}, OutputLimit, StepLimit }
```

Error types are plain enums with `Display`; no `anyhow` in the library. The CLI may use `anyhow`.

## 8. Milestones and acceptance criteria

Each milestone is a PR-sized unit. Do not start the next before the current one's tests pass in CI.

**M0 — Skeleton.** Crate compiles with `#![forbid(unsafe_code)]`; CI runs `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, once with default features and once with `--features cli`. `ast.rs` has the full data model including node ids. Test directory layout from §5 exists.

**M1 — Grammar parser and check.**
- Parses every `.abnf` in `tests/grammars/`: RFC 5234 Appendix B core rules; the ABNF self-definition in three variants — RFC 5234 §4 as published, RFC 5234 §4 with Erratum 2968, and the **canonical self-grammar** (RFC 5234 §4 + Erratum 2968 + the RFC 7405 §2.2 `char-val` amendments), which is the one M2 uses; RFC 8259 JSON; RFC 3986 URI; RFC 5322 §3 address grammar; RFC 3339 date-time; RFC 9110 selected header field grammars (exercises core-rule shadowing and `obs-text`).
- Every fixture passes `check()` except deliberately broken fixtures under `tests/grammars/invalid/` (undefined rule, duplicate, `=/` without base, direct and indirect left recursion, `min > max` repeat, descending numeric range), each of which must fail with the expected `CheckError` variant. A fixture with a 25-digit repeat count fails to *parse* with `NumberTooLarge`.
- Round-trip: `Grammar::parse(g.to_string()) == g` for every valid fixture, including fixtures parsed with `strict_crlf = true` (equality ignores parse options and node ids). `*1a` and `[a]` parse to equal grammars.
- `lint()` and `lint_from()` produce the expected warnings on hand-written cases, including `UnproductiveRule` and `UnproductiveAlternative`; the RFC 9110 fixture yields `ShadowsCoreRule` warnings and no errors.
- Analyses (nullable, `min_len` and `witness` per node; `reaches_prose` and `reaches_unrepresentable` per rule) are unit-tested on hand-written grammars, including: a `min_len` that saturates (three nested `4294967295` repetitions) and is still `Finite`; a prose-containing rule that does *not* trigger left recursion or `UnproductiveAlternative`; and the tie case `a = b / "x"`, `b = a / "y"`, whose witnesses point at the terminals.
- Node ids: two textually different grammars with the same canonical form yield identical node ids.
- Acceptance is about parsing and checking only. Nothing in M1 recognizes input.

**M2 — Recognizer.**
- Set-of-positions implementation per §6.2–§6.3, with memoization and the in-progress guard.
- `tests/repetition.rs` passes every case in the §6.3 table, plus the brute-force property test. For the large-bound cases the test asserts `recognizer.steps() <= c * (input_len + 1)` for a fixed small `c`, which checks the O(input) property directly; a generous timeout remains only as a hang guard, never as the assertion.
- `tests/corpus/rfc8259/` populated from JSONTestSuite: every `y_*` accepted, every `n_*` rejected, `i_*` recorded not asserted. Any `n_` case that pure ABNF accepts (encoding-level rejections are outside the grammar) is moved to `i_` and listed in `tests/corpus/rfc8259/NOTES.md`.
- `self_definition.rs`: the recognizer, running the **canonical self-grammar** (RFC 5234 + Erratum 2968 + RFC 7405), accepts the CRLF-normalized text of every valid fixture in `tests/grammars/`, including fixtures that use `%s`/`%i` strings. The test normalizes line endings itself so results do not depend on Git checkout settings.
- Compatibility limits: a fixture with a prose value yields `ProseValueReachable` from a start rule that reaches it, and `Ok` from one that does not.
- Hand-written unit tests for every construct in §4, including `=/`, `%s` vs `%i`, nested optionals, and core-rule shadowing.

**M3 — Generator.**
- `generate_roundtrip.rs`: for each valid fixture and 200 seeds, `generate(start)` is accepted by the recognizer. Zero failures.
- Coverage bound: for the JSON grammar, with `coverage = true`, `uncovered(start)` reaches zero within `N` calls, where `N` is the initial `uncovered(start)`. Asserted exactly, not probabilistically. Repeated on a hand-written grammar containing an unproductive alternative: the bound still holds, and the dead branch is never emitted.
- `NoFiniteExpansion` returned immediately for an unproductive start rule; `ProseValueReachable` for a prose-reaching one. Random mode (`coverage = false`) on `start = "ok" / bad`, `bad = "x" bad` terminates on every one of 1000 seeds.
- Zero-count repetition: on `start = *("a" / "b")` in coverage mode, both branches are covered in at most two calls; a hand-written grammar with a `*0(...)` body reports zero units inside it.
- Witness termination: with `max_depth = 0`, generation from `a` in `a = b / "x"`, `b = a / "y"` terminates and yields `x` or `y`.
- Resource bound: `start = 1000000000*"a"` returns `OutputLimit` under default options rather than running.
- Determinism: two fresh generators with the same seed and options produce identical sequences over 100 calls.

**M4 — Differential testing (non-blocking during implementation; required before the 1.0 release).**
- `scripts/differential.sh` runs `abnfgen -c` on each fixture, feeds outputs to `abnf-oracle match`, asserts acceptance.
- Where available, compares verdicts against `go-abnf` or Python `abnf`. Any disagreement is a bug in one of the three and must be understood before being waived; expected disagreements on octet-range terminals (§3) are documented in advance.

## 9. Testing conventions

- Corpus files are the source of truth, one input per file, named `NNN-short-description.txt`, stored as raw bytes; tests read them as UTF-8 and treat decoding failure as a test error, not a rejection.
- Every bug found by differential testing gets a corpus file before it gets a fix.
- Grammar fixtures carry a header comment naming the RFC, section, and errata applied.
- Unit tests next to the code; integration tests in `tests/`.
- Property tests via `proptest` (dev-dependency): generated strings are accepted; recognizer agrees with the brute-force enumerator on tiny grammars; randomly mutated generated strings never panic.

## 10. Dependencies

- Library target: **none**. RNG is inline (`rng.rs`).
- `cli` feature (opt-in): `clap`, `anyhow`.
- Dev: `proptest`.
- MSRV: current stable at project start; edition 2024. `#![forbid(unsafe_code)]`.

## 11. CLI (feature `cli`)

```
abnf-oracle check  <grammar.abnf> [--start R ...]              # parse + check + lint; --start enables unreachable warnings
abnf-oracle match  <grammar.abnf> --rule R (--input S | --file F | --dir D) [--max-steps N]
abnf-oracle gen    <grammar.abnf> --rule R [--seed N] [--count N] [--coverage] [--depth N] [--spread N] [--max-output-len N] [--max-steps N]
abnf-oracle rules  <grammar.abnf>                              # list rules with nullable / min_len / flags
```

Exit codes for `match`:

| Situation | Exit |
|---|---|
| Single input accepted | 0 |
| Single input rejected | 1 |
| Any error: grammar fails to parse or check, unknown rule, compatibility limit, step limit, **input is not valid UTF-8** | 2 |
| `--dir`: every file accepted | 0 |
| `--dir`: at least one file rejected, no errors | 1 |
| `--dir`: at least one error (error dominates rejection) | 2 |

`--dir` prints one line per file: `ACCEPT`, `REJECT`, or `ERROR <reason>`. Invalid UTF-8 is an input-decoding error (exit 2), never a grammar rejection, so that corpus and differential tests cannot confuse the two.

## 12. Resolved questions from earlier revisions

1. Case folding beyond ASCII: **no**. RFC 5234 defines none; documented in §6.1.
2. Comment/formatting preservation: **no**; semantic-only model, canonical `Display` (§6.7).
3. Memoization key: `(rule_index, pos)` per input (§6.2). Inline groups are not memoized; revisit only with measurements.
4. JSONTestSuite `n_` cases that pure ABNF accepts: documented in `tests/corpus/rfc8259/NOTES.md` (§8, M2).
5. Arbitrary-precision numerics: **no**; `u64` with `NumberTooLarge` (§6.1). The alternative served no real grammar.
6. Descending numeric ranges as the empty language: **no**; structural error (§6.3). An oracle should call a typo a typo.
7. Left recursion as a per-start-rule compatibility limit: **no**; stays a global structural error, with the rationale recorded in §6.4.
8. Disabling core rules as a `ParseOptions` switch: **removed** for v1. It is semantic configuration, would have to participate in equality and `Display`, and no use case survives shadowing (§4.1). If it returns, it returns as part of the grammar model, not as a parse option.
9. Tri-state "unknown" nullability for prose: **no**; the fixed non-nullable/`min_len = 1` assumption gives the same safety with less code (§6.4).
10. Arbitrary-precision `min_len`: **no**; saturating `u64` plus witnesses (§6.4).

## 13. Decisions before M1 (normative)

Each item traces to the review that motivated it (D1–D16 first review, D17–D24 second, D25–D31 third). Implement these as written.

- **D1** A `Recognizer` or `Generator` can only be constructed from a `CheckedGrammar`. `Grammar::check` consumes the `Grammar`. There is no unchecked path.
- **D2** A `Recognizer` is bound to one input at construction. The memo table lives inside it and is never reused across inputs.
- **D3** Repetition is implemented exactly as §6.3, including the phase-1 equality early exit, with the regression table and the brute-force property test as acceptance.
- **D4** The matching domain is Unicode scalar values. Representability is defined in §6.1; unrepresentable terminals are a per-rule compatibility limit, not a parse or check failure. Octet semantics are out of scope (§3).
- **D5** `check()` reports only structural errors. Reachability lint requires start rules via `lint_from`. Warnings never fail `check()`.
- **D6** Definition precedence follows the table in §4.1. Explicit definitions shadow core rules with a lint warning, never an error.
- **D7** No ambiguity detection or derivation counting in v1. The former M2 ambiguity test is removed; both self-definition variants remain as parse fixtures only.
- **D8** M1 acceptance covers parsing and checking only; self-recognition is M2 and operates on CRLF-normalized text.
- **D9** `min_len` / productivity is computed in `check()` per AST node; unproductive rules and unproductive alternatives are lint warnings; `generate()` fails fast with `NoFiniteExpansion` on an unproductive start rule.
- **D10** Coverage units are `(alternation node id, branch index)` restricted to branches with finite `min_len`; selection is deterministic-first per §6.8; M3 asserts the exact bound.
- **D11** Prose values are a per-start-rule compatibility limit; no three-valued matching. Resolver hook deferred to v1.1.
- **D12** `Display` and `PartialEq` cover the user grammar only; `=/` is merged; core rules are implicit environment; parse options and node ids are excluded from `PartialEq`.
- **D13** The `cli` feature is opt-in. The library target has zero required dependencies. Directory exit codes per §11.
- **D14** Invalid UTF-8 input is an error (exit 2), never a rejection.
- **D15** Determinism is guaranteed for a fresh generator with the same seed, options and call sequence, within a crate version. RNG is inline SplitMix64.
- **D16** Step limits are exposed via `MatchOptions::max_steps` and `--max-steps`; default unlimited; exceeding is an error, never a silent reject.
- **D17** Numeric literals and repetition bounds are `u64`; overflow is `ParseError::NumberTooLarge`. Goal 1 is narrowed accordingly.
- **D18** `min > max` in a repetition and `lo > hi` in a numeric range are structural errors (`InvalidRepeatRange`, `InvalidNumericRange`).
- **D19** Phase 1 of repetition exits early on an *exact* fixpoint (`f(cur) == cur`) or emptiness, so large bounds cost at most input-length iterations.
- **D20** The generator never enters a branch with infinite `min_len`, in any mode. Coverage units exclude such branches. `UnproductiveAlternative` is a lint.
- **D21** `PartialEq` ignores parse options; they are provenance. Round-trip holds for any options.
- **D22** Node ids are internal, excluded from `PartialEq`, and assigned by pre-order traversal of the canonical AST after `=/` merging.
- **D23** The self-recognition fixture is the canonical self-grammar: RFC 5234 §4 + Erratum 2968 + RFC 7405 §2.2. It must recognize every fixture, including those using `%s`/`%i`.
- **D24** M4 is non-blocking during implementation and required before the 1.0 release. No complexity guarantee is claimed for the recognizer; `max_steps` is the resource bound.
- **D25** In coverage mode, a repetition whose body can reach an uncovered unit and whose `max >= 1` uses a count of at least `max(min, 1)`. Units inside a `max == 0` repetition are not coverage units.
- **D26** For analyses, prose is assumed non-nullable with `min_len = 1`. Prose never creates a first-graph edge and never triggers unproductive lints.
- **D27** `min_len` is `MinLen::{Finite(u64), Infinite}` with saturating arithmetic. Saturated is finite and productive.
- **D28** `check()` records a witness branch per productive alternation. Exhausted-budget generation follows witnesses, never "smallest `min_len`".
- **D29** Generation has explicit resource bounds (`max_output_len`, `max_steps`) with corresponding `GenError` variants; defaults are finite.
- **D30** `ParseOptions` holds syntactic options only. The core-rule environment is fixed in v1 and not configurable. `PartialEq` ignoring parse options is therefore exact.
- **D31** Canonicalization rewrites `*1a` to `[a]` and `1*1a` to `a` on the AST before node ids and coverage units are computed. Performance tests assert step counts via `Recognizer::steps()`, not wall-clock time.

## 14. v2 candidates (explicitly not v1)

- Ambiguity detection with a saturating derivation count (0 / 1 / 2+).
- Left-recursion support via an Earley-style recognizer.
- Octet mode: input as bytes, terminals matched against byte values.
- Prose-value resolver hook (and with it, real nullability/`min_len` for prose).
- Configurable core-rule environment, as semantic grammar configuration participating in equality and `Display`.
- Parse forest output.

## 15. Definition of done (v1.0)

- M0–M3 complete; M4 run with results recorded (required for release, per D24).
- README with a 20-line usage example and a clear statement of what this crate is *not* (§3).
- Published to crates.io.
- Library code under ~3,500 lines excluding tests. If it grows past that, something from §3 or §14 has crept in.
