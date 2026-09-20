//! The deterministic generator (SCOPE.md 6.8).
//!
//! A walk over the arena that emits a string the grammar accepts. Two things make it terminate
//! on grammars where a naive walk would not:
//!
//! * **Unproductive branches are never entered**, in any mode. Once inside `bad = "x" bad`
//!   there may be no alternation left to steer by, so the ban is absolute rather than a
//!   fallback for an exhausted budget (D20).
//! * **Witness mode.** When the depth budget runs out the walk stops choosing and starts
//!   following witnesses, which form a well-founded derivation and so reach terminals in
//!   finitely many steps (D28). Choosing "the branch with the smallest `min_len`" is not a
//!   substitute: saturated values compare meaninglessly, and the choice would turn on node
//!   numbering rather than on the grammar.
//!
//! Termination is not the same as practicality. `start = 1000000000*"a"` is productive and has
//! no shorter expansion, so the walk terminates and would still produce a gigabyte;
//! [`GenOptions::max_output_len`] is what makes that an error instead.
//!
//! Coverage mode ([`GenOptions::coverage`]) steers towards branches not yet taken, with a
//! guarantee rather than a probability: while any reachable branch is untaken, each successful
//! call takes at least one.

use std::collections::{HashMap, VecDeque};

use crate::ast::{CharVal, Node, NodeId, NumVal, Witness};
use crate::check::CheckedGrammar;
use crate::error::GenError;
use crate::rng::SplitMix64;

/// How many extra repetitions a repetition may take beyond its minimum.
pub const DEFAULT_SPREAD: usize = 3;
/// How deep the walk may recurse before it starts following witnesses.
///
/// Named apart from [`crate::DEFAULT_MAX_DEPTH`], which is the recognizer's: that one is a
/// safety limit whose breach is an error, this one is a steering budget whose exhaustion just
/// changes how choices are made.
pub const DEFAULT_GEN_DEPTH: usize = 16;
/// How long a generated string may get before generation gives up.
pub const DEFAULT_MAX_OUTPUT_LEN: usize = 1 << 20;
/// How many nodes the walk may visit before it gives up.
pub const DEFAULT_MAX_STEPS: u64 = 1 << 24;

/// How to generate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenOptions {
    /// How deep to recurse before switching to witness mode. Default [`DEFAULT_GEN_DEPTH`].
    ///
    /// This is a *steering* budget, not a safety limit: exhausting it does not fail, it makes
    /// the walk take the shortest derivation it knows. The safety limits are
    /// [`GenOptions::max_output_len`] and [`GenOptions::max_steps`].
    pub max_depth: usize,

    /// How many repetitions beyond the minimum to consider. Default [`DEFAULT_SPREAD`].
    ///
    /// A repetition takes a count uniformly from `min..=min(max, min + spread)`, so `*"a"`
    /// with the default produces nought to three characters rather than an unbounded run.
    pub spread: usize,

    /// Steer towards branches that have not been taken yet, rather than choosing randomly.
    ///
    /// With this set, and subject to the resource limits, **each successful call covers at
    /// least one new coverage unit while any reachable one remains uncovered** — so full
    /// coverage takes at most `uncovered(rule)` calls. That is a guarantee rather than a
    /// probability, and it does not depend on [`GenOptions::max_depth`] (D10, D38).
    pub coverage: bool,

    /// Emit case-insensitive strings exactly as written, rather than varying their case.
    ///
    /// Varying case is the more useful default for a fuzzer — `"abc"` really does match `AbC` —
    /// but it makes output unpredictable, so a test that compares generated text against an
    /// expected string wants this set.
    pub preserve_case: bool,

    /// Give up once the output reaches this many scalar values. Default
    /// [`DEFAULT_MAX_OUTPUT_LEN`].
    ///
    /// Finite by default, because termination alone does not make generation practical: a
    /// grammar can demand a gigabyte without being wrong (D29).
    pub max_output_len: Option<usize>,

    /// Give up after visiting this many nodes. Default [`DEFAULT_MAX_STEPS`].
    pub max_steps: Option<u64>,
}

impl Default for GenOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_GEN_DEPTH,
            spread: DEFAULT_SPREAD,
            coverage: false,
            preserve_case: false,
            max_output_len: Some(DEFAULT_MAX_OUTPUT_LEN),
            max_steps: Some(DEFAULT_MAX_STEPS),
        }
    }
}

/// Generates strings that a rule accepts.
///
/// Deterministic: the same seed, options and sequence of calls produce the same strings, within
/// a crate version (D15). Constructible only from a [`CheckedGrammar`] (D1).
pub struct Generator<'g> {
    grammar: &'g CheckedGrammar,
    rng: SplitMix64,
    options: GenOptions,
    coverage: Coverage,
    /// Units taken during the call in progress.
    ///
    /// Held apart until the call succeeds: a call that ends in `OutputLimit` produced no
    /// output, so counting what it touched would let the bound be satisfied by strings nobody
    /// ever saw.
    pending: Vec<Unit>,
    steps: u64,
    /// Scalars emitted so far in the current call.
    ///
    /// Counted rather than measured: `String::chars().count()` walks the whole buffer, so
    /// checking the limit that way costs O(n) per character and makes filling it O(n^2). A
    /// million-scalar default would take tens of seconds to refuse.
    emitted: usize,
}

impl<'g> Generator<'g> {
    /// Creates a generator seeded with `seed`.
    #[must_use]
    pub fn new(grammar: &'g CheckedGrammar, seed: u64) -> Self {
        Self {
            grammar,
            rng: SplitMix64::new(seed),
            options: GenOptions::default(),
            coverage: Coverage::new(grammar),
            pending: Vec::new(),
            steps: 0,
            emitted: 0,
        }
    }

    /// Sets the generation options.
    #[must_use]
    pub fn with_options(mut self, options: GenOptions) -> Self {
        self.options = options;
        self
    }

    /// How many nodes have been visited, across every call.
    ///
    /// The counter [`GenOptions::max_steps`] limits, and what tests assert against rather than
    /// wall-clock time (D31).
    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.steps
    }

    /// How many coverage units `rule` can reach that have not been covered yet.
    ///
    /// Counted over the generatable graph, so a branch the generator would never take is not
    /// counted against it (D10). Zero means every branch reachable from this rule has been
    /// taken by some successful call.
    ///
    /// With [`GenOptions::coverage`] set, this is also the *bound*: each successful call
    /// reduces it by at least one, so it reaches zero within this many calls.
    ///
    /// # Errors
    ///
    /// As [`Generator::generate`], for the same reasons: a rule that cannot be generated from
    /// has no coverage to report.
    pub fn uncovered(&mut self, rule: &str) -> Result<usize, GenError> {
        let body = self.start_rule(rule)?;
        let index = self
            .grammar
            .rules()
            .iter()
            .position(|candidate| candidate.body == body)
            .unwrap_or(usize::MAX);

        Ok(self.coverage.uncovered_reachable(self.grammar, index, body))
    }

    /// Generates one string that `rule` accepts.
    ///
    /// # Errors
    ///
    /// [`GenError::UnknownRule`] if there is no such rule; the compatibility limits of
    /// SCOPE.md 6.6 if the rule can reach a prose value or an unrepresentable terminal;
    /// [`GenError::NoFiniteExpansion`] if it matches nothing at all; or a resource limit.
    pub fn generate(&mut self, rule: &str) -> Result<String, GenError> {
        let body = self.start_rule(rule)?;

        let mut out = String::new();
        // Fresh per call, so one call's limit is not spent by the last.
        self.steps = 0;
        self.emitted = 0;
        self.pending.clear();
        self.walk(body, 0, &mut out)?;

        // Committed only now that the call has succeeded.
        for unit in std::mem::take(&mut self.pending) {
            self.coverage.cover(unit);
        }
        Ok(out)
    }

    /// Resolves a start rule, refusing the ones that cannot be generated from.
    fn start_rule(&self, rule: &str) -> Result<NodeId, GenError> {
        let found = self
            .grammar
            .rule(rule)
            .ok_or_else(|| GenError::UnknownRule(rule.to_owned()))?;

        // Prose and unrepresentable terminals are refused before anything is emitted, the same
        // way the recognizer refuses them before reading input (D11, D4).
        self.grammar.can_generate(rule)?;

        // An unproductive rule is refused immediately rather than discovered by walking into
        // it: there is no string to produce, so there is nothing to attempt (D9).
        if !self.grammar.min_len(found.body).is_finite() {
            return Err(GenError::NoFiniteExpansion {
                rule: found.name.as_str().to_owned(),
            });
        }
        Ok(found.body)
    }

    /// Emits one node's contribution to the output.
    fn walk(&mut self, node: NodeId, depth: usize, out: &mut String) -> Result<(), GenError> {
        self.step()?;

        match self.grammar.node(node) {
            Node::Alt { branches } => {
                let branches = branches.clone();
                let chosen = self.choose_branch(node, &branches, depth);
                self.walk(branches[chosen], depth, out)
            }
            Node::Concat { items } => {
                for item in items.clone() {
                    self.walk(item, depth, out)?;
                }
                Ok(())
            }
            Node::Repeat { repeat, body, .. } => {
                let (repeat, body) = (*repeat, *body);
                let count = self.choose_count(repeat.min, repeat.max, body, depth);
                for _ in 0..count {
                    self.walk(body, depth, out)?;
                }
                Ok(())
            }
            Node::Optional { body } => {
                let body = *body;
                // `[x]` is `x / empty`: branch 0 takes the body, branch 1 skips it.
                if self.choose_branch_of_optional(node, body, depth) == 0 {
                    self.walk(body, depth, out)?;
                }
                Ok(())
            }
            Node::RuleRef { .. } => {
                let target = self
                    .grammar
                    .target(node)
                    .expect("`check` resolved every reference");
                let body = self.grammar.rule_by_id(target).body;
                self.walk(body, depth + 1, out)
            }
            Node::CharVal(value) => {
                let value = value.clone();
                self.emit_char_val(&value, out)
            }
            Node::NumVal { value, .. } => {
                let value = value.clone();
                self.emit_num_val(&value, out)
            }
            Node::ProseVal(_) => {
                // Unreachable: the start rule was refused if it could reach prose, and the
                // reachability that decides that walks the same nodes this does.
                debug_assert!(false, "prose reached despite the start-rule gate");
                Ok(())
            }
        }
    }

    /// Picks a branch of an alternation, and records the unit that choice covers.
    fn choose_branch(&mut self, node: NodeId, branches: &[NodeId], depth: usize) -> usize {
        // Never a branch that matches nothing, in any mode (D20).
        let usable: Vec<usize> = (0..branches.len())
            .filter(|index| self.grammar.min_len(branches[*index]).is_finite())
            .collect();
        debug_assert!(
            !usable.is_empty(),
            "a productive alternation has a productive branch"
        );

        let targets: Vec<NodeId> = usable.iter().map(|branch| branches[*branch]).collect();
        let chosen = self.select(node, &usable, &targets, depth);
        self.pending.push(Unit {
            node,
            branch: chosen as u32,
        });
        chosen
    }

    /// Picks between taking an optional's body and skipping it.
    ///
    /// `[x]` is `x / empty`, so this is an alternation with branch 0 the body and branch 1 the
    /// empty arm. The empty arm leads nowhere, which the selection rule sees as an unreachable
    /// target.
    fn choose_branch_of_optional(&mut self, node: NodeId, body: NodeId, depth: usize) -> usize {
        let (usable, targets) = if self.grammar.min_len(body).is_finite() {
            // `node` stands in for "leads nowhere": it holds no uncovered unit of its own that
            // taking the empty arm could reach, so its distance is never the deciding one.
            (vec![0, 1], vec![body, node])
        } else {
            // Skipping is always available, and is all that is left.
            (vec![1], vec![node])
        };

        let chosen = self.select(node, &usable, &targets, depth);
        self.pending.push(Unit {
            node,
            branch: chosen as u32,
        });
        chosen
    }

    /// The selection rule of SCOPE.md 6.8, shared by alternations and optionals.
    ///
    /// `targets[i]` is the node `usable[i]` leads to.
    fn select(
        &mut self,
        node: NodeId,
        usable: &[usize],
        targets: &[NodeId],
        depth: usize,
    ) -> usize {
        if self.options.coverage {
            // 1. A branch not yet taken. Taking it covers a unit outright, which is what makes
            //    the per-call guarantee hold.
            let untaken = usable.iter().copied().find(|branch| {
                let unit = Unit {
                    node,
                    branch: *branch as u32,
                };
                !self.coverage.is_covered(unit) && !self.pending.contains(&unit)
            });
            if let Some(branch) = untaken {
                return branch;
            }

            // 2. Otherwise the branch closest to something uncovered — the *chase*. Distance
            //    strictly decreases along it, so it ends; "any branch that reaches one, ties at
            //    random" would not, which is the failure D38 exists to prevent. Ties by branch
            //    index keep it deterministic.
            let mut nearest: Option<(u32, usize)> = None;
            for (position, branch) in usable.iter().copied().enumerate() {
                let target = targets[position];
                // An optional's empty arm leads back to the node itself: nothing new lies
                // beyond it, so it never wins the chase.
                if target == node {
                    continue;
                }
                let distance = self.coverage.distance_to_uncovered(target);
                if distance != UNREACHABLE && nearest.is_none_or(|(best, _)| distance < best) {
                    nearest = Some((distance, branch));
                }
            }
            if let Some((_, branch)) = nearest {
                return branch;
            }
            // 3. Nothing reachable is uncovered: fall through to the ordinary rules below.
        }

        if depth >= self.options.max_depth {
            // Witness mode: the shortest derivation this node knows, which terminates because
            // witnesses were recorded in the order `min_len` was settled (D28).
            //
            // In coverage mode this is reached only once every reachable unit is covered, so
            // the depth budget can never cut a chase short (D38).
            if let Some(Witness::Branch(branch)) = self.grammar.witness(node)
                && usable.contains(&(branch as usize))
            {
                return branch as usize;
            }
        }
        let pick = self.rng.below(usable.len() as u64) as usize;
        usable[pick]
    }

    /// Picks how many times to repeat.
    fn choose_count(&mut self, min: u64, max: Option<u64>, body: NodeId, depth: usize) -> u64 {
        // In coverage mode a repetition whose body can reach something uncovered must run at
        // least once, or a zero count would skip past units the bound is counting on (D25).
        if self.options.coverage
            && max.is_none_or(|max| max >= 1)
            && self.coverage.distance_to_uncovered(body) != UNREACHABLE
        {
            return min.max(1);
        }
        if depth >= self.options.max_depth {
            return min; // witness mode: a repetition's witness is always its minimum
        }
        let ceiling = max
            .unwrap_or(u64::MAX)
            .min(min.saturating_add(self.options.spread as u64));
        if ceiling <= min {
            return min;
        }
        min + self.rng.below(ceiling - min + 1)
    }

    fn emit_char_val(&mut self, value: &CharVal, out: &mut String) -> Result<(), GenError> {
        for ch in value.value.chars() {
            // A case-insensitive string matches either case, so vary it — that is the whole
            // point of the distinction, and a generator that always emitted one case would
            // never exercise the other.
            let emitted = if value.case_sensitive || self.options.preserve_case {
                ch
            } else if self.rng.bool() {
                ch.to_ascii_uppercase()
            } else {
                ch.to_ascii_lowercase()
            };
            self.push(emitted, out)?;
        }
        Ok(())
    }

    fn emit_num_val(&mut self, value: &NumVal, out: &mut String) -> Result<(), GenError> {
        match value {
            NumVal::Scalar(scalar) => {
                let ch = scalar_at(*scalar).expect("`check` refused unrepresentable terminals");
                self.push(ch, out)
            }
            NumVal::Range { lo, hi } => {
                // Uniform among the scalars the range holds, which is not the same as uniform
                // over `lo..=hi`: the surrogate block is skipped (SCOPE.md 6.1).
                let count = NumVal::scalars_in(*lo, *hi);
                debug_assert!(count > 0, "`check` refused empty ranges");
                let index = self.rng.below(count);
                let ch = NumVal::nth_scalar(*lo, *hi, index).expect("index is in range");
                self.push(ch, out)
            }
            NumVal::Concat(values) => {
                for scalar in values {
                    let ch = scalar_at(*scalar).expect("`check` refused unrepresentable terminals");
                    self.push(ch, out)?;
                }
                Ok(())
            }
        }
    }

    /// Appends one scalar, respecting the output limit.
    fn push(&mut self, ch: char, out: &mut String) -> Result<(), GenError> {
        if self
            .options
            .max_output_len
            .is_some_and(|limit| self.emitted >= limit)
        {
            return Err(GenError::OutputLimit);
        }
        self.emitted += 1;
        out.push(ch);
        Ok(())
    }

    /// Counts one node visit, and stops if that exceeds the limit.
    fn step(&mut self) -> Result<(), GenError> {
        self.steps += 1;
        if self
            .options
            .max_steps
            .is_some_and(|limit| self.steps > limit)
        {
            return Err(GenError::StepLimit);
        }
        Ok(())
    }
}

/// The scalar value `value` names, if it names one.
fn scalar_at(value: u64) -> Option<char> {
    char::from_u32(u32::try_from(value).ok()?)
}

/// Distance meaning "no uncovered unit is reachable from here".
const UNREACHABLE: u32 = u32::MAX;

/// One thing the generator can be asked to exercise: a branch of an alternation.
///
/// `[x]` contributes two, via its `x / empty` model, and since `*1a` canonicalizes to `[a]`
/// the two spellings are indistinguishable here (SCOPE.md 6.8, D10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Unit {
    node: NodeId,
    branch: u32,
}

/// The coverage machinery: which branches exist, which have been taken, and how far away the
/// nearest untaken one is.
///
/// Everything here is computed over the **generatable graph** — the arena minus every branch
/// with infinite `min_len` and every `max == 0` repetition body — because the generator never
/// enters either, so counting what is inside them would make the coverage bound unreachable by
/// construction (SCOPE.md 6.8).
struct Coverage {
    /// Every candidate unit, in node order.
    units: Vec<Unit>,
    /// Where each unit sits in `units`.
    index: HashMap<Unit, usize>,
    /// Which units have been covered by a successful call.
    covered: Vec<bool>,
    /// Which units are reachable from each start rule, cached on first use.
    reachable: HashMap<usize, Vec<usize>>,
    /// Successors of each node over the generatable graph.
    edges: Vec<Vec<NodeId>>,
    /// Distance from each node to the nearest uncovered unit.
    distance: Vec<u32>,
    /// Whether `distance` needs recomputing before it is next read.
    stale: bool,
}

impl Coverage {
    /// Enumerates the units of a grammar and builds the graph they live on.
    fn new(grammar: &CheckedGrammar) -> Self {
        let node_count = grammar.node_count();
        let mut units = Vec::new();
        let mut edges = vec![Vec::new(); node_count];

        for (index, outgoing) in edges.iter_mut().enumerate() {
            let node = NodeId::from_index(index as u32);
            match grammar.node(node) {
                Node::Alt { branches } => {
                    for (branch, child) in branches.iter().enumerate() {
                        // A branch that matches nothing is not a unit and not an edge: the
                        // generator will never take it, so neither may the bound (D20).
                        if grammar.min_len(*child).is_finite() {
                            units.push(Unit {
                                node,
                                branch: branch as u32,
                            });
                            outgoing.push(*child);
                        }
                    }
                }
                Node::Optional { body } => {
                    // Two units: branch 0 takes the body, branch 1 skips it. Skipping is always
                    // available, so it is a unit even when the body is not.
                    if grammar.min_len(*body).is_finite() {
                        units.push(Unit { node, branch: 0 });
                        outgoing.push(*body);
                    }
                    units.push(Unit { node, branch: 1 });
                }
                Node::Concat { items } => outgoing.extend(items.iter().copied()),
                Node::Repeat { repeat, body, .. } => {
                    // `*0(x)` can never run its body, so nothing inside it is reachable (D36).
                    if !repeat.is_never() {
                        outgoing.push(*body);
                    }
                }
                Node::RuleRef { .. } => {
                    if let Some(target) = grammar.target(node) {
                        outgoing.push(grammar.rule_by_id(target).body);
                    }
                }
                Node::CharVal(_) | Node::NumVal { .. } | Node::ProseVal(_) => {}
            }
        }

        let index = units
            .iter()
            .enumerate()
            .map(|(position, unit)| (*unit, position))
            .collect();
        let count = units.len();
        Self {
            units,
            index,
            covered: vec![false; count],
            reachable: HashMap::new(),
            edges,
            distance: vec![UNREACHABLE; node_count],
            stale: true,
        }
    }

    /// How many units reachable from `body` are still uncovered.
    fn uncovered_reachable(
        &mut self,
        grammar: &CheckedGrammar,
        rule: usize,
        body: NodeId,
    ) -> usize {
        // Populate the cache first, so the borrow of `reachable` ends before `covered` is read.
        self.reachable_from(grammar, rule, body);
        self.reachable[&rule]
            .iter()
            .filter(|position| !self.covered[**position])
            .count()
    }

    /// The units reachable from `rule`'s body, cached.
    ///
    /// Reachability does not change as coverage grows — the generatable graph is fixed — so
    /// this is computed once per start rule.
    fn reachable_from(&mut self, grammar: &CheckedGrammar, rule: usize, body: NodeId) -> &[usize] {
        if !self.reachable.contains_key(&rule) {
            let mut seen = vec![false; grammar.node_count()];
            let mut stack = vec![body];
            seen[body.index()] = true;
            let mut found = Vec::new();

            while let Some(node) = stack.pop() {
                // A unit belongs to the node it is a branch of, so reaching the node is what
                // makes its units reachable.
                for (position, unit) in self.units.iter().enumerate() {
                    if unit.node == node {
                        found.push(position);
                    }
                }
                for next in &self.edges[node.index()] {
                    if !seen[next.index()] {
                        seen[next.index()] = true;
                        stack.push(*next);
                    }
                }
            }
            found.sort_unstable();
            self.reachable.insert(rule, found);
        }
        &self.reachable[&rule]
    }

    /// Marks a unit covered, if it is one.
    fn cover(&mut self, unit: Unit) {
        if let Some(position) = self.index.get(&unit)
            && !self.covered[*position]
        {
            self.covered[*position] = true;
            // Covering a unit can only *raise* distances, so a stale table still decreases
            // strictly along a chase — it may merely chase something already covered. Recompute
            // lazily rather than on every step.
            self.stale = true;
        }
    }

    /// Recomputes `distance` if coverage has changed since it was last read.
    ///
    /// Multi-source reverse breadth-first search: every node holding an uncovered unit is at
    /// distance zero, and every other node is one more than its nearest successor.
    fn refresh(&mut self) {
        if !self.stale {
            return;
        }
        self.stale = false;

        let node_count = self.distance.len();
        self.distance.fill(UNREACHABLE);

        // Reverse edges, so the search can walk from a target back towards its predecessors.
        let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); node_count];
        for (from, targets) in self.edges.iter().enumerate() {
            for target in targets {
                incoming[target.index()].push(from);
            }
        }

        let mut queue = VecDeque::new();
        for (position, unit) in self.units.iter().enumerate() {
            if !self.covered[position] {
                let index = unit.node.index();
                if self.distance[index] != 0 {
                    self.distance[index] = 0;
                    queue.push_back(index);
                }
            }
        }
        while let Some(node) = queue.pop_front() {
            let next = self.distance[node] + 1;
            for previous in &incoming[node] {
                if self.distance[*previous] > next {
                    self.distance[*previous] = next;
                    queue.push_back(*previous);
                }
            }
        }
    }

    /// How far `node` is from the nearest uncovered unit.
    fn distance_to_uncovered(&mut self, node: NodeId) -> u32 {
        self.refresh();
        self.distance[node.index()]
    }

    fn is_covered(&self, unit: Unit) -> bool {
        self.index
            .get(&unit)
            .is_some_and(|position| self.covered[*position])
    }
}
