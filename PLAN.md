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
place error quality matters.

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
5. **Representability** (§6.1). A range is representable iff it contains at least one scalar
   value: `lo <= 0x10FFFF && !(lo >= 0xD800 && hi <= 0xDFFF)`. Precompute, per representable
   range, how many scalars it contains, so the generator can index into it uniformly in O(1)
   (subtract the surrogate block when the range spans it).
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
| **1.3** | `core_rules.rs` + fixture transcription: core rules, 3 self-definition variants, RFC 8259, 3986, 5322 §3, 3339, 9110 subset; header comment per file naming RFC/section/errata; `tests/parse_grammars.rs`; the ASCII-cleanliness scan | every fixture parses; the scan passes |
| **1.4** | `check.rs` steps 1–3: rule-table build, lowering + node ids, hygienic resolution; `Display` + `PartialEq` for `CheckedGrammar` | `Grammar::parse(cg.to_string()).check() == cg`; duplicate / orphan `=/` / `shadows_core` fixtures fail with the right variant; two textually different grammars with one canonical form get identical node ids |
| **1.5** | `check.rs` steps 4–5: range validation, representability | `5*2"a"` and `%x5A-41` fixtures fail; a surrogate-spanning range is representable, `%xD800-DFFF` is not |
| **1.6** | `check.rs` steps 6–7: `nullable`, `min_len`, `witness` (§4.1) | unit tests incl. the saturating case (three nested `4294967295` repeats → `Finite(u64::MAX)`), the `a = b / "x"` tie, prose non-nullable |
| **1.7** | `check.rs` steps 8–9: first-graph, left recursion, per-rule reachability | direct and indirect left-recursion fixtures fail; the prose fixture does not; a `*0(…)` body contributes no edges |
| **1.8** | `lint.rs` + `lint_from` | expected warnings on hand-written cases; the RFC 9110 fixture yields `ShadowsCoreRule` and no errors |
| **1.9** | CLI `check` and `rules` subcommands | manual smoke run over each fixture |

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
| **2.4** | Corpus harness, JSONTestSuite import, `NOTES.md` | every `y_*` accepted, every `n_*` rejected, `i_*` recorded |
| **2.5** | `tests/self_definition.rs` (D23) | the canonical self-grammar accepts every fixture's CRLF-normalized text |
| **2.6** | Brute-force enumerator + proptest (§4.3); memo-off equivalence test | property tests green at the default case count |
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
| R1 | **Stack overflow in `recognize` on deeply nested input.** JSONTestSuite ships `n_structures_100000_opening_arrays.json` and friends; recursion depth there is ~input length. | Run corpus tests on a thread with an explicit 64 MB stack (`std::thread::Builder::stack_size`). If it still blows, classify those specific cases in `NOTES.md` as depth-limited and document the limit. Decide in PR 2.4, not later. |
| R2 | **Rule 2 implemented as "any branch that reaches an uncovered unit" rather than `argmin dist`.** It is the natural simplification, it passes the JSON coverage test, and it is wrong — D38's termination argument needs the strictly decreasing measure. | The chase test is the guard: rev 5.2 records that the naive version fails it with `OutputLimit`. Keep the debug assertion of R10 as the second line of defence, and do not relax the chase fixture to something shallower. |
| R3 | Witness cycles from a round-robin `min_len` fixpoint. | Knuth worklist (§4.1), plus a debug assertion that following witnesses from any productive node terminates within `nodes.len()` steps. |
| R4 | Fixture transcription errors across seven RFC grammars, by hand. | Header comment naming RFC + section + errata; a test asserting each fixture checks clean; cross-check against `abnfgen` / go-abnf in M4. Transcribe from RFC text, never from memory or third-party copies. |
| R5 | Git line-ending mangling on Windows silently changing corpus bytes. | `.gitattributes` in PR 0.1, plus a test reading one known corpus file and asserting its exact byte length. |
| R6 | ~~Errata 2968 / 3076 wording taken from memory rather than the errata page.~~ **Fired, and was caught in M1.1.** SCOPE rev 5.2 had the two errata's subjects transposed and treated 3076 as a numeric-value clarification, which would have left `rulelist` ambiguous in the canonical fixture. | Fixed in SCOPE rev 5.3 / D39. The mitigation stands for PR 1.3: paste the corrected productions into each fixture header, from the errata page, never from memory. |
| R7 | **Line-budget pressure** (~3,500 lines, §15). The two-layer split added a representation and a lowering pass; D38 added the generatable graph and the BFS; M1.2 added `display.rs`. | One printer for both layers (§3.10); check at each milestone with `tokei`. If it overruns, the honest first cut is terser `Display` impls in `error.rs`, not a required behaviour. |
| R8 | The ASCII-cleanliness scan and the deliberately non-ASCII invalid fixture contradict each other. | Scope the scan to exclude `tests/grammars/invalid/parse/` (PR 1.1 / 1.3). Small, but it will fail CI on the day the fixture lands if nobody planned for it. |
| R9 | Memo clone cost making the JSON corpus slow enough to annoy. | Accept until M4; run corpus tests in `--release`; only then consider borrowed sets. |
| R10 | A bug in the chase degrades into a walk bounded only by `max_steps` — slow and hard to diagnose. | Debug assertion `dist[chosen] < dist[current]` on every chase step (§4.4), so the invariant fails loudly in tests rather than quietly in the field. |

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

**Actuals** (non-blank, non-test lines; update as modules land). After M1.2, with `ast.rs`,
`parse.rs`, `display.rs`, `error.rs` and `rng.rs` essentially complete: **1,672 lines, of which
1,167 are code and 505 are doc comments.** Against the 2,120 still budgeted for `core_rules`,
`check`, `lint`, `recognize` and `generate`, that projects to ~3,790 — over §15's ~3,500, and the
re-derived budget above (3,680) is already over it too.

The overage is documentation, not code: 30% of every written line so far is a doc comment, which
is what `#![deny(missing_docs)]` costs on a public data model this wide, plus the running
citations back to SCOPE that make the code auditable against the spec. On a code-only count the
crate sits at 1,167 and would land near 2,600 — comfortably inside.

So §15 needs a measuring convention, not a diet. Recommendation: count code lines only, and say
so in §15. The alternative — hitting 3,500 on a whole-line count — means deleting the
cross-references to SCOPE decisions, and those are what let a reader check the implementation
against the spec at all. Decide at M1.7, when `check.rs` has landed and the largest remaining
unknown is resolved.

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
