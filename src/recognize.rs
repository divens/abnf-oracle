//! The set-of-positions recognizer (SCOPE.md 6.2, 6.3).
//!
//! For an element and a start position, this computes the set of **all** end positions at which
//! the element can finish. Unordered alternation and ambiguity are then correct by
//! construction: nothing ever commits to the first branch that matched, because nothing ever
//! returns a single answer to commit to. Any code here shaped like `if let Some(end) = …` would
//! be reintroducing PEG semantics by accident.
//!
//! A recognizer is bound to one input for its lifetime (D2). The memo table is keyed by
//! `(rule, position)` and means nothing against a different input, so there is no API for
//! swapping one in.
//!
//! Memoization is a pure optimization: results with it disabled must be identical, and the
//! tests run the whole §6.3 regression table both ways to keep that honest.

use std::collections::{BTreeSet, HashMap};

use crate::ast::{CharVal, Node, NodeId, NumVal, Repeat};
use crate::check::CheckedGrammar;
use crate::error::MatchError;

/// How deep the recognizer may recurse before giving up: [`MatchOptions::max_depth`]'s default.
///
/// Chosen to be safe on a 1 MB stack, the smallest a caller is realistically on, and measured
/// rather than guessed — see the table on [`MatchOptions::max_depth`].
pub const DEFAULT_MAX_DEPTH: usize = 256;

/// Resource limits for recognition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchOptions {
    /// Give up after this many steps, rather than running unboundedly.
    ///
    /// Default `None`. Exceeding it is [`MatchError::StepLimit`], never a silent rejection:
    /// "does not match" and "could not decide" are different answers (D16).
    pub max_steps: Option<u64>,

    /// Give up after recursing this deep. Default [`DEFAULT_MAX_DEPTH`].
    ///
    /// # Why this one has a default when `max_steps` does not
    ///
    /// Exceeding a step limit returns [`MatchError::StepLimit`]; exceeding the *stack* aborts
    /// the process, and no caller can catch that or report it as "could not decide". An oracle
    /// is fed adversarial input by design, so the safe default is a limit rather than a crash.
    ///
    /// # Depth, stack, and how they relate
    ///
    /// Depth counts **grammar nodes entered**, not input characters. Nesting in the input turns
    /// into depth because a recursive rule re-enters itself, at a rate the grammar fixes: for
    /// RFC 8259 one level of `[` costs six nodes, so the default allows JSON nested about 40
    /// deep. A flat input costs almost nothing however long it is — length is not the problem,
    /// nesting is.
    ///
    /// Each level costs roughly 2 KB of stack, measured on a debug build; a release build uses
    /// less. That fixes the ceiling at about one level per 2 KB:
    ///
    /// | stack | deepest safe `max_depth` |
    /// |---|---|
    /// | 512 KB | 128 |
    /// | 1 MB | 400 |
    /// | 2 MiB — what [`std::thread`] gives a spawned thread | 900 |
    /// | 64 MB | ~30,000 |
    ///
    /// The default sits under the 1 MB row, so it is safe wherever the recognizer is called
    /// from. Raising it means giving the recognizer a bigger stack to match:
    ///
    /// ```no_run
    /// # use abnf_oracle::{Grammar, MatchOptions, Recognizer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let src = "start = \"a\"\r\n";
    /// # let grammar = Grammar::parse(src)?.check().map_err(|e| format!("{e:?}"))?;
    /// std::thread::Builder::new()
    ///     .stack_size(64 * 1024 * 1024)
    ///     .spawn(move || {
    ///         let mut recognizer = Recognizer::new(&grammar, "a").with_options(MatchOptions {
    ///             max_depth: Some(40_000),
    ///             ..MatchOptions::default()
    ///         });
    ///         recognizer.accepts("start")
    ///     })?
    ///     .join()
    ///     .expect("no overflow")?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// `None` removes the limit, which is only safe if the input's nesting is already bounded
    /// by something else.
    pub max_depth: Option<usize>,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            max_steps: None,
            max_depth: Some(DEFAULT_MAX_DEPTH),
        }
    }
}

/// What is known about one `(rule, position)` pair.
enum Memo {
    /// Being evaluated: seeing this again at the same position is left recursion.
    InProgress,
    /// Evaluated, with these end positions.
    Done(BTreeSet<usize>),
}

/// Decides whether an input is in the language of a rule.
///
/// Bound to one input, and constructible only from a [`CheckedGrammar`] (D1), so there is no
/// way to recognize against a grammar whose structure was never validated.
pub struct Recognizer<'g, 'i> {
    grammar: &'g CheckedGrammar,
    src: &'i str,
    /// The input as scalar values. Positions index this, not the bytes: the matching domain is
    /// Unicode scalar values (SCOPE.md 6.1).
    input: Vec<char>,
    memo: HashMap<(usize, usize), Memo>,
    steps: u64,
    /// How deep the walk currently is, so the limit can be enforced on the way down.
    depth: usize,
    options: MatchOptions,
    memoize: bool,
}

impl<'g, 'i> Recognizer<'g, 'i> {
    /// Binds a recognizer to a grammar and one input.
    #[must_use]
    pub fn new(grammar: &'g CheckedGrammar, input: &'i str) -> Self {
        Self {
            grammar,
            src: input,
            input: input.chars().collect(),
            memo: HashMap::new(),
            steps: 0,
            depth: 0,
            options: MatchOptions::default(),
            memoize: true,
        }
    }

    /// Sets resource limits.
    #[must_use]
    pub fn with_options(mut self, options: MatchOptions) -> Self {
        self.options = options;
        self
    }

    /// The input this recognizer is bound to.
    #[must_use]
    pub fn input(&self) -> &'i str {
        self.src
    }

    /// How many steps have been taken: one per repetition iteration and one per rule body
    /// evaluated.
    ///
    /// Counted over the whole life of this recognizer and never reset, so
    /// [`MatchOptions::max_steps`] is a budget for everything asked of it rather than for one
    /// call. A recognizer is bound to one input, and its memo table is shared across calls, so
    /// the work really is cumulative. [`crate::Generator::steps`] is the other way round.
    ///
    /// This is the counter [`MatchOptions::max_steps`] limits. Performance tests assert against
    /// it rather than against wall-clock time (D31).
    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.steps
    }

    /// Turns the memo table off. For this crate's own tests, which assert that it changes
    /// nothing but the work done.
    #[doc(hidden)]
    #[must_use]
    pub fn without_memoization(mut self) -> Self {
        self.memoize = false;
        self
    }

    /// Whether the whole input is in the language of `rule`.
    ///
    /// # Errors
    ///
    /// The compatibility limits of SCOPE.md 6.6, checked before any input is examined, or
    /// [`MatchError::StepLimit`]. Never an error for "does not match" — that is `Ok(false)`.
    pub fn accepts(&mut self, rule: &str) -> Result<bool, MatchError> {
        let ends = self.end_positions(rule, 0)?;
        Ok(ends.contains(&self.input.len()))
    }

    /// Every position at which `rule` can finish, starting from `start`.
    ///
    /// The full answer, not the longest or the first: an ambiguous rule reports every ending it
    /// has, which is what makes alternation unordered rather than merely untried.
    ///
    /// # Errors
    ///
    /// As [`Recognizer::accepts`].
    pub fn end_positions(
        &mut self,
        rule: &str,
        start: usize,
    ) -> Result<BTreeSet<usize>, MatchError> {
        // Before examining any input (SCOPE.md 6.5): a rule that can reach a prose value has no
        // matching semantics at all, and saying "does not match" would be a lie.
        self.grammar.can_recognize(rule)?;
        let body = self
            .grammar
            .rule(rule)
            .ok_or_else(|| MatchError::UnknownRule(rule.to_owned()))?
            .body;
        self.match_node(body, start)
    }

    /// The end positions of `node`, starting at `pos`.
    ///
    /// Every descent goes through here, so this is where the depth limit is enforced — before
    /// the frame that would overflow is pushed rather than after.
    fn match_node(&mut self, node: NodeId, pos: usize) -> Result<BTreeSet<usize>, MatchError> {
        self.depth += 1;
        if self
            .options
            .max_depth
            .is_some_and(|limit| self.depth > limit)
        {
            self.depth -= 1;
            return Err(MatchError::DepthLimit);
        }
        let ends = self.match_node_inner(node, pos);
        self.depth -= 1;
        ends
    }

    fn match_node_inner(
        &mut self,
        node: NodeId,
        pos: usize,
    ) -> Result<BTreeSet<usize>, MatchError> {
        // Taken out of `self` so the grammar can be read while the memo table is written.
        let grammar = self.grammar;
        match grammar.node(node) {
            Node::Alt { branches } => {
                let mut ends = BTreeSet::new();
                for branch in branches {
                    ends.extend(self.match_node(*branch, pos)?);
                }
                Ok(ends)
            }
            Node::Concat { items } => {
                let mut positions = BTreeSet::from([pos]);
                for item in items {
                    positions = self.advance(*item, &positions)?;
                    if positions.is_empty() {
                        break;
                    }
                }
                Ok(positions)
            }
            Node::Repeat { repeat, body, .. } => self.match_repeat(*repeat, *body, pos),
            Node::Optional { body } => {
                // `[x]` is `x / empty`, so the start position is always an ending.
                let mut ends = self.match_node(*body, pos)?;
                ends.insert(pos);
                Ok(ends)
            }
            Node::RuleRef { name, .. } => self.match_rule_ref(node, name.as_str(), pos),
            Node::CharVal(value) => Ok(end_set(self.match_char_val(value, pos))),
            Node::NumVal { value, .. } => Ok(end_set(self.match_num_val(value, pos))),
            Node::ProseVal(_) => {
                // Unreachable: `end_positions` refuses a start rule that can reach prose, and
                // the reachability that decides it walks exactly the nodes this walk does.
                debug_assert!(false, "prose reached despite the start-rule gate");
                Ok(BTreeSet::new())
            }
        }
    }

    /// Every position reachable by matching `node` once from each of `from`.
    fn advance(
        &mut self,
        node: NodeId,
        from: &BTreeSet<usize>,
    ) -> Result<BTreeSet<usize>, MatchError> {
        let mut reached = BTreeSet::new();
        for start in from {
            reached.extend(self.match_node(node, *start)?);
        }
        Ok(reached)
    }

    /// Repetition, exactly as SCOPE.md 6.3 specifies it.
    ///
    /// Revision 1 of the spec got this wrong, so it is worth being explicit about the two
    /// traps. Phase 1's early exit is *equality* of the step result, not a subset test: the
    /// step function is deterministic, so `f(cur) == cur` means every later iteration returns
    /// `cur` too, which is what bounds `1000000000*["a"]` to one iteration rather than a
    /// billion. Phase 2 extends the *frontier*, not the whole result, so a position first
    /// reached at count `c` is not re-explored at every later count.
    fn match_repeat(
        &mut self,
        repeat: Repeat,
        body: NodeId,
        pos: usize,
    ) -> Result<BTreeSet<usize>, MatchError> {
        // Phase 1: positions reachable after exactly `min` repetitions. The count matters here
        // even when the positions stop changing, so emptiness and an exact fixpoint are the
        // only ways out early.
        let mut current = BTreeSet::from([pos]);
        for _ in 0..repeat.min {
            self.step()?;
            let next = self.advance(body, &current)?;
            if next.is_empty() {
                return Ok(BTreeSet::new());
            }
            if next == current {
                break;
            }
            current = next;
        }

        // Phase 2: from exactly-min, extend by up to `max - min` more. Breadth-first by count,
        // subtracting what is already known, so this terminates even for an unbounded maximum.
        //
        // The budget counts *down* from how many repetitions are still allowed rather than up
        // from `min`. Counting up overflows on `18446744073709551615*["a"]`, which is a legal
        // bound (D17) and reaches here with `min` at `u64::MAX`: the loop is entered once, the
        // frontier empties, and the increment wraps — panicking in a debug build, which is the
        // build this crate mostly runs in. Counting down also makes the shape plain: when the
        // maximum is unbounded there is no counter, because nothing but an empty frontier can
        // stop the loop.
        debug_assert!(
            repeat.max.is_none_or(|max| max >= repeat.min),
            "`check` rejects an inverted repeat range (D18), so this cannot underflow"
        );
        let mut remaining = repeat.max.map(|max| max - repeat.min);

        let mut result = current.clone();
        let mut frontier = current;
        while remaining.is_none_or(|left| left > 0) && !frontier.is_empty() {
            self.step()?;
            let reached = self.advance(body, &frontier)?;
            frontier = reached.difference(&result).copied().collect();
            result.extend(&frontier);
            if let Some(left) = remaining.as_mut() {
                *left -= 1;
            }
        }
        Ok(result)
    }

    /// A rule reference, through the memo table.
    fn match_rule_ref(
        &mut self,
        node: NodeId,
        name: &str,
        pos: usize,
    ) -> Result<BTreeSet<usize>, MatchError> {
        let grammar = self.grammar;
        let Some(target) = grammar.target(node) else {
            debug_assert!(false, "unresolved reference survived `check`");
            return Ok(BTreeSet::new());
        };
        let key = (grammar.rule_index(target), pos);

        if self.memoize {
            match self.memo.get(&key) {
                Some(Memo::Done(ends)) => return Ok(ends.clone()),
                // Belt and braces: `check` rejects left recursion outright, so this cannot
                // fire on a grammar that got this far (SCOPE.md 6.2).
                Some(Memo::InProgress) => {
                    return Err(MatchError::LeftRecursionDetected {
                        rule: name.to_owned(),
                    });
                }
                None => {
                    self.memo.insert(key, Memo::InProgress);
                }
            }
        }

        let outcome = self
            .step()
            .and_then(|()| self.match_node(grammar.rule_by_id(target).body, pos));

        let ends = match outcome {
            Ok(ends) => ends,
            Err(error) => {
                // `InProgress` must not outlive the call that wrote it. A resource limit
                // unwinds past the `Done` insertion below, and a later call on this same
                // recognizer would then read the leftover marker as left recursion -- which
                // `check` has already ruled out, making it a wrong answer rather than a
                // limit. Every frame cleans up as the error passes through, so the table is
                // whole again by the time the caller sees the error.
                if self.memoize {
                    self.memo.remove(&key);
                }
                return Err(error);
            }
        };

        if self.memoize {
            self.memo.insert(key, Memo::Done(ends.clone()));
        }
        Ok(ends)
    }

    /// Where a quoted string ends, if it matches at `pos`.
    fn match_char_val(&self, value: &CharVal, pos: usize) -> Option<usize> {
        let expected: Vec<char> = value.value.chars().collect();
        let end = pos.checked_add(expected.len())?;
        let actual = self.input.get(pos..end)?;
        let matches = actual.iter().zip(&expected).all(|(got, want)| {
            if value.case_sensitive {
                got == want
            } else {
                // ASCII folding only: RFC 5234 defines no other, and guessing at Unicode case
                // rules would make the oracle disagree with every other ABNF tool (SCOPE.md 6.1).
                got.eq_ignore_ascii_case(want)
            }
        });
        matches.then_some(end)
    }

    /// Where a numeric terminal ends, if it matches at `pos`.
    fn match_num_val(&self, value: &NumVal, pos: usize) -> Option<usize> {
        match value {
            NumVal::Scalar(expected) => {
                let found = u64::from(*self.input.get(pos)?);
                (found == *expected).then_some(pos + 1)
            }
            NumVal::Range { lo, hi } => {
                let found = u64::from(*self.input.get(pos)?);
                (*lo <= found && found <= *hi).then_some(pos + 1)
            }
            NumVal::Concat(values) => {
                let end = pos.checked_add(values.len())?;
                let actual = self.input.get(pos..end)?;
                let matches = actual
                    .iter()
                    .zip(values)
                    .all(|(got, want)| u64::from(*got) == *want);
                matches.then_some(end)
            }
        }
    }

    /// Counts one step, and stops if that exceeds the limit.
    fn step(&mut self) -> Result<(), MatchError> {
        self.steps += 1;
        if self
            .options
            .max_steps
            .is_some_and(|limit| self.steps > limit)
        {
            return Err(MatchError::StepLimit);
        }
        Ok(())
    }
}

/// A terminal's result: the one end position it has, or none at all.
fn end_set(end: Option<usize>) -> BTreeSet<usize> {
    end.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Grammar;

    fn checked(source: &str) -> CheckedGrammar {
        Grammar::parse(source)
            .unwrap_or_else(|e| panic!("{source:?}: {e}"))
            .check()
            .unwrap_or_else(|e| panic!("{source:?}: {e:?}"))
    }

    /// Whether `input` matches `start` in the given grammar, asserted to be independent of the
    /// memo table (invariant 4).
    fn matches(source: &str, input: &str) -> bool {
        let grammar = checked(source);
        let with = Recognizer::new(&grammar, input)
            .accepts("start")
            .expect("recognizes");
        let without = Recognizer::new(&grammar, input)
            .without_memoization()
            .accepts("start")
            .expect("recognizes");
        assert_eq!(
            with, without,
            "memoization changed the answer for {input:?}"
        );
        with
    }

    /// Whether `input` matches a one-rule grammar whose body is `elements`.
    fn body_matches(elements: &str, input: &str) -> bool {
        matches(&format!("start = {elements}\r\n"), input)
    }

    // -- one test per construct of SCOPE.md 4 ------------------------------------------------

    #[test]
    fn quoted_strings_fold_ascii_case_by_default() {
        assert!(body_matches("\"abc\"", "abc"));
        assert!(body_matches("\"abc\"", "ABC"));
        assert!(body_matches("\"abc\"", "AbC"));
        assert!(!body_matches("\"abc\"", "abd"));
        assert!(!body_matches("\"abc\"", "ab"));
        assert!(!body_matches("\"abc\"", "abcd"));
    }

    #[test]
    fn case_sensitive_and_case_insensitive_strings_differ() {
        assert!(body_matches("%s\"aBc\"", "aBc"));
        assert!(!body_matches("%s\"aBc\"", "abc"), "%s is exact (RFC 7405)");
        assert!(body_matches("%i\"aBc\"", "abc"));
        assert!(body_matches("%i\"aBc\"", "ABC"));
    }

    #[test]
    fn folding_never_reaches_beyond_ascii() {
        // RFC 5234 defines no other folding, and inventing one would put this crate at odds
        // with every other ABNF tool (SCOPE.md 6.1, 12 item 1). Two things enforce it: a
        // char-val cannot contain a non-ASCII character in the first place (D35), and a
        // numeric terminal is exact.
        assert!(body_matches("\"i\"", "I"), "ASCII letters fold");
        assert!(body_matches("%xE9", "\u{e9}"));
        assert!(
            !body_matches("%xE9", "\u{c9}"),
            "a numeric value never folds"
        );
        assert!(
            Grammar::parse("start = \"\u{e9}\"\r\n").is_err(),
            "and a non-ASCII quoted string is not even writable"
        );
    }

    #[test]
    fn the_empty_string_matches_nothing_at_all() {
        assert!(body_matches("\"\"", ""));
        assert!(!body_matches("\"\"", "a"));
    }

    #[test]
    fn numeric_terminals() {
        assert!(body_matches("%x41", "A"));
        assert!(!body_matches("%x41", "a"), "a numeric value is exact");
        assert!(body_matches("%d65", "A"));
        assert!(body_matches("%b1000001", "A"));
    }

    #[test]
    fn numeric_ranges_and_concatenations() {
        assert!(body_matches("%x41-5A", "Q"));
        assert!(!body_matches("%x41-5A", "q"));
        assert!(body_matches("%x41.42.43", "ABC"));
        assert!(!body_matches("%x41.42.43", "AB"));
    }

    #[test]
    fn a_terminal_above_the_basic_plane_matches_its_scalar() {
        // Positions are scalar values, not bytes, so one astral character is one position.
        assert!(body_matches("%x1D11E", "\u{1D11E}"));
        assert!(body_matches("%x1D11E \"x\"", "\u{1D11E}x"));
    }

    #[test]
    fn an_unrepresentable_terminal_simply_never_matches() {
        // Not an error here: `can_recognize` refuses it as a start rule, and a rule that merely
        // contains one is refused too. Reached through a rule that is allowed, it matches
        // nothing, which is the honest answer (SCOPE.md 6.1).
        let grammar = checked("start = \"a\" / other\r\nother = \"b\"\r\n");
        let mut recognizer = Recognizer::new(&grammar, "a");
        assert!(recognizer.accepts("start").expect("recognizes"));
    }

    #[test]
    fn alternation_is_unordered() {
        assert!(body_matches("\"a\" / \"ab\"", "a"));
        assert!(
            body_matches("\"a\" / \"ab\"", "ab"),
            "a PEG would have committed to the first branch and rejected this"
        );
    }

    #[test]
    fn ambiguity_reports_every_ending() {
        // The set-of-positions model in one assertion: `*"a" *"a"` can split three characters
        // four ways, and all of them end where the input does.
        let grammar = checked("start = *\"a\" *\"a\"\r\n");
        let mut recognizer = Recognizer::new(&grammar, "aaa");
        let ends = recognizer.end_positions("start", 0).expect("recognizes");
        assert_eq!(ends, BTreeSet::from([0, 1, 2, 3]));
        assert!(recognizer.accepts("start").expect("recognizes"));
    }

    #[test]
    fn concatenation_needs_every_part() {
        assert!(body_matches("\"a\" \"b\"", "ab"));
        assert!(!body_matches("\"a\" \"b\"", "a"));
        assert!(!body_matches("\"a\" \"b\"", "ba"));
    }

    #[test]
    fn grouping_binds_tighter_than_alternation() {
        assert!(body_matches("\"a\" (\"b\" / \"c\")", "ac"));
        assert!(!body_matches("\"a\" (\"b\" / \"c\")", "c"));
        assert!(
            body_matches("\"a\" \"b\" / \"c\"", "c"),
            "without the group, the alternation wins"
        );
    }

    #[test]
    fn optionals_nest() {
        assert!(body_matches("[\"a\"]", ""));
        assert!(body_matches("[\"a\"]", "a"));
        assert!(body_matches("[\"a\" [\"b\"]]", ""));
        assert!(body_matches("[\"a\" [\"b\"]]", "a"));
        assert!(body_matches("[\"a\" [\"b\"]]", "ab"));
        assert!(
            !body_matches("[\"a\" [\"b\"]]", "b"),
            "the inner one needs the outer"
        );
    }

    #[test]
    fn prose_is_refused_before_the_input_is_looked_at() {
        let grammar = checked("start = <anything>\r\n");
        let mut recognizer = Recognizer::new(&grammar, "");
        assert!(matches!(
            recognizer.accepts("start"),
            Err(MatchError::ProseValueReachable { .. })
        ));
    }

    // -- rules ---------------------------------------------------------------------------------

    #[test]
    fn rule_references_are_followed() {
        assert!(matches(
            "start = middle \"!\"\r\nmiddle = \"hi\"\r\n",
            "hi!"
        ));
        assert!(!matches(
            "start = middle \"!\"\r\nmiddle = \"hi\"\r\n",
            "ho!"
        ));
    }

    #[test]
    fn incremental_alternatives_are_matched_as_one_rule() {
        let source = "start = \"a\"\r\nstart =/ \"b\"\r\nstart =/ \"c\"\r\n";
        assert!(matches(source, "a"));
        assert!(matches(source, "b"));
        assert!(matches(source, "c"));
        assert!(!matches(source, "d"));
    }

    #[test]
    fn core_rules_need_no_definition() {
        assert!(matches("start = 1*ALPHA\r\n", "abcXYZ"));
        assert!(!matches("start = 1*ALPHA\r\n", "abc1"));
        assert!(matches("start = 1*DIGIT\r\n", "2026"));
        assert!(matches("start = CRLF\r\n", "\r\n"));
        assert!(
            matches("start = 1*HEXDIG\r\n", "dEaDbEeF"),
            "HEXDIG folds case"
        );
    }

    #[test]
    fn a_user_rule_shadows_a_core_rule_for_user_references_only() {
        // D33, as behaviour rather than as a resolution table: the user's DIGIT changes what
        // their reference means, and core HEXDIG carries on reaching the core DIGIT.
        let grammar = checked("start = DIGIT\r\nhex = HEXDIG\r\nDIGIT = \"x\"\r\n");

        assert!(
            Recognizer::new(&grammar, "x")
                .accepts("start")
                .expect("recognizes")
        );
        assert!(
            !Recognizer::new(&grammar, "7")
                .accepts("start")
                .expect("recognizes")
        );
        assert!(
            Recognizer::new(&grammar, "7")
                .accepts("hex")
                .expect("recognizes"),
            "core HEXDIG still reaches core DIGIT"
        );
        assert!(
            !Recognizer::new(&grammar, "x")
                .accepts("hex")
                .expect("recognizes")
        );
    }

    #[test]
    fn recursion_through_a_consuming_prefix_terminates() {
        let source = "start = \"(\" start \")\" / \"x\"\r\n";
        assert!(matches(source, "x"));
        assert!(matches(source, "(x)"));
        assert!(matches(source, "(((x)))"));
        assert!(!matches(source, "((x)"));
    }

    #[test]
    fn an_unknown_rule_is_an_error() {
        let grammar = checked("start = \"a\"\r\n");
        assert!(matches!(
            Recognizer::new(&grammar, "a").accepts("nope"),
            Err(MatchError::UnknownRule(name)) if name == "nope"
        ));
    }

    #[test]
    fn end_positions_can_start_anywhere() {
        let grammar = checked("start = \"b\"\r\n");
        let mut recognizer = Recognizer::new(&grammar, "ab");
        assert_eq!(
            recognizer.end_positions("start", 1).expect("recognizes"),
            BTreeSet::from([2])
        );
        assert!(
            recognizer
                .end_positions("start", 0)
                .expect("recognizes")
                .is_empty()
        );
    }

    // -- the depth limit ---------------------------------------------------------------------

    #[test]
    fn ordinary_nesting_is_well_within_the_default() {
        // The default has to be generous enough that normal input never meets it.
        let grammar = checked("start = \"(\" start \")\" / \"x\"\r\n");
        let input = format!("{}x{}", "(".repeat(30), ")".repeat(30));
        assert!(
            Recognizer::new(&grammar, &input)
                .accepts("start")
                .expect("recognizes")
        );
    }

    #[test]
    fn deeper_than_the_default_is_an_error_not_a_rejection() {
        // The whole point of the limit: an answer the caller can act on, rather than a stack
        // overflow, which aborts the process and cannot be caught.
        let grammar = checked("start = \"(\" start \")\" / \"x\"\r\n");
        let input = format!("{}x{}", "(".repeat(5_000), ")".repeat(5_000));
        assert!(matches!(
            Recognizer::new(&grammar, &input).accepts("start"),
            Err(MatchError::DepthLimit)
        ));
    }

    #[test]
    fn the_limit_can_be_raised_or_removed() {
        let grammar = checked("start = \"(\" start \")\" / \"x\"\r\n");
        let input = format!("{}x{}", "(".repeat(200), ")".repeat(200));

        assert!(matches!(
            Recognizer::new(&grammar, &input)
                .with_options(MatchOptions {
                    max_depth: Some(16),
                    ..MatchOptions::default()
                })
                .accepts("start"),
            Err(MatchError::DepthLimit)
        ));
        assert!(
            Recognizer::new(&grammar, &input)
                .with_options(MatchOptions {
                    max_depth: Some(2_000),
                    ..MatchOptions::default()
                })
                .accepts("start")
                .expect("recognizes")
        );
        assert!(
            Recognizer::new(&grammar, &input)
                .with_options(MatchOptions {
                    max_depth: None,
                    ..MatchOptions::default()
                })
                .accepts("start")
                .expect("recognizes"),
            "None removes the limit"
        );
    }

    #[test]
    fn length_alone_costs_no_depth() {
        // Depth tracks nesting, not size: a flat input of any length stays shallow, which is
        // what makes a low default workable.
        let grammar = checked("start = *\"a\"\r\n");
        let input = "a".repeat(100_000);
        assert!(
            Recognizer::new(&grammar, &input)
                .accepts("start")
                .expect("recognizes")
        );
    }

    #[test]
    fn the_depth_limit_does_not_leak_between_calls() {
        // The counter is decremented on the way back up, including on the error path, so a
        // recognizer that hit the limit once still works afterwards.
        //
        // Asserted on the *same* recognizer, which is the point: an earlier version of this
        // test built a second one on different input, and so checked nothing about reuse.
        let grammar = checked("start = \"(\" start \")\" / \"x\"\r\n");
        // Four is enough to decide "x))" and not enough for "((x))": alternation is unordered,
        // so the deep branch is explored even when the shallow one would succeed.
        let mut recognizer = Recognizer::new(&grammar, "((x))").with_options(MatchOptions {
            max_depth: Some(4),
            ..MatchOptions::default()
        });
        assert!(matches!(
            recognizer.accepts("start"),
            Err(MatchError::DepthLimit)
        ));

        // Asking the same question again must give the same honest answer. Anything else --
        // `LeftRecursionDetected` in particular -- would be a verdict invented by leftover
        // state, on a grammar `check` has already proved is not left-recursive.
        match recognizer.accepts("start") {
            Err(MatchError::DepthLimit) => {}
            other => panic!("a repeated question should get a repeated answer, got {other:?}"),
        }

        // And the recognizer is still usable for work it *can* do: from position 2 the input
        // is "x))", which needs almost no depth. This is the key the failed descent left
        // half-written, so it only decides if the table was repaired on the way out.
        let ends = recognizer
            .end_positions("start", 2)
            .expect("usable after a depth limit");
        assert!(
            ends.contains(&3),
            "expected to match \"x\" at 2, got {ends:?}"
        );
    }

    #[test]
    fn a_step_limit_does_not_poison_the_memo() {
        // `match_rule_ref` writes `Memo::InProgress` before descending. If a limit unwinds past
        // the `Done` insertion, that marker outlives the call, and the next one reads it as
        // left recursion -- a wrong answer rather than a limit.
        let grammar = checked("start = a a a\r\na = b b\r\nb = \"x\" / \"y\"\r\n");
        let mut recognizer = Recognizer::new(&grammar, "xxxxxx").with_options(MatchOptions {
            max_steps: Some(3),
            ..MatchOptions::default()
        });

        assert!(matches!(
            recognizer.accepts("start"),
            Err(MatchError::StepLimit)
        ));

        // The step budget is spent and is not reset per call, so refusing again is correct.
        // Refusing *for the wrong reason* is the bug.
        match recognizer.accepts("start") {
            Err(MatchError::StepLimit) => {}
            other => panic!("expected the spent budget to persist, got {other:?}"),
        }
    }

    #[test]
    fn a_limit_deep_in_the_grammar_still_leaves_the_memo_whole() {
        // Not only the outermost rule: the limit can trip several frames down, and every frame
        // has to clean up as the error passes through it. Sweeping the budget walks the failure
        // point through each level of the chain in turn.
        let grammar = checked("start = a\r\na = b\r\nb = c\r\nc = d\r\nd = \"x\" / \"y\"\r\n");

        for budget in 1..=8 {
            let mut recognizer = Recognizer::new(&grammar, "x").with_options(MatchOptions {
                max_steps: Some(budget),
                ..MatchOptions::default()
            });
            let first = recognizer.accepts("start");
            let second = recognizer.accepts("start");

            // Whatever the first call decided, the second must agree with it. The two can only
            // differ if the first left something behind.
            assert_eq!(
                first, second,
                "budget {budget}: the answer changed when the question was repeated"
            );
            assert!(
                !matches!(second, Err(MatchError::LeftRecursionDetected { .. })),
                "budget {budget}: invented left recursion in a grammar that has none"
            );
        }
    }

    #[test]
    fn memoization_changes_the_work_and_not_the_answer() {
        // A grammar that revisits the same rule at the same position many times.
        let source = "start = a a a a\r\na = [\"x\"] [\"x\"] [\"x\"]\r\n";
        let grammar = checked(source);

        let mut memoized = Recognizer::new(&grammar, "xxxxxx");
        let with = memoized.accepts("start").expect("recognizes");

        let mut plain = Recognizer::new(&grammar, "xxxxxx").without_memoization();
        let without = plain.accepts("start").expect("recognizes");

        assert_eq!(with, without);
        assert!(
            memoized.steps() < plain.steps(),
            "memoized {} steps, plain {}",
            memoized.steps(),
            plain.steps()
        );
    }
}
