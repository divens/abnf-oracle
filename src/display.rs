//! Canonical serialization (SCOPE.md 6.7).
//!
//! One definition per line, single spaces, CRLF endings, `%x` for every numeric value. The
//! point is not tidiness: `Grammar::parse(g.to_string()) == g` is an M1 acceptance criterion,
//! so this module and [`crate::Grammar`]'s `PartialEq` have to agree about what a grammar *is*.
//! Anything the printer drops must be something equality ignores, and the round-trip tests are
//! what hold the two together.
//!
//! Comments are not preserved, at either layer: the model is semantic (SCOPE.md 12, item 2).
//!
//! # Parentheses
//!
//! The printer emits a group exactly when dropping it would change the parse, which is the
//! other half of "redundant groups are dropped" in the canonicalization pass. An alternation
//! needs parentheses inside a concatenation or a repetition; a concatenation needs them inside
//! a repetition, since `*a b` is `(*a) b`; a repetition needs them inside a repetition, since
//! ABNF has no `**a`. Everything else — a concatenation in an alternation branch, anything
//! inside `[...]` — stands bare.

use core::fmt;

use crate::ast::{CharVal, DefinedAs, Definition, Element, Grammar, NumVal, Repeat};
use crate::check::CheckedGrammar;

impl fmt::Display for Grammar {
    /// Writes the grammar in canonical form: the definitions as written, one per line.
    ///
    /// `=/` is *not* merged here. That happens in `check`, and `CheckedGrammar` has its own
    /// canonical form (D12).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.definitions().is_empty() {
            // A grammar with no rules is legal — `rulelist` is satisfied by blank lines alone —
            // but the empty string is not, because `rulelist` requires at least one item. A
            // single blank line is the only text ABNF has for "no rules", and printing nothing
            // here would break the round-trip on the one grammar that has nothing to print.
            return f.write_str("\r\n");
        }
        for definition in self.definitions() {
            write!(f, "{definition}")?;
            // Every line is terminated, including the last: `rule` ends in `c-nl`, so a grammar
            // without a final line ending would not parse back.
            f.write_str("\r\n")?;
        }
        Ok(())
    }
}

impl fmt::Display for CheckedGrammar {
    /// Writes the merged rule table: one rule per line, `=/` folded in, in first-definition
    /// order. Implicit core rules are never printed — they are the resolution environment, not
    /// part of the grammar (D12).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.rules().is_empty() {
            return f.write_str("\r\n");
        }
        for rule in self.rules() {
            // Rebuilt from the arena rather than printed by a second, arena-shaped printer:
            // one canonical form should have exactly one implementation (PLAN.md 3.10).
            write!(f, "{} = ", rule.name)?;
            write_element(f, &self.element_at(rule.body), Context::Free)?;
            f.write_str("\r\n")?;
        }
        Ok(())
    }
}

impl fmt::Display for Definition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operator = match self.defined_as {
            DefinedAs::Base => "=",
            DefinedAs::Incremental => "=/",
        };
        write!(f, "{} {operator} ", self.name)?;
        write_element(f, &self.body, Context::Free)
    }
}

impl fmt::Display for Element {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_element(f, self, Context::Free)
    }
}

/// Where an element sits, which decides whether it needs parentheses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    /// A rule body, an alternation branch, or inside `(...)` or `[...]`: nothing needs
    /// parentheses, because the surrounding construct already delimits it.
    Free,
    /// An item of a concatenation.
    Concat,
    /// The body of a repetition, which the grammar restricts to a single `element`.
    Repeat,
}

fn needs_parens(element: &Element, context: Context) -> bool {
    match context {
        Context::Free => false,
        Context::Concat => matches!(element, Element::Alt(_)),
        Context::Repeat => matches!(
            element,
            Element::Alt(_) | Element::Concat(_) | Element::Repeat { .. }
        ),
    }
}

fn write_element(f: &mut fmt::Formatter<'_>, element: &Element, context: Context) -> fmt::Result {
    if needs_parens(element, context) {
        f.write_str("(")?;
        write_element(f, element, Context::Free)?;
        return f.write_str(")");
    }

    match element {
        Element::Alt(branches) => write_joined(f, branches, " / ", Context::Free),
        Element::Concat(items) => write_joined(f, items, " ", Context::Concat),
        Element::Repeat { repeat, body } => {
            write_repeat(f, *repeat)?;
            write_element(f, body, Context::Repeat)
        }
        Element::Optional(body) => {
            f.write_str("[")?;
            write_element(f, body, Context::Free)?;
            f.write_str("]")
        }
        Element::RuleRef { name, .. } => write!(f, "{name}"),
        Element::CharVal(char_val) => write_char_val(f, char_val),
        Element::NumVal(num_val) => write_num_val(f, num_val),
        Element::ProseVal { text, .. } => write!(f, "<{text}>"),
    }
}

fn write_joined(
    f: &mut fmt::Formatter<'_>,
    elements: &[Element],
    separator: &str,
    context: Context,
) -> fmt::Result {
    for (index, element) in elements.iter().enumerate() {
        if index > 0 {
            f.write_str(separator)?;
        }
        write_element(f, element, context)?;
    }
    Ok(())
}

/// Writes repetition bounds, never emitting a form canonicalization has ruled out: not `0*n`,
/// not `*1a` (that is `[a]`), not `1*1a` (that is `a`).
fn write_repeat(f: &mut fmt::Formatter<'_>, repeat: Repeat) -> fmt::Result {
    match (repeat.min, repeat.max) {
        (0, None) => f.write_str("*"),
        (min, None) => write!(f, "{min}*"),
        (0, Some(max)) => write!(f, "*{max}"),
        (min, Some(max)) if min == max => write!(f, "{min}"),
        (min, Some(max)) => write!(f, "{min}*{max}"),
    }
}

fn write_char_val(f: &mut fmt::Formatter<'_>, char_val: &CharVal) -> fmt::Result {
    // `%i"abc"` is never emitted: the bare form means the same thing and is what RFC 5234
    // grammars actually use.
    if char_val.case_sensitive {
        f.write_str("%s")?;
    }
    write!(f, "\"{}\"", char_val.value)
}

fn write_num_val(f: &mut fmt::Formatter<'_>, num_val: &NumVal) -> fmt::Result {
    f.write_str("%x")?;
    match num_val {
        NumVal::Scalar(value) => write_hex(f, *value),
        NumVal::Range { lo, hi } => {
            write_hex(f, *lo)?;
            f.write_str("-")?;
            write_hex(f, *hi)
        }
        NumVal::Concat(values) => {
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    f.write_str(".")?;
                }
                write_hex(f, *value)?;
            }
            Ok(())
        }
    }
}

/// Uppercase hex, zero-padded to an even number of digits: `%x0D`, `%x41`, `%x0100`.
fn write_hex(f: &mut fmt::Formatter<'_>, value: u64) -> fmt::Result {
    let digits = format!("{value:X}");
    if digits.len() % 2 == 1 {
        f.write_str("0")?;
    }
    f.write_str(&digits)
}

#[cfg(test)]
mod tests {
    use crate::ast::{Grammar, ParseOptions};

    /// The canonical form of a rule body, with the `x = ` and the line ending stripped.
    fn canon(elements: &str) -> String {
        let grammar = Grammar::parse(&format!("x = {elements}\r\n"))
            .unwrap_or_else(|error| panic!("{elements:?} should parse: {error}"));
        let printed = grammar.to_string();
        printed
            .strip_prefix("x = ")
            .and_then(|rest| rest.strip_suffix("\r\n"))
            .unwrap_or_else(|| panic!("unexpected shape: {printed:?}"))
            .to_owned()
    }

    /// Asserts both halves of the M1 round-trip criterion: reparsing the canonical form gives
    /// an equal grammar, and printing it again gives identical text.
    fn round_trip(src: &str) {
        let original = Grammar::parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}"));
        let printed = original.to_string();
        let reparsed = Grammar::parse(&printed).unwrap_or_else(|e| {
            panic!("canonical form of {src:?} did not parse: {e}\n{printed:?}")
        });

        assert_eq!(
            original, reparsed,
            "round-trip changed {src:?} via {printed:?}"
        );
        assert_eq!(
            printed,
            reparsed.to_string(),
            "canonical form is not a fixpoint for {src:?}"
        );
    }

    // -- the 6.7 spelling table, row by row -------------------------------------------------

    #[test]
    fn numeric_values_print_as_even_padded_uppercase_hex() {
        assert_eq!(canon("%d13"), "%x0D");
        assert_eq!(canon("%x41"), "%x41");
        assert_eq!(canon("%x100"), "%x0100");
        assert_eq!(canon("%x10FFFF"), "%x10FFFF");
        assert_eq!(
            canon("%b1000001"),
            "%x41",
            "the radix as written is not retained"
        );
        assert_eq!(canon("%x0"), "%x00");
    }

    #[test]
    fn numeric_ranges_and_concatenations() {
        assert_eq!(canon("%x41-5A"), "%x41-5A");
        assert_eq!(canon("%d65-90"), "%x41-5A");
        assert_eq!(canon("%d65.66.67"), "%x41.42.43");
        assert_eq!(canon("%d13.10"), "%x0D.0A");
    }

    #[test]
    fn quoted_strings() {
        assert_eq!(canon("\"abc\""), "\"abc\"");
        assert_eq!(canon("%i\"abc\""), "\"abc\"", "%i is never emitted");
        assert_eq!(canon("%s\"abc\""), "%s\"abc\"");
        assert_eq!(canon("%S\"abc\""), "%s\"abc\"");
        assert_eq!(canon("\"\""), "\"\"");
    }

    #[test]
    fn repetition_bounds() {
        assert_eq!(canon("*\"a\""), "*\"a\"");
        assert_eq!(canon("3\"a\""), "3\"a\"");
        assert_eq!(canon("2*5\"a\""), "2*5\"a\"");
        assert_eq!(canon("*3\"a\""), "*3\"a\"");
        assert_eq!(canon("3*\"a\""), "3*\"a\"");
        assert_eq!(canon("0*3\"a\""), "*3\"a\"", "never 0*");
        assert_eq!(canon("0*\"a\""), "*\"a\"");
    }

    #[test]
    fn optional_spellings_collapse_to_one() {
        assert_eq!(canon("[\"a\"]"), "[\"a\"]");
        assert_eq!(canon("*1\"a\""), "[\"a\"]");
        assert_eq!(canon("0*1\"a\""), "[\"a\"]");
        assert_eq!(canon("1*1\"a\""), "\"a\"");
        assert_eq!(canon("1\"a\""), "\"a\"", "1a is a bare element");
    }

    #[test]
    fn parentheses_appear_exactly_where_they_change_the_parse() {
        // Needed: an alternation inside a concatenation or a repetition.
        assert_eq!(canon("(\"a\" / \"b\") \"c\""), "(\"a\" / \"b\") \"c\"");
        assert_eq!(canon("\"a\" (\"b\" / \"c\")"), "\"a\" (\"b\" / \"c\")");
        assert_eq!(canon("*(\"a\" / \"b\")"), "*(\"a\" / \"b\")");
        // Needed: a concatenation inside a repetition, since `*a b` is `(*a) b`.
        assert_eq!(canon("*(\"a\" \"b\")"), "*(\"a\" \"b\")");
        // Needed: a repetition inside a repetition, since ABNF has no `**a`.
        assert_eq!(canon("*(2*\"a\")"), "*(2*\"a\")");
        // Not needed: a concatenation in an alternation branch.
        assert_eq!(canon("\"a\" \"b\" / \"c\""), "\"a\" \"b\" / \"c\"");
        // Not needed: anything inside brackets.
        assert_eq!(canon("[\"a\" / \"b\"]"), "[\"a\" / \"b\"]");
        assert_eq!(canon("*[\"a\" \"b\"]"), "*[\"a\" \"b\"]");
    }

    #[test]
    fn redundant_groups_are_dropped() {
        assert_eq!(canon("(\"a\")"), "\"a\"");
        assert_eq!(canon("((\"a\"))"), "\"a\"");
        assert_eq!(canon("\"a\" (\"b\" \"c\")"), "\"a\" \"b\" \"c\"");
        assert_eq!(canon("\"a\" / (\"b\" / \"c\")"), "\"a\" / \"b\" / \"c\"");
        assert_eq!(canon("(\"a\" \"b\") \"c\""), "\"a\" \"b\" \"c\"");
    }

    #[test]
    fn whitespace_is_normalized() {
        assert_eq!(canon("\"a\"     /\t\"b\""), "\"a\" / \"b\"");
        assert_eq!(canon("\"a\"  \"b\""), "\"a\" \"b\"");
    }

    #[test]
    fn definition_operators_and_line_endings() {
        let grammar = Grammar::parse("foo=\"a\"\nfoo=/\"b\"\n").expect("parses");
        assert_eq!(grammar.to_string(), "foo = \"a\"\r\nfoo =/ \"b\"\r\n");
    }

    #[test]
    fn comments_are_not_preserved() {
        let grammar = Grammar::parse("; header\r\nfoo = \"a\" ; why\r\n").expect("parses");
        assert_eq!(grammar.to_string(), "foo = \"a\"\r\n");
    }

    #[test]
    fn prose_and_rule_references_survive_verbatim() {
        assert_eq!(canon("<any token>"), "<any token>");
        assert_eq!(canon("ALPHA"), "ALPHA");
        assert_eq!(canon("a1-b2"), "a1-b2", "rule names print as first written");
    }

    // -- round-trip --------------------------------------------------------------------------

    #[test]
    fn round_trips_through_the_canonical_form() {
        for src in [
            "foo = \"a\"\r\n",
            "foo = \"a\" / \"b\" / \"c\"\r\n",
            "foo = \"a\" \"b\"\r\n",
            "foo = (\"a\" / \"b\") \"c\"\r\n",
            "foo = [\"a\"]\r\n",
            "foo = *\"a\"\r\n",
            "foo = 2*5(\"a\" / \"b\")\r\n",
            "foo = %x41-5A\r\n",
            "foo = %d13.10\r\n",
            "foo = %s\"aBc\"\r\n",
            "foo = <prose>\r\n",
            "foo = ALPHA DIGIT\r\n",
            "foo = \"a\"\r\nfoo =/ \"b\"\r\n",
            "; comment\r\nfoo = \"a\"  ; trailing\r\n\r\nbar = foo\r\n",
            "foo = \"a\"\r\n      \"b\"\r\n",
            "foo = *(2*3\"a\")\r\n",
            "\r\n",
        ] {
            round_trip(src);
        }
    }

    #[test]
    fn the_empty_grammar_prints_as_a_blank_line() {
        // `rulelist` needs at least one item, so the empty string is not a grammar even though
        // a grammar with no rules is perfectly legal.
        let empty = Grammar::parse("\r\n").expect("parses");
        assert!(empty.definitions().is_empty());
        assert_eq!(empty.to_string(), "\r\n");
        assert_eq!(Grammar::parse(&empty.to_string()).expect("reparses"), empty);
    }

    #[test]
    fn round_trips_under_strict_crlf() {
        // Parse options are provenance and do not affect equality (D21); the canonical form is
        // CRLF, so it parses back under the stricter setting too.
        let strict = ParseOptions { strict_crlf: true };
        let src = "foo = \"a\" / [\"b\"]\r\nfoo =/ %x41\r\n";
        let original = Grammar::parse_with(src, strict.clone()).expect("parses");
        let reparsed = Grammar::parse_with(&original.to_string(), strict).expect("reparses");
        assert_eq!(original, reparsed);
    }

    #[test]
    fn lenient_and_strict_parses_of_the_same_grammar_are_equal() {
        let lenient = Grammar::parse("foo = \"a\"\n").expect("parses");
        let strict = Grammar::parse_with("foo = \"a\"\r\n", ParseOptions { strict_crlf: true })
            .expect("parses");
        assert_eq!(lenient, strict);
        assert_eq!(lenient.to_string(), strict.to_string());
    }

    #[test]
    fn spellings_that_differ_only_syntactically_are_equal() {
        for (left, right) in [
            ("foo = *1\"a\"\r\n", "foo = [\"a\"]\r\n"),
            ("foo = 1*1\"a\"\r\n", "foo = \"a\"\r\n"),
            ("foo = %d65\r\n", "foo = %x41\r\n"),
            ("foo = %i\"a\"\r\n", "foo = \"a\"\r\n"),
            ("foo = (\"a\")\r\n", "foo = \"a\"\r\n"),
            (
                "foo = \"a\" / (\"b\" / \"c\")\r\n",
                "foo = \"a\" / \"b\" / \"c\"\r\n",
            ),
            ("foo = \"a\"\r\n", "FOO = \"a\"\r\n"),
        ] {
            let left_grammar = Grammar::parse(left).expect("parses");
            let right_grammar = Grammar::parse(right).expect("parses");
            assert_eq!(left_grammar, right_grammar, "{left:?} vs {right:?}");
        }
    }

    #[test]
    fn spellings_that_differ_semantically_are_not_equal() {
        for (left, right) in [
            ("foo = \"a\" \"b\"\r\n", "foo = \"a\" / \"b\"\r\n"),
            ("foo = %s\"a\"\r\n", "foo = \"a\"\r\n"),
            ("foo = *\"a\"\r\n", "foo = 1*\"a\"\r\n"),
            ("foo = \"a\"\r\n", "foo =/ \"a\"\r\n"),
            ("foo = \"a\"\r\n", "bar = \"a\"\r\n"),
            ("foo = \"a\"\r\n", "foo = \"A\"\r\n"),
        ] {
            let left_grammar = Grammar::parse(left).expect("parses");
            let right_grammar = Grammar::parse(right).expect("parses");
            assert_ne!(left_grammar, right_grammar, "{left:?} vs {right:?}");
        }
    }
}
