//! The grammar data model, in two layers (SCOPE.md 6.7, D12, D32).
//!
//! The **syntactic** layer — [`Grammar`], [`Definition`], [`Element`] — is what the parser
//! produces: the ordered list of definitions exactly as written, after local rewrites only. It
//! does not merge `=/`, resolve names, or carry node ids.
//!
//! The **semantic** layer — [`Rule`], [`Node`], [`NodeId`] — is the flat arena that `check`
//! lowers the merged rule table into. A [`NodeId`] *is* an index into that arena, so the
//! "pre-order traversal of the merged, normalized rule table" that SCOPE.md 6.7 requires of node
//! ids falls out of the order nodes are emitted in, with no separate numbering pass.
//!
//! Equality at both layers deliberately ignores provenance: spans never participate (see [`Ignored`])
//! and rule names compare ASCII-case-insensitively (see [`RuleName`]). Without the first,
//! `*1a` and `[a]` would not compare equal and the round-trip tests in M1 could not hold.

use core::fmt;
use core::hash::{Hash, Hasher};

/// A byte range in the grammar source text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    /// Byte offset of the first byte, inclusive.
    pub start: u32,
    /// Byte offset one past the last byte.
    pub end: u32,
}

impl Span {
    /// Creates a span covering `start..end`.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Returns the span as a range usable for slicing the source text.
    #[must_use]
    pub const fn range(self) -> core::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// A value that is carried but never compared: every `Ignored<T>` equals every other.
///
/// Used for spans. Two grammars that differ only in where their elements sit in the source text
/// are the same grammar — M1 requires `*1a` and `[a]` to parse to *equal* grammars, and those
/// necessarily have different spans. Wrapping the span rather than hand-writing `PartialEq` for
/// every type that carries one is what lets those types keep deriving it, so a field added later
/// cannot be silently left out of a comparison.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ignored<T>(
    /// The carried value.
    pub T,
);

impl<T> PartialEq for Ignored<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T> Eq for Ignored<T> {}

impl<T> Hash for Ignored<T> {
    fn hash<H: Hasher>(&self, _state: &mut H) {}
}

/// A rule name.
///
/// Rule names are ASCII-case-insensitive (RFC 5234 2.1), so comparison and hashing fold case
/// while the spelling as written is preserved for `Display`.
#[derive(Clone, Debug)]
pub struct RuleName(String);

impl RuleName {
    /// Creates a rule name from its spelling as written.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Returns the name as it was written in the grammar.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the ASCII-lowercased form used to key the rule table.
    #[must_use]
    pub fn key(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

impl PartialEq for RuleName {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl Eq for RuleName {}

impl Hash for RuleName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for byte in self.0.as_bytes() {
            state.write_u8(byte.to_ascii_lowercase());
        }
        // Terminator, so that ("ab", "c") and ("a", "bc") do not collide when several names are
        // hashed into the same state.
        state.write_u8(0xff);
    }
}

impl fmt::Display for RuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Syntactic options for parsing. Never semantic (SCOPE.md 4, D30).
///
/// Parse options are provenance: they are excluded from [`Grammar`] equality and are never
/// serialized by `Display`. Nothing that changes what a grammar *means* may be added here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// Require CRLF line endings, rather than accepting CRLF, LF or CR.
    pub strict_crlf: bool,
}

/// Which definition operator introduced a [`Definition`]: `defined-as` in RFC 5234 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DefinedAs {
    /// `name = elements`.
    Base,
    /// `name =/ elements`.
    Incremental,
}

/// One `name = elements` or `name =/ elements` definition, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// The rule name being defined.
    pub name: RuleName,
    /// Which operator was used.
    pub defined_as: DefinedAs,
    /// The right-hand side.
    pub body: Element,
    /// Where the definition sits in the source.
    pub span: Ignored<Span>,
}

/// Repetition bounds, inclusive on both ends.
///
/// `max` of `None` is unbounded. A sentinel is deliberately avoided: `*18446744073709551615` is
/// a legal bound and would collide with one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Repeat {
    /// Lower bound.
    pub min: u64,
    /// Upper bound, or `None` for unbounded.
    pub max: Option<u64>,
}

impl Repeat {
    /// `n<element>` — exactly `n` repetitions.
    #[must_use]
    pub const fn exactly(n: u64) -> Self {
        Self {
            min: n,
            max: Some(n),
        }
    }

    /// `n*<element>` — at least `n` repetitions, unbounded above.
    #[must_use]
    pub const fn at_least(n: u64) -> Self {
        Self { min: n, max: None }
    }

    /// `min*max<element>`.
    #[must_use]
    pub const fn bounded(min: u64, max: u64) -> Self {
        Self {
            min,
            max: Some(max),
        }
    }

    /// Whether the upper bound is unbounded.
    #[must_use]
    pub const fn is_unbounded(&self) -> bool {
        self.max.is_none()
    }

    /// Whether `min <= max`.
    ///
    /// `check` rejects the inverted case with `InvalidRepeatRange` (D18); the recognizer may
    /// assume this holds.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        match self.max {
            Some(max) => self.min <= max,
            None => true,
        }
    }

    /// Whether the body can never match, i.e. `max == 0`.
    ///
    /// Such a body contributes no first-graph edges and holds no coverage units
    /// (SCOPE.md 6.4, 6.8, D36).
    #[must_use]
    pub const fn is_never(&self) -> bool {
        matches!(self.max, Some(0))
    }
}

/// A quoted string terminal: `char-val` in RFC 5234 4.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CharVal {
    /// The characters between the quotes, as written.
    pub value: String,
    /// `true` for `%s"..."`, `false` for `"..."` and `%i"..."` (RFC 7405).
    pub case_sensitive: bool,
}

/// A numeric terminal, in any of the three radices. The radix is not retained: `%d65`,
/// `%b1000001` and `%x41` all parse to the same node and print as `%x41` (SCOPE.md 6.7, D37).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NumVal {
    /// A single value, e.g. `%x41`.
    Scalar(u64),
    /// An inclusive range, e.g. `%x41-5A`.
    Range {
        /// Lower endpoint.
        lo: u64,
        /// Upper endpoint.
        hi: u64,
    },
    /// A concatenation, e.g. `%x41.42.43` (Erratum 3076).
    Concat(Vec<u64>),
}

/// An element of a rule body, in the syntactic layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Element {
    /// `a / b / c`. Unordered (SCOPE.md 2, goal 2).
    Alt(Vec<Element>),
    /// `a b c`.
    Concat(Vec<Element>),
    /// `2*5a`.
    Repeat {
        /// The bounds.
        repeat: Repeat,
        /// The repeated element.
        body: Box<Element>,
    },
    /// `[a]`.
    ///
    /// Kept distinct from `Repeat { 0, Some(1) }` — which canonicalization rewrites into it —
    /// so that `Display` prints `[a]` without inspecting bounds, and so that the generator can
    /// treat it as the two-branch alternation `a / empty` for coverage.
    Optional(Box<Element>),
    /// A reference to another rule.
    RuleRef {
        /// The referenced name.
        name: RuleName,
        /// Where the reference sits in the source.
        span: Ignored<Span>,
    },
    /// A quoted string terminal.
    CharVal(CharVal),
    /// A numeric terminal.
    NumVal(NumVal),
    /// `<prose>`: parsed and preserved, never matched or generated (SCOPE.md 6.5).
    ProseVal {
        /// The text between the angle brackets.
        text: String,
        /// Where the prose value sits in the source.
        span: Ignored<Span>,
    },
}

/// A parsed grammar: the ordered definitions as written, after local rewrites only.
///
/// This layer knows nothing about rule resolution. `=/` is still separate from `=`, names are
/// unresolved, and there are no node ids; all of that is `check`'s job (D32). A `Grammar` never
/// carries hidden validity state — a grammar with a duplicate definition parses fine and fails
/// at `check`.
#[derive(Clone, Debug)]
pub struct Grammar {
    defs: Vec<Definition>,
    opts: ParseOptions,
}

impl Grammar {
    /// Creates a grammar from its definitions and the options it was parsed with.
    #[must_use]
    pub fn new(defs: Vec<Definition>, opts: ParseOptions) -> Self {
        Self { defs, opts }
    }

    /// The definitions, in source order, `=` and `=/` alike.
    #[must_use]
    pub fn definitions(&self) -> &[Definition] {
        &self.defs
    }

    /// The options this grammar was parsed with.
    ///
    /// Provenance only: excluded from equality, never serialized (D21).
    #[must_use]
    pub fn parse_options(&self) -> &ParseOptions {
        &self.opts
    }
}

impl PartialEq for Grammar {
    /// Compares the definition list only. Parse options are provenance and are ignored (D21).
    fn eq(&self, other: &Self) -> bool {
        self.defs == other.defs
    }
}

impl Eq for Grammar {}

/// An index into a checked grammar's node arena.
///
/// Assigned by pre-order traversal of the merged rule table, so two grammars with the same
/// canonical form have identical node ids (SCOPE.md 6.7, D22).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u32);

impl NodeId {
    /// Creates a node id from an arena index.
    #[must_use]
    pub const fn from_index(index: u32) -> Self {
        Self(index)
    }

    /// Returns the arena index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Identifies a rule in the merged table, distinguishing user rules from the implicit
/// core-rule environment.
///
/// The distinction is what makes core-rule resolution hygienic: a reference inside a core body
/// resolves against core rules only, so a user's `DIGIT = "x"` cannot silently redefine
/// `HEXDIG` (SCOPE.md 4.1, D33).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleId {
    /// A rule defined by the user grammar.
    User(u32),
    /// A rule from RFC 5234 Appendix B.
    Core(u32),
}

/// A rule in the merged table: `=/` folded into its base, body lowered into the arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The rule name, spelled as it was first defined.
    pub name: RuleName,
    /// The root node of the merged body.
    pub body: NodeId,
}

/// A node in the merged-grammar arena.
///
/// Mirrors [`Element`], except that children are [`NodeId`]s rather than boxes, and rule
/// references carry no resolution — resolution lives in a side table indexed by node id, so
/// that lowering needs no placeholder value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// `a / b / c`.
    Alt {
        /// The branches, in source order. Branch indices are part of coverage-unit identity.
        branches: Vec<NodeId>,
    },
    /// `a b c`.
    Concat {
        /// The items, in source order.
        items: Vec<NodeId>,
    },
    /// `2*5a`.
    Repeat {
        /// The bounds.
        repeat: Repeat,
        /// The repeated node.
        body: NodeId,
    },
    /// `[a]`, modeled as `a / empty` for coverage purposes.
    Optional {
        /// The optional node.
        body: NodeId,
    },
    /// A reference to another rule, resolved via the side table.
    RuleRef {
        /// The referenced name.
        name: RuleName,
        /// Where the reference sits in the source.
        span: Ignored<Span>,
    },
    /// A quoted string terminal.
    CharVal(CharVal),
    /// A numeric terminal.
    NumVal(NumVal),
    /// `<prose>`.
    ProseVal(String),
}

/// The length of the shortest string a node can match (SCOPE.md 6.4, D27).
///
/// `Finite` saturates at `u64::MAX`: nested repetitions can overflow even when every literal
/// bound fits in 64 bits, and a saturated value is still finite, hence still *productive*.
/// Ordering is the natural one — every `Finite` is less than `Infinite`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MinLen {
    /// A shortest match exists and is this long, saturating at `u64::MAX`.
    Finite(u64),
    /// The node matches nothing: it is unproductive.
    Infinite,
}

impl MinLen {
    /// The empty match.
    pub const ZERO: Self = Self::Finite(0);

    /// Whether a shortest match exists, i.e. whether the node is productive.
    #[must_use]
    pub const fn is_finite(self) -> bool {
        matches!(self, Self::Finite(_))
    }

    /// Concatenation: saturating addition, absorbing on `Infinite`.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        match (self, other) {
            (Self::Finite(a), Self::Finite(b)) => Self::Finite(a.saturating_add(b)),
            _ => Self::Infinite,
        }
    }

    /// Repetition: `count` copies of `self`, saturating.
    ///
    /// Zero copies match the empty string whatever the body is, so `Infinite * 0` is
    /// `Finite(0)` — which is why `*bad` is productive even when `bad` is not.
    #[must_use]
    pub const fn saturating_mul(self, count: u64) -> Self {
        if count == 0 {
            return Self::ZERO;
        }
        match self {
            Self::Finite(a) => Self::Finite(a.saturating_mul(count)),
            Self::Infinite => Self::Infinite,
        }
    }

    /// Alternation: the shorter of two alternatives.
    #[must_use]
    pub const fn min(self, other: Self) -> Self {
        match (self, other) {
            (Self::Finite(a), Self::Finite(b)) => Self::Finite(if a < b { a } else { b }),
            (Self::Finite(a), Self::Infinite) => Self::Finite(a),
            (Self::Infinite, other) => other,
        }
    }
}

/// The choice a node's shortest derivation makes (SCOPE.md 6.4, D28).
///
/// Recorded by `check` when a node's `min_len` is finalized, so every witness points at a node
/// that was finalized earlier. Following witnesses therefore terminates, which is what the
/// generator relies on when its depth budget runs out. Comparing `min_len` magnitudes is not a
/// substitute: ties can cycle and saturated values compare meaninglessly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Witness {
    /// For an alternation or optional: the branch index that attained the shortest match.
    Branch(u32),
    /// For a repetition: the count, which is always `min`.
    Count(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_names_compare_case_insensitively() {
        assert_eq!(RuleName::new("Foo"), RuleName::new("fOO"));
        assert_ne!(RuleName::new("foo"), RuleName::new("foobar"));
        assert_eq!(RuleName::new("Foo").as_str(), "Foo");
        assert_eq!(RuleName::new("Foo").key(), "foo");
    }

    #[test]
    fn rule_name_hash_agrees_with_eq() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(RuleName::new("DIGIT"));
        assert!(set.contains(&RuleName::new("digit")));
    }

    #[test]
    fn spans_never_affect_equality() {
        let a = Element::RuleRef {
            name: RuleName::new("x"),
            span: Ignored(Span::new(0, 1)),
        };
        let b = Element::RuleRef {
            name: RuleName::new("X"),
            span: Ignored(Span::new(40, 41)),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn parse_options_do_not_affect_grammar_equality() {
        let def = Definition {
            name: RuleName::new("a"),
            defined_as: DefinedAs::Base,
            body: Element::CharVal(CharVal {
                value: "x".into(),
                case_sensitive: false,
            }),
            span: Ignored(Span::default()),
        };
        let lenient = Grammar::new(vec![def.clone()], ParseOptions::default());
        let strict = Grammar::new(vec![def], ParseOptions { strict_crlf: true });
        assert_eq!(lenient, strict);
    }

    #[test]
    fn repeat_bounds() {
        assert!(Repeat::exactly(3).is_valid());
        assert!(Repeat::at_least(3).is_valid());
        assert!(Repeat::at_least(3).is_unbounded());
        assert!(!Repeat::bounded(5, 2).is_valid());
        assert!(Repeat::bounded(0, 0).is_never());
        assert!(!Repeat::bounded(0, 1).is_never());
    }

    #[test]
    fn min_len_orders_finite_below_infinite() {
        assert!(MinLen::Finite(u64::MAX) < MinLen::Infinite);
        assert!(MinLen::Finite(1) < MinLen::Finite(2));
        assert!(MinLen::ZERO.is_finite());
        assert!(!MinLen::Infinite.is_finite());
    }

    #[test]
    fn min_len_arithmetic_saturates() {
        assert_eq!(
            MinLen::Finite(2).saturating_add(MinLen::Finite(3)),
            MinLen::Finite(5)
        );
        assert_eq!(
            MinLen::Finite(u64::MAX).saturating_add(MinLen::Finite(1)),
            MinLen::Finite(u64::MAX)
        );
        assert_eq!(
            MinLen::Finite(1).saturating_add(MinLen::Infinite),
            MinLen::Infinite
        );
        assert_eq!(
            MinLen::Finite(u64::MAX).saturating_mul(2),
            MinLen::Finite(u64::MAX),
            "a saturated value is still finite, hence still productive"
        );
    }

    #[test]
    fn zero_repetitions_of_an_unproductive_body_match_the_empty_string() {
        // `*bad` is productive even when `bad` is not: zero copies match the empty string.
        assert_eq!(MinLen::Infinite.saturating_mul(0), MinLen::ZERO);
        assert_eq!(MinLen::Infinite.saturating_mul(1), MinLen::Infinite);
    }

    #[test]
    fn min_picks_the_shorter_alternative() {
        assert_eq!(MinLen::Finite(3).min(MinLen::Finite(1)), MinLen::Finite(1));
        assert_eq!(MinLen::Infinite.min(MinLen::Finite(7)), MinLen::Finite(7));
        assert_eq!(MinLen::Finite(7).min(MinLen::Infinite), MinLen::Finite(7));
        assert_eq!(MinLen::Infinite.min(MinLen::Infinite), MinLen::Infinite);
    }
}
