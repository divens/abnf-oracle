//! [`Grammar`] to [`CheckedGrammar`]: the merged rule table, and the analyses over it.
//!
//! This is where the syntactic layer becomes the semantic one (D12, D32). Three things happen,
//! in this order, and the order matters:
//!
//! 1. **The rule table is built.** Definitions are grouped by ASCII-lowercased name; the first
//!    `=` defines a rule and every later `=/` appends its alternatives to it. A second `=` is a
//!    duplicate, and a `=/` with no base is an orphan.
//! 2. **The table is lowered into a node arena.** Each node is emitted before its children, so
//!    a [`NodeId`] *is* the pre-order position that SCOPE.md 6.7 requires of node ids, with no
//!    separate numbering pass and no unassigned sentinel.
//! 3. **References are resolved**, user rules first and then the implicit core environment —
//!    except inside a core rule body, which sees core rules only (D33).
//!
//! Only *structural* problems are errors here. Compatibility limits are recorded per rule and
//! enforced per start rule; lint warnings never fail a check (SCOPE.md 6.6, D5). Errors
//! accumulate: a grammar with four undefined references reports four, not the first.
//!
//! M1.5 through M1.7 add the analyses — representability, `nullable`, `min_len` with its
//! witness, the first-graph and its cycles, and the per-rule prose and representability flags.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::collections::BinaryHeap;
use std::collections::btree_map::Entry;
use std::sync::OnceLock;

use crate::ast::{
    DefinedAs, Element, Grammar, MinLen, Node, NodeId, NumVal, Rule, RuleId, RuleName, Span,
    Witness,
};
use crate::core_rules;
use crate::error::CheckError;

/// A grammar that passed structural validation.
///
/// The only route to a recognizer or a generator, so there is no way to match against a grammar
/// whose structure was never checked (D1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedGrammar {
    /// User rules first, then the core rules, so a user [`NodeId`] is exactly the pre-order
    /// position it would have in the user grammar alone.
    rules: Vec<Rule>,
    /// How many of `rules` are the user's.
    user_rules: usize,
    /// The node arena. `NodeId` indexes this.
    nodes: Vec<Node>,
    /// What each `RuleRef` node resolves to; `None` for every other kind of node.
    resolved: Vec<Option<RuleId>>,
    /// Per node: whether it can match the empty string (SCOPE.md 6.4).
    nullable: Vec<bool>,
    /// Per node: the length of its shortest match, saturating (D27).
    min_len: Vec<MinLen>,
    /// Per node: the choice that shortest match makes, for the generator (D28).
    witness: Vec<Option<Witness>>,
}

impl Grammar {
    /// Merges incremental alternatives, resolves names, and validates structure.
    ///
    /// Consumes the grammar: there is no way back to the syntactic layer, and no way to reach a
    /// recognizer or generator except through the result (D1).
    ///
    /// # Errors
    ///
    /// Returns every [`CheckError`] found, not just the first.
    pub fn check(self) -> Result<CheckedGrammar, Vec<CheckError>> {
        let mut errors = Vec::new();
        let merged = merge(self, &mut errors);

        let mut builder = Builder::new(&merged);
        builder.lower_rules(&merged, Scope::User);
        builder.lower_rules(core_table(), Scope::Core);
        errors.append(&mut builder.errors);

        if errors.is_empty() {
            Ok(builder.finish(merged.len()))
        } else {
            Err(errors)
        }
    }
}

impl CheckedGrammar {
    /// The user's rules, in first-definition order. Core rules are not included: they are the
    /// resolution environment, not part of the grammar (D12).
    #[must_use]
    pub fn rules(&self) -> &[Rule] {
        &self.rules[..self.user_rules]
    }

    /// Looks a rule up by name, case-insensitively, user rules before core rules.
    #[must_use]
    pub fn rule(&self, name: &str) -> Option<&Rule> {
        let key = name.to_ascii_lowercase();
        self.rules()
            .iter()
            .find(|rule| rule.name.key() == key)
            .or_else(|| self.core_rules().iter().find(|rule| rule.name.key() == key))
    }

    /// The implicit RFC 5234 Appendix B environment.
    #[must_use]
    pub fn core_rules(&self) -> &[Rule] {
        &self.rules[self.user_rules..]
    }

    /// The node at `id`.
    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    /// The rule a `RuleRef` node resolves to, or `None` for any other node.
    #[must_use]
    pub fn target(&self, id: NodeId) -> Option<RuleId> {
        self.resolved[id.index()]
    }

    /// The rule `id` names.
    #[must_use]
    pub fn rule_by_id(&self, id: RuleId) -> &Rule {
        match id {
            RuleId::User(index) => &self.rules[index as usize],
            RuleId::Core(index) => &self.rules[self.user_rules + index as usize],
        }
    }

    /// Whether the node at `id` can match the empty string.
    #[must_use]
    pub fn is_nullable(&self, id: NodeId) -> bool {
        self.nullable[id.index()]
    }

    /// The length of the shortest string the node at `id` can match.
    ///
    /// `MinLen::Infinite` means the node matches nothing — it is *unproductive*. That is a lint,
    /// not an error: the recognizer handles it fine on finite input, it simply never accepts
    /// (SCOPE.md 6.4).
    #[must_use]
    pub fn min_len(&self, id: NodeId) -> MinLen {
        self.min_len[id.index()]
    }

    /// The choice the shortest match at `id` makes, if it has one.
    ///
    /// Alternations and optionals record a branch, repetitions their minimum count. Following
    /// these terminates, which is what lets the generator finish once its depth budget is gone
    /// (D28).
    #[must_use]
    pub fn witness(&self, id: NodeId) -> Option<Witness> {
        self.witness[id.index()]
    }

    /// How many nodes the arena holds, user and core together.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Rebuilds the subtree at `id` as an [`Element`].
    ///
    /// Allocates, which does not matter for the one caller: the canonical printer, which would
    /// otherwise have to be written twice, once per representation.
    pub(crate) fn element_at(&self, id: NodeId) -> Element {
        match self.node(id) {
            Node::Alt { branches } => {
                Element::Alt(branches.iter().map(|b| self.element_at(*b)).collect())
            }
            Node::Concat { items } => {
                Element::Concat(items.iter().map(|i| self.element_at(*i)).collect())
            }
            Node::Repeat { repeat, body, span } => Element::Repeat {
                repeat: *repeat,
                body: Box::new(self.element_at(*body)),
                span: *span,
            },
            Node::Optional { body } => Element::Optional(Box::new(self.element_at(*body))),
            Node::RuleRef { name, span } => Element::RuleRef {
                name: name.clone(),
                span: *span,
            },
            Node::CharVal(value) => Element::CharVal(value.clone()),
            Node::NumVal { value, span } => Element::NumVal {
                value: value.clone(),
                span: *span,
            },
            Node::ProseVal(text) => Element::ProseVal {
                text: text.clone(),
                span: crate::ast::Ignored(Span::default()),
            },
        }
    }
}

/// The core rules as a merged table, built once.
///
/// They contain no `=/`, so merging them is only a change of representation — but they go
/// through the same lowering as user rules, which is the point.
fn core_table() -> &'static [Merged] {
    static TABLE: OnceLock<Vec<Merged>> = OnceLock::new();
    TABLE.get_or_init(|| {
        core_rules::core_grammar()
            .definitions()
            .iter()
            .map(|definition| Merged {
                name: definition.name.clone(),
                body: definition.body.clone(),
            })
            .collect()
    })
}

/// One entry of a merged rule table: a name and the body every `=` and `=/` for it produced.
pub(crate) struct Merged {
    pub(crate) name: RuleName,
    pub(crate) body: Element,
}

/// Groups definitions by name and folds `=/` into the rule it extends.
fn merge(grammar: Grammar, errors: &mut Vec<CheckError>) -> Vec<Merged> {
    let mut order: Vec<String> = Vec::new();
    let mut collected: BTreeMap<String, (RuleName, Vec<Element>)> = BTreeMap::new();

    for definition in grammar.into_definitions() {
        let key = definition.name.key();
        match definition.defined_as {
            DefinedAs::Base => match collected.entry(key) {
                Entry::Occupied(_) => errors.push(CheckError::DuplicateDefinition {
                    name: definition.name.as_str().to_owned(),
                    span: definition.span.0,
                }),
                Entry::Vacant(slot) => {
                    order.push(slot.key().clone());
                    slot.insert((definition.name, vec![definition.body]));
                }
            },
            DefinedAs::Incremental => match collected.get_mut(&key) {
                Some((_, alternatives)) => alternatives.push(definition.body),
                None => errors.push(CheckError::IncrementalWithoutBase {
                    name: definition.name.as_str().to_owned(),
                    // Extending an implicit rule has no coherent meaning under hygienic
                    // resolution, so say so rather than just "no prior definition" (D34).
                    shadows_core: core_rules::is_core_rule(&key),
                    span: definition.span.0,
                }),
            },
        }
    }

    order
        .into_iter()
        .filter_map(|key| collected.remove(&key))
        .map(|(name, alternatives)| Merged {
            name,
            body: combine(alternatives),
        })
        .collect()
}

/// Folds a rule's alternatives into one body.
///
/// Flattened into a single alternation rather than nested, so that branch indices — which are
/// half of a coverage unit's identity (D10) — do not depend on how many `=/` lines the author
/// happened to use.
fn combine(alternatives: Vec<Element>) -> Element {
    if alternatives.len() == 1 {
        return alternatives.into_iter().next().expect("length checked");
    }
    let mut branches = Vec::with_capacity(alternatives.len());
    for alternative in alternatives {
        match alternative {
            Element::Alt(nested) => branches.extend(nested),
            other => branches.push(other),
        }
    }
    Element::Alt(branches)
}

/// Which environment a rule reference is being resolved in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// A user rule body: user rules first, then core.
    User,
    /// A core rule body: core rules only, so that shadowing cannot reach inside (D33).
    Core,
}

struct Builder {
    rules: Vec<Rule>,
    nodes: Vec<Node>,
    resolved: Vec<Option<RuleId>>,
    user_index: BTreeMap<String, RuleId>,
    core_index: BTreeMap<String, RuleId>,
    errors: Vec<CheckError>,
}

impl Builder {
    /// Indexes every rule name up front, so lowering can resolve references as it goes instead
    /// of leaving placeholders behind to patch.
    fn new(user: &[Merged]) -> Self {
        let mut user_index = BTreeMap::new();
        for (position, rule) in user.iter().enumerate() {
            user_index.insert(rule.name.key(), RuleId::User(position as u32));
        }
        let mut core_index = BTreeMap::new();
        for (position, rule) in core_table().iter().enumerate() {
            core_index.insert(rule.name.key(), RuleId::Core(position as u32));
        }
        Self {
            rules: Vec::new(),
            nodes: Vec::new(),
            resolved: Vec::new(),
            user_index,
            core_index,
            errors: Vec::new(),
        }
    }

    fn lower_rules(&mut self, table: &[Merged], scope: Scope) {
        for rule in table {
            let body = self.lower(&rule.body, scope);
            self.rules.push(Rule {
                name: rule.name.clone(),
                body,
            });
        }
    }

    /// Emits `element` and its subtree, parent before children.
    fn lower(&mut self, element: &Element, scope: Scope) -> NodeId {
        // Reserve this node's id before descending, so ids come out in pre-order.
        let id = NodeId::from_index(self.nodes.len() as u32);
        self.nodes.push(Node::ProseVal(String::new()));
        self.resolved.push(None);

        let node = match element {
            Element::Alt(branches) => Node::Alt {
                branches: branches.iter().map(|b| self.lower(b, scope)).collect(),
            },
            Element::Concat(items) => Node::Concat {
                items: items.iter().map(|i| self.lower(i, scope)).collect(),
            },
            Element::Repeat { repeat, body, span } => {
                if !repeat.is_valid() {
                    self.errors.push(CheckError::InvalidRepeatRange {
                        min: repeat.min,
                        max: repeat.max.expect("an unbounded repetition is always valid"),
                        span: span.0,
                    });
                }
                Node::Repeat {
                    repeat: *repeat,
                    body: self.lower(body, scope),
                    span: *span,
                }
            }
            Element::Optional(body) => Node::Optional {
                body: self.lower(body, scope),
            },
            Element::RuleRef { name, span } => {
                match self.resolve(name, scope) {
                    Some(target) => self.resolved[id.index()] = Some(target),
                    None => self.errors.push(CheckError::UndefinedRule {
                        name: name.as_str().to_owned(),
                        span: span.0,
                    }),
                }
                Node::RuleRef {
                    name: name.clone(),
                    span: *span,
                }
            }
            Element::CharVal(value) => Node::CharVal(value.clone()),
            Element::NumVal { value, span } => {
                if let NumVal::Range { lo, hi } = value
                    && lo > hi
                {
                    // An oracle should call a typo a typo rather than silently matching
                    // nothing (SCOPE.md 12, item 6).
                    self.errors.push(CheckError::InvalidNumericRange {
                        lo: *lo,
                        hi: *hi,
                        span: span.0,
                    });
                }
                Node::NumVal {
                    value: value.clone(),
                    span: *span,
                }
            }
            Element::ProseVal { text, .. } => Node::ProseVal(text.clone()),
        };

        self.nodes[id.index()] = node;
        id
    }

    fn resolve(&self, name: &RuleName, scope: Scope) -> Option<RuleId> {
        let key = name.key();
        match scope {
            Scope::User => self
                .user_index
                .get(&key)
                .or_else(|| self.core_index.get(&key))
                .copied(),
            Scope::Core => self.core_index.get(&key).copied(),
        }
    }

    fn finish(self, user_rules: usize) -> CheckedGrammar {
        let analysis = Analysis {
            rules: &self.rules,
            user_rules,
            nodes: &self.nodes,
            resolved: &self.resolved,
        };
        let nullable = analysis.nullable();
        let (min_len, witness) = analysis.min_len();
        CheckedGrammar {
            rules: self.rules,
            user_rules,
            nodes: self.nodes,
            resolved: self.resolved,
            nullable,
            min_len,
            witness,
        }
    }
}

/// The arena, and enough context to follow a rule reference through it.
struct Analysis<'a> {
    rules: &'a [Rule],
    user_rules: usize,
    nodes: &'a [Node],
    resolved: &'a [Option<RuleId>],
}

impl Analysis<'_> {
    /// The root node of the rule `id` names.
    fn body_of(&self, id: RuleId) -> NodeId {
        match id {
            RuleId::User(index) => self.rules[index as usize].body,
            RuleId::Core(index) => self.rules[self.user_rules + index as usize].body,
        }
    }

    /// What a rule reference at `index` depends on.
    fn target_body(&self, index: usize) -> Option<NodeId> {
        self.resolved[index].map(|target| self.body_of(target))
    }

    /// Which nodes can match the empty string.
    ///
    /// A plain least fixpoint. It is boolean, monotone, and carries no witness, so it needs
    /// none of the machinery `min_len` does.
    fn nullable(&self) -> Vec<bool> {
        let mut nullable = vec![false; self.nodes.len()];
        let mut changed = true;
        while changed {
            changed = false;
            // Children always have higher indices than their parents, so walking backwards
            // propagates bottom-up and most grammars settle in one pass.
            for index in (0..self.nodes.len()).rev() {
                let value = match &self.nodes[index] {
                    Node::Alt { branches } => branches.iter().any(|b| nullable[b.index()]),
                    Node::Concat { items } => items.iter().all(|i| nullable[i.index()]),
                    // Zero repetitions match the empty string whatever the body is, which is
                    // why `*bad` is nullable even when `bad` matches nothing at all.
                    Node::Repeat { repeat, body, .. } => repeat.min == 0 || nullable[body.index()],
                    Node::Optional { .. } => true,
                    Node::RuleRef { .. } => self
                        .target_body(index)
                        .is_some_and(|body| nullable[body.index()]),
                    // `""` is a legal char-val, and the only nullable terminal.
                    Node::CharVal(value) => value.value.is_empty(),
                    // Prose is assumed non-nullable: the assumption that cannot mislead, since
                    // the only rules it could be wrong about are ones v1 refuses anyway (D26).
                    Node::NumVal { .. } | Node::ProseVal(_) => false,
                };
                if value && !nullable[index] {
                    nullable[index] = true;
                    changed = true;
                }
            }
        }
        nullable
    }

    /// The shortest match of every node, and the choice that achieves it.
    ///
    /// Knuth's generalization of Dijkstra, not a round-robin fixpoint, and the difference is
    /// the whole point. A node is *finalized* exactly once, in non-decreasing order of its
    /// final `min_len`, so every witness recorded at finalization names a node finalized
    /// strictly earlier. Following witnesses therefore terminates — which is what the
    /// generator rests on when its depth budget runs out (D28). Round-robin arrives at the
    /// same numbers but cannot give that guarantee: with `a = b / "x"` and `b = a / "y"`, both
    /// settle at 1 and the witnesses it records can point at each other.
    ///
    /// The value functions are all *superior* — each is monotone and at least as large as the
    /// arguments it uses — which is the condition that makes finalizing the smallest candidate
    /// sound. An alternation is not `min` over unknowns but one relaxation per branch, exactly
    /// as a shortest-path edge.
    fn min_len(&self) -> (Vec<MinLen>, Vec<Option<Witness>>) {
        let count = self.nodes.len();
        let mut value = vec![MinLen::Infinite; count];
        let mut finalized = vec![false; count];
        let mut witness: Vec<Option<Witness>> = vec![None; count];
        // How many steps the shortest derivation takes. Ties in `min_len` are broken by
        // preferring the shallower one, which is what makes a witness chain point at terminals
        // rather than wandering through equally short rules first: with `a = b / "x"` and
        // `b = a / "y"`, both alternations settle at 1, and depth is what picks the terminal.
        // It also bounds the work witness mode does, since the chain it follows is the
        // shortest available, not merely one of the shortest strings.
        let mut depth = vec![u32::MAX; count];

        let parents = self.reverse_dependencies();
        let mut heap: BinaryHeap<Reverse<(u64, u32, usize)>> = BinaryHeap::new();

        // Seed with every node that needs no child: the terminals, and the constructions that
        // match the empty string outright.
        for (index, slot) in value.iter_mut().enumerate() {
            if let MinLen::Finite(length) = self.axiom(index) {
                *slot = MinLen::Finite(length);
                heap.push(Reverse((length, 0, index)));
            }
        }

        while let Some(Reverse((length, steps, index))) = heap.pop() {
            if finalized[index] {
                continue; // A better candidate was taken already; this entry is stale.
            }
            finalized[index] = true;
            value[index] = MinLen::Finite(length);
            depth[index] = steps;
            witness[index] = self.witness_for(index, &value, &finalized, &depth, length);

            for parent in &parents[index] {
                let parent_index = parent.index();
                if finalized[parent_index] {
                    continue;
                }
                if let Some((candidate, candidate_depth)) =
                    self.relax(parent_index, &value, &finalized, &depth)
                    && (candidate, candidate_depth)
                        < (finite_or_max(value[parent_index]), depth[parent_index])
                {
                    value[parent_index] = MinLen::Finite(candidate);
                    depth[parent_index] = candidate_depth;
                    heap.push(Reverse((candidate, candidate_depth, parent_index)));
                }
            }
        }

        // Whatever was never finalized cannot be reached from any terminal: it matches nothing.
        for ((length, witness), done) in value.iter_mut().zip(witness.iter_mut()).zip(&finalized) {
            if !done {
                *length = MinLen::Infinite;
                *witness = None;
            }
        }

        debug_assert!(
            self.witnesses_are_well_founded(&witness, &finalized),
            "a witness chain cycles, so the generator would not terminate"
        );
        (value, witness)
    }

    /// The value of a node that depends on no child.
    fn axiom(&self, index: usize) -> MinLen {
        match &self.nodes[index] {
            Node::CharVal(value) => MinLen::Finite(value.value.chars().count() as u64),
            Node::NumVal { value, .. } => MinLen::Finite(match value {
                NumVal::Scalar(_) | NumVal::Range { .. } => 1,
                NumVal::Concat(values) => values.len() as u64,
            }),
            Node::ProseVal(_) => MinLen::Finite(1),
            Node::Optional { .. } => MinLen::ZERO,
            Node::Repeat { repeat, .. } if repeat.min == 0 => MinLen::ZERO,
            Node::Concat { items } if items.is_empty() => MinLen::ZERO,
            _ => MinLen::Infinite,
        }
    }

    /// A node's best value given what has been finalized so far, with the number of steps the
    /// derivation takes.
    fn relax(
        &self,
        index: usize,
        value: &[MinLen],
        finalized: &[bool],
        depth: &[u32],
    ) -> Option<(u64, u32)> {
        let finite = |id: &NodeId| match value[id.index()] {
            MinLen::Finite(length) if finalized[id.index()] => Some((length, depth[id.index()])),
            _ => None,
        };
        match &self.nodes[index] {
            // Each branch is its own way of reaching the alternation, so this is ordinary
            // shortest-path relaxation, not a `min` over unknowns.
            Node::Alt { branches } => branches
                .iter()
                .filter_map(finite)
                .min()
                .map(|(length, steps)| (length, steps + 1)),
            Node::Concat { items } => {
                let parts: Option<Vec<(u64, u32)>> = items.iter().map(finite).collect();
                parts.map(|parts| {
                    let length = parts.iter().fold(MinLen::ZERO, |sum, (part, _)| {
                        sum.saturating_add(MinLen::Finite(*part))
                    });
                    let steps = parts.iter().map(|(_, s)| *s).max().unwrap_or(0) + 1;
                    (finite_or_max(length), steps)
                })
            }
            Node::Repeat { repeat, body, .. } => {
                if repeat.min == 0 {
                    Some((0, 0))
                } else {
                    finite(body).map(|(length, steps)| {
                        // Saturating: three nested `4294967295` repetitions overflow `u64`
                        // even though every bound fits in one, and a saturated value is still
                        // finite, hence still productive (D27).
                        let total = MinLen::Finite(length).saturating_mul(repeat.min);
                        (finite_or_max(total), steps + 1)
                    })
                }
            }
            Node::Optional { .. } => Some((0, 0)),
            Node::RuleRef { .. } => self
                .target_body(index)
                .and_then(|body| finite(&body))
                .map(|(length, steps)| (length, steps + 1)),
            _ => match self.axiom(index) {
                MinLen::Finite(length) => Some((length, 0)),
                MinLen::Infinite => None,
            },
        }
    }

    /// The choice a node's shortest derivation makes.
    fn witness_for(
        &self,
        index: usize,
        value: &[MinLen],
        finalized: &[bool],
        depth: &[u32],
        length: u64,
    ) -> Option<Witness> {
        match &self.nodes[index] {
            Node::Alt { branches } => branches
                .iter()
                .enumerate()
                .filter(|(_, b)| finalized[b.index()] && value[b.index()] == MinLen::Finite(length))
                // Shallowest first, then leftmost, so the choice is deterministic and the
                // chain the generator follows is as short as the grammar allows.
                .min_by_key(|(branch, b)| (depth[b.index()], *branch))
                .map(|(branch, _)| Witness::Branch(branch as u32)),
            // `[x]` is `x / empty`, and the empty arm is both the shortest and the one that
            // descends into nothing.
            Node::Optional { .. } => Some(Witness::Branch(1)),
            Node::Repeat { repeat, .. } => Some(Witness::Count(repeat.min)),
            _ => None,
        }
    }

    /// For each node, the nodes whose value depends on it.
    ///
    /// A rule reference depends on the body of the rule it resolves to, so the arena's parent
    /// links alone would leave the graph disconnected at every reference.
    fn reverse_dependencies(&self) -> Vec<Vec<NodeId>> {
        let mut parents = vec![Vec::new(); self.nodes.len()];
        for (index, node) in self.nodes.iter().enumerate() {
            let parent = NodeId::from_index(index as u32);
            match node {
                Node::Alt { branches } => {
                    for child in branches {
                        parents[child.index()].push(parent);
                    }
                }
                Node::Concat { items } => {
                    for child in items {
                        parents[child.index()].push(parent);
                    }
                }
                Node::Repeat { body, .. } | Node::Optional { body } => {
                    parents[body.index()].push(parent);
                }
                Node::RuleRef { .. } => {
                    if let Some(body) = self.target_body(index) {
                        parents[body.index()].push(parent);
                    }
                }
                Node::CharVal(_) | Node::NumVal { .. } | Node::ProseVal(_) => {}
            }
        }
        parents
    }

    /// Whether following witnesses from any productive node terminates.
    ///
    /// The property the generator's termination rests on (D28, PLAN.md R3). A regression here
    /// should be a failing test, not a hang in the field, so it is asserted in debug builds
    /// every time a grammar is checked.
    fn witnesses_are_well_founded(&self, witness: &[Option<Witness>], finalized: &[bool]) -> bool {
        for (start, _) in finalized.iter().enumerate().filter(|(_, done)| **done) {
            let mut at = start;
            let mut steps = 0;
            // Only alternations delegate to a single child; every other node either stops or
            // descends into children the walker handles itself.
            while let (Node::Alt { branches }, Some(Witness::Branch(branch))) =
                (&self.nodes[at], witness[at])
            {
                at = branches[branch as usize].index();
                steps += 1;
                if steps > self.nodes.len() {
                    return false;
                }
            }
        }
        true
    }
}

fn finite_or_max(value: MinLen) -> u64 {
    match value {
        MinLen::Finite(length) => length,
        MinLen::Infinite => u64::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::NumVal;

    fn check(src: &str) -> CheckedGrammar {
        Grammar::parse(src)
            .unwrap_or_else(|e| panic!("{src:?} should parse: {e}"))
            .check()
            .unwrap_or_else(|errors| panic!("{src:?} should check: {errors:?}"))
    }

    fn errors(src: &str) -> Vec<CheckError> {
        Grammar::parse(src)
            .unwrap_or_else(|e| panic!("{src:?} should parse: {e}"))
            .check()
            .expect_err("should not check")
    }

    #[test]
    fn incremental_alternatives_are_merged_into_their_base() {
        let checked = check("foo = \"a\"\r\nfoo =/ \"b\"\r\nfoo =/ \"c\"\r\n");
        assert_eq!(checked.rules().len(), 1);
        assert_eq!(checked.to_string(), "foo = \"a\" / \"b\" / \"c\"\r\n");
    }

    #[test]
    fn merging_flattens_rather_than_nests() {
        // Branch indices are half of a coverage unit's identity, so they must not depend on how
        // the author split the alternatives across `=/` lines.
        let split = check("foo = \"a\" / \"b\"\r\nfoo =/ \"c\" / \"d\"\r\n");
        let whole = check("foo = \"a\" / \"b\" / \"c\" / \"d\"\r\n");
        assert_eq!(split, whole);
    }

    #[test]
    fn incremental_alternatives_match_the_base_name_case_insensitively() {
        let checked = check("foo = \"a\"\r\nFOO =/ \"b\"\r\n");
        assert_eq!(checked.rules().len(), 1);
        assert_eq!(checked.to_string(), "foo = \"a\" / \"b\"\r\n");
    }

    #[test]
    fn a_rule_defined_twice_is_a_duplicate() {
        let found = errors("foo = \"a\"\r\nfoo = \"b\"\r\n");
        assert!(
            matches!(found.as_slice(), [CheckError::DuplicateDefinition { name, .. }] if name == "foo"),
            "{found:?}"
        );
    }

    #[test]
    fn an_incremental_alternative_needs_a_base() {
        let found = errors("foo = \"a\"\r\nbar =/ \"b\"\r\n");
        assert!(
            matches!(
                found.as_slice(),
                [CheckError::IncrementalWithoutBase { name, shadows_core: false, .. }]
                    if name == "bar"
            ),
            "{found:?}"
        );
    }

    #[test]
    fn extending_an_implicit_core_rule_says_so() {
        let found = errors("foo = DIGIT\r\nDIGIT =/ \"x\"\r\n");
        assert!(
            matches!(
                found.as_slice(),
                [CheckError::IncrementalWithoutBase { name, shadows_core: true, .. }]
                    if name == "DIGIT"
            ),
            "{found:?}"
        );
        assert!(found[0].to_string().contains("core definition"));
    }

    #[test]
    fn an_inverted_repeat_range_is_a_structural_error() {
        let found = errors("start = 5*2\"a\"\r\n");
        assert!(
            matches!(
                found.as_slice(),
                [CheckError::InvalidRepeatRange { min: 5, max: 2, .. }]
            ),
            "{found:?}"
        );
    }

    #[test]
    fn an_inverted_numeric_range_is_a_structural_error() {
        let found = errors("start = %x5A-41\r\n");
        assert!(
            matches!(
                found.as_slice(),
                [CheckError::InvalidNumericRange {
                    lo: 0x5A,
                    hi: 0x41,
                    ..
                }]
            ),
            "{found:?}"
        );
    }

    #[test]
    fn range_errors_point_at_the_range() {
        let src = "start = \"ok\" %x5A-41\r\n";
        let found = errors(src);
        let [CheckError::InvalidNumericRange { span, .. }] = found.as_slice() else {
            panic!("{found:?}")
        };
        assert_eq!(
            &src[span.range()],
            "%x5A-41",
            "the span must locate the typo"
        );
    }

    #[test]
    fn repeat_errors_point_at_the_bounds() {
        let src = "start = \"ok\" 5*2\"a\"\r\n";
        let found = errors(src);
        let [CheckError::InvalidRepeatRange { span, .. }] = found.as_slice() else {
            panic!("{found:?}")
        };
        assert_eq!(&src[span.range()], "5*2");
    }

    #[test]
    fn equal_bounds_and_unbounded_repetitions_are_valid() {
        check("start = 2*2\"a\" 3\"b\" *\"c\" 4*\"d\"\r\n");
    }

    #[test]
    fn an_unrepresentable_terminal_is_not_a_structural_error() {
        // It is a compatibility limit, enforced per start rule, not a reason to reject the
        // grammar (SCOPE.md 6.1, D4). M1.7 records which rules can reach one.
        let checked = check("start = %xD800-DFFF / %x110000\r\n");
        assert_eq!(checked.rules().len(), 1);
    }

    #[test]
    fn octet_grammars_check() {
        // RFC 9110's obs-text and RFC 5322's obs-* rules must load (SCOPE.md 3).
        check("obs-text = %x80-FF\r\nstart = obs-text\r\n");
    }

    #[test]
    fn undefined_references_are_all_reported() {
        let found = errors("foo = bar baz / qux\r\n");
        assert_eq!(found.len(), 3, "errors accumulate: {found:?}");
        assert!(
            found
                .iter()
                .all(|e| matches!(e, CheckError::UndefinedRule { .. }))
        );
    }

    #[test]
    fn core_rules_need_no_definition() {
        let checked = check("foo = ALPHA DIGIT HEXDIG CRLF\r\n");
        assert_eq!(checked.rules().len(), 1);
        assert_eq!(checked.core_rules().len(), core_rules::CORE_RULE_COUNT);
    }

    #[test]
    fn core_rule_resolution_is_hygienic() {
        // A user `DIGIT` changes what the user's references mean and nothing else: core HEXDIG
        // still reaches core DIGIT (D33). M2 checks the matching behaviour; here it is the
        // resolution table that has to be right.
        let checked = check("start = DIGIT HEXDIG\r\nDIGIT = \"x\"\r\n");

        let start = checked.rule("start").expect("start exists");
        let Node::Concat { items } = checked.node(start.body) else {
            panic!("expected a concatenation")
        };
        assert!(
            matches!(checked.target(items[0]), Some(RuleId::User(_))),
            "a user reference to DIGIT resolves to the user's rule"
        );

        let hexdig = checked.rule("HEXDIG").expect("core HEXDIG");
        let Node::Alt { branches } = checked.node(hexdig.body) else {
            panic!("expected an alternation")
        };
        assert!(
            matches!(checked.target(branches[0]), Some(RuleId::Core(_))),
            "core HEXDIG's reference to DIGIT stays in the core environment"
        );
    }

    #[test]
    fn shadowing_a_core_rule_is_not_an_error() {
        // RFC 8259 does exactly this with `char` (D40).
        let checked = check("char = \"x\"\r\nfoo = char\r\n");
        assert_eq!(checked.rules().len(), 2);
        let foo = checked.rule("foo").expect("foo exists");
        assert!(matches!(checked.target(foo.body), Some(RuleId::User(_))));
    }

    #[test]
    fn node_ids_are_the_pre_order_of_the_merged_table() {
        let checked = check("foo = \"a\" \"b\"\r\n");
        let foo = checked.rule("foo").expect("foo exists");
        assert_eq!(
            foo.body,
            NodeId::from_index(0),
            "the parent is emitted first"
        );
        let Node::Concat { items } = checked.node(foo.body) else {
            panic!("expected a concatenation")
        };
        assert_eq!(items, &[NodeId::from_index(1), NodeId::from_index(2)]);
    }

    #[test]
    fn one_canonical_form_gives_one_set_of_node_ids() {
        // Two spellings of the same grammar, textually different in every way that does not
        // matter: case, grouping, repetition spelling, radix, and `=/` versus one line.
        let left = check("foo = *1(\"a\") / %d98\r\n");
        let right = check("FOO = [\"a\"]\r\nfoo =/ %x62\r\n");
        assert_eq!(left, right);
        assert_eq!(left.node_count(), right.node_count());
        for index in 0..left.node_count() {
            let id = NodeId::from_index(index as u32);
            assert_eq!(left.node(id), right.node(id), "node {index} differs");
        }
    }

    #[test]
    fn the_arena_holds_user_nodes_before_core_nodes() {
        let checked = check("foo = \"a\"\r\n");
        // One user node, then everything Appendix B needs.
        assert_eq!(
            checked.rule("foo").expect("foo").body,
            NodeId::from_index(0)
        );
        assert!(checked.node_count() > 1);
        assert!(matches!(
            checked.node(NodeId::from_index(0)),
            Node::CharVal(_)
        ));
    }

    #[test]
    fn numeric_values_survive_lowering() {
        let checked = check("foo = %x41-5A\r\n");
        let foo = checked.rule("foo").expect("foo exists");
        assert!(matches!(
            checked.node(foo.body),
            Node::NumVal {
                value: NumVal::Range { lo: 0x41, hi: 0x5A },
                ..
            }
        ));
    }

    #[test]
    fn a_grammar_with_no_rules_checks() {
        let checked = check("\r\n");
        assert!(checked.rules().is_empty());
        assert_eq!(checked.core_rules().len(), core_rules::CORE_RULE_COUNT);
    }

    /// The analyses of a one-rule grammar's body.
    fn body_of_start(src: &str) -> (CheckedGrammar, NodeId) {
        let checked = check(src);
        let body = checked.rule("start").expect("start exists").body;
        (checked, body)
    }

    fn nullable(elements: &str) -> bool {
        let (checked, body) = body_of_start(&format!("start = {elements}\r\n"));
        checked.is_nullable(body)
    }

    fn min_len(elements: &str) -> MinLen {
        let (checked, body) = body_of_start(&format!("start = {elements}\r\n"));
        checked.min_len(body)
    }

    #[test]
    fn nullability_of_each_construction() {
        assert!(
            nullable("\"\""),
            "an empty char-val is the one nullable terminal"
        );
        assert!(nullable("[\"a\"]"));
        assert!(nullable("*\"a\""));
        assert!(nullable("0*3\"a\""));
        assert!(nullable("\"\" \"\""), "a concatenation of nullables");
        assert!(
            nullable("\"a\" / \"\""),
            "an alternation with one nullable branch"
        );

        assert!(!nullable("\"a\""));
        assert!(!nullable("%x41"));
        assert!(!nullable("1*\"a\""));
        assert!(
            !nullable("\"a\" \"\""),
            "a concatenation needs every item nullable"
        );
        assert!(!nullable("\"a\" / \"b\""));
    }

    #[test]
    fn nullability_follows_rule_references() {
        let checked = check("start = empty\r\nempty = \"\"\r\n");
        let start = checked.rule("start").expect("start exists").body;
        assert!(checked.is_nullable(start));
        // LWSP is the one core rule that matches the empty string.
        let lwsp = checked.rule("LWSP").expect("core LWSP").body;
        assert!(checked.is_nullable(lwsp));
        let digit = checked.rule("DIGIT").expect("core DIGIT").body;
        assert!(!checked.is_nullable(digit));
    }

    #[test]
    fn prose_is_non_nullable_and_one_character_long() {
        // The assumption that cannot mislead: prose never makes a branch look unproductive and
        // never creates a first-graph edge, and the only rules it could be wrong about are the
        // ones v1 refuses to recognize or generate from anyway (D26).
        assert!(!nullable("<any token>"));
        assert_eq!(min_len("<any token>"), MinLen::Finite(1));
    }

    #[test]
    fn shortest_matches_of_the_terminals() {
        assert_eq!(min_len("\"\""), MinLen::ZERO);
        assert_eq!(min_len("\"abc\""), MinLen::Finite(3));
        assert_eq!(min_len("%x41"), MinLen::Finite(1));
        assert_eq!(min_len("%x41-5A"), MinLen::Finite(1));
        assert_eq!(
            min_len("%x41.42.43"),
            MinLen::Finite(3),
            "a concatenation of three"
        );
    }

    #[test]
    fn shortest_matches_compose() {
        assert_eq!(
            min_len("\"ab\" \"cde\""),
            MinLen::Finite(5),
            "concatenation sums"
        );
        assert_eq!(
            min_len("\"abc\" / \"d\""),
            MinLen::Finite(1),
            "alternation minimizes"
        );
        assert_eq!(
            min_len("[\"abc\"]"),
            MinLen::ZERO,
            "an optional can be skipped"
        );
        assert_eq!(
            min_len("*\"abc\""),
            MinLen::ZERO,
            "so can a repetition with no minimum"
        );
        assert_eq!(
            min_len("3\"ab\""),
            MinLen::Finite(6),
            "repetition multiplies"
        );
        assert_eq!(
            min_len("2*5\"ab\""),
            MinLen::Finite(4),
            "by the minimum, not the maximum"
        );
    }

    #[test]
    fn shortest_matches_saturate_rather_than_overflow() {
        // Three nested repetitions whose bounds each fit in 32 bits, whose product does not.
        let nested = min_len("4294967295(4294967295(4294967295\"x\"))");
        assert_eq!(nested, MinLen::Finite(u64::MAX));
        assert!(
            nested.is_finite(),
            "saturated is still finite, hence still productive (D27)"
        );

        // Two levels still fit, so the saturation above is real arithmetic, not a short circuit.
        assert_eq!(
            min_len("4294967295(4294967295\"x\")"),
            MinLen::Finite(4_294_967_295 * 4_294_967_295)
        );
    }

    #[test]
    fn a_rule_that_matches_nothing_is_unproductive() {
        let checked = check("thing = \"x\" thing\r\n");
        let thing = checked.rule("thing").expect("thing exists").body;
        assert_eq!(checked.min_len(thing), MinLen::Infinite);
        assert_eq!(
            checked.witness(thing),
            None,
            "there is no shortest derivation to record"
        );
        // Unproductive is a lint, not an error: the grammar checked (SCOPE.md 6.4).
    }

    #[test]
    fn an_unproductive_branch_does_not_infect_its_alternation() {
        let checked = check("start = \"ok\" / bad\r\nbad = \"x\" bad\r\n");
        let start = checked.rule("start").expect("start exists").body;
        assert_eq!(checked.min_len(start), MinLen::Finite(2));
        assert_eq!(
            checked.witness(start),
            Some(Witness::Branch(0)),
            "the productive branch is the shortest derivation"
        );
        let bad = checked.rule("bad").expect("bad exists").body;
        assert_eq!(checked.min_len(bad), MinLen::Infinite);
    }

    #[test]
    fn tied_shortest_matches_still_give_well_founded_witnesses() {
        // The case that defeats a round-robin fixpoint: both rules settle at 1, so comparing
        // `min_len` cannot order them and the witnesses can end up pointing at each other
        // (SCOPE.md 6.4, D28). Depth breaks the tie, so each points at its terminal.
        let checked = check("a = b / \"x\"\r\nb = a / \"y\"\r\n");
        let a = checked.rule("a").expect("a exists").body;
        let b = checked.rule("b").expect("b exists").body;

        assert_eq!(checked.min_len(a), MinLen::Finite(1));
        assert_eq!(checked.min_len(b), MinLen::Finite(1));
        assert_eq!(
            checked.witness(a),
            Some(Witness::Branch(1)),
            "a takes \"x\""
        );
        assert_eq!(
            checked.witness(b),
            Some(Witness::Branch(1)),
            "b takes \"y\""
        );
    }

    #[test]
    fn witnesses_for_optionals_and_repetitions() {
        let (checked, body) = body_of_start("start = [\"a\"]\r\n");
        assert_eq!(
            checked.witness(body),
            Some(Witness::Branch(1)),
            "[x] is x / empty, and the empty arm is both shortest and childless"
        );

        let (checked, body) = body_of_start("start = 2*5\"a\"\r\n");
        assert_eq!(checked.witness(body), Some(Witness::Count(2)));

        let (checked, body) = body_of_start("start = *\"a\"\r\n");
        assert_eq!(checked.witness(body), Some(Witness::Count(0)));
    }

    #[test]
    fn following_witnesses_terminates_on_every_fixture_shape() {
        // The property the generator's termination rests on. The same check runs as a debug
        // assertion inside `check` itself, so this pins the intent where a reader will see it.
        for src in [
            "a = b / \"x\"\r\nb = a / \"y\"\r\n",
            "start = \"ok\" / bad\r\nbad = \"x\" bad\r\n",
            // Right recursion only: `*("a" / start)` and `[start] "x"` would each put `start`
            // in its own first set, which M1.7 rejects outright.
            "start = \"a\" *(\"b\" / start)\r\n",
            "start = \"x\" [start]\r\n",
            "start = ALPHA / DIGIT / HEXDIG\r\n",
        ] {
            let checked = check(src);
            for rule in checked.rules() {
                if checked.min_len(rule.body).is_finite() {
                    let mut at = rule.body;
                    for _ in 0..=checked.node_count() {
                        let Node::Alt { branches } = checked.node(at) else {
                            break;
                        };
                        let Some(Witness::Branch(branch)) = checked.witness(at) else {
                            panic!("{src:?}: a productive alternation has no witness")
                        };
                        at = branches[branch as usize];
                    }
                }
            }
        }
    }

    #[test]
    fn analyses_reach_the_core_environment() {
        let checked = check("start = HEXDIG\r\n");
        let hexdig = checked.rule("HEXDIG").expect("core HEXDIG").body;
        assert_eq!(checked.min_len(hexdig), MinLen::Finite(1));
        assert!(!checked.is_nullable(hexdig));
    }

    #[test]
    fn checked_grammars_round_trip() {
        for src in [
            "foo = \"a\"\r\n",
            "foo = \"a\"\r\nfoo =/ \"b\"\r\n",
            "foo = ALPHA *(DIGIT / \"-\")\r\n",
            "foo = [\"a\"] %x41-5A\r\n",
            "a = b\r\nb = \"x\"\r\n",
        ] {
            let checked = check(src);
            let reparsed = Grammar::parse(&checked.to_string())
                .expect("canonical form parses")
                .check()
                .expect("canonical form checks");
            assert_eq!(checked, reparsed, "{src:?} changed under round-trip");
            assert_eq!(checked.to_string(), reparsed.to_string());
        }
    }
}
