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
//! # Example
//!
//! ```
//! use abnf_oracle::{Generator, Grammar, Recognizer};
//!
//! // A grammar, as it would appear in an RFC.
//! let source = concat!(
//!     "full-date     = date-fullyear \"-\" date-month \"-\" date-mday\r\n",
//!     "date-fullyear = 4DIGIT\r\n",
//!     "date-month    = 2DIGIT\r\n",
//!     "date-mday     = 2DIGIT\r\n",
//! );
//!
//! // Parse, then check. `check` merges incremental definitions, resolves every name and runs
//! // the analyses; it is the only way to reach a recognizer or a generator.
//! let grammar = Grammar::parse(source)
//!     .expect("valid ABNF")
//!     .check()
//!     .expect("no structural errors");
//!
//! // Recognize.
//! assert!(Recognizer::new(&grammar, "2026-09-20").accepts("full-date").unwrap());
//! assert!(!Recognizer::new(&grammar, "20260920").accepts("full-date").unwrap());
//!
//! // Generate. The same seed always gives the same string.
//! let mut generator = Generator::new(&grammar, 42);
//! let produced = generator.generate("full-date").expect("generates");
//!
//! // What the generator produces, the recognizer accepts.
//! assert!(Recognizer::new(&grammar, &produced).accepts("full-date").unwrap());
//! ```
//!
//! # Errors are not rejections
//!
//! A limit, a prose value or an unrepresentable terminal makes a question *unanswerable*, which
//! is different from answering "no". Every such case is an `Err`, never `Ok(false)`, so a
//! caller can never mistake "could not decide" for "does not match".

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
pub use error::{CheckError, CheckErrors, GenError, LintWarning, MatchError, ParseError};
pub use generate::{
    DEFAULT_GEN_DEPTH, DEFAULT_MAX_OUTPUT_LEN, DEFAULT_MAX_STEPS, DEFAULT_SPREAD, GenOptions,
    Generator,
};
pub use recognize::{DEFAULT_MAX_DEPTH, MatchOptions, Recognizer};
