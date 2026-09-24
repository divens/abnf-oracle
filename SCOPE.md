# Scoping document: `abnf-oracle` (revision 5.14)

A small, correct, dependency-light Rust crate that parses ABNF grammars (RFC 5234, RFC 7405), recognizes whether an input matches a rule, and generates random inputs that match a rule. Built to be a **testing oracle**, not a production parser.

Working crate name: `abnf-oracle` (rename freely; `abnf` on crates.io is taken by an unmaintained crate with a different scope).

Revisions 2–5 incorporate three rounds of external review and one round of implementation-planning questions; 5.1 resolves a conflict between the depth budget and the coverage guarantee raised during implementation; 5.2 corrects three M3 acceptance criteria that could not be run as written; 5.3 corrects the description of Errata 2968 and 3076, whose subjects were transposed, and adds 3076 to the canonical self-grammar; 5.4 completes §6.7's parenthesization rule, which covered two of the four cases that need parentheses; 5.5 corrects the claim that RFC 3986 and RFC 9110 restate the core rules, which neither does; 5.6 replaces the witness tie-case grammar, which was itself left-recursive and so could never be checked, and states the witness rule in terms of derivation length; 5.7 removes the whitespace from the §6.3 repetition table, where six of the twelve rows were spelled in a way ABNF does not admit; 5.8 adds `MatchOptions::max_depth`, after M2.4 found that deeply nested input overflowed the stack and aborted the process; 5.9 wraps `check`'s errors in a `CheckErrors` newtype, because `Vec` is a foreign type and so can never implement `std::error::Error`, which forced every caller to handle this one error specially instead of using `?`; 5.10 makes memoization transactional after an external review of v0.1.0 found that a resource limit left `InProgress` behind and turned a later call on the same recognizer into a fabricated left-recursion report; 5.11 removes the repetition counter that overflowed on the largest legal bound, 5.12 keys the coverage cache by node id so implicit core rules stop sharing one entry, and 5.13 makes lint reachability respect `max == 0` — all from the same review. 5.14 is documentation only: M4, the repository layout and the §7 and §11 sketches had fallen behind what was actually built and shipped. Every normative decision those reviews forced is collected in §13, "Decisions before M1"; the rest of the document is written to agree with it.

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
5. Be trustworthy enough to serve as one side of a differential test against `go-abnf`, Python `abnf`, and `abnfgen`. (As shipped: python-abnf and go-abnf; see M4.)
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
| Numeric concatenation | `%x41.42.43` | RFC 5234 §2.3; no erratum involved |
| Prose value | `<some description>` | Parsed; not matchable or generatable (§6.5). Content restricted to `%x20-3D / %x3F-7E` exactly as RFC 5234 |
| Comments | `; text` to end of line | Content restricted to `WSP / VCHAR` (ASCII) exactly as RFC 5234; non-ASCII is a `ParseError` (§4.2) |
| Line continuation | Continuation lines begin with whitespace | |
| Core rules | `ALPHA`, `DIGIT`, `CRLF`, `WSP`, … | Appendix B, always implicit in v1 (not configurable); see §4.1 |

Errata. RFC 5234 has exactly two verified errata, and both correct the ABNF-of-ABNF in §4 — each removing an ambiguity in it that the other does not:

- **2968** `elements = alternation *c-wsp` becomes `elements = alternation *WSP`.
- **3076** `rulelist = 1*( rule / (*c-wsp c-nl) )` becomes `rulelist = 1*( rule / (*WSP c-nl) )`.

Neither changes the language that grammar describes, only how many ways a given input can be derived; but a deterministic parser has to be shaped around them, so the grammar-text parser implements both. Both the corrected and uncorrected self-definitions are kept as fixtures (§8).

Numeric concatenation (`%d13.10`) needs no erratum: it is described in RFC 5234 §2.3 and appears in the `bin-val` / `dec-val` / `hex-val` productions as published.

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
| Grammar defines a core-rule name (`DIGIT = …`) | Valid; the explicit definition **shadows** the implicit core rule. Lint warning `ShadowsCoreRule`, never an error — RFC 8259 defines `char`, which shadows the core rule `CHAR`, so treating this as an error would reject the JSON grammar outright |
| Grammar defines a core-rule name twice explicitly | Structural error, as for any duplicate |
| `DIGIT =/ "x"` with no explicit `DIGIT =` | **Structural error** `IncrementalWithoutBase { name, shadows_core: true }`; its `Display` suggests `DIGIT = <core definition> / "x"`. Extending an implicit rule has no coherent meaning under hygienic resolution (below) |

**Hygienic core rules.** Rule references *inside core-rule bodies* always resolve within the core environment; user shadowing affects only references written in the user grammar. So `DIGIT = "x"` changes what a user reference to `DIGIT` means, but core `HEXDIG` (defined as `DIGIT / "A" / … / "F"`) still matches `7`. The leaky alternative would let a lint-warned convenience silently redefine `HEXDIG`, `LWSP` and everything downstream. Under hygiene, a grammar that happens to reuse a core rule name for something unrelated — RFC 8259's `char` is `unescaped / escape (…)`, nothing like `CHAR = %x01-7F` — changes only its own references, which is the intended outcome. User references resolve user-first, then core.

The internal model is therefore two layers (§6.7): a **syntactic** `Grammar` — the ordered list of definitions exactly as written, each `name = …` or `name =/ …` — and a **semantic** `CheckedGrammar` — the merged rule table produced by `check()`, resolved against the fixed **core-rule environment**. Duplicate definitions and `=/` without base are detected while building the rule table, so they are `CheckError`s, not `ParseError`s, and a `Grammar` never carries hidden validity state. `Display` at either layer serializes the user grammar only.

### 4.2 Parser strictness

The hand-written parser accepts exactly the language of the canonical self-grammar (RFC 5234 §4 + Errata 2968 and 3076 + RFC 7405 §2.2), **modulo line-ending normalization**: the self-grammar demands CRLF, while this parser normalizes LF and CR to CRLF-equivalent before parsing unless `strict_crlf` is set. Apart from that one deliberate leniency there is none — comments and prose values are ASCII-only as the RFC specifies, and a non-ASCII byte anywhere in grammar text is `ParseError::NonAscii { span }`. This invariant is what makes M2's self-recognition test and M3's self-generation test meaningful in both directions; a lenient parser would make them vacuous. Consequence: every fixture under `tests/grammars/` must be ASCII-clean, enforced by a test that scans them. A `ParseOptions::lenient_comments` syntactic option may be added later if real grammars need UTF-8 comments; it would not affect the default path.

## 5. Architecture

```
abnf-oracle/
├── Cargo.toml                 features: cli (opt-in, NOT default)
├── SCOPE.md                   ← this file
├── src/
│   ├── lib.rs                 public API re-exports
│   ├── ast.rs                 Grammar, Rule, Element, Repeat, CharVal, NumVal, node ids
│   ├── parse.rs               grammar text → Grammar (hand-written recursive descent; local rewrites only, no merging)
│   ├── core_rules.rs          Appendix B rules as a Grammar constant
│   ├── check.rs               rule-table build (=/ merge, duplicates, core resolution) → CheckedGrammar; analyses; node ids
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
│   ├── recognize_property.rs  the recognizer against an enumerated-language oracle (§4.3)
│   ├── repetition.rs          nullable-repetition regression cases (§6.3)
│   ├── generate_roundtrip.rs  generated strings are accepted
│   ├── generate_coverage.rs   the coverage bound holds (§6.8)
│   ├── self_definition.rs     the RFC 5234+7405 self-grammar recognizes every fixture's text
│   ├── self_generation.rs     grammars it generates, the parser accepts (D35, the other half)
│   └── cli.rs                 the exit codes of §11, through the built binary
└── scripts/
    ├── extract-fixtures.py    rebuild tests/grammars from published RFC text
    ├── differential.py        compare verdicts against other implementations
    ├── goharness/             go-abnf behind this crate's command shape, for the above
    └── DIFFERENTIAL.md        what the last run found
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

The recognizer descends recursively, so nesting in the *input* becomes depth on the call stack, and a stack overflow aborts the process rather than returning anything. `MatchOptions::max_depth` therefore bounds recursion and returns `MatchError::DepthLimit`. Unlike every other limit here its default is **finite**, because the failure it prevents cannot be caught and reported: an unlimited default would mean the safe behaviour is the one the caller has to opt into. Depth counts grammar nodes entered rather than input characters, so a flat input of any length costs nothing; the cost is about 2 KB of stack per level, which is what fixes the default. Raising the limit means giving the recognizer a larger stack to match.

### 6.3 Repetition

Bounds are validated before any of this runs: `min > max` (e.g. `5*2"a"`) is `CheckError::InvalidRepeatRange { min, max }`, and a descending numeric range (`%x5A-41`) is `CheckError::InvalidNumericRange { lo, hi }`. Both are structural errors; the recognizer may assume `min <= max` and `lo <= hi`.

Revision 1's fixpoint cutoff was wrong for nullable elements with a positive minimum (`3*3["a"]` on empty input must accept). The corrected algorithm has two phases:

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

Mandatory regression cases in `tests/repetition.rs`. Note the spelling: `repetition = [repeat] element` admits nothing between the bounds and what they repeat, so `3*3["a"]` is valid ABNF and `3*3 ["a"]` is not — a space there would need `repetition = [repeat] *c-wsp element`, which RFC 5234 does not say.

| Grammar | Input | Expected |
|---|---|---|
| `3*3["a"]` | `` (empty) | accept |
| `3*3["a"]` | `aa` | accept |
| `3*3["a"]` | `aaaa` | reject |
| `2*2"a"` | `a` | reject |
| `*["a"]` | `` | accept, terminates |
| `1*("a" / "")` | `` | accept |
| `2*4"ab"` | `ababab` | accept |
| `2*4"ab"` | `ababababab` | reject |
| `1000000000*["a"]` | `` | accept, promptly (phase-1 early exit) |
| `1000000000*"a"` | `aaa` | reject, promptly |
| `5*2"a"` | — | `CheckError::InvalidRepeatRange` |
| `%x5A-41` | — | `CheckError::InvalidNumericRange` |

Additionally, a property test compares the recognizer against a brute-force enumerator on random tiny grammars (≤ 4 rules, ≤ 3 nesting) and inputs of length ≤ 6.

### 6.4 Nullability, left recursion, productivity

`check()` computes the following analyses by fixpoint iteration over the user grammar plus the core-rule environment. `nullable` and `min_len` are computed **per AST node**; the rest per rule.

- **nullable(n)**: can `n` match the empty string?
- **min_len(n)**: length of the shortest string `n` can match, as `MinLen::Finite(u64)` or `MinLen::Infinite`. Arithmetic on the finite variant **saturates** at `u64::MAX`: nested repetitions can produce a shortest expansion that overflows even though every literal fits (§6.1), and a saturated value is still *finite*, hence still productive. A node is **productive** iff `min_len` is finite.
- **witness(n)**: for every alternation node with finite `min_len`, the branch that achieves it by the **shortest derivation** — fewest steps, ties broken by branch index; for every repetition, the count `min`. Witnesses form a well-founded derivation: each names a node whose own `min_len` was settled before the node referring to it, so following witnesses from any productive node reaches terminals in finitely many steps. This is the generator's termination device (§6.8). Comparing `min_len` magnitudes is *not* a substitute: ties are ordinary — `a = b / "x"` with `b = "z" a / "y"` settles both rules at 1, and both branches of `a` at 1 — and saturated values compare meaninglessly, so the branch chosen would turn on an implementation accident rather than on the grammar. Ordering by derivation length also bounds the work: the chain followed is the shortest the grammar allows, not merely one that yields a shortest string.
- **first-graph**: edge `A → B` iff `B` can be the first thing matched by `A`, i.e. `B` occurs in `A`'s body preceded only by nullable elements. The body of a repetition with `max == 0` can never match anything, so it contributes no edges; likewise it is excluded from `reaches_prose` and `reaches_unrepresentable`. (D18 guarantees `max == 0` implies `min == 0`; such a node is nullable with `min_len = Finite(0)`.) This is the analysis-side twin of the coverage rule in §6.8.

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

There are two layers, each with its own canonical `Display` and its own round-trip requirement.

**`Grammar` (syntactic).** An ordered list of definitions as written, each `name = elements` or `name =/ elements`. `parse` applies only *local* rewrites that need no knowledge of other rules: `*1a → [a]`, `1*1a → a`, numeric spelling normalization. It does **not** merge `=/`, resolve names, or assign node ids. `Display` prints one definition per line in that order. `PartialEq` compares the definition list, ignoring parse options (syntactic provenance, never serialized — which is why `ParseOptions` must never hold semantic configuration). Round-trip: `Grammar::parse(g.to_string()) == g` for a `g` parsed with any options.

**`CheckedGrammar` (semantic).** The merged rule table: every `=/` folded into its base rule in order, names resolved user-first then core (hygienically, §4.1), node ids assigned. `Display` prints one merged rule per line in first-definition order; implicit core rules are never printed. `PartialEq` compares merged rules, ignoring node ids. Round-trip: `Grammar::parse(cg.to_string()).check() == cg`.

Comments are not preserved at either layer (semantic-only model).

**Node ids** are assigned in `check()` by pre-order traversal of the merged, normalized rule table in first-definition order. Two grammars with the same merged canonical form therefore have identical node ids, which is what makes the generator's determinism contract (§6.8) meaningful across equal grammars, and coverage units (§6.8) are computed on the same shape `CheckedGrammar::Display` prints.

**Canonical spelling** (both layers):

| Element | Canonical form |
|---|---|
| Numeric value | `%x` with uppercase hex, zero-padded to an even digit count: `%x0D`, `%x41`, `%x0100`, `%x10FFFF` |
| Numeric range | `%x41-5A` (each endpoint padded as above) |
| Numeric concatenation | `%x41.42.43` |
| Case-insensitive string | bare `"abc"`; `%i"abc"` is never emitted |
| Case-sensitive string | `%s"abc"` |
| Empty string | `""` |
| Repetition | `*a`, `3a`, `2*5a`, `*3a`, `3*a`; never `0*a`, never `*1a` (that is `[a]`), never `1*1a` (that is `a`) |
| Parenthesization | Emitted exactly where dropping them would change the parse, and nowhere else (below) |
| Whitespace | single spaces between elements, ` = ` / ` =/ ` around the definition operator, no trailing whitespace, CRLF line endings |

**Parentheses.** An element is parenthesized in exactly two positions:

- as an **item of a concatenation**, if it is an alternation;
- as the **body of a repetition**, if it is an alternation, a concatenation, or another repetition.

Nowhere else. A concatenation in an alternation branch, a repetition as an item of a concatenation, an optional in any position, and the contents of `(…)` or `[…]` all stand bare; `[x]` never takes outer parentheses, and a group that survives none of the above is dropped.

The three repetition-body cases are the ones easy to miss, and each breaks the round-trip on its own: `*(a b)` printed as `*a b` reads back as `(*a) b`; `*(a / b)` printed as `*a / b` reads back as `(*a) / b`; and `*(2*a)` printed as `*2*a` does not parse at all, because RFC 5234's `element` admits neither `concatenation`, `alternation` nor `repetition` — only a rule name, a group, an option, or a terminal.

### 6.8 Generator

A recursive walk over the AST with a depth budget. Every choice point has a stable **node id** assigned on the merged rule table in `check()` (§6.7).

- Terminal: emit it. Ranges pick uniformly among representable scalars. Case-insensitive strings vary case randomly unless `preserve_case` is set.
- Alternation (including `[a]` as `a / empty`): choose a branch per the selection rule below, **never a branch whose `min_len` is `∞`**. This ban is absolute and applies in every mode, not only under an exhausted depth budget: once inside an unproductive branch (`bad = "x" bad`) there may be no alternation left to steer by, and the walk would never end.
- Repetition `min..max`: choose a count uniformly in `[min, min(max, min + spread)]`, `spread` default 3 — except in coverage mode, where if the body can reach an uncovered coverage unit and `max >= 1`, the count is at least `max(min, 1)`. A repetition with `max == 0` makes its body unreachable; units inside it are not coverage units.
- Rule reference: recurse. When the depth budget is exhausted, the walk switches to **witness mode**: every subsequent alternation takes its `witness` branch and every repetition takes count `min` (§6.4). Because witnesses form a well-founded derivation, this terminates regardless of `min_len` ties or saturation. Choosing "the branch with the smallest `min_len`" is *not* an acceptable substitute. In coverage mode the depth budget is **suspended along a chase** (below): witness mode engages only when no uncovered unit is reachable from the current node.
- Resource bound: `GenOptions::max_output_len` (default `1 << 20` scalars) and `GenOptions::max_steps` (default `1 << 24` node visits). Exceeding either returns `GenError::OutputLimit` / `GenError::StepLimit`. Mandatory work is not bounded by depth — `start = 1000000000*"a"` is productive and has no shorter expansion — so termination alone does not make generation practical; the bound does.
- Prose value or unrepresentable terminal: cannot be reached, since the start rule was rejected at `generate()` entry if it could reach one.

**Generatable graph.** For everything coverage-related, reachability is computed over the *generatable* graph: the AST minus every alternation branch with infinite `min_len` and every `max == 0` repetition body, since the generator never enters either. This matters for nested units — in `start = "ok" / bad`, `bad = ("p" / "q") bad`, the branches `"p"` and `"q"` have finite `min_len` of their own but sit inside an unproductive branch, so they are not generatable and must not be counted.

**Coverage unit**: `(alternation node id, branch index)` **where the branch's `min_len` is finite and the branch is reachable from the start rule over the generatable graph**. Syntactically reachable branches with infinite `min_len` are not coverage units — they are reported by the `UnproductiveAlternative` lint instead (§6.4). Repetition counts are not coverage units; `[a]` contributes two units via its `a / empty` model, and since `*1a` canonicalizes to `[a]` (§6.7) the two spellings are indistinguishable here.

**Distance to uncovered.** `dist_to_uncovered(node)` is the number of edges, over the generatable graph, from `node` to the nearest uncovered coverage unit; `∞` if none is reachable. Computed by fixpoint from the current coverage set and recomputed whenever coverage changes (coverage only grows within a `generate()` call, so a lazy recompute at each call boundary plus incremental updates when a unit is newly covered is sufficient).

**Selection rule in coverage mode** (deterministic-first):

1. Among the branches of the current alternation, prefer any branch that is itself uncovered.
2. Otherwise, prefer the branch with the smallest `dist_to_uncovered`, ties broken by branch index. This is a **chase**: distance strictly decreases along it, so it ends within `dist` steps, deterministically. The depth budget does not apply while chasing; `max_steps` and `max_output_len` remain the hard backstops.
3. Otherwise (`dist_to_uncovered` is `∞` for every branch), choose randomly, and witness mode may engage as usual when the depth budget is exhausted.

"Prefer any branch that reaches an uncovered unit, ties at random" is *not* an acceptable substitute for rule 2: with `a = b / c`, `b = "z" a`, `c = "x" / "y"` and only `(c, "y")` uncovered, both branches of `a` reach it, and random tie-breaking can loop through `b` unboundedly, emitting `"z"` each lap until `max_output_len` trips. (The `"z"` prefix is what keeps `b` off the first-graph; `b = a` would be left recursion and fail `check()`.) That is the same failure D28 fixed for witnesses, and the same cure — a well-founded measure — applies.

Consequence, which M3 asserts: while any coverage unit reachable from the start rule remains uncovered, each successful `generate()` call covers at least one new unit. The coverage-aware repetition count above is what makes this hold through `*(...)`, where a zero count would otherwise cover nothing, and the chase is what makes it hold on grammars deeper than `max_depth`, where witness mode would otherwise cut the walk short of the unit. Therefore, subject to configured resource limits, full coverage requires at most `N` successful calls, where `N` is the number of reachable (and by definition generatable) coverage units. This is a guarantee, not a probability, and it does not depend on `max_depth`.

**Determinism contract**: given the same canonical grammar, the same `GenOptions`, the same seed, and the same sequence of API calls on a fresh `Generator`, the sequence of generated strings is identical. This is guaranteed within a crate version. Cross-version stability is *not* promised: corpora are committed as files (§9), so the seed is not the artifact. The RNG is an inline SplitMix64 to avoid a dependency and to make cross-version stability likely in practice.

## 7. Public API sketch

```rust
pub struct Grammar { /* ordered definitions as written (= and =/), locally rewritten; parse options */ }

impl Grammar {
    pub fn parse(src: &str) -> Result<Grammar, ParseError>;               // core rules implicit
    pub fn parse_with(src: &str, opts: ParseOptions) -> Result<Grammar, ParseError>;
    pub fn parse_options(&self) -> &ParseOptions;                         // provenance; not part of PartialEq
    pub fn check(self) -> Result<CheckedGrammar, CheckErrors>;        // consumes; merges =/, resolves names, assigns ids, runs analyses; structural errors only
}

/// A grammar that passed structural validation. The only route to a Recognizer or Generator.
pub struct CheckedGrammar { /* merged rule table with node ids + analyses: nullable, min_len (saturating) and witness per node; reaches_prose and reaches_unrepresentable per rule */ }

pub enum MinLen { Finite(u64), Infinite }   // Finite saturates at u64::MAX

impl CheckedGrammar {
    pub fn rule(&self, name: &str) -> Option<&Rule>;                     // case-insensitive
    pub fn lint(&self) -> Vec<LintWarning>;                              // unreferenced, unproductive rule/alternative, shadowing
    pub fn lint_from(&self, start_rules: &[&str]) -> Vec<LintWarning>;   // adds unreachable
    pub fn can_recognize(&self, rule: &str) -> Result<(), MatchError>;   // compatibility check without input
}

pub struct MatchOptions { pub max_steps: Option<u64>,      // default: None (unlimited)
                          pub max_depth: Option<usize> }   // default: Some(DEFAULT_MAX_DEPTH)

/// Bound to exactly one input for its lifetime; the memo table is valid only for that input.
pub struct Recognizer<'g, 'i> { /* &CheckedGrammar, &[char] input, memo, options */ }

impl<'g, 'i> Recognizer<'g, 'i> {
    pub fn new(grammar: &'g CheckedGrammar, input: &'i str) -> Self;
    pub fn with_options(self, opts: MatchOptions) -> Self;
    pub fn accepts(&mut self, rule: &str) -> Result<bool, MatchError>;
    pub fn end_positions(&mut self, rule: &str, start: usize) -> Result<BTreeSet<usize>, MatchError>;
    pub fn steps(&self) -> u64;                       // repetition/rule evaluations over this recognizer's whole life; never reset
}

pub struct GenOptions { pub max_depth: usize, pub spread: usize, pub coverage: bool, pub preserve_case: bool,
                        pub max_output_len: Option<usize>, pub max_steps: Option<u64> }

pub struct Generator<'g> { /* &CheckedGrammar, SplitMix64, coverage state, dist_to_uncovered, options */ }

impl<'g> Generator<'g> {
    pub fn new(grammar: &'g CheckedGrammar, seed: u64) -> Self;
    pub fn with_options(self, opts: GenOptions) -> Self;
    pub fn generate(&mut self, rule: &str) -> Result<String, GenError>;
    pub fn uncovered(&mut self, rule: &str) -> Result<usize, GenError>;  // units reachable over the generatable graph, not yet covered; caches, hence &mut
    pub fn steps(&self) -> u64;                       // node visits in the current call; reset by each generate(), unlike Recognizer::steps
}

pub enum MatchError { ProseValueReachable {..}, UnrepresentableTerminal {..}, UnknownRule(String), LeftRecursionDetected {..}, StepLimit, DepthLimit }
pub enum GenError   { ProseValueReachable {..}, UnrepresentableTerminal {..}, UnknownRule(String), NoFiniteExpansion {..}, OutputLimit, StepLimit }
```

Error types are plain enums with `Display`; no `anyhow` in the library. The CLI may use `anyhow`.

## 8. Milestones and acceptance criteria

Each milestone is a PR-sized unit. Do not start the next before the current one's tests pass in CI.

**M0 — Skeleton.** Crate compiles with `#![forbid(unsafe_code)]`; CI runs `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, once with default features and once with `--features cli`. `ast.rs` has the full data model including node ids. Test directory layout from §5 exists.

**M1 — Grammar parser and check.**
- Parses every `.abnf` in `tests/grammars/`: RFC 5234 Appendix B core rules; the ABNF self-definition in three variants — RFC 5234 §4 as published, RFC 5234 §4 with both verified errata applied, and the **canonical self-grammar** (RFC 5234 §4 + Errata 2968 and 3076 + the RFC 7405 §2.2 `char-val` amendments), which is the one M2 uses; RFC 8259 JSON; RFC 3986 URI; RFC 5322 §3 address grammar; RFC 3339 date-time; RFC 9110 Appendix A collected ABNF (exercises `obs-text = %x80-FF` and prose values: its URI rules are `<…>` references into RFC 3986).
- Every fixture passes `check()` except deliberately broken fixtures under `tests/grammars/invalid/` (undefined rule, duplicate, `=/` without base, `=/` on an implicit core rule with `shadows_core: true`, direct and indirect left recursion, `min > max` repeat, descending numeric range), each of which must fail with the expected `CheckError` variant. A fixture with a 25-digit repeat count fails to *parse* with `NumberTooLarge`; a fixture with a non-ASCII byte in a comment fails to parse with `NonAscii`.
- A test scans every file under `tests/grammars/` and fails on any non-ASCII byte (§4.2).
- Round-trip at both layers: `Grammar::parse(g.to_string()) == g` and `Grammar::parse(cg.to_string()).check() == cg` for every valid fixture, including fixtures parsed with `strict_crlf = true`. `*1a` and `[a]` parse to equal grammars. Canonical spelling is unit-tested against the §6.7 table.
- `lint()` and `lint_from()` produce the expected warnings on hand-written cases, including `UnproductiveRule` and `UnproductiveAlternative`; the RFC 8259 fixture yields exactly one `ShadowsCoreRule` warning, for `char` against the core rule `CHAR`, and no errors; the core-rules fixture yields sixteen and no errors.
- Analyses (nullable, `min_len` and `witness` per node; `reaches_prose` and `reaches_unrepresentable` per rule) are unit-tested on hand-written grammars, including: a `min_len` that saturates (three nested `4294967295` repetitions) and is still `Finite`; a prose-containing rule that does *not* trigger left recursion or `UnproductiveAlternative`; and the tie case `a = b / "x"`, `b = "z" a / "y"`, whose witnesses point at the terminals.
- Node ids: two textually different grammars with the same canonical form yield identical node ids.
- Acceptance is about parsing and checking only. Nothing in M1 recognizes input.

**M2 — Recognizer.**
- Set-of-positions implementation per §6.2–§6.3, with memoization and the in-progress guard.
- `tests/repetition.rs` passes every case in the §6.3 table, plus the brute-force property test. For the large-bound cases the test asserts `recognizer.steps() <= c * (input_len + 1)` for a fixed small `c`, which checks the O(input) property directly; a generous timeout remains only as a hang guard, never as the assertion.
- `tests/corpus/rfc8259/` populated from JSONTestSuite: every `y_*` accepted, every `n_*` rejected, `i_*` recorded not asserted. Any `n_` case that pure ABNF accepts (encoding-level rejections are outside the grammar) is moved to `i_` and listed in `tests/corpus/rfc8259/NOTES.md`.
- `self_definition.rs`: the recognizer, running the **canonical self-grammar** (RFC 5234 + Errata 2968 and 3076 + RFC 7405), accepts the CRLF-normalized text of every valid fixture in `tests/grammars/`, including fixtures that use `%s`/`%i` strings. The test normalizes line endings itself so results do not depend on Git checkout settings.
- Compatibility limits: a fixture with a prose value yields `ProseValueReachable` from a start rule that reaches it, and `Ok` from one that does not.
- Hand-written unit tests for every construct in §4, including `=/`, `%s` vs `%i`, nested optionals, and core-rule shadowing.
- Hygiene: with `DIGIT = "x"` and a user rule referencing `HEXDIG`, `HEXDIG` still matches `7` and a user reference to `DIGIT` matches `x` and not `7`; exactly one `ShadowsCoreRule` warning, no errors.

**M3 — Generator.**
- `generate_roundtrip.rs`: for each valid fixture and 200 seeds, `generate(start)` is accepted by the recognizer. Zero failures.
- Coverage bound: for the JSON grammar, with `coverage = true`, `uncovered(start)` reaches zero within `N` calls, where `N` is the initial `uncovered(start)`. Asserted exactly, not probabilistically. Repeated on a hand-written grammar containing an unproductive alternative: the bound still holds, and the dead branch is never emitted.
- Coverage is independent of depth: a hand-written grammar whose only uncovered unit sits behind a chain of five rule references, generated with `max_depth = 2` and `coverage = true`, still reaches `uncovered(start) == 0` within `N` calls. JSON is too shallow to catch a regression here; this test exists because of that.
- Chase is deterministic: on `a = b / c`, `b = "z" a`, `c = "x" / "y"` with `(c, "y")` the last uncovered unit, the call that covers it visits `a` at most `dist_to_uncovered(a)` times, asserted via `Generator::steps()`; the output contains no `z`; and the result is identical across two seeds. A naive tie-at-random implementation fails this test with `OutputLimit`, not a hang.
- Nested units inside an unproductive branch are not counted: `start = "ok" / "alt" / bad`, `bad = ("p" / "q") bad` reports `uncovered(start) == 2` — the two productive branches of `start` — and none for the `bad` branch or the `"p" / "q"` alternation inside it, even though those two branches have finite `min_len` of their own. Coverage completes in at most two calls.
- `NoFiniteExpansion` returned immediately for an unproductive start rule; `ProseValueReachable` for a prose-reaching one. Random mode (`coverage = false`) on `start = "ok" / bad`, `bad = "x" bad` terminates on every one of 1000 seeds.
- Zero-count repetition: on `start = *("a" / "b")` in coverage mode, both branches are covered in at most two calls; a hand-written grammar with a `*0(...)` body reports zero units inside it.
- Self-generation: 500 strings generated from the canonical self-grammar's `rulelist`, in coverage mode, all parse with `Grammar::parse`. Together with M2's self-recognition this checks §4.2's invariant in both directions.
- Witness termination: with `max_depth = 0`, generation from `a` in `a = b / "x"`, `b = "z" a / "y"` terminates and yields `x`, deterministically, because the witness of `a` is the branch with the shortest derivation (§6.4).
- Resource bound: `start = 1000000000*"a"` returns `OutputLimit` under default options rather than running.
- Determinism: two fresh generators with the same seed and options produce identical sequences over 100 calls.

**M4 — Differential testing (non-blocking during implementation; required before the 1.0 release).**
- `scripts/differential.py` drives every implementation that is installed, in three directions per fixture: strings this crate generates that the other must accept, strings the other generates that this crate must accept, and a shared corpus both must decide the same way. `scripts/goharness/` exposes go-abnf through this crate's own command shape and exit codes so the driver treats both alike.
- Any disagreement is a bug in one of the implementations — or in the harness, which is a suspect too — and must be understood before being waived. Attributed ones live in `ATTRIBUTED` in the driver, so a known divergence stays green while a new one fails the run; nothing goes there until it has been reduced to a minimal case and checked against a third implementation.
- **As shipped:** python-abnf 2.9.0 and go-abnf v0.5.1. `abnfgen` was not run — it is a C program with no Windows package — and go-abnf's generator covers the direction it would have. The run found a real bug in go-abnf, and `scripts/DIFFERENTIAL.md` records the results, the divergences and the reasoning.
- The octet-range disagreements this section expected to document in advance **did not appear**: RFC 9110's `obs-text = %x80-FF` round-trips through both implementations, so all three read those terminals as code points rather than bytes. The §3 limitation is real; it would take a genuinely byte-oriented implementation to expose it.

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
abnf-oracle match  <grammar.abnf> --rule R (--input S | --file F | --dir D) [--max-steps N] [--max-depth N]
abnf-oracle gen    <grammar.abnf> --rule R [--seed N] [--count N] [--coverage] [--depth N] [--spread N]
                                   [--preserve-case] [--max-output-len N] [--max-steps N] [--out DIR]
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
11. Qualifying the coverage guarantee with "subject to `max_depth`" instead of suspending the depth budget during a chase: **no**. A guarantee that fails on any grammar deeper than a tuning knob is not one; the chase is bounded by `dist_to_uncovered` and backstopped by `max_steps`, so suspending the budget costs nothing (§6.8).

## 13. Decisions before M1 (normative)

Each item traces to the review that motivated it (D1–D16 first review, D17–D24 second, D25–D31 third, D32–D37 implementation-planning questions, D38 implementation follow-up, D39 through D42 caught while building the checker and recognizer). Implement these as written.

- **D1** A `Recognizer` or `Generator` can only be constructed from a `CheckedGrammar`. `Grammar::check` consumes the `Grammar`. There is no unchecked path.
- **D2** A `Recognizer` is bound to one input at construction. The memo table lives inside it and is never reused across inputs.
- **D3** Repetition is implemented exactly as §6.3, including the phase-1 equality early exit, with the regression table and the brute-force property test as acceptance.
- **D4** The matching domain is Unicode scalar values. Representability is defined in §6.1; unrepresentable terminals are a per-rule compatibility limit, not a parse or check failure. Octet semantics are out of scope (§3).
- **D5** `check()` reports only structural errors. Reachability lint requires start rules via `lint_from`. Warnings never fail `check()`.
- **D6** Definition precedence follows the table in §4.1. Explicit definitions shadow core rules with a lint warning, never an error.
- **D7** No ambiguity detection or derivation counting in v1. The former M2 ambiguity test is removed; both self-definition variants remain as parse fixtures only.
- **D8** M1 acceptance covers parsing and checking only; self-recognition is M2 and operates on CRLF-normalized text.
- **D9** `min_len` / productivity is computed in `check()` per AST node; unproductive rules and unproductive alternatives are lint warnings; `generate()` fails fast with `NoFiniteExpansion` on an unproductive start rule.
- **D10** Coverage units are `(alternation node id, branch index)` with finite `min_len`, reachable from the start rule over the **generatable graph** (no branch with infinite `min_len`, no `max == 0` body on the path). Selection is deterministic-first per §6.8: an uncovered branch first, else the branch with the smallest `dist_to_uncovered` with ties by index, else random. The depth budget is suspended during a chase, so the M3 bound holds independently of `max_depth`.
- **D11** Prose values are a per-start-rule compatibility limit; no three-valued matching. Resolver hook deferred to v1.1.
- **D12** `Grammar` and `CheckedGrammar` represent two distinct layers of the user grammar (§6.7). `Grammar` preserves the ordered `=` / `=/` definitions after local canonicalization only; `CheckedGrammar` contains the semantic rule table after incremental definitions are merged and names are resolved. Implicit core rules are part of the resolution environment and are never serialized as user rules. Each layer has its own `Display`, `PartialEq`, and round-trip contract. Parse options are excluded from `PartialEq` at the `Grammar` layer, and node ids are excluded at the `CheckedGrammar` layer.
- **D13** The `cli` feature is opt-in. The library target has zero required dependencies. Directory exit codes per §11.
- **D14** Invalid UTF-8 input is an error (exit 2), never a rejection.
- **D15** Determinism is guaranteed for a fresh generator with the same seed, options and call sequence, within a crate version. RNG is inline SplitMix64.
- **D16** Step limits are exposed via `MatchOptions::max_steps` and `--max-steps`; default unlimited; exceeding is an error, never a silent reject.
- **D17** Numeric literals and repetition bounds are `u64`; overflow is `ParseError::NumberTooLarge`. Goal 1 is narrowed accordingly.
- **D18** `min > max` in a repetition and `lo > hi` in a numeric range are structural errors (`InvalidRepeatRange`, `InvalidNumericRange`).
- **D19** Phase 1 of repetition exits early on an *exact* fixpoint (`f(cur) == cur`) or emptiness, so large bounds cost at most input-length iterations.
- **D20** The generator never enters a branch with infinite `min_len`, in any mode. Coverage units exclude such branches and everything nested inside them. `UnproductiveAlternative` is a lint.
- **D21** `PartialEq` ignores parse options; they are provenance. Round-trip holds for any options.
- **D22** Node ids are internal, excluded from `PartialEq`, and assigned in `check()` by pre-order traversal of the merged rule table (superseded in detail by D32).
- **D23** The self-recognition fixture is the canonical self-grammar: RFC 5234 §4 + Errata 2968 and 3076 + RFC 7405 §2.2. It must recognize every fixture, including those using `%s`/`%i`.
- **D24** M4 is non-blocking during implementation and required before the 1.0 release. No complexity guarantee is claimed for the recognizer; `max_steps` is the resource bound.
- **D25** In coverage mode, a repetition whose body can reach an uncovered unit and whose `max >= 1` uses a count of at least `max(min, 1)`. Units inside a `max == 0` repetition are not coverage units.
- **D26** For analyses, prose is assumed non-nullable with `min_len = 1`. Prose never creates a first-graph edge and never triggers unproductive lints.
- **D27** `min_len` is `MinLen::{Finite(u64), Infinite}` with saturating arithmetic. Saturated is finite and productive.
- **D28** `check()` records a witness branch per productive alternation. Exhausted-budget generation follows witnesses, never "smallest `min_len`".
- **D29** Generation has explicit resource bounds (`max_output_len`, `max_steps`) with corresponding `GenError` variants; defaults are finite.
- **D30** `ParseOptions` holds syntactic options only. The core-rule environment is fixed in v1 and not configurable. `PartialEq` ignoring parse options is therefore exact.
- **D31** Canonicalization rewrites `*1a` to `[a]` and `1*1a` to `a` on the AST before node ids and coverage units are computed. Performance tests assert step counts via `Recognizer::steps()`, not wall-clock time.
- **D32** `Grammar` is syntactic (ordered definitions, local rewrites only); `CheckedGrammar` is semantic (merged, resolved, id-assigned). `=/` merging happens in `check()`, so duplicate definition and `=/`-without-base are `CheckError`s and a `Grammar` never carries hidden validity state. Each layer has its own `Display`, `PartialEq` and round-trip (§6.7).
- **D33** Core-rule resolution is hygienic: references inside core bodies resolve within the core environment; user shadowing affects only user references.
- **D34** `=/` on a core-rule name with no explicit base is `IncrementalWithoutBase { shadows_core: true }`, whose message suggests the `NAME = <core> / extra` workaround.
- **D35** The parser is strict per RFC 5234 for comments and prose values: both are ASCII-only. Any non-ASCII byte anywhere in grammar text is `ParseError::NonAscii`, checked before tokenization; comments and prose are simply the only positions where a non-ASCII byte could otherwise have been mistaken for valid content. Subject only to the documented line-ending normalization (§4.2) and the `u64` numeric-magnitude restriction in §6.1 / D17, the hand-written parser accepts exactly the language of the canonical self-grammar (RFC 5234 §4 + Errata 2968 and 3076 + RFC 7405 §2.2). Fixtures are required to be ASCII-clean and are checked accordingly. M3 verifies the reverse direction by generating from the canonical self-grammar and requiring every generated grammar to parse successfully.
- **D36** `max == 0` repetition bodies contribute no first-graph edges and are excluded from `reaches_prose` / `reaches_unrepresentable`.
- **D37** Canonical spelling follows the table in §6.7: even-padded uppercase `%x`, bare case-insensitive strings, `%s` for case-sensitive, and the stated parenthesization rules.
- **D38** The coverage guarantee is independent of `max_depth`. In coverage mode, witness mode engages only when `dist_to_uncovered` is `∞` at the current node; while it is finite the walk chases by strictly decreasing distance (ties by branch index), so the chase is bounded and deterministic. `max_steps` / `max_output_len` remain the hard backstops.
- **D39** RFC 5234 has two verified errata and both are §4 grammar corrections: 2968 fixes `elements`, 3076 fixes `rulelist`. Revisions before 5.3 described 3076 as clarifying numeric-value concatenation, which it does not — that is base RFC 5234 §2.3 — and attributed the `rulelist` fix to 2968. The canonical self-grammar is RFC 5234 §4 + **both** errata + RFC 7405 §2.2; the parser implements both corrections.
- **D40** No fixture RFC restates the core rules: RFC 9110 §5 includes them "by reference", and RFC 3986 defines none. Revisions before 5.5 justified `ShadowsCoreRule` by a verbatim restatement in those two documents, which does not exist. The decision is unchanged and the evidence is stronger: RFC 8259 defines `char`, unrelated to `CHAR = %x01-7F`, so shadowing must stay a lint or the JSON grammar would not check.
- **D41** The witness of an alternation is the branch achieving its `min_len` by the shortest derivation, ties by branch index. Revisions before 5.6 defined it as "the branch that first attained the value during fixpoint iteration", which presumes an algorithm the implementation does not use and leaves the choice undetermined; and they illustrated the tie with `a = b / "x"`, `b = a / "y"`, which is left-recursive and so never reaches the analyses at all.
- **D42** Recursion depth is bounded by `MatchOptions::max_depth`, whose default is finite — the only limit in this crate that is. Exceeding the stack aborts the process, which no caller can catch or report as "could not decide", so the safe behaviour cannot be the opt-in one. Exceeding the limit is `MatchError::DepthLimit`, never a rejection. Measured during M2.4: about 2 KB of stack per level of depth, so the default is set to be safe on a 1 MB stack.
- **D43** `Grammar::check` returns `CheckErrors`, a newtype over `Vec<CheckError>`, rather than the `Vec` itself. The orphan rule makes `impl Error for Vec<CheckError>` impossible, so the bare `Vec` could not flow through `?` and made this the one error in the crate needing special handling. The newtype derefs to `[CheckError]` and iterates by value and by reference, so it reads as the collection it is, and adds `render(src)` for the all-errors-at-once output the CLI already produced by hand.
- **D44** Memoization is transactional. `Memo::InProgress` is written before descending into a rule and removed again if a resource limit unwinds past the point where `Done` would be written, so the table is whole by the time the caller sees the error. Without it a reused recognizer reads the leftover marker as left recursion and reports `LeftRecursionDetected` for a grammar `check` has already proved has none — a wrong answer, which is worse than the limit it replaces. Reuse is a designed pattern: `end_positions` is parameterized on rule and start position, so repeated calls on one recognizer are ordinary use. Note what this does *not* change: `steps` is a per-recognizer budget and is not reset, so retrying after a `StepLimit` correctly reports `StepLimit` again.
- **D45** Phase 2 of the repetition algorithm counts *down* from the repetitions still allowed, rather than up from `min`. `repeat` admits `u64::MAX` as a bound (D17), so counting up overflows on `18446744073709551615*["a"]` — one iteration, then an increment that wraps, panicking in a debug build and wrapping silently in release. Counting down also states the invariant plainly: when the maximum is unbounded there is no counter at all, because nothing but an empty frontier ends the loop. Safe because `check` rejects an inverted range (D18), so `max - min` cannot underflow.
- **D46** Coverage reachability is cached by the start rule's body `NodeId`, not by an index into the user-rule slice. `CheckedGrammar::rules()` returns user rules only while `rule()` resolves core rules too, so generating from `ALPHA` is supported but has no user index — every implicit core rule fell back to one shared sentinel key, and `uncovered("HEXDIG")` after `uncovered("ALPHA")` returned `ALPHA`'s count. A `NodeId` is unique across both tables, and keying by it deletes the index reconstruction rather than correcting it. Generated strings were never affected: the chase reads `dist_to_uncovered`, not this cache.
- **D47** `UnreachableRule` is semantic and `UnreferencedRule` is textual, and the two are computed by different walks. Reachability stops where a `max == 0` repetition stops (D36), matching the checker, the compatibility analysis and the generator; mention does not, because a name inside a body that can never run is still written there. So `start = *0dead` reports `dead` unreachable but not unreferenced. Collapsing the two — the obvious reading of "the walker descends where it should not" — would make `UnreferencedRule` contradict its own definition.

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
