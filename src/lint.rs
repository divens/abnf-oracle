//! Advisory warnings: a pure function of a checked grammar (SCOPE.md 6.6, D5).
//!
//! Nothing here can fail a check, and neither the recognizer nor the generator consults any of
//! it. That separation is deliberate — a lint exists to tell a grammar author something they
//! probably want to know, and a grammar that trips every warning still matches exactly what it
//! says it matches.
//!
//! [`CheckedGrammar::lint`] reports what can be known without entry points.
//! [`CheckedGrammar::lint_from`] adds unreachability, which cannot be known without them: a
//! grammar may legitimately expose several entry points, and picking one arbitrarily would
//! turn the other's rules into false positives.
//!
//! Warnings come out in rule-definition order, and in a fixed order within a rule, so that
//! output can be compared between runs.

use std::collections::BTreeSet;

use crate::ast::{Node, NodeId};
use crate::check::CheckedGrammar;
use crate::core_rules;
use crate::error::LintWarning;

impl CheckedGrammar {
    /// Warnings that need no entry points.
    ///
    /// Note that the entry rule of any grammar is reported as unreferenced — nothing mentions
    /// it, which is what makes it the entry rule. Use [`CheckedGrammar::lint_from`] when the
    /// entry points are known.
    #[must_use]
    pub fn lint(&self) -> Vec<LintWarning> {
        self.lint_inner(None)
    }

    /// Warnings, including rules unreachable from `start_rules`.
    ///
    /// Names matching no rule are ignored: with no error channel here, the alternative would be
    /// to report every rule as unreachable from a typo. Validate entry points with
    /// [`CheckedGrammar::rule`] first if that matters.
    #[must_use]
    pub fn lint_from(&self, start_rules: &[&str]) -> Vec<LintWarning> {
        self.lint_inner(Some(start_rules))
    }

    fn lint_inner(&self, start_rules: Option<&[&str]>) -> Vec<LintWarning> {
        let referenced = self.referenced_by_others();
        let reachable = start_rules.map(|starts| self.reachable_from(starts));

        let mut warnings = Vec::new();
        for (index, rule) in self.rules().iter().enumerate() {
            let name = rule.name.as_str().to_owned();

            // Never an error: RFC 8259 defines `char`, which shadows the core rule `CHAR`, so
            // treating this as a failure would reject the JSON grammar outright (D6, D40).
            if core_rules::is_core_rule(&rule.name.key()) {
                warnings.push(LintWarning::ShadowsCoreRule { name: name.clone() });
            }

            if self.min_len(rule.body).is_finite() {
                // Only inside an otherwise productive rule: a wholly dead rule is already
                // reported as one, and repeating it branch by branch is noise (SCOPE.md 6.4).
                for branch in self.unproductive_branches(rule.body) {
                    warnings.push(LintWarning::UnproductiveAlternative {
                        rule: name.clone(),
                        branch,
                    });
                }
            } else {
                warnings.push(LintWarning::UnproductiveRule { name: name.clone() });
            }

            if !referenced.contains(&index) {
                warnings.push(LintWarning::UnreferencedRule { name: name.clone() });
            }
            if let Some(reachable) = &reachable
                && !reachable.contains(&index)
            {
                warnings.push(LintWarning::UnreachableRule { name });
            }
        }
        warnings
    }

    /// The rules some *other* rule mentions.
    ///
    /// Self-references do not count. A rule that only refers to itself is still unused by the
    /// rest of the grammar, and that is what the warning is for.
    fn referenced_by_others(&self) -> BTreeSet<usize> {
        let mut referenced = BTreeSet::new();
        for (index, rule) in self.rules().iter().enumerate() {
            for target in self.references_in(rule.body) {
                if target != index {
                    referenced.insert(target);
                }
            }
        }
        referenced
    }

    /// The rules reachable from `start_rules` by following references.
    fn reachable_from(&self, start_rules: &[&str]) -> BTreeSet<usize> {
        let mut reachable = BTreeSet::new();
        let mut queue: Vec<usize> = start_rules
            .iter()
            .filter_map(|name| {
                let key = name.to_ascii_lowercase();
                self.rules().iter().position(|rule| rule.name.key() == key)
            })
            .collect();
        reachable.extend(queue.iter().copied());

        while let Some(index) = queue.pop() {
            let body = self.rules()[index].body;
            for target in self.references_in(body) {
                if target < self.rules().len() && reachable.insert(target) {
                    queue.push(target);
                }
            }
        }
        reachable
    }

    /// The rules a body references, as indices into the combined table.
    fn references_in(&self, body: NodeId) -> BTreeSet<usize> {
        let mut found = BTreeSet::new();
        self.walk(body, &mut |grammar, node| {
            if matches!(grammar.node(node), Node::RuleRef { .. })
                && let Some(target) = grammar.target(node)
            {
                found.insert(grammar.rule_index(target));
            }
        });
        found
    }

    /// The branch indices of alternations in this body that can never match.
    ///
    /// The index is within its own alternation, which is all [`LintWarning`] can carry; two
    /// alternations in one rule with a dead branch at the same index therefore produce one
    /// warning rather than two indistinguishable ones.
    fn unproductive_branches(&self, body: NodeId) -> BTreeSet<u32> {
        let mut dead = BTreeSet::new();
        self.walk(body, &mut |grammar, node| {
            if let Node::Alt { branches } = grammar.node(node) {
                for (index, branch) in branches.iter().enumerate() {
                    if !grammar.min_len(*branch).is_finite() {
                        dead.insert(index as u32);
                    }
                }
            }
        });
        dead
    }

    /// Visits every node of a body, parents before children.
    fn walk(&self, node: NodeId, visit: &mut impl FnMut(&Self, NodeId)) {
        visit(self, node);
        match self.node(node) {
            Node::Alt { branches } => {
                for branch in branches.clone() {
                    self.walk(branch, visit);
                }
            }
            Node::Concat { items } => {
                for item in items.clone() {
                    self.walk(item, visit);
                }
            }
            Node::Repeat { body, .. } | Node::Optional { body } => {
                let body = *body;
                self.walk(body, visit);
            }
            Node::RuleRef { .. } | Node::CharVal(_) | Node::NumVal { .. } | Node::ProseVal(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::Grammar;
    use crate::error::LintWarning;

    fn lint(src: &str) -> Vec<LintWarning> {
        Grammar::parse(src)
            .unwrap_or_else(|e| panic!("{src:?}: {e}"))
            .check()
            .unwrap_or_else(|e| panic!("{src:?}: {e:?}"))
            .lint()
    }

    fn lint_from(src: &str, starts: &[&str]) -> Vec<LintWarning> {
        Grammar::parse(src)
            .unwrap_or_else(|e| panic!("{src:?}: {e}"))
            .check()
            .unwrap_or_else(|e| panic!("{src:?}: {e:?}"))
            .lint_from(starts)
    }

    fn names(warnings: &[LintWarning]) -> Vec<String> {
        warnings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_rule_nothing_else_mentions_is_unreferenced() {
        let warnings = lint("start = middle\r\nmiddle = \"x\"\r\nspare = \"y\"\r\n");
        let unreferenced: Vec<&str> = warnings
            .iter()
            .filter_map(|w| match w {
                LintWarning::UnreferencedRule { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        // `start` too: nothing mentions it, which is what makes it the entry rule.
        assert_eq!(unreferenced, ["start", "spare"]);
    }

    #[test]
    fn a_self_reference_does_not_count_as_a_reference() {
        let warnings = lint("start = \"x\" [start]\r\n");
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, LintWarning::UnreferencedRule { name } if name == "start")),
            "{:?}",
            names(&warnings)
        );
    }

    #[test]
    fn a_rule_that_matches_nothing_is_reported_once() {
        let warnings = lint("start = \"ok\" / bad\r\nbad = \"x\" bad\r\n");
        let unproductive: Vec<&str> = warnings
            .iter()
            .filter_map(|w| match w {
                LintWarning::UnproductiveRule { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(unproductive, ["bad"]);

        // And the branch that reaches it is called out separately, because a dead branch in a
        // working rule is more often a real mistake than a wholly dead rule is.
        assert!(
            warnings.iter().any(|w| matches!(
                w,
                LintWarning::UnproductiveAlternative { rule, branch: 1 } if rule == "start"
            )),
            "{:?}",
            names(&warnings)
        );
    }

    #[test]
    fn a_dead_rule_is_not_also_reported_branch_by_branch() {
        let warnings = lint("bad = \"x\" bad / \"y\" bad\r\n");
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w, LintWarning::UnproductiveAlternative { .. })),
            "a wholly dead rule is already reported as one: {:?}",
            names(&warnings)
        );
    }

    #[test]
    fn a_dead_branch_is_found_when_nested() {
        let warnings = lint("start = \"a\" (\"b\" / bad)\r\nbad = \"x\" bad\r\n");
        assert!(
            warnings.iter().any(|w| matches!(
                w,
                LintWarning::UnproductiveAlternative { rule, branch: 1 } if rule == "start"
            )),
            "{:?}",
            names(&warnings)
        );
    }

    #[test]
    fn shadowing_a_core_rule_warns() {
        let warnings = lint("char = \"x\"\r\nstart = char\r\n");
        let shadows: Vec<&str> = warnings
            .iter()
            .filter_map(|w| match w {
                LintWarning::ShadowsCoreRule { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(shadows, ["char"], "case-insensitively, against CHAR");
    }

    #[test]
    fn unreachability_needs_entry_points() {
        let src = "start = middle\r\nmiddle = \"x\"\r\norphan = \"y\"\r\n";

        assert!(
            !lint(src)
                .iter()
                .any(|w| matches!(w, LintWarning::UnreachableRule { .. })),
            "lint() cannot know the entry points"
        );

        let unreachable: Vec<String> = lint_from(src, &["start"])
            .iter()
            .filter_map(|w| match w {
                LintWarning::UnreachableRule { name } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(unreachable, ["orphan"]);
    }

    #[test]
    fn several_entry_points_are_all_honoured() {
        let src = "one = \"a\"\r\ntwo = \"b\"\r\nthree = \"c\"\r\n";
        let unreachable: Vec<String> = lint_from(src, &["one", "two"])
            .iter()
            .filter_map(|w| match w {
                LintWarning::UnreachableRule { name } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            unreachable,
            ["three"],
            "a grammar may expose several entry points"
        );
    }

    #[test]
    fn entry_points_are_matched_case_insensitively_and_typos_are_ignored() {
        let src = "start = \"a\"\r\n";
        assert!(
            !lint_from(src, &["START"])
                .iter()
                .any(|w| matches!(w, LintWarning::UnreachableRule { .. }))
        );
        // A name matching nothing seeds nothing; it does not make the whole grammar unreachable
        // by accident, but it does leave every rule unreachable, which is the honest answer.
        let warnings = lint_from(src, &["nope"]);
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, LintWarning::UnreachableRule { name } if name == "start"))
        );
    }

    #[test]
    fn core_rules_are_never_linted() {
        // They are the resolution environment, not the user's grammar; reporting sixteen
        // unreferenced rules for every grammar would make the output useless.
        let warnings = lint("start = ALPHA\r\n");
        assert!(
            warnings
                .iter()
                .all(|w| !matches!(w, LintWarning::UnreferencedRule { name } if name == "ALPHA")),
            "{:?}",
            names(&warnings)
        );
        assert_eq!(
            warnings.len(),
            1,
            "only `start` is unreferenced: {:?}",
            names(&warnings)
        );
    }

    #[test]
    fn a_clean_grammar_warns_only_about_its_entry_rule() {
        let warnings = lint_from("start = middle\r\nmiddle = \"x\"\r\n", &["start"]);
        assert_eq!(names(&warnings), ["rule `start` is never referenced"]);
    }

    #[test]
    fn warnings_come_out_in_a_stable_order() {
        let src = "char = \"x\"\r\nstart = char / bad\r\nbad = \"y\" bad\r\n";
        let first = names(&lint(src));
        assert_eq!(first, names(&lint(src)));
        // Rule-definition order, and a fixed order within each rule.
        assert_eq!(
            first,
            [
                "rule `char` shadows the core rule of the same name",
                "branch 1 of an alternation in rule `start` can never match",
                "rule `start` is never referenced",
                "rule `bad` can never match: it has no finite expansion",
            ]
        );
    }
}
