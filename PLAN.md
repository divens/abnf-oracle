# Implementation plan: `abnf-oracle`

Companion to `SCOPE.md` (revision 5.2). SCOPE.md is normative on *what* is built and why;
this document is *how*, *in what order*, and *what still needs attention*.

Status at time of writing: empty repository, no commits, `SCOPE.md` only.
Toolchain present: rustc/cargo 1.97.1 (edition 2024 available, MSRV target = 1.97).

Revision 5 settled D32–D37; 5.1 settled D38; 5.2 corrected the three M3 acceptance criteria this
plan flagged. Spec and plan now agree on every normative point: §2 records what 5.1 and 5.2
changed for the code, and §2.1 is a single follow-up on one of the rewritten criteria.

---

## 1. Invariants to hold throughout

These are the things that, if broken quietly, make the crate untrustworthy as an oracle.
Every PR is reviewed against them.

1. **No unchecked path.** `Recognizer` and `Generator` are constructible only from
   `CheckedGrammar` (D1). `Grammar::check` consumes.
2. **Two layers, no leakage** (D12/D32). `parse` produces syntax: ordered definitions as
   written, local rewrites only, no merging, no name resolution, no node ids. `check` produces
   semantics: the merged rule table, resolved hygienically against the core environment, with
   node ids. A `Grammar` never carries hidden validity state.
3. **Unordered alternation.** Nothing anywhere returns "the first branch that matched".
   Every element evaluates to a *set* of end positions (§6.2). Any code shaped like
   `if let Some(end) = try_branch(..) { return end }` is a bug.
4. **Memoization is an optimization only.** Results with the memo table disabled must be
   identical. Keep a `#[cfg(test)]` switch that bypasses the memo and run the §6.3 regression
   table and the property test both ways.
5. **Analyses are per AST node, not per rule**, for `nullable`, `min_len`, `witness` (D9, D27).
   `reaches_prose` / `reaches_unrepresentable` are per rule (§6.5, §6.6).
6. **All coverage reasoning happens over the generatable graph**, never the raw AST (D10/D20):
   no branch with infinite `min_len`, no `max == 0` body, and everything nested inside either is
   invisible to unit enumeration, distance and reachability alike.
7. **Three error categories stay separate** (§6.6): structural (`CheckError`, fails `check`),
   compatibility (per start rule, enforced by `Recognizer`/`Generator`), lint (advisory).
8. **The parser accepts exactly the canonical self-grammar's language** (D35), modulo
   line-ending normalization and the `u64` magnitude limit. M2 checks one direction, M3 the
   other. Any leniency added to the parser makes both tests vacuous.
9. **Library target has zero required dependencies** (D13). `cli` is opt-in.
   `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]` on public items.
10. **Limits are errors, never silent rejects** (D16, D29).

---

## 2. What revisions 5.1 and 5.2 changed for the implementation

**5.1 — D38.** It adopts the proposed fix — witness mode engages only when no uncovered unit is
reachable — and then goes further in a way that matters. The earlier proposal was *necessary but
not sufficient*: suspending the depth budget keeps the walk alive, but "prefer any branch that
reaches an uncovered unit, tie-break at random" can still wander forever between two branches
that both reach the target. Rev 5.1 replaces reaching with a **well-founded measure** —
`dist_to_uncovered`, ties by branch index — so the chase strictly descends and terminates. That
is the same shape of fix D28 applied to witnesses, and the plan was one step short of it.

Consequences for the code:

- **§3.7 is redesigned.** `subtree_units: Vec<BitSet>` is gone. The generator now carries a
  `dist_to_uncovered: Vec<u32>` recomputed by reverse BFS over the generatable graph, plus an
  explicit generatable-graph construction (§4.4). Net effect on the module is roughly neutral in
  size — a BFS replaces the bitset machinery — but it is a different algorithm, not a tweak.
- **Coverage units are now start-rule-relative.** Rev 5's definition was "finite `min_len`, not
  inside a `max == 0` body"; 5.1 adds "reachable from the start rule over the generatable
  graph", which excludes units nested inside an unproductive branch even when their own
  `min_len` is finite. Unit enumeration therefore needs a forward reachability pass per start
  rule, cached.
- **Selection rule 2 becomes `argmin dist`, ties by branch index** — deterministic, no RNG
  consulted while chasing.
- **Witness mode's trigger is `dist == ∞`**, not depth alone (D38).
- M3 gains three acceptance tests (depth independence, chase determinism, nested units not
  counted), which land in PR 3.2.

**5.2 — the three M3 corrections.** All landed as recommended: the chase grammar is now
`a = b / c`, `b = "z" a`, `c = "x" / "y"` (in §6.8's rationale as well as in M3), the
nested-units fixture is `start = "ok" / "alt" / bad` asserting `uncovered(start) == 2`, and
`Generator::steps()` is in §7. Two of the rewrites are sharper than what was proposed, and the
tests should honour the difference:

- The chase criterion now also asserts **the output contains no `z`** — a direct check that the
  chase never takes branch `b`, rather than an indirect one through step counts — and records
  that a naive tie-at-random implementation fails it with `OutputLimit` rather than hanging, so
  the failure is diagnosable from the test output alone.
- The nested-units fixture grew a third branch, so the assertion is `== 2` on a grammar where two
  units genuinely exist. That tests the exclusion rule rather than an off-by-one.

One consequence for PR 3.1: `Generator::steps()` counts *node visits*, like its `Recognizer`
twin, not visits to a particular rule. The chase criterion's "visits `a` at most
`dist_to_uncovered(a)` times" is therefore asserted as a bound on total node visits, which
upper-bounds the per-rule count. No separate per-node counter is needed.

Nothing else moved: §§1–6.7, 9–11 and D1–D38 are unchanged, and the plan's treatment of them
stands.

### 2.1 One follow-up on the new chase criterion

The rewritten chase test asserts that the covering call's "output contains no `z`" and that "the
result is identical across two seeds". Both are assertions about exact output text — and by §6.8
the generator "varies case randomly" on case-insensitive strings unless `preserve_case` is set.
Every terminal in that fixture (`"z"`, `"x"`, `"y"`) is a bare quoted string, hence
case-insensitive, so the covering call emits `y` or `Y` depending on the seed and the two-seed
assertion fails against a *correct* implementation.

*Recommended fix:* run that one test with `preserve_case = true`. It makes both assertions exact
without weakening either, and it keeps the test about the chase rather than about case variation.
Case-folding the comparison instead would rescue "contains no `z`" but not "identical across two
seeds".

The other two new criteria need nothing — neither asserts on output text.

---

## 3. Module design

Dependency order (§5): `error` ← `ast` ← {`parse`, `core_rules`} ← `check` ← {`lint`,
`recognize`, `generate`}; `rng` standalone; the CLI depends on everything, nothing depends on
it.

### 3.1 `ast.rs` — two layers (write it first, in M0)

**Syntactic layer** — what `parse` produces:

```rust
pub struct Grammar {
    defs: Vec<Definition>,   // exactly as written, in source order
    opts: ParseOptions,      // provenance; excluded from PartialEq (D21/D30)
}

pub struct Definition { name: RuleName, defined_as: DefinedAs, body: Element, span: Ignored<Span> }
pub enum DefinedAs { Base, Incremental }      // `=` and `=/`

pub enum Element {                             // recursive tree, no ids
    Alt(Vec<Element>),
    Concat(Vec<Element>),
    Repeat { min: u64, max: Option<u64>, body: Box<Element> },   // None = unbounded
    Optional(Box<Element>),                    // [x]
    RuleRef { name: RuleName, span: Ignored<Span> },
    CharVal(CharVal),
    NumVal(NumVal),
    ProseVal { text: String, span: Ignored<Span> },
}

pub struct CharVal { value: String, case_sensitive: bool }     // %s vs default/%i
pub enum NumVal { Scalar(u64), Range { lo: u64, hi: u64 }, Concat(Vec<u64>) }
```

**Semantic layer** — what `check` produces. Recommendation: lower the merged rule table into a
**flat arena**, so `NodeId` *is* the arena index:

```rust
pub struct NodeId(u32);

pub struct CheckedGrammar {
    rules: Vec<MergedRule>,          // user rules 0..n, then core rules n..n+16
    nodes: Vec<Node>,                // pre-order; NodeId indexes this
    resolved: Vec<RuleId>,           // per RuleRef node
    nullable: Vec<bool>,             // per node
    min_len: Vec<MinLen>,            // per node
    witness: Vec<Option<Witness>>,   // per node
    reaches_prose: Vec<bool>,        // per rule
    reaches_unrepr: Vec<bool>,       // per rule
}
pub enum RuleId { User(u32), Core(u32) }
```

Module split: the arena *types* (`NodeId`, `Node`, `Rule`, `RuleId`, `MinLen`, `Witness`) live in
`ast.rs`, since they are data model; `CheckedGrammar` itself lives in `check.rs`, which is what
SCOPE §5's module table says produces it. So M0 ships the types and M1.4 ships the struct that
holds the analyses over them.

Why an arena rather than putting an `id` field on `Element`: §6.7 defines node ids as
"pre-order traversal of the merged, normalized rule table", which is exactly arena order, so
the ids come out of the construction for free instead of needing a numbering pass and an
unassigned sentinel at the parse layer. Analyses become dense `Vec<T>` indexed by the same
number, and so does `dist_to_uncovered` (§4.4). The recognizer and generator walk by index
rather than chasing `Box`es. The cost is a second element representation, and with it the risk
of a second printer; §3.10 says how that is avoided.

**Naming.** Public types take their names from the productions of the ABNF self-grammar
(RFC 5234 §4) wherever one exists: `CharVal`, `NumVal`, `ProseVal`, `DefinedAs`, `RuleName`,
`Repeat`, `Element`. That keeps the type list readable to anyone who has read the RFC — which is
this crate's whole audience — and lines the data model up name-for-name with the
one-function-per-production parser (§3.2), so the D35 invariant can be audited by reading the two
side by side. Where no production exists, the name says what the thing does: `Ignored`, `MinLen`,
`Witness`, `NodeId`.

**Spans.** `Element::Repeat` and `Element::NumVal` carry one, because `InvalidRepeatRange` and
`InvalidNumericRange` are reported from `check` and would otherwise point at offset 0. No other
element needs one yet; add them where an error needs to be located, not on principle.

Three details that are easy to get wrong:

- **Spans must not participate in `PartialEq`.** M1 requires `*1a` and `[a]` to parse to *equal*
  grammars, and those have different spans. SCOPE.md does not say this because it is an
  implementation concern, but it is load-bearing for every round-trip test. Wrap spans in a
  newtype whose `PartialEq` is unconditionally `true` (`struct Ignored<T>(T);`) so the rest can be
  derived — otherwise every `PartialEq` is hand-written and one missed field silently breaks a
  round-trip test.
- **Rule names compare ASCII-case-insensitively.** Names are case-insensitive everywhere else
  (§4.1); making `PartialEq` agree costs one line and avoids a surprising inequality between
  `Foo = "a"` and `foo = "a"`.
- `Repeat.max: Option<u64>` rather than a sentinel — `None` is genuinely unbounded, and a
  sentinel would collide with the legal literal `*18446744073709551615`.

**One printer, not two.** Both layers need the §6.7 spelling table, and duplicating ~90 lines
of printer is a poor use of the line budget (§7). The `ElemView` borrowed-view enum sketched in
earlier revisions does not work: for `Element` the children are `&Element` values, for `Node`
they are `NodeId`s needing the arena, and no non-allocating enum spans both. See §3.10 for what
replaces it.

### 3.2 `parse.rs` — recursive descent, syntax only

Cursor over `&str` with a byte-offset position. After the ASCII gate the input is known ASCII,
so the scanner can work on `&[u8]` with exact byte spans throughout.

Pipeline:

1. **ASCII gate** (D35). `src.bytes().position(|b| b >= 0x80)` → `ParseError::NonAscii { span }`.
   Before tokenization, so every later function may assume ASCII.
2. **Line endings — handle in the scanner, do not rewrite the source.** §4.2 describes
   normalization "before parsing", but rewriting `CRLF`/`CR` → `LF` into a new `String` shifts
   every subsequent byte offset and forces an offset map to keep error spans honest. Making
   `c_nl` accept `CRLF | LF | CR` (and only `CRLF` under `strict_crlf`, else
   `ParseError::ExpectedCrlf { span }`) is observationally identical, ~20 lines cheaper, and
   keeps spans exact against the original text.
3. **`rulelist` per RFC 5234 §4 + Errata 2968 and 3076.** One function per production —
   `rulelist`, `rule`, `defined_as`, `elements`, `alternation`, `concatenation`, `repetition`,
   `repeat`, `element`, `group`, `option`, `char_val`, `num_val`, `prose_val`, `c_wsp`, `c_nl`,
   `comment` — so invariant 8 is auditable by reading the source against the fixture.
   *Done in M1.1, and the errata were fetched rather than recalled, which is how SCOPE rev 5.3's
   correction came about: 2968 fixes `elements`, 3076 fixes `rulelist`, and the canonical
   self-grammar needs both. Also established there: the self-grammar spells its radix and string
   markers as case-insensitive char-vals, so `%X41` and `%S"a"` are part of the language.*
4. **Line continuation** falls out of `c-wsp = WSP / (c-nl WSP)`; do not pre-join lines.
5. **Numbers** accumulate with `checked_mul`/`checked_add`; overflow →
   `ParseError::NumberTooLarge { span }` (D17). Applies to `%d`/`%x`/`%b` values, range
   endpoints and repeat bounds alike.
6. **`%s` / `%i`** prefixes (RFC 7405) only immediately before `DQUOTE`.
7. **Local rewrites only** (D32, and the §6.7 spelling table): `Repeat{0,Some(1),b}` →
   `Optional(b)`; `Repeat{1,Some(1),b}` → `b`; `0*n` → `*n`; numeric spelling normalized to
   even-padded uppercase `%x`; redundant groups dropped, i.e. flatten a `Concat` directly
   inside a `Concat` and an `Alt` directly inside an `Alt`, and unwrap single-child `Alt` /
   `Concat`. Apply bottom-up to a fixpoint — the rewrites cascade through the flattening. An
   `Alt` inside a `Concat` is *not* redundant; `Display` re-inserts its parentheses.
   Nothing here needs knowledge of another rule, which is the test for whether a rewrite
   belongs at this layer.

`ParseError` is a struct-like enum with a `span` on every variant plus a `render(src)` helper
producing a caret-underlined line — grammar authors are humans, and §3 says this is the one
place error quality matters. `CheckError` gets the same, with `span()` returning `Option` since
`LeftRecursion` has no single location: a cycle belongs to several rules at once and pointing at
any one of them would be arbitrary. Landed in M1.9, where the CLI became the consumer; the
fixture harness uses it too, rather than keeping its own copy.

### 3.3 `core_rules.rs`

The Appendix B rules as a `&'static str` constant, parsed and checked once behind a
`OnceLock` — the crate eats its own dog food rather than hand-building an AST. A unit test
asserts the constant parses, checks, and defines exactly the expected 16 names
(`ALPHA BIT CHAR CR CRLF CTL DIGIT DQUOTE HEXDIG HTAB LF LWSP OCTET SP VCHAR WSP`).

Two things to get right:

- `LWSP` is defined via `CRLF` and is nullable — the one core rule that is nullable, and the
  one that will exercise nullable-repetition paths in real fixtures.
- `OCTET = %x00-FF` is representable under the scalar model (§6.1) but means "code points
  U+0000–U+00FF", per §3's documented limitation. Say so in a doc comment where someone
  debugging an octet-grammar mismatch will find it.

Hygiene (D33) falls out of the lowering order: core rules are lowered into the same arena at
indices `n..n+16`, and a `RuleRef` in a core body resolves against the core table only, while a
`RuleRef` in a user body resolves user-first, then core. One `resolve_in: Scope` parameter
threaded through the resolution pass — no other mechanism needed.

Bootstrapping note: `core_rules` cannot call `check()` on itself through the normal path
without recursion (the checker consults the core environment). Lower the core table first, in
its own pass with `Scope::Core`, then lower user rules against it.

### 3.4 `check.rs` — the biggest module

Errors accumulate — return all of them, never just the first; grammar authors want the full
list.

1. **Rule-table build.** Group definitions by ASCII-lowercased name. The first `Base` defines
   the rule; each subsequent `Incremental` appends its body as further alternatives, flattened
   into one top-level `Alt` so branch indices and canonical output agree with §6.7. A second
   `Base` → `DuplicateDefinition`. An `Incremental` with no prior `Base` →
   `IncrementalWithoutBase { name, shadows_core }`, with `shadows_core` set when the name is a
   core rule (D34). Rule order is first-definition order.
2. **Lowering.** Walk the merged table in pre-order, emitting arena nodes; `NodeId` is the
   emission index (D22/§6.7). Core rules are lowered after user rules.
3. **Resolution.** Per `RuleRef`, user-first then core for user bodies, core-only for core
   bodies (D33). Unknown → `UndefinedRule { name, span }`.
4. **Range validation.** `min > max` → `InvalidRepeatRange`; `lo > hi` → `InvalidNumericRange`
   (D18).
5. **Representability** (§6.1). Lives on `NumVal` as arithmetic — `is_representable`,
   `scalars_in`, `nth_scalar` — not as a per-node table: the scalar count of `lo..=hi` is
   already O(1) to compute, so there is nothing to precompute and nothing to keep in sync. The
   per-rule `reaches_unrepresentable` flag (step 9) walks the arena calling `is_representable`.
   `nth_scalar` is what lets the generator pick uniformly, and it steps over the surrogate
   block rather than pretending it is not there.
6. **`nullable`** — round-robin least fixpoint over nodes, init `false`. Prose is non-nullable
   (D26).
7. **`min_len` + `witness`** — §4.1 below. Knuth-style worklist, *not* round-robin, because the
   witness must be well founded by construction (D28).
8. **First-graph + left recursion.** Edges rule → rule; `max == 0` bodies contribute none
   (D36); prose creates none (D26). Cycle detection by iterative DFS with colour marking,
   reporting the cycle path in definition order → `LeftRecursion { cycle: Vec<String> }`.
   Global, not per start rule (§6.4) — including for unreferenced rules.
9. **Per-rule reachability**: `reaches_prose`, `reaches_unrepresentable`. Transitive closure
   over the rule-reference graph, excluding `max == 0` bodies (D36), seeded from rules whose
   own body contains one directly.

`can_recognize(rule)` is then a two-flag lookup.

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

Three decisions the table does not capture, each made in M1.8:

- **Core rules are never linted.** They are the resolution environment, not the user's grammar;
  sixteen unreferenced-rule warnings on every grammar would make the output worthless.
- **A self-reference does not count as a reference.** `start = "x" [start]` is still unused by
  the rest of the grammar, which is what the warning is for. The consequence is that the entry
  rule of *any* grammar is reported unreferenced — correct, and exactly why `lint_from` exists.
- **`lint_from` ignores names matching no rule.** With no error channel in the signature, the
  alternative is to report every rule as unreachable because of a typo.

`UnproductiveAlternative` deliberately does not fire for every branch of a wholly unproductive
rule — that is already `UnproductiveRule`, and duplicating it is noise.

### 3.6 `recognize.rs`

```rust
pub struct Recognizer<'g, 'i> {
    grammar: &'g CheckedGrammar,
    src: &'i str,          // kept for spans and diagnostics
    input: Vec<char>,      // positional access; `&str` is not `&[char]`
    memo: HashMap<(u32, u32), Memo>,   // (rule index in the merged table, pos)
    steps: u64,
    opts: MatchOptions,
}
enum Memo { InProgress, Done(BTreeSet<usize>) }
```

`match_node(&mut self, node: NodeId, pos: usize) -> Result<BTreeSet<usize>, MatchError>`
exactly per §6.2, with repetition per §6.3 verbatim (D3/D19, and the single most bug-prone
twenty lines in the crate; transcribe the pseudocode literally and keep a pointer to the SCOPE
section in a comment).

- `steps` increments once per repetition iteration and once per rule-body evaluation (memo
  miss). That is what makes M2's `steps() <= c * (input_len + 1)` assertion meaningful (D31).
- The `InProgress` guard returns `LeftRecursionDetected` — belt-and-braces, unreachable after
  `check`. Keep a `#[cfg(test)]` counter asserting it never fires across the fixture set.
- Returning owned `BTreeSet` clones on memo hits is the obvious allocation cost. Accept it for
  v1 (performance is a non-goal); if it ever matters, return `&BTreeSet` with an interner. Do
  not optimize before M4 is green.
- `accepts(rule, input)` checks the compatibility flags *first* — §6.5 requires the error
  before any input is examined.

### 3.7 `generate.rs` — redesigned for D38

```rust
pub struct Generator<'g> {
    grammar: &'g CheckedGrammar,
    rng: SplitMix64,
    units: Vec<Unit>,                    // (alt node, branch index), candidates
    unit_of: HashMap<(NodeId, u32), u32>,
    covered: BitSet,                     // indexed by unit index; global across start rules
    reachable: HashMap<RuleId, Vec<u32>>,// units reachable from this start rule, cached
    dist: Vec<u32>,                      // dist_to_uncovered per NodeId; u32::MAX = ∞
    dist_dirty: bool,
    steps: u64,                          // node visits; exposed via steps() per §7
    opts: GenOptions,
}
```

Precomputation at `Generator::new`: enumerate *candidate* units (`Alt` branches and both
`Optional` arms with finite `min_len`), and build the generatable-graph adjacency (§4.4). Per
start rule, on first use, compute the reachable unit set by forward traversal and cache it —
that is what makes units start-rule-relative per D10 and what `uncovered(rule)` counts.

Walk: `gen_node(node, depth, out)`.

- Alternation / `Optional`, coverage mode, in order (§6.8):
  1. a branch that is itself an uncovered unit;
  2. else `argmin dist[branch]`, ties by lowest branch index — the chase. No RNG is consulted
     here, which is what makes the chase deterministic and the M3 assertion meaningful;
  3. else (every branch `∞`) random, and witness mode may engage if depth is exhausted.
- Never a branch with `min_len == Infinite` (D20), in any mode — enforced by construction,
  since such branches are not edges of the generatable graph.
- Repetition: count uniform in `[min, min(max, min + spread)]`; in coverage mode, when the body
  can reach an uncovered unit (`dist[body] < ∞`) and `max >= 1`, count is at least
  `max(min, 1)` (D25).
- **Witness mode** engages when the depth budget is exhausted *and* `dist[node] == ∞` (D38).
  In random mode the second condition is vacuous, so behaviour there is unchanged.
- Limits: `max_output_len` (default `1 << 20`), `max_steps` (default `1 << 24`) → `GenError`
  (D29). These are the only backstops during a chase.

Three implementation details SCOPE does not state, which the M3 criteria depend on:

- **Coverage commits on success only, but rule 1 reads a live view.** Units taken during a walk
  go into a scratch set; rule 1 tests `covered ∪ scratch` so one call does not keep re-covering
  the same branch, and the scratch set merges into `covered` only when `generate()` returns
  `Ok`. Without the commit gate, an `OutputLimit` failure would mark units covered that never
  appeared in any output and M3's exact bound would break.
- **Distance staleness is a performance concern, not a correctness one.** Covering a unit
  mid-walk can only *raise* distances, so a stale `dist` still decreases strictly along the
  chase — it may merely chase a unit that was just covered. Set `dist_dirty` when a unit is
  covered and recompute lazily on next use; the BFS is O(nodes) over a grammar-sized graph, so
  even recomputing on every newly covered unit is cheap. This matches §6.8's "lazy recompute at
  each call boundary plus incremental updates".
- **`uncovered(rule)` must not mutate.** It takes `&self` (§7) but needs the reachable-unit
  cache; either populate the cache eagerly at `new` for every rule, or hold it in a `RefCell`.
  Eager is simpler and grammar-sized — prefer it, and note that it makes `new` O(rules × nodes).

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

`ParseError` needs `NonAscii`, `ExpectedCrlf`, `NumberTooLarge`, plus the ordinary syntactic
ones. `CheckError` needs `DuplicateDefinition`, `IncrementalWithoutBase { name, shadows_core }`,
`UndefinedRule`, `InvalidRepeatRange`, `InvalidNumericRange`, `LeftRecursion`.

### 3.9c The self-grammar as an independent check

M2.2 found that SCOPE's `3*3 ["a"]` is not valid ABNF, reasoning from `repetition = [repeat]
element`. That was one reading of one production. M2.5 settles it without appeal to my reading:
the canonical self-grammar, extracted mechanically from RFC 5234, **rejects** that text. When a
question is "what does ABNF permit", running the RFC's own grammar beats arguing from it — and
after M3's self-generation lands, the same fixture answers in both directions.

### 3.9b D36 is load-bearing, not a corner case

RFC 3986 defines `path-empty = 0<pchar>` — zero repetitions of a *prose value*, meaning the
empty path. Because a body that can never match is unreachable (D36), `path-empty` reaches no
prose. Without that rule it would, and so would `hier-part`, `URI` and every rule routed through
them, leaving the URI fixture almost entirely unusable as a start-rule set. Pinned by a test
asserting every rule in that fixture is a usable start rule.

### 3.9a Fixtures are derived, not typed

`scripts/extract-fixtures.py` builds every `tests/grammars/*.abnf` from the RFC plain text.
Three things in that format defeat naive extraction, each found by a failing test rather than
by inspection:

- **A page break can fall mid-rule.** RFC 5322 splits `obs-zone` across one. Removing only the
  footer, form feed and running header leaves a gap that reads as the end of a block, silently
  truncating the rule — so the blank lines around the boundary go too.
- **Indentation is not consistent within a document.** RFC 8259 prints its grammar at one
  indent in §2 and another later, so each block is dedented by its own base. Getting this wrong
  does not raise an error: a rule left at the wrong indent parses as a *continuation* of the
  rule above it, quietly merging two rules into one.
- **Prose can look like a continuation.** RFC 3339's NOTE paragraphs are indented deeper than
  the grammar they follow, so a blank line has to end a block unless a sibling rule follows.

### 3.10 `display.rs` — canonical form for both layers

Not in SCOPE §5's file list; it is here because `ast.rs` is already the widest file in the crate
and canonical spelling is its own concern, extended rather than rewritten when `CheckedGrammar`
gains a `Display` in M1.4.

The printer is written once, against the syntactic `Element`, with a three-valued context
(`Free` / `Concat` / `Repeat`) deciding parentheses. For M1.4, rather than abstracting over both
representations, give `CheckedGrammar` a small `fn element_at(&self, NodeId) -> Element` that
rebuilds a subtree from the arena and hand it to the same printer. It allocates, which does not
matter for `Display`, and it is ~15 lines against ~90 duplicated ones.

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

**Order ties by derivation depth, not by node index** (found in M1.6). Well-foundedness holds
either way, but the *choice* of witness does not: in `a = b / "x"`, `b = a / "y"` both
alternations settle at `min_len` 1, and breaking the tie by arena position finalizes `b` through
the reference to `a` before `"y"` is ever popped — so `b`'s witness names `a`, and SCOPE's
"witnesses point at the terminals" is false while the guarantee still holds. Keying the worklist
on `(min_len, depth)` and picking the shallowest branch that achieves the value fixes it, makes
the choice deterministic, and bounds witness-mode work by the shortest *derivation* rather than
merely the shortest string.

Needs a reverse-dependency map (child node → parent node; rule → referring `RuleRef` nodes),
built in one pass over the arena. `nullable` does *not* need this treatment: boolean, monotone,
no witness, so plain round-robin is fine and simpler.

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

**It works, and that was measured rather than assumed.** 72% of generated grammars survive
`check` (the rest are left-recursive), averaging 2.4 rules, so the properties run on real input
rather than mostly skipping. More to the point: reintroducing revision 1's bug — phase 1 exiting
on `next ⊆ cur` instead of `next == cur` — **fails the property test and passes all twelve
mandatory rows**. The counterexample it shrank to,

```abnf
r0 = 3*3(("a" r1))
r1 = 0*1(%x61)
```

accepts `"aa"` under the bug: the reachable set goes `{0} → {1,2} → {2}`, *shrinking* into a
subset, so a subset test stops after two of the three required repetitions. No mandatory row has
a shrinking position set. That case is now a permanent row in `tests/repetition.rs`, which is
the right end state — the property test finds them, the cheap table keeps them.

### 4.4 The generatable graph and `dist_to_uncovered` (new in 5.1)

Everything coverage-related runs on this graph, so build it once, explicitly, rather than
re-deriving the exclusions at each use site.

```
nodes:  every arena node (user rules and core rules)
edges:  Alt a       -> branch b        for each branch with finite min_len   (D20)
        Optional o  -> body            (the ε arm is a unit, not an edge)
        Concat c    -> each item
        Repeat r    -> body            unless max == 0                       (D25/D36)
        RuleRef n   -> root node of the resolved rule body
        terminals   -> none
```

Two derived quantities:

- **Reachable units, per start rule.** Forward traversal from the start rule's body root; a
  candidate unit `(a, i)` counts iff `a` is reached. This is what excludes `("p" / "q")` inside
  an unproductive `bad` — `bad` is not an edge of the graph, so nothing under it is reached.
  `uncovered(rule)` = reachable units minus covered.
- **`dist_to_uncovered`.** Multi-source reverse BFS. Seeds are the `Alt`/`Optional` nodes having
  at least one uncovered unit branch, at distance 0; every other node gets `1 + min` over its
  outgoing edges; unreached nodes are `∞`. One pass, O(nodes + edges).

The chase then picks `argmin dist[branch]` with ties by index, and `dist` strictly decreases
along it — the branch chosen from a node at distance *d* has distance *d − 1* or less — so it
reaches a seed within *d* steps and rule 1 fires there. That is the well-foundedness argument
D38 relies on; it is worth an assertion in debug builds (`dist[chosen] < dist[current]`), since
a bug here degrades into a hang bounded only by `max_steps`.

### 4.5 Self-generation (M3, mandatory per D35)

500 strings generated from the canonical self-grammar's `rulelist` in coverage mode, each
required to parse. Three things the test must get right:

- **Assert `parse`, not `check`.** Generated grammars reference rule names that were never
  defined, so `check()` will fail on nearly all of them. That is expected and is not what this
  test is about.
- **Pin `spread` to a small value.** The self-grammar spells numbers as `1*DIGIT` / `1*HEXDIG`,
  so a large `spread` produces literals wider than `u64` and the test starts failing with
  `NumberTooLarge` — a legitimate parse error (D17) that has nothing to do with the invariant.
  With the default `spread = 3` the generated literals are at most 4 digits. Either pin it or
  treat `NumberTooLarge` as a pass; pinning is clearer.
- Generated text is ASCII-clean by construction (every terminal in the self-grammar is an ASCII
  range), so the D35 gate should never fire here. If it ever does, the bug is in the generator
  or the fixture, and the test should say so rather than tolerating it.

---

## 5. Milestones as PRs

Each PR is independently reviewable and leaves CI green. Do not start the next before the
current one's tests pass (SCOPE §8).

### M0 — Skeleton

| PR | Contents | Done when |
|---|---|---|
| **0.1** | `Cargo.toml` (edition 2024, `cli` feature opt-in, `proptest` dev-dep), `src/lib.rs` with crate lints, empty modules, `src/bin/abnf-oracle.rs` stub, `.gitattributes`, `LICENSE` (MIT/Apache-2.0 dual), `README.md` stub, `tests/` + `tests/grammars/` + `tests/corpus/` layout | `cargo build` and `cargo build --features cli` |
| **0.2** | `.github/workflows/ci.yml`: `fmt --check`, `clippy -- -D warnings`, `test`, each × {default, `--features cli`}, plus `cargo doc` under `RUSTDOCFLAGS=-D warnings` — the only check that catches a broken intra-doc link; `rust-toolchain.toml` pinning stable | CI green on the empty crate |
| **0.3** | `ast.rs` both layers per §3.1 (incl. the span newtype); `error.rs` enum skeletons; `rng.rs` + committed test vectors | `cargo test` passes |

`.gitattributes` matters more than it looks on a Windows checkout: `*.abnf text eol=lf`,
`tests/corpus/** -text` (corpus files are raw bytes, §9), `*.rs text eol=lf`.

### M1 — Parser and check

Nine PRs; this is the bulk of the project.

| PR | Contents | Done when |
|---|---|---|
| **1.1** | `parse.rs`: ASCII gate, scanner-level line endings, `strict_crlf`, all productions (§3.2 steps 1–6) | unit tests for every §4 construct parse to the expected AST; `NonAscii` and `ExpectedCrlf` fire where expected |
| **1.2** | Local rewrites (§3.2 step 7); `Display` + `PartialEq` for the **syntactic** layer | `Grammar::parse(g.to_string()) == g`; `*1a` ≡ `[a]`; canonical spelling unit-tested against the §6.7 table row by row |
| **1.3** | `core_rules.rs` + `scripts/extract-fixtures.py` deriving all nine fixtures from RFC text; `tests/parse_grammars.rs`; the ASCII-cleanliness scan | every fixture parses and round-trips, under both line-ending settings; the scan passes |
| **1.4** | `check.rs` steps 1–3: rule-table build, lowering + node ids, hygienic resolution; `Display` + `PartialEq` for `CheckedGrammar`; the four check-error fixtures under `invalid/` | `Grammar::parse(cg.to_string()).check() == cg` on every fixture; duplicate / orphan `=/` / `shadows_core` fixtures fail with the right variant; two textually different grammars with one canonical form get identical node ids |
| **1.5** | `check.rs` steps 4–5: range validation, representability; spans on `Repeat` and `NumVal` so both errors can point at the offending text | `5*2"a"` and `%x5A-41` fixtures fail, with spans covering exactly `5*2` and `%x5A-41`; a surrogate-spanning range is representable, `%xD800-DFFF` is not; `%x80-FF` stays representable |
| **1.6** | `check.rs` steps 6–7: `nullable`, `min_len`, `witness` (§4.1), with `(min_len, depth)` ordering | unit tests incl. the saturating case (three nested `4294967295` repeats → `Finite(u64::MAX)`), the `a = b / "x"` tie resolving to the terminals, prose non-nullable; the well-foundedness assertion runs on every check in debug builds |
| **1.7** | `check.rs` steps 8–9: first-graph, left recursion, per-rule reachability; `can_recognize` | direct and indirect left-recursion fixtures fail; no RFC fixture is left-recursive; a `*0(…)` body contributes no edges and hides what it holds; RFC 9110 refuses 25 of 142 start rules for prose and keeps the rest |
| **1.8** | `lint.rs` + `lint_from` | expected warnings on hand-written cases; RFC 8259 yields exactly one `ShadowsCoreRule`, for `char`, and the core-rules fixture sixteen; no fixture has a dead rule or branch |
| **1.9** | CLI `check` and `rules`; `ParseError::render` / `CheckError::render` | `tests/cli.rs` drives the built binary: every valid fixture exits 0 from both subcommands with matching rule counts, every broken one exits 2 with a located error, warnings never change the exit code |

Invalid fixtures needed (`tests/grammars/invalid/`), one file each: undefined rule, duplicate
definition, `=/` without base, `=/` on a core-rule name without base (D34, `shadows_core: true`),
direct left recursion, indirect left recursion, `5*2"a"`, `%x5A-41`, a 25-digit repeat count,
and a non-ASCII byte in a comment. The last two fail at *parse*, not check — and the non-ASCII
one must live outside the directory scanned by the ASCII-cleanliness test, or that test will
fail on it. Put it in `tests/grammars/invalid/parse/` and scope the scan to exclude that
subdirectory.

### M2 — Recognizer

| PR | Contents | Done when |
|---|---|---|
| **2.1** | `recognize.rs` core: terminals, concat, alternation, rule refs, memo, in-progress guard, `steps`, `MatchOptions` | hand-written unit tests per §4 construct, incl. `=/`, `%s` vs `%i`, nested optionals |
| **2.2** | Repetition per §6.3 + `tests/repetition.rs` full table + step-count assertions | all 12 rows pass; large-bound rows assert `steps() <= c*(len+1)` |
| **2.3** | Hygiene test (D33) | `DIGIT = "x"` + a rule referencing `HEXDIG`: `HEXDIG` matches `7`, the user `DIGIT` matches `x` and not `7`, exactly one `ShadowsCoreRule`, no errors |
| **2.4** | Corpus harness, JSONTestSuite import, `NOTES.md`, `PROVENANCE.md` | all 95 `y_` accepted and all 174 decodable `n_` rejected — no disagreement with the suite at all; 12 `n_` moved to `indeterminate/` for invalid UTF-8 and 2 for depth, each listed in `NOTES.md` |
| **2.5** | `tests/self_definition.rs` (D23); the two `invalid/parse/` fixtures and an RFC 7405 fixture, both gaps left by M1 | the canonical self-grammar accepts every fixture, including itself and one using `%s`/`%i`; rejects ten negative controls the parser also rejects; and the one documented divergence — a 25-digit repeat, valid ABNF that `u64` refuses — is pinned from both sides |
| **2.6** | Brute-force enumerator + proptest (§4.3); memo-off equivalence test | property tests green at the default case count, with 72% of generated grammars surviving `check`; **validated by mutation** — reintroducing revision 1's subset bug fails the property test and passes all twelve mandatory rows |
| **2.7** | Compatibility limits end to end; CLI `match` with §11 exit codes, incl. invalid UTF-8 → exit 2 (D14) | the prose fixture errors from a reaching start rule and returns `Ok` from a non-reaching one |

JSONTestSuite is MIT-licensed: vendor `test_parsing/` only, with its `LICENSE` and a
`PROVENANCE.md` naming the imported commit.

### M3 — Generator

| PR | Contents | Done when |
|---|---|---|
| **3.1** | `generate.rs`: walk, terminals, ranges, case variation, `preserve_case`, depth budget, witness mode, limits, `steps()` (§7) | `generate_roundtrip.rs`: every fixture × 200 seeds accepted by the recognizer, zero failures |
| **3.2** | Generatable graph, unit enumeration, `dist_to_uncovered`, the chase, coverage-aware repetition counts, commit-on-success (§3.7, §4.4) | the exact bound holds on RFC 8259 and on a grammar with an unproductive alternative; depth-independence case (5-deep chain at `max_depth = 2`); chase determinism on `a = b / c`, `b = "z" a`, `c = "x" / "y"` with `preserve_case` (§2.1); nested units not counted (`uncovered(start) == 2`); `*("a" / "b")` covers both in ≤ 2 calls; a `*0(…)` body reports zero units |
| **3.3** | Self-generation test (§4.5, D35) | 500 generated `rulelist` strings all parse |
| **3.4** | Determinism and resource-bound tests; `uncovered()`; CLI `gen` | 100-call identical sequences; `1000000000*"a"` → `OutputLimit`; the `max_depth = 0` witness case; random mode terminates on 1000 seeds |

Every M3 fixture is now runnable as specified; the only adjustment PR 3.2 makes on its own
authority is setting `preserve_case` on the chase test (§2.1).

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
| R1 | **Stack overflow in `recognize` on deeply nested input.** **Fired in M2.4, and worse than expected**: the cost is ~10 KB of stack per level of input nesting, so the 2 MB a test thread gets by default overflows at depth **200**, not at the 100,000 the two giant fixtures reach. Measured: 8 MB reaches 500, 64 MB reaches ~5,000, 256 MB reaches 20,000. | Corpus tests run on a 64 MB stack, which covers everything in the suite except the two 100,000-deep files; those are classified depth-limited in `NOTES.md`. **Not fully mitigated**: with default options the *library* aborts the process rather than returning an error, and `max_steps` bounds it only if the caller sets one. See §6.1 below. |
| R2 | **Rule 2 implemented as "any branch that reaches an uncovered unit" rather than `argmin dist`.** It is the natural simplification, it passes the JSON coverage test, and it is wrong — D38's termination argument needs the strictly decreasing measure. | The chase test is the guard: rev 5.2 records that the naive version fails it with `OutputLimit`. Keep the debug assertion of R10 as the second line of defence, and do not relax the chase fixture to something shallower. |
| R3 | ~~Witness cycles from a round-robin `min_len` fixpoint.~~ **Closed in M1.6.** | Knuth worklist (§4.1) with `(min_len, depth)` ordering, plus a debug assertion — live on every `check`, so it runs over all nine fixtures and every test grammar — that following witnesses from any productive node terminates within `nodes.len()` steps. |
| R4 | ~~Fixture transcription errors across seven RFC grammars, by hand.~~ **Retired in M1.3: nothing was transcribed by hand.** | `scripts/extract-fixtures.py` derives all nine fixtures from the RFC texts and is verified to reproduce them byte for byte. Three independent checks passed: every content line appears verbatim in its source RFC, no fixture has an undefined rule reference, and the two derived fixtures differ from their base by exactly the errata substitutions and the RFC 7405 splice. The M4 cross-check against `abnfgen` / go-abnf still stands. |
| R5 | Git line-ending mangling on Windows silently changing corpus bytes. | `.gitattributes` in PR 0.1, plus a test reading one known corpus file and asserting its exact byte length. |
| R6 | ~~Errata 2968 / 3076 wording taken from memory rather than the errata page.~~ **Fired, and was caught in M1.1.** SCOPE rev 5.2 had the two errata's subjects transposed and treated 3076 as a numeric-value clarification, which would have left `rulelist` ambiguous in the canonical fixture. | Fixed in SCOPE rev 5.3 / D39. The mitigation stands for PR 1.3: paste the corrected productions into each fixture header, from the errata page, never from memory. |
| R7 | **Line-budget pressure** (~3,500 lines, §15). The two-layer split added a representation and a lowering pass; D38 added the generatable graph and the BFS; M1.2 added `display.rs`. | One printer for both layers (§3.10); check at each milestone with `tokei`. If it overruns, the honest first cut is terser `Display` impls in `error.rs`, not a required behaviour. |
| R8 | The ASCII-cleanliness scan and the deliberately non-ASCII invalid fixture contradict each other. | Scope the scan to exclude `tests/grammars/invalid/parse/` (PR 1.1 / 1.3). Small, but it will fail CI on the day the fixture lands if nobody planned for it. |
| R9 | Memo clone cost making the JSON corpus slow enough to annoy. | Accept until M4; run corpus tests in `--release`; only then consider borrowed sets. |
| R10 | A bug in the chase degrades into a walk bounded only by `max_steps` — slow and hard to diagnose. | Debug assertion `dist[chosen] < dist[current]` on every chase step (§4.4), so the invariant fails loudly in tests rather than quietly in the field. |

### 6.1 The depth limit R1 uncovered

**Resolved in M2.4 by option 2 below, now SCOPE D42.** Kept here for the measurements, which
are what set the default.

The recognizer recurses once per grammar node, and one level of input nesting costs several
nodes — six, for RFC 8259. Each level costs about 2 KB of stack in a debug build, measured by
bisecting the overflow point:

| stack | deepest depth that survives |
|---|---|
| 512 KB | 128 |
| 1 MB | 400 |
| 2 MiB — a spawned thread's default | 900 |
| 64 MB | ~30,000 |

The failure mode was the problem more than the ceiling: a stack overflow aborts the process, so
a caller cannot treat it as "could not decide" the way `StepLimit` is treated. The options were
to document it, to add a limit, or to make the walk iterative. Adding `MatchOptions::max_depth`
won: it is small, it turns a crash into the error shape the rest of the design already uses, and
it leaves the recursive implementation — the thing that makes `recognize.rs` readable against
§6.2 — alone.

The default is `256`, which sits under the 1 MB row so it holds wherever the recognizer is
called from, and allows JSON nested about 40 deep. It is the only limit in the crate with a
finite default, because it is the only one whose absence cannot be reported.

---

## 7. Line budget (§15: under ~3,500 lines of library code)

| Module | Budget | Δ vs rev 5 plan |
|---|---|---|
| `ast.rs` | 300 | — |
| `parse.rs` | 650 | +100 (canonicalization) |
| `display.rs` | 200 | new (§3.10) |
| `core_rules.rs` | 120 | — |
| `check.rs` | 850 | — |
| `lint.rs` | 200 | — |
| `recognize.rs` | 350 | — |
| `generate.rs` | 600 | +50 (generatable graph, BFS, per-start-rule cache) |
| `rng.rs` | 60 | — |
| `error.rs` | 300 | — |
| `lib.rs` | 100 | — |
| **Total** | **3,680** | **+250** |

CLI (~300 lines) and tests are excluded from the budget per §15.

**Actuals** (non-blank, non-test lines; update as modules land). After M1.4: **2,135 lines,
1,503 code and 632 doc.** `check.rs` is 323 of its 850 with steps 1-3 of 9 done, which is on
track — the remaining steps are analyses over a table that now exists. `parse.rs` came in at 625
against 650, `display.rs` at 182 against 200.

The M1.2 reading holds: roughly 30% of every written line is a doc comment, and the projection
is ~3,400 on a whole-line count against §15's ~3,500 — tighter than comfortable but no longer
clearly over, because `check.rs` is running under budget. On a code-only count the crate would
land near 2,400. Still worth settling the convention in §15 at M1.7, when `check.rs` is done and
the last large unknown is resolved.

---

## 8. Suggested order of work

1. **M0, in one sitting.** Nothing is blocked any more — every normative question is settled, and
   the one open item (§2.1) is a `preserve_case` flag decided when PR 3.2 is written. M0 is
   mechanical and it unblocks everything.
2. M1.1–1.3 (parse + fixtures) before M1.4–1.8 (check). Having real fixtures in the tree makes
   every subsequent analysis testable against grammars people actually wrote.
3. M1.4 (rule-table build + lowering + ids) is the structural centre of the crate — both
   round-trip contracts and every downstream analysis hang off it. Get its `Display` and
   `PartialEq` tests green before building anything on top.
4. M1.6 (`min_len` / `witness`) is the most subtle PR. Write §4.1's algorithm with its own unit
   tests before wiring it to anything.
5. M2.2 (repetition) is the second most subtle. It has a mandatory test table — write the table
   first, then the code.
6. M3 last, and only once the recognizer is trusted: every generator acceptance criterion is
   stated in terms of the recognizer. Within M3, 3.1 before 3.2 — the chase is much easier to
   debug when plain generation is already known good.
