//! The deterministic generator, with coverage mode (SCOPE.md 6.8).
//!
//! A walk over the arena with a depth budget. In coverage mode the walk steers: it prefers an
//! uncovered branch, else the branch with the smallest distance to an uncovered coverage unit,
//! else random. That distance is a well-founded measure, which is what makes the chase
//! terminate and what makes the coverage guarantee independent of `max_depth` (D38). When the
//! budget runs out with nothing left to chase, the walk follows witnesses instead, which
//! terminates for the same reason (D28).
//!
//! Everything coverage-related is computed over the *generatable* graph: no branch with
//! infinite `min_len`, no `max == 0` repetition body, and nothing nested inside either.
//!
//! Lands in M3.
