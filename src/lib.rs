//! ABNF grammars as a testing oracle.
//!
//! Parses ABNF grammars (RFC 5234 as amended by RFC 7405 and Errata 2968 and 3076), decides
//! whether an input string is in the language of a rule, and generates strings that are.
//!
//! The emphasis is on being *right*, not fast. Alternation is unordered, backtracking is full,
//! ambiguity is tolerated, and the recognizer computes the set of all end positions an element
//! can reach rather than committing to the first branch that matches. What this crate is
//! deliberately **not** is listed in `SCOPE.md` 3: no parse trees, no ambiguity detection, no
//! PEG or regex semantics, no byte-oriented matching, and no performance promises.
//!
//! # Shape of the API
//!
//! Three types, used in order:
//!
//! 1. [`Grammar`] — the syntactic layer. What the parser produces: definitions as written.
//! 2. `CheckedGrammar` — the semantic layer, produced by `Grammar::check`. Incremental
//!    alternatives are merged, names are resolved, and the analyses the recognizer and
//!    generator rely on have been run. It is the only route to either of them, so there is no
//!    way to recognize against a grammar whose structure was never validated.
//! 3. `Recognizer` / `Generator` — bound to a checked grammar and a start rule.
//!
//! # Status
//!
//! Under construction. M0 (skeleton and data model) is in place; parsing, checking, recognition
//! and generation land in M1–M3. See `PLAN.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod ast;
mod check;
mod core_rules;
mod display;
mod error;
mod generate;
mod lint;
mod parse;
mod recognize;
mod rng;

pub use ast::{
    CharVal, DefinedAs, Definition, Element, Grammar, Ignored, MinLen, Node, NodeId, NumVal,
    ParseOptions, Repeat, Rule, RuleId, RuleName, Span, Witness,
};
pub use check::CheckedGrammar;
pub use error::{CheckError, GenError, LintWarning, MatchError, ParseError};
