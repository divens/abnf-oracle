//! [`Grammar`](crate::Grammar) to `CheckedGrammar`: the rule table, and the analyses on it.
//!
//! In order: build the merged rule table (fold `=/` into its base, reject duplicates and
//! orphans), lower it into the node arena assigning node ids, resolve names against the user
//! table then the core environment, validate ranges, then run the analyses — `nullable`,
//! `min_len` with its witness, the first-graph and its cycles, and the per-rule prose and
//! representability flags.
//!
//! Only *structural* problems are errors here. Compatibility limits are recorded per rule and
//! enforced per start rule; lint warnings never fail a check (SCOPE.md 6.6, D5).
//!
//! Errors accumulate: a grammar with four undefined references reports four, not one.
//!
//! Lands in M1.4 through M1.7.
