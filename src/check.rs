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

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::OnceLock;

use crate::ast::{DefinedAs, Element, Grammar, Node, NodeId, NumVal, Rule, RuleId, RuleName, Span};
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
        CheckedGrammar {
            rules: self.rules,
            user_rules,
            nodes: self.nodes,
            resolved: self.resolved,
        }
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
