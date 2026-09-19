//! Grammar text to [`Grammar`](crate::Grammar): hand-written recursive descent.
//!
//! Syntax only. Local rewrites that need no knowledge of other rules are applied here (`*1a` to
//! `[a]`, `1*1a` to `a`, `0*n` to `*n`, numeric spelling, redundant groups); merging `=/`,
//! resolving names and assigning node ids are `check`'s job (SCOPE.md 6.7, D32).
//!
//! The function set mirrors the productions of the canonical self-grammar one for one, so that
//! "the parser accepts exactly the language of the canonical self-grammar" (SCOPE.md 4.2, D35)
//! can be audited by reading this file against `tests/grammars/`.
//!
//! Lands in M1.1 and M1.2.
