//! RFC 5234 Appendix B.1 core rules, as an implicit grammar.
//!
//! Always available, never configurable in v1 (SCOPE.md 4, 12 item 8). The rules are kept as
//! ABNF source and parsed by this crate's own parser rather than hand-built as an AST, so the
//! core environment goes through exactly the same code path as any other grammar.
//!
//! Resolution is hygienic: a reference inside a core rule body resolves against core rules
//! only, so a user's `DIGIT = "x"` changes what *their* `DIGIT` means without silently
//! redefining `HEXDIG`, `LWSP` and everything downstream (SCOPE.md 4.1, D33). That is `check`'s
//! job; this module only supplies the environment.

// Consumed by `check`, which lands in M1.4. Remove this when it does.
#![allow(dead_code)]

use std::sync::OnceLock;

use crate::ast::Grammar;

/// The number of rules in RFC 5234 Appendix B.1.
pub(crate) const CORE_RULE_COUNT: usize = 16;

/// RFC 5234 Appendix B.1, verbatim.
///
/// Duplicated in `tests/grammars/rfc5234-core.abnf`, which the fixture suite parses and M2's
/// self-recognition test runs the canonical self-grammar over. A test below asserts the two
/// have not drifted.
pub(crate) const CORE_RULES: &str = r#"; RFC 5234 Appendix B.1 - Core Rules
; Transcribed from https://www.rfc-editor.org/rfc/rfc5234.txt
; Errata applied: none (no verified erratum touches Appendix B)

ALPHA          =  %x41-5A / %x61-7A   ; A-Z / a-z

BIT            =  "0" / "1"

CHAR           =  %x01-7F
                       ; any 7-bit US-ASCII character,
                       ;  excluding NUL
CR             =  %x0D
                       ; carriage return

CRLF           =  CR LF
                       ; Internet standard newline

CTL            =  %x00-1F / %x7F
                       ; controls

DIGIT          =  %x30-39
                       ; 0-9

DQUOTE         =  %x22
                       ; " (Double Quote)

HEXDIG         =  DIGIT / "A" / "B" / "C" / "D" / "E" / "F"

HTAB           =  %x09
                       ; horizontal tab

LF             =  %x0A
                       ; linefeed

LWSP           =  *(WSP / CRLF WSP)
                       ; Use of this linear-white-space rule
                       ;  permits lines containing only white
                       ;  space that are no longer legal in
                       ;  mail headers and have caused
                       ;  interoperability problems in other
                       ;  contexts.
                       ; Do not use when defining mail
                       ;  headers and use with caution in
                       ;  other contexts.

OCTET          =  %x00-FF
                       ; 8 bits of data

SP             =  %x20

VCHAR          =  %x21-7E
                       ; visible (printing) characters

WSP            =  SP / HTAB
                       ; white space
"#;

/// The core-rule environment, parsed once.
pub(crate) fn core_grammar() -> &'static Grammar {
    static CORE: OnceLock<Grammar> = OnceLock::new();
    CORE.get_or_init(|| {
        Grammar::parse(CORE_RULES).expect("the core rules are a constant and must parse")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_core_rules_parse_to_appendix_b() {
        let grammar = core_grammar();
        let names: Vec<&str> = grammar
            .definitions()
            .iter()
            .map(|definition| definition.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "ALPHA", "BIT", "CHAR", "CR", "CRLF", "CTL", "DIGIT", "DQUOTE", "HEXDIG", "HTAB",
                "LF", "LWSP", "OCTET", "SP", "VCHAR", "WSP"
            ]
        );
        assert_eq!(names.len(), CORE_RULE_COUNT);
    }

    #[test]
    fn the_constant_matches_the_fixture() {
        // Two copies exist because the library must not depend on a test fixture and the
        // fixture must be a real file for M2. This is what keeps them the same text.
        let fixture = include_str!("../tests/grammars/rfc5234-core.abnf");
        // Compared line by line: `lines` strips a trailing CR, so a checkout that landed the
        // fixture with CRLF does not fail this for the wrong reason.
        assert!(
            CORE_RULES.lines().eq(fixture.lines()),
            "src/core_rules.rs and tests/grammars/rfc5234-core.abnf have drifted"
        );
    }

    #[test]
    fn lwsp_is_the_one_nullable_core_rule() {
        // Worth pinning: LWSP is defined through CRLF and matches the empty string, which makes
        // it the core rule that exercises nullable repetition in real fixtures (SCOPE.md 6.4).
        assert!(CORE_RULES.contains("LWSP           =  *(WSP / CRLF WSP)"));
    }
}
