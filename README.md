# abnf-oracle

A small, dependency-light Rust crate that parses ABNF grammars (RFC 5234, RFC 7405),
decides whether an input matches a rule, and generates random inputs that do.

Built to be a **testing oracle** for projects that ship an ABNF grammar as the
formalization of their language — correctness over speed, accept/reject over parse
trees, simple code over clever code.

## Status

Under construction. The data model and skeleton (M0) are in place; the parser and
checker (M1), recognizer (M2) and generator (M3) are not yet written. See `SCOPE.md`
for the specification and `PLAN.md` for the build order.

## What this is not

Deliberately out of scope, and not planned: parse trees, ambiguity detection, PEG or
regex semantics, streaming, byte-oriented matching, grammar transformations, and a
parser generator. There are no performance guarantees — pathological grammars are
bounded by explicit step limits, not by complexity promises. `SCOPE.md` section 3 has
the full list and the reasoning.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
