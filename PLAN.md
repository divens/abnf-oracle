# Implementation plan: `abnf-oracle`

Companion to `SCOPE.md` (revision 4). SCOPE.md is normative on *what* is built and why;
this document is *how*, *in what order*, and *what still needs a decision*.

Status at time of writing: empty repository, no commits, `SCOPE.md` only.
Toolchain present: rustc/cargo 1.97.1 (edition 2024 available, MSRV target = 1.97).

Where this plan resolves something SCOPE.md leaves ambiguous, it is listed in §2 as a
**proposed decision D32+** with a recommendation. Those want a yes/no before M1 code lands,
because each one changes either the public error surface or grammar semantics.

---

## 1. Invariants to hold throughout

These are the things that, if broken quietly, make the crate untrustworthy as an oracle.
Every PR is reviewed against them.

1. **No unchecked path.** `Recognizer` and `Generator` are constructible only from
   `CheckedGrammar` (D1). `Grammar::check` consumes.
2. **Unordered alternation.** Nothing anywhere returns "the first branch that matched".
   Every element evaluates to a *set* of end positions (§6.2). Any code shaped like
   `if let Some(end) = try_branch(..) { return end }` is a bug.
3. **Memoization is an optimization only.** Results with the memo table disabled must be
   identical. Keep a `#[cfg(test)]` switch that bypasses the memo and run the §6.3
   regression table and the property test both ways.
4. **Analyses are per AST node, not per rule**, for `nullable`, `min_len`, `witness` (D9, D27).
   `reaches_prose` / `reaches_unrepresentable` are per rule (§6.5, §6.6).
5. **Canonical AST before ids.** Normalization (`=/` merge, `*1a` → `[a]`, `1*1a` → `a`)
   happens before node-id assignment, so node ids are a function of canonical form only
   (D22, D31).
6. **Three error categories stay separate** (§6.6): structural (`CheckError`, fails `check`),
   compatibility (per start rule, enforced by `Recognizer`/`Generator`), lint (advisory).
   No leakage in either direction.
7. **Library target has zero required dependencies** (D13). `cli` is opt-in.
   `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]` on public items.
8. **Limits are errors, never silent rejects** (D16, D29).

---

## 2. Decisions needed before M1 (proposed, not yet in SCOPE.md)

SCOPE.md's D1–D31 are settled and get implemented as written. The following are genuine forks
that D1–D31 do not cover; implementing M1 requires picking one branch of each. Each has a
recommendation; if the recommendation is accepted, fold it into SCOPE.md §13 as D32+ so the
spec stays the single source of truth.

### D32 — Where do `=/`-merge failures get reported?

SCOPE.md wants `Grammar` to be merged, canonical and id-assigned (§6.7, D22) — `Display`,
`PartialEq` and the round-trip requirement all operate on the merged form. But it also lists
*duplicate definition* and *`=/` without base* as `CheckError` (§6.6), and those are detected
precisely *during* the merge, which must happen inside `parse`.

**Recommendation:** `Grammar::parse` performs syntax → merge → canonicalize → assign ids, and
carries a private `issues: Vec<CheckError>` for structural problems found while assembling the
rule table. The offending definition is dropped from the merged AST (best effort, so `Display`
still works), and `check()` returns `issues` first, before running any analysis. Public
behaviour is then exactly what §6.6 and M1 specify; the wart is internal and documented in
`check.rs`.

Rejected alternative: promoting them to `ParseError`. Cleaner internally, but it contradicts
§6.6 and M1's acceptance criteria, and it makes `parse` fail on grammars whose canonical text
is perfectly displayable.

The same mechanism can carry `InvalidRepeatRange` and `InvalidNumericRange` (D18) if it turns
out cheaper to detect them in the parser — though the default is to detect those in `check()`'s
AST walk, where they naturally belong.

### D33 — Do core rules see user shadowing? (semantic, highest impact)

`DIGIT = "x"` shadows the core `DIGIT` (§4.1, D6). The core `HEXDIG` is defined as
`DIGIT / "A" / … / "F"`. Does that `DIGIT` reference resolve to the user's rule or to core
`DIGIT`?

**Recommendation: hygienic.** Rule references *inside core-rule bodies* always resolve within
the core environment. User shadowing affects only references written in the user grammar.
Rationale: the leaky reading means `DIGIT = "x"` silently redefines `HEXDIG`, `LWSP` and
anything else downstream, turning a lint-warned convenience into an invisible semantic
landmine. RFC 9110 and RFC 3986 restate core rules verbatim, so under the hygienic reading
their shadowing is a no-op, which is the desired outcome.

Test: `DIGIT = "x"` plus a rule referencing `HEXDIG`; assert `HEXDIG` still matches `7`, and
that a user reference to `DIGIT` matches `x` and not `7`. One `ShadowsCoreRule` warning, no
errors.

### D34 — `DIGIT =/ "x"` with no explicit base

§4.1 says `=/` with no prior `=` is a structural error. Under the hygienic reading of D33,
extending a core rule this way has no coherent meaning anyway.

**Recommendation:** structural error, per §4.1 as written, but with a distinct message:
`IncrementalWithoutBase { name, shadows_core: true }` whose `Display` suggests the
`NAME = <core definition> / extra` workaround. Cheap, and it turns a confusing failure into a
one-line fix.

### D35 — Parser strictness for comments and prose values

RFC 5234 restricts comments to `*(WSP / VCHAR)` and prose to `%x20-3D / %x3F-7E` — ASCII only.
Real-world `.abnf` files sometimes carry UTF-8 in comments.

**Recommendation: strict, exactly as the RFC.** This buys an invariant worth more than the
leniency: *the hand-written parser accepts exactly the language of the canonical
self-grammar*. M2's `self_definition.rs` checks one direction of that on fixtures; M3 can check
the other direction by generating from the self-grammar and feeding the output back to
`Grammar::parse` (§4.4). A lenient parser makes that invariant false and the self-test
meaningless. Consequence: all fixtures must be ASCII-clean — enforced by a test that scans
`tests/grammars/**` for non-ASCII bytes.

### D36 — `*0 elem`: does the body count for the first-graph?

D25 already says coverage units inside a `max == 0` repetition do not exist. The first-graph
(§6.4) should be consistent: a body that can never match cannot be "the first thing matched".

**Recommendation:** exclude `max == 0` repetition bodies from first-graph edges and from
`reaches_prose` / `reaches_unrepresentable`. Note `min > max` is already a structural error
(D18), so `max == 0` implies `min == 0`. `min_len` of such a node stays `Finite(0)`.

### D37 — Canonical numeric and string spelling

Needed for round-trip determinism; §6.7 says "`%x` for all numeric values" but not the digit
format.

**Recommendation:** uppercase hex, zero-padded to an even digit count (`%x0D`, `%x41`,
`%x10FFFF`); ranges `%x41-5A`; concatenation `%x41.42.43`. Case-insensitive strings print as
the bare `"abc"` form (never `%i`), case-sensitive as `%s"abc"`. Empty string prints `""`.
Parenthesization: an alternation nested inside a concatenation or a repetition is
parenthesized; a concatenation inside an alternation branch is not; `[x]` never needs outer
parens.

---

## 3. Module design

Dependency order (§5): `error` ← `ast` ← {`parse`, `core_rules`} ← `check` ← {`lint`,
`recognize`, `generate`}; `rng` standalone; the CLI depends on everything, nothing depends on
it.

### 3.1 `ast.rs` — data model (write it first, in M0)

```rust
pub struct NodeId(u32);                     // index into per-node analysis arrays
pub struct Span { start: u32, end: u32 }    // byte offsets into the original source

pub struct Grammar {
    rules: Vec<Rule>,                  // user rules, definition order, =/ merged
    index: BTreeMap<String, usize>,    // ASCII-lowercased name -> rules index
    node_count: usize,                 // ids are 0..node_count over user rules
    opts: ParseOptions,                // provenance only; excluded from PartialEq (D21/D30)
    issues: Vec<CheckError>,           // D32
}

pub struct Rule { name: String, body: Element, span: Span }   // name as first written

pub enum Element {
    Alt      { id: NodeId, branches: Vec<Element> },
    Concat   { id: NodeId, items: Vec<Element> },
    Repeat   { id: NodeId, min: u64, max: Option<u64>, body: Box<Element> },  // None = inf
    Optional { id: NodeId, body: Box<Element> },   // [x]; a two-branch alt for coverage
    RuleRef  { id: NodeId, name: String, span: Span },
    Str      { id: NodeId, lit: StringLit },
    Num      { id: NodeId, val: NumVal },
    Prose    { id: NodeId, text: String, span: Span },
}

pub struct StringLit { value: String, case_sensitive: bool }   // %s vs default/%i
pub enum NumVal { Scalar(u64), Range { lo: u64, hi: u64 }, Concat(Vec<u64>) }
```

Notes:

- `Optional` is a distinct variant rather than sugar for `Alt[x, Str("")]`. Keeping it distinct
  lets `Display` print `[x]` without shape-sniffing, and keeps `"a" / ""` (a legal ABNF
  spelling, since `char-val` may be empty) visibly different in the AST from `["a"]` while
  behaving identically. For coverage it exposes two units: `(id, 0)` = body taken,
  `(id, 1)` = skipped (§6.8).
- Every element carries an id. Ids index dense `Vec<_>` analysis arrays — no hash maps in the
  hot paths.
- `NumVal` keeps the radix out of the model: `%d65`, `%b1000001` and `%x41` parse to the same
  node and print as `%x41` (D37; §6.7 requires it).
- `Repeat.max: Option<u64>` rather than a sentinel — `None` is genuinely unbounded, and a
  sentinel would collide with the legal literal `*18446744073709551615`.

### 3.2 `parse.rs` — recursive descent

Cursor over `&str` with a byte-offset position; peek via `chars().next()` on the remaining
slice, so spans are byte ranges and map cheaply to line/column for errors.

Pipeline:

1. **Line-ending normalization.** Default: `CRLF`/`CR`/`LF` → `LF`. With `strict_crlf`, a bare
   `LF` or `CR` is `ParseError::ExpectedCrlf { span }`. Do it as a pre-pass producing an owned
   `String` plus an offset map back to the original bytes for error spans (or record a flag and
   translate at render time — the map is ~20 lines and worth it for error quality).
2. **`rulelist` per RFC 5234 §4 + Erratum 2968.** Structure the functions to mirror the
   corrected grammar one-to-one: `rulelist`, `rule`, `defined_as`, `elements`, `alternation`,
   `concatenation`, `repetition`, `repeat`, `element`, `group`, `option`, `char_val`,
   `num_val`, `prose_val`, `c_wsp`, `c_nl`, `comment`. One function per production keeps the
   "parser ≡ self-grammar" invariant (D35) auditable by reading.
   *Task: pull the corrected productions from the RFC 5234 errata page when transcribing the
   fixture; do not rely on memory for the erratum wording.*
3. **Line continuation** falls out of `c-wsp = WSP / (c-nl WSP)`; do not pre-join lines.
4. **Numbers** accumulate with `checked_mul`/`checked_add`; overflow →
   `ParseError::NumberTooLarge { span }` (D17). Applies to `%d`/`%x`/`%b` values, range
   endpoints and repeat bounds alike.
5. **`%s` / `%i`** prefixes (RFC 7405) only immediately before `DQUOTE`.
6. **Assembly** (D32): group definitions by ASCII-lowercased name, merge `=/` into the base in
   source order, detect duplicates and orphan `=/`.
7. **Normalization**: `Repeat{0, Some(1), b}` → `Optional(b)`; `Repeat{1, Some(1), b}` → `b`;
   flatten single-element `Alt`/`Concat`. Apply bottom-up to a fixpoint — the rewrites cascade
   through the flattening.
8. **Id assignment**: pre-order over the canonical AST, rules in definition order.

`ParseError` is a struct-like enum with a `span` on every variant plus a `render(src)` helper
producing a caret-underlined line — grammar authors are humans, and §3 says this is the one
place error quality matters.

### 3.3 `core_rules.rs`

The Appendix B rules as a `&'static str` constant, parsed once behind a `OnceLock<Grammar>` —
the crate eats its own dog food rather than hand-building an AST. A unit test asserts the
constant parses, checks, and defines exactly the expected 16 names
(`ALPHA BIT CHAR CR CRLF CTL DIGIT DQUOTE HEXDIG HTAB LF LWSP OCTET SP VCHAR WSP`).

Two things to get right:

- `LWSP` is defined via `CRLF` and is nullable — the one core rule that is nullable, and the
  one that will exercise nullable-repetition paths in real fixtures.
- `OCTET = %x00-FF` is representable under the scalar model (§6.1) but means "code points
  U+0000–U+00FF", per §3's documented limitation. Say so in a doc comment where someone
  debugging an octet-grammar mismatch will find it.

Under D33 (hygienic), core rules resolve among themselves. Implementation: `CheckedGrammar`
holds user rules at indices `0..n` and core rules at `n..n+16`; core node ids are allocated
after user node ids, so user ids remain exactly "pre-order of the canonical user AST" (D22) and
the analysis arrays still cover everything in one contiguous space.

```rust
pub enum RuleId { User(u32), Core(u32) }
```

Resolution: a `RuleRef` in a user body looks in `index`, then core. A `RuleRef` in a core body
looks in core only.

### 3.4 `check.rs` — the hard module

Order of operations. Errors accumulate — return all of them, never just the first; grammar
authors want the full list.

1. Drain `grammar.issues` (D32).
2. **Resolve references.** Unknown name → `UndefinedRule { name, span }`. Build the
   `NodeId -> RuleId` resolution table.
3. **Range validation.** `min > max` → `InvalidRepeatRange`; `lo > hi` → `InvalidNumericRange`
   (D18).
4. **Representability** (§6.1) per numeric terminal. A range is representable iff it contains
   at least one scalar value: `lo <= 0x10FFFF && !(lo >= 0xD800 && hi <= 0xDFFF)`. Precompute,
   per representable range, how many scalars it contains, so the generator can index into it
   uniformly in O(1) (subtract the surrogate block when the range spans it).
5. **`nullable`** — round-robin least fixpoint over nodes, init `false`. Prose is non-nullable
   (D26).
6. **`min_len` + `witness`** — §4.1 below. Knuth-style worklist, *not* round-robin, because the
   witness must be well founded by construction (D28).
7. **First-graph + left recursion.** Edges rule → rule; excludes `max == 0` bodies (D36); prose
   creates no edge (D26). Cycle detection by iterative DFS with colour marking, reporting the
   cycle path in definition order → `LeftRecursion { cycle: Vec<String> }`. Global, not per
   start rule (§6.4) — including for unreferenced rules.
8. **Per-rule reachability**: `reaches_prose`, `reaches_unrepresentable`. Transitive closure
   over the rule-reference graph (excluding `max == 0` bodies, D36), seeded from rules whose
   own body contains one directly.

`CheckedGrammar` then holds the `Grammar`, the resolution table, dense `Vec<bool>` /
`Vec<MinLen>` / `Vec<Option<Witness>>` indexed by `NodeId`, and per-rule flag vectors.
`can_recognize(rule)` is a two-flag lookup.

All walks in `check` and `lint` use explicit worklists rather than recursion — grammars are
small but nesting is unbounded, and the recursion budget is better spent in `recognize`
(risk R1).

### 3.5 `lint.rs`

A pure function of `CheckedGrammar`; never fails; never consulted by anything else (D5).

| Warning | Needs start rules | Source |
|---|---|---|
| `UnreferencedRule` | no | no rule body mentions it |
| `UnreachableRule` | yes (`lint_from`) | not in the transitive closure of the given starts |
| `UnproductiveRule` | no | rule `min_len == Infinite` |
| `UnproductiveAlternative` | no | an `Alt` branch with `min_len == Infinite` inside a rule whose own `min_len` is finite |
| `ShadowsCoreRule` | no | user rule name matches a core rule name (ASCII-insensitive) |

`UnproductiveAlternative` deliberately does not fire for every branch of a wholly unproductive
rule — that is already `UnproductiveRule`, and duplicating it is noise.

### 3.6 `recognize.rs`

```rust
pub struct Recognizer<'g, 'i> {
    grammar: &'g CheckedGrammar,
    src: &'i str,          // kept for spans and diagnostics
    input: Vec<char>,      // positional access; `&str` is not `&[char]`
    memo: HashMap<(u32, u32), Memo>,   // (rule index in the unified space, pos)
    steps: u64,
    opts: MatchOptions,
}
enum Memo { InProgress, Done(BTreeSet<usize>) }
```

`match_elem(&mut self, elem, pos) -> Result<BTreeSet<usize>, MatchError>` exactly per §6.2,
with repetition per §6.3 verbatim (phase-1 equality early exit — D3/D19, and the single most
bug-prone twenty lines in the crate; transcribe the pseudocode literally and keep a pointer to
the SCOPE section in a comment).

- `steps` increments once per repetition iteration and once per rule-body evaluation (memo
  miss). That is what makes M2's `steps() <= c * (input_len + 1)` assertion meaningful (D31).
- The `InProgress` guard returns `LeftRecursionDetected` — belt-and-braces, unreachable after
  `check`. Assert in a `#[cfg(test)]` counter that it never fires across the whole fixture set.
- Returning owned `BTreeSet` clones on memo hits is the obvious allocation cost. Accept it for
  v1 (performance is a non-goal); if it ever matters, return `&BTreeSet` with an interner. Do
  not optimize before M4 is green.
- `accepts(rule, input)` checks the compatibility flags *first* — §6.5 requires the error
  before any input is examined.

### 3.7 `generate.rs`

```rust
pub struct Generator<'g> {
    grammar: &'g CheckedGrammar,
    rng: SplitMix64,
    covered: BitSet,              // dense, indexed by coverage-unit index
    unit_index: HashMap<(NodeId, u32), u32>,
    subtree_units: Vec<BitSet>,   // per NodeId: units reachable from this node (transitive)
    opts: GenOptions,
}
```

Precomputation, once, at `Generator::new`: enumerate coverage units (D10/D25 — `Alt` branches
and both `Optional` arms, finite `min_len` only, excluding anything under a `max == 0`
repetition), then compute `subtree_units` by fixpoint over the rule graph. Rule references make
it transitive, so iterate to stability rather than a single post-order pass.

Walk: `gen_node(node, depth, out, newly_covered)`.

- Alternation / `Optional`: selection per §6.8 — (1) an uncovered branch, (2) a branch whose
  `subtree_units & !covered` is non-empty, (3) random. Never a branch with
  `min_len == Infinite` (D20), in any mode.
- Repetition: count uniform in `[min, min(max, min + spread)]`; in coverage mode, when the body
  can reach an uncovered unit and `max >= 1`, count is at least `max(min, 1)` (D25).
- Rule reference: recurse with `depth + 1`.
- **Witness mode** on budget exhaustion: alternations take their `witness` branch, repetitions
  take `min` (D28). Well founded by construction (§4.1).
- Limits: `max_output_len` (default `1 << 20`), `max_steps` (default `1 << 24`) → `GenError`
  (D29).

Two implementation details the spec does not state, which the M3 acceptance criteria depend on:

- **Coverage commits on success only.** Units taken during a walk go into a scratch set and
  merge into `covered` only when `generate()` returns `Ok`. Otherwise an `OutputLimit` failure
  marks units covered that never appeared in any output, and M3's exact bound breaks.
- **In coverage mode the depth budget must not preempt steering.** If `max_depth` trips witness
  mode while an uncovered unit is still reachable from the current node, the call can return
  having covered nothing, and the "at least one new unit per successful call" guarantee fails
  on any grammar deeper than the budget. Proposed rule: *in coverage mode, witness mode engages
  only when `subtree_units[node] & !covered` is empty* — depth is bounded everywhere except
  along the path actively chasing an uncovered unit, which is finite because that unit is
  reachable and productive. `max_steps` / `max_output_len` stay the hard backstops. Fold into
  D10 if accepted (tracked as R2).

### 3.8 `rng.rs`

SplitMix64, ~40 lines: state `+= 0x9E37_79B9_7F4A_7C15`; `z ^= z >> 30`;
`z *= 0xBF58_476D_1CE4_E5B9`; `z ^= z >> 27`; `z *= 0x94D0_49BB_1331_11EB`; `z ^= z >> 31`.
Plus `below(&mut self, n: u64) -> u64` using rejection sampling, not modulo — modulo bias would
be invisible and would quietly skew range and branch selection. Commit test vectors for seeds
0 and 1 so a refactor cannot silently change the stream (D15).

### 3.9 `error.rs`

Five plain enums with `Display` + `std::error::Error`, no `thiserror` (D13). `CheckError`
carries spans where it has them; `MatchError` / `GenError` carry the rule name and the
offending terminal or prose text so the CLI can print something actionable.
`#[non_exhaustive]` on all five — the §14 v2 candidates will add variants, and that should not
be a breaking change.

---

## 4. Algorithms worth writing down before coding

### 4.1 `min_len` and `witness` — Knuth's algorithm, not round-robin

§6.4 requires the witness to be well founded: "each was fixed at an earlier iteration than the
node that refers to it". A naive round-robin fixpoint does not give you that — a node's value
can be attained at iteration *k* by a branch whose own value later drops further, and recording
the witness when the value was first attained can produce a cycle (`a = b / "x"`,
`b = a / "y"` is exactly the case D28 calls out).

Use Knuth's generalization of Dijkstra (1977): a priority worklist where each node is
*finalized* exactly once, in non-decreasing order of its final `min_len`.

```
init:  all nodes Infinite; heap empty
       push every terminal node with its own length
       push every node needing no children (Repeat with min == 0 -> 0,
                                            Optional -> 0, empty Concat -> 0)
loop:  pop the smallest unfinalized node; finalize it; record its witness
       (Alt/Optional: the child that supplied this value; Repeat: count = min)
       for each parent / referring node, recompute a candidate:
           Alt/Optional -> min over finalized children
           Concat       -> saturating sum, once all children are finalized
           Repeat       -> min saturating_mul min_len(body), once body is finalized
           RuleRef      -> min_len(target), once target is finalized
       push improved candidates
end:   nodes never finalized are Infinite, i.e. unproductive
```

Every witness therefore points at an already-finalized node, so following witnesses strictly
descends the finalization order and terminates — exactly what the generator's witness mode
needs — and it holds even when values saturate at `u64::MAX` (D27) or tie.

Needs a reverse-dependency map (child node → parent node; rule → referring `RuleRef` nodes),
built in one pass. `nullable` does *not* need this treatment: boolean, monotone, no witness, so
plain round-robin is fine and simpler.

### 4.2 Repetition (§6.3) — transcribe, don't paraphrase

Revision 1 of the spec got this wrong; the corrected pseudocode is in SCOPE.md §6.3. Copy it
into the source as a comment and implement line by line. Two traps:

- Phase 1's early exit is `next == cur` (exact equality), **not** `next ⊆ cur`.
- Phase 2's frontier is `f(frontier) − result`, not `f(result)`.

Worked check for `1000000000*["a"]` on empty input: phase 1, k = 1 gives `{0}` (via the empty
arm), which equals `cur`, so it breaks after one step — one step, not a billion. For
`1000000000*"a"` on `"aaa"`: `{1} {2} {3} {}` → empty at k = 4, returns `{}`. Both satisfy
`steps() <= c * (input_len + 1)`.

### 4.3 Brute-force enumerator for the property test (M2)

The §6.3 property test is only worth something if the reference is *independently* derived — a
second set-of-positions implementation would share any conceptual bug. Use language enumeration
instead.

For a tiny grammar over alphabet `{a, b}` and a bound `L = 6`, compute for every rule the set of
strings of length ≤ `L` it derives, by fixpoint: start every rule at `∅` and recompute (`Alt` =
union, `Concat` = pairwise concatenation truncated at `L`, `Repeat` = bounded iteration,
terminals = singletons) until stable. Then `accepts(r, s) ⟺ s ∈ lang(r)`. Obviously correct by
inspection, about 60 lines, and shares nothing with the recognizer.

Random tiny grammars (proptest): ≤ 4 rules, nesting ≤ 3, alphabet `{a, b}` plus `""`, repeat
bounds ≤ 3, no prose. Filter to grammars that pass `check()` — which discards the
left-recursive ones the enumerator would loop on anyway.

### 4.4 Bonus self-test (M3, beyond SCOPE's acceptance criteria)

D23 requires the canonical self-grammar to *recognize* every fixture's text. The converse —
everything the self-grammar generates, our parser accepts — is nearly free once M3 lands:
generate from `rulelist` with the coverage generator and feed each output to `Grammar::parse`.
Assert `Ok`, tolerating a documented short list (`NumberTooLarge` from a generated 30-digit
`%x`; `check()` failures generally, since generated grammars reference undefined rules). Worth
doing: it is the only test that probes the parser from the language side rather than the
fixture side, and it directly guards the D35 invariant.

---

## 5. Milestones as PRs

Each PR is independently reviewable and leaves CI green. Do not start the next before the
current one's tests pass (SCOPE §8).

### M0 — Skeleton

| PR | Contents | Done when |
|---|---|---|
| **0.1** | `Cargo.toml` (edition 2024, `cli` feature opt-in, `proptest` dev-dep), `src/lib.rs` with crate lints, empty modules, `src/bin/abnf-oracle.rs` stub, `.gitattributes`, `LICENSE` (MIT/Apache-2.0 dual), `README.md` stub, `tests/` + `tests/grammars/` + `tests/corpus/` layout | `cargo build` and `cargo build --features cli` |
| **0.2** | `.github/workflows/ci.yml`: `fmt --check`, `clippy -- -D warnings`, `test`, each × {default, `--features cli`}; `rust-toolchain.toml` pinning stable | CI green on the empty crate |
| **0.3** | `ast.rs` complete per §3.1; `error.rs` enum skeletons; `rng.rs` + committed test vectors | `cargo test` passes |

`.gitattributes` matters more than it looks on a Windows checkout: `*.abnf text eol=lf`,
`tests/corpus/** -text` (corpus files are raw bytes, §9), `*.rs text eol=lf`.

### M1 — Grammar parser and check

| PR | Contents | Done when |
|---|---|---|
| **1.1** | `parse.rs`: cursor, line-ending normalization, `strict_crlf`, all productions (§3.2 steps 1–5) | unit tests for every §4 construct parse to the expected AST |
| **1.2** | Assembly, normalization, node ids (§3.2 steps 6–8); `Display`; `PartialEq` | `parse(g.to_string()) == g` on hand-written grammars; `*1a` ≡ `[a]`; two spellings of one canonical form get identical node ids |
| **1.3** | `core_rules.rs` + fixture transcription: core rules, 3 self-definition variants, RFC 8259, 3986, 5322 §3, 3339, 9110 subset; header comment per file naming RFC/section/errata; `tests/parse_grammars.rs` | every fixture parses; the ASCII-cleanliness test passes (D35) |
| **1.4** | `check.rs` steps 1–4 (resolution, ranges, representability) + `CheckedGrammar` | every `tests/grammars/invalid/` fixture fails with the exact expected variant |
| **1.5** | `check.rs` steps 5–6: `nullable`, `min_len`, `witness` (§4.1) | unit tests incl. the saturating case (three nested `4294967295` repeats → `Finite(u64::MAX)`), the `a = b / "x"` tie, prose non-nullable |
| **1.6** | `check.rs` steps 7–8: first-graph, left recursion, per-rule reachability | direct and indirect left-recursion fixtures fail; the prose fixture does not |
| **1.7** | `lint.rs` + `lint_from` | expected warnings on hand-written cases; the RFC 9110 fixture yields `ShadowsCoreRule` and no errors |
| **1.8** | CLI `check` and `rules` subcommands | manual smoke run over each fixture |

Invalid fixtures needed (`tests/grammars/invalid/`), one file each: undefined rule, duplicate
definition, `=/` without base, `=/` on a core rule name without base (D34), direct left
recursion, indirect left recursion, `5*2"a"`, `%x5A-41`, and a 25-digit repeat count — the last
of which fails at *parse*, not check.

### M2 — Recognizer

| PR | Contents | Done when |
|---|---|---|
| **2.1** | `recognize.rs` core: terminals, concat, alternation, rule refs, memo, in-progress guard, `steps`, `MatchOptions` | hand-written unit tests per §4 construct, incl. `=/`, `%s` vs `%i`, nested optionals, core-rule shadowing (D33) |
| **2.2** | Repetition per §6.3 + `tests/repetition.rs` full table + step-count assertions | all 12 rows pass; large-bound rows assert `steps() <= c*(len+1)` |
| **2.3** | Corpus harness, JSONTestSuite import, `NOTES.md` | every `y_*` accepted, every `n_*` rejected, `i_*` recorded |
| **2.4** | `tests/self_definition.rs` (D23) | the canonical self-grammar accepts every fixture's CRLF-normalized text |
| **2.5** | Brute-force enumerator + proptest (§4.3); memo-off equivalence test | property tests green at the default case count |
| **2.6** | Compatibility limits end to end; CLI `match` with §11 exit codes, incl. invalid UTF-8 → exit 2 (D14) | the prose fixture errors from a reaching start rule and returns `Ok` from a non-reaching one |

JSONTestSuite is MIT-licensed: vendor with its `LICENSE` and a `PROVENANCE.md` naming the
imported commit. Vendor `test_parsing/` only, not the whole repository.

### M3 — Generator

| PR | Contents | Done when |
|---|---|---|
| **3.1** | `generate.rs`: walk, terminals, ranges, case variation, `preserve_case`, depth budget, witness mode, limits | `generate_roundtrip.rs`: every fixture × 200 seeds accepted by the recognizer, zero failures |
| **3.2** | Coverage mode: unit enumeration, `subtree_units`, selection rule, coverage-aware repetition counts, commit-on-success | the exact bound holds on RFC 8259 and on a hand-written grammar with an unproductive alternative |
| **3.3** | Determinism and resource-bound tests; `uncovered()`; CLI `gen` | 100-call identical sequences; `1000000000*"a"` → `OutputLimit`; the `max_depth = 0` witness case |
| **3.4** | Bonus self-test (§4.4) | generated `rulelist` output re-parses |

### M4 — Differential (non-blocking during implementation, required before 1.0 — D24)

| PR | Contents |
|---|---|
| **4.1** | `scripts/differential.sh`: `abnfgen -c` per fixture → `abnf-oracle match`, assert accept |
| **4.2** | go-abnf / Python `abnf` verdict comparison where installed; pre-document the expected octet-range disagreements (§3) |
| **4.3** | README (20-line example plus an explicit "what this is not"), docs pass, crates.io metadata, release |

Every disagreement found here becomes a corpus file *before* it becomes a fix (§9).

---

## 6. Risk register

| # | Risk | Mitigation |
|---|---|---|
| R1 | **Stack overflow in `recognize` on deeply nested input.** JSONTestSuite ships `n_structures_100000_opening_arrays.json` and friends; recursion depth there is ~input length. | Run corpus tests on a thread with an explicit 64 MB stack (`std::thread::Builder::stack_size`). If it still blows, classify those specific cases in `NOTES.md` as depth-limited and document the limit. Decide in PR 2.3, not later. |
| R2 | Coverage guarantee defeated by the depth budget (§3.7). | Proposed rule: witness mode engages only when no uncovered unit is reachable. Needs a decision; test with a deliberately deep chain grammar. |
| R3 | Witness cycles from a round-robin `min_len` fixpoint. | Knuth worklist (§4.1), plus a debug assertion that following witnesses from any productive node terminates within `node_count` steps. |
| R4 | Fixture transcription errors across seven RFC grammars, by hand. | Header comment naming RFC + section + errata; a test asserting each fixture checks clean; cross-check against `abnfgen` / go-abnf in M4. Transcribe from RFC text, never from memory or third-party copies. |
| R5 | Git line-ending mangling on Windows silently changing corpus bytes. | `.gitattributes` in PR 0.1, plus a test reading one known corpus file and asserting its exact byte length. |
| R6 | Errata 2968 / 3076 wording taken from memory rather than the errata page. | PR 1.3 task: fetch both errata and paste the corrected productions into the fixture header. |
| R7 | Line-budget overrun (~3,500 lines, §15). | Budget in §7; check at each milestone with `tokei`. An overrun means something from §3 or §14 crept in — cut it, do not raise the budget. |
| R8 | `abnf-oracle` unavailable on crates.io. | Check in PR 0.1, before the name is baked into docs and fixtures. |
| R9 | Memo clone cost making the JSON corpus slow enough to annoy. | Accept until M4; run corpus tests in `--release`; only then consider borrowed sets. |

---

## 7. Line budget (§15: under ~3,500 lines of library code)

| Module | Budget |
|---|---|
| `ast.rs` | 250 |
| `parse.rs` | 600 |
| `core_rules.rs` | 120 |
| `check.rs` | 700 |
| `lint.rs` | 200 |
| `recognize.rs` | 350 |
| `generate.rs` | 550 |
| `rng.rs` | 60 |
| `error.rs` | 300 |
| `lib.rs` | 100 |
| **Total** | **3,230** |

CLI (~300 lines) and tests are excluded from the budget per §15.

---

## 8. Suggested order of work

1. Answer D32–D37 (§2). D33 is the one that changes observable semantics; the rest are
   contained.
2. M0 in one sitting — it is mechanical and unblocks everything.
3. M1.1–1.3 (parse + fixtures) before M1.4–1.7 (check). Having real fixtures in the tree makes
   every subsequent analysis testable against grammars people actually wrote.
4. M1.5 (`min_len` / `witness`) is the most subtle PR in the crate. Write §4.1's algorithm with
   its own unit tests before wiring it to anything.
5. M2.2 (repetition) is the second most subtle. It has a mandatory test table — write the table
   first, then the code.
6. M3 last, and only once the recognizer is trusted: every generator acceptance criterion is
   stated in terms of the recognizer.
