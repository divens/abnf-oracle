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
//! Coverage mode lands in M3.2.

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

    /// Steer towards uncovered branches rather than choosing randomly. Lands in M3.2.
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
        self.walk(body, 0, &mut out)?;
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
                let count = self.choose_count(repeat.min, repeat.max, depth);
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

    /// Picks a branch of an alternation.
    fn choose_branch(&mut self, node: NodeId, branches: &[NodeId], depth: usize) -> usize {
        // Never a branch that matches nothing, in any mode (D20).
        let usable: Vec<usize> = (0..branches.len())
            .filter(|index| self.grammar.min_len(branches[*index]).is_finite())
            .collect();
        debug_assert!(
            !usable.is_empty(),
            "a productive alternation has a productive branch"
        );

        if depth >= self.options.max_depth {
            // Witness mode: the shortest derivation this node knows, which terminates because
            // witnesses were recorded in the order `min_len` was settled (D28).
            if let Some(Witness::Branch(branch)) = self.grammar.witness(node) {
                return branch as usize;
            }
        }
        let pick = self.rng.below(usable.len() as u64) as usize;
        usable[pick]
    }

    /// Picks between taking an optional's body and skipping it.
    fn choose_branch_of_optional(&mut self, node: NodeId, body: NodeId, depth: usize) -> usize {
        if !self.grammar.min_len(body).is_finite() {
            return 1; // skipping is always available, and is all that is left
        }
        if depth >= self.options.max_depth {
            // The witness of an optional is always the empty arm: it is both shortest and the
            // one that descends into nothing.
            return match self.grammar.witness(node) {
                Some(Witness::Branch(branch)) => branch as usize,
                _ => 1,
            };
        }
        usize::from(!self.rng.bool())
    }

    /// Picks how many times to repeat.
    fn choose_count(&mut self, min: u64, max: Option<u64>, depth: usize) -> u64 {
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
