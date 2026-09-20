//! The five error types, kept deliberately separate (SCOPE.md 6.6).
//!
//! * [`ParseError`] — the grammar text is not valid ABNF.
//! * [`CheckError`] — structural: the grammar cannot be used at all.
//! * [`LintWarning`] — advisory only; never fails `check`.
//! * [`MatchError`] / [`GenError`] — a *start rule* cannot be used, or a resource limit was hit.
//!
//! The middle category is why a limit is never reported as a rejection: "does not match" and
//! "could not decide" are different answers, and an oracle that conflates them is worthless
//! (D14, D16, D29).
//!
//! All five are `#[non_exhaustive]`: the v2 candidates in SCOPE.md 14 will add variants, and
//! that should not be a breaking change.

use core::fmt;

use crate::ast::Span;

/// The grammar text could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// A non-ASCII byte appeared in the grammar text.
    ///
    /// Checked before tokenization. RFC 5234 restricts comments to `WSP / VCHAR` and prose to
    /// `%x20-3D / %x3F-7E`, both ASCII, and those are the only places a non-ASCII byte could
    /// otherwise have been mistaken for content (SCOPE.md 4.2, D35).
    NonAscii {
        /// Where the offending byte is.
        span: Span,
    },
    /// A bare LF or CR appeared while `strict_crlf` was set.
    ExpectedCrlf {
        /// Where the line ending is.
        span: Span,
    },
    /// A numeric literal or repetition bound does not fit in 64 bits (D17).
    NumberTooLarge {
        /// Where the literal is.
        span: Span,
    },
    /// Something else was required here.
    Expected {
        /// What the parser required, e.g. `"="` or `"rule name"`.
        what: &'static str,
        /// Where it was required.
        span: Span,
    },
    /// The grammar text ended in the middle of a construct.
    UnexpectedEof {
        /// The end of the input.
        span: Span,
    },
}

impl ParseError {
    /// Where in the source text the error is.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::NonAscii { span }
            | Self::ExpectedCrlf { span }
            | Self::NumberTooLarge { span }
            | Self::Expected { span, .. }
            | Self::UnexpectedEof { span } => *span,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonAscii { .. } => f.write_str(
                "non-ASCII byte in grammar text; ABNF comments and prose values are ASCII-only",
            ),
            Self::ExpectedCrlf { .. } => f.write_str("expected CRLF line ending"),
            Self::NumberTooLarge { .. } => f.write_str("numeric value does not fit in 64 bits"),
            Self::Expected { what, .. } => write!(f, "expected {what}"),
            Self::UnexpectedEof { .. } => f.write_str("unexpected end of grammar text"),
        }
    }
}

impl ParseError {
    /// Renders the error against the grammar text it came from.
    ///
    /// Grammar authors are the ones who fix these, so the location matters more than the
    /// wording (SCOPE.md 3).
    #[must_use]
    pub fn render(&self, src: &str) -> String {
        render_at(src, self.span(), &self.to_string())
    }
}

impl std::error::Error for ParseError {}

/// A structural problem: the grammar cannot be used at all.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CheckError {
    /// A rule was defined twice with `=`.
    DuplicateDefinition {
        /// The rule name.
        name: String,
        /// Where the second definition is.
        span: Span,
    },
    /// `name =/ …` with no prior `name = …`.
    IncrementalWithoutBase {
        /// The rule name.
        name: String,
        /// Whether the name is one of the implicit core rules (D34).
        shadows_core: bool,
        /// Where the incremental definition is.
        span: Span,
    },
    /// A rule reference names no user rule and no core rule.
    UndefinedRule {
        /// The referenced name.
        name: String,
        /// Where the reference is.
        span: Span,
    },
    /// A repetition whose lower bound exceeds its upper bound (D18).
    InvalidRepeatRange {
        /// Lower bound.
        min: u64,
        /// Upper bound.
        max: u64,
        /// Where the repetition is.
        span: Span,
    },
    /// A numeric range whose lower endpoint exceeds its upper endpoint (D18).
    InvalidNumericRange {
        /// Lower endpoint.
        lo: u64,
        /// Upper endpoint.
        hi: u64,
        /// Where the range is.
        span: Span,
    },
    /// A cycle in the first-graph: the recognizer could not terminate on it.
    ///
    /// Global, not per start rule — an unreachable left-recursive rule still fails `check`.
    /// Left recursion is always a grammar bug for an ABNF recognizer, unlike prose values and
    /// unrepresentable terminals, which are legitimate ABNF this crate does not support
    /// (SCOPE.md 6.4).
    LeftRecursion {
        /// The cycle, in definition order.
        cycle: Vec<String>,
    },
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateDefinition { name, .. } => {
                write!(f, "rule `{name}` is defined more than once")
            }
            Self::IncrementalWithoutBase {
                name,
                shadows_core: false,
                ..
            } => {
                write!(f, "`{name} =/ …` has no prior `{name} = …`")
            }
            Self::IncrementalWithoutBase {
                name,
                shadows_core: true,
                ..
            } => write!(
                f,
                "`{name} =/ …` has no prior `{name} = …`; `{name}` is an implicit core rule and \
                 cannot be extended in place — write `{name} = <core definition> / …` to shadow \
                 and extend it"
            ),
            Self::UndefinedRule { name, .. } => write!(f, "undefined rule `{name}`"),
            Self::InvalidRepeatRange { min, max, .. } => {
                write!(f, "repetition lower bound {min} exceeds upper bound {max}")
            }
            Self::InvalidNumericRange { lo, hi, .. } => {
                write!(
                    f,
                    "numeric range starts at {lo:#X} and ends below it at {hi:#X}"
                )
            }
            Self::LeftRecursion { cycle } => {
                write!(f, "left recursion: {}", cycle.join(" -> "))
            }
        }
    }
}

impl CheckError {
    /// Where in the source this error is, if it has a single location.
    ///
    /// `LeftRecursion` has none: a cycle is a property of several rules at once, and pointing
    /// at any one of them would be arbitrary.
    #[must_use]
    pub fn span(&self) -> Option<Span> {
        match self {
            Self::DuplicateDefinition { span, .. }
            | Self::IncrementalWithoutBase { span, .. }
            | Self::UndefinedRule { span, .. }
            | Self::InvalidRepeatRange { span, .. }
            | Self::InvalidNumericRange { span, .. } => Some(*span),
            Self::LeftRecursion { .. } => None,
        }
    }

    /// Renders the error against the grammar text it came from.
    #[must_use]
    pub fn render(&self, src: &str) -> String {
        match self.span() {
            Some(span) => render_at(src, span, &self.to_string()),
            None => self.to_string(),
        }
    }
}

impl std::error::Error for CheckError {}

/// Every structural problem found by [`crate::Grammar::check`], in the order found.
///
/// A newtype rather than a bare `Vec<CheckError>` so that it can implement [`std::error::Error`]
/// — `Vec` is a foreign type, so it never could, and callers were forced to handle this one
/// error specially instead of writing `?` like everywhere else.
///
/// It behaves like a slice of [`CheckError`] for every purpose except that:
///
/// ```
/// use abnf_oracle::Grammar;
///
/// fn load(source: &str) -> Result<(), Box<dyn std::error::Error>> {
///     let grammar = Grammar::parse(source)?.check()?;
///     let _ = grammar;
///     Ok(())
/// }
///
/// // Four undefined references are four errors, not the first one four times.
/// let broken = "start = a b c d\r\n";
/// let errors = Grammar::parse(broken).unwrap().check().unwrap_err();
/// assert_eq!(errors.len(), 4);
/// for error in &errors {
///     assert!(error.span().is_some());
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckErrors(Vec<CheckError>);

impl CheckErrors {
    /// The errors, as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[CheckError] {
        &self.0
    }

    /// Takes the errors out.
    #[must_use]
    pub fn into_vec(self) -> Vec<CheckError> {
        self.0
    }

    /// Renders every error against the grammar text, one per line, with a closing count.
    ///
    /// This is what a grammar author wants to read: all of them at once, so a grammar with four
    /// undefined references takes one round of fixing rather than four.
    #[must_use]
    pub fn render(&self, src: &str) -> String {
        let mut out = String::new();
        for error in &self.0 {
            out.push_str(&error.render(src));
            out.push('\n');
        }
        let count = self.0.len();
        out.push_str(&format!(
            "{count} structural {}",
            if count == 1 { "error" } else { "errors" }
        ));
        out
    }
}

impl core::ops::Deref for CheckErrors {
    type Target = [CheckError];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<CheckError>> for CheckErrors {
    fn from(errors: Vec<CheckError>) -> Self {
        Self(errors)
    }
}

impl From<CheckErrors> for Vec<CheckError> {
    fn from(errors: CheckErrors) -> Self {
        errors.0
    }
}

impl IntoIterator for CheckErrors {
    type Item = CheckError;
    type IntoIter = std::vec::IntoIter<CheckError>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a CheckErrors {
    type Item = &'a CheckError;
    type IntoIter = core::slice::Iter<'a, CheckError>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl fmt::Display for CheckErrors {
    /// One error reads as itself; several are counted and then listed.
    ///
    /// No source text is available here, so there are no carets — [`CheckErrors::render`] is
    /// the one to use when the grammar text is at hand.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.as_slice() {
            [] => f.write_str("no structural errors"),
            [only] => write!(f, "{only}"),
            many => {
                write!(f, "{} structural errors:", many.len())?;
                for error in many {
                    write!(f, "\n  {error}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for CheckErrors {}

/// An advisory warning. Never fails `check` (D5).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LintWarning {
    /// No rule body mentions this rule.
    UnreferencedRule {
        /// The rule name.
        name: String,
    },
    /// Not reachable from the start rules given to `lint_from`.
    UnreachableRule {
        /// The rule name.
        name: String,
    },
    /// The rule matches nothing: its shortest match is infinite.
    UnproductiveRule {
        /// The rule name.
        name: String,
    },
    /// One branch of an alternation matches nothing, inside an otherwise productive rule.
    UnproductiveAlternative {
        /// The rule the alternation is in.
        rule: String,
        /// The branch index within the alternation.
        branch: u32,
    },
    /// An explicit definition shadows an implicit core rule.
    ///
    /// Never an error: RFC 3986 and RFC 9110 restate core rules verbatim (SCOPE.md 4.1, D6).
    ShadowsCoreRule {
        /// The rule name.
        name: String,
    },
}

impl fmt::Display for LintWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnreferencedRule { name } => {
                write!(f, "rule `{name}` is never referenced")
            }
            Self::UnreachableRule { name } => {
                write!(
                    f,
                    "rule `{name}` is not reachable from the given start rules"
                )
            }
            Self::UnproductiveRule { name } => {
                write!(
                    f,
                    "rule `{name}` can never match: it has no finite expansion"
                )
            }
            Self::UnproductiveAlternative { rule, branch } => write!(
                f,
                "branch {branch} of an alternation in rule `{rule}` can never match"
            ),
            Self::ShadowsCoreRule { name } => {
                write!(f, "rule `{name}` shadows the core rule of the same name")
            }
        }
    }
}

/// Recognition could not be attempted, or could not be finished.
///
/// Never returned to mean "the input does not match" — that is `Ok(false)`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MatchError {
    /// The start rule can reach a prose value, which has no matching semantics (SCOPE.md 6.5).
    ProseValueReachable {
        /// The start rule.
        rule: String,
        /// The prose text reached.
        prose: String,
    },
    /// The start rule can reach a terminal that no Unicode scalar value can match (SCOPE.md 6.1).
    UnrepresentableTerminal {
        /// The start rule.
        rule: String,
        /// The terminal, in canonical spelling.
        terminal: String,
    },
    /// No such rule in the grammar or the core environment.
    UnknownRule(
        /// The requested name.
        String,
    ),
    /// The in-progress guard fired. Unreachable after `check`, which rejects left recursion
    /// outright; kept as belt and braces (SCOPE.md 6.2).
    LeftRecursionDetected {
        /// The rule the guard fired on.
        rule: String,
    },
    /// `MatchOptions::max_steps` was exceeded. Never a silent reject (D16).
    StepLimit,
    /// `MatchOptions::max_depth` was exceeded.
    ///
    /// The recognizer descends recursively, so nesting in the *input* becomes depth on the
    /// call stack. Without this limit a deeply nested input overflows the stack, and a stack
    /// overflow aborts the process — it cannot be caught and reported as "could not decide",
    /// which is what every other limit here does.
    DepthLimit,
}

impl fmt::Display for MatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProseValueReachable { rule, prose } => write!(
                f,
                "rule `{rule}` can reach the prose value <{prose}>, which has no matching semantics"
            ),
            Self::UnrepresentableTerminal { rule, terminal } => write!(
                f,
                "rule `{rule}` can reach terminal {terminal}, which matches no Unicode scalar value"
            ),
            Self::UnknownRule(name) => write!(f, "unknown rule `{name}`"),
            Self::LeftRecursionDetected { rule } => {
                write!(f, "left recursion detected at rule `{rule}`")
            }
            Self::StepLimit => f.write_str("step limit exceeded"),
            Self::DepthLimit => f.write_str(
                "recursion depth limit exceeded: the input nests deeper than `MatchOptions::max_depth` allows",
            ),
        }
    }
}

impl std::error::Error for MatchError {}

/// Generation could not be attempted, or could not be finished.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GenError {
    /// The start rule can reach a prose value, which cannot be generated (SCOPE.md 6.5).
    ProseValueReachable {
        /// The start rule.
        rule: String,
        /// The prose text reached.
        prose: String,
    },
    /// The start rule can reach a terminal with no representable scalar value (SCOPE.md 6.1).
    UnrepresentableTerminal {
        /// The start rule.
        rule: String,
        /// The terminal, in canonical spelling.
        terminal: String,
    },
    /// No such rule in the grammar or the core environment.
    UnknownRule(
        /// The requested name.
        String,
    ),
    /// The start rule is unproductive: it has no finite expansion (D9).
    NoFiniteExpansion {
        /// The start rule.
        rule: String,
    },
    /// `GenOptions::max_output_len` was exceeded.
    ///
    /// Mandatory work is not bounded by depth — `start = 1000000000*"a"` is productive and has
    /// no shorter expansion — so this bound, not termination, is what makes generation
    /// practical (SCOPE.md 6.8, D29).
    OutputLimit,
    /// `GenOptions::max_steps` was exceeded.
    StepLimit,
}

impl fmt::Display for GenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProseValueReachable { rule, prose } => write!(
                f,
                "rule `{rule}` can reach the prose value <{prose}>, which cannot be generated"
            ),
            Self::UnrepresentableTerminal { rule, terminal } => write!(
                f,
                "rule `{rule}` can reach terminal {terminal}, which has no representable value"
            ),
            Self::UnknownRule(name) => write!(f, "unknown rule `{name}`"),
            Self::NoFiniteExpansion { rule } => {
                write!(f, "rule `{rule}` has no finite expansion")
            }
            Self::OutputLimit => f.write_str("output length limit exceeded"),
            Self::StepLimit => f.write_str("step limit exceeded"),
        }
    }
}

impl std::error::Error for GenError {}

/// Renders `message` against the source, with a caret under `span`.
///
/// Grammar text is ASCII (D35), so byte offsets are column numbers. The exception is
/// [`ParseError::NonAscii`], whose whole point is a byte that is not — there the column counts
/// bytes, which is still where the reader should look.
fn render_at(src: &str, span: Span, message: &str) -> String {
    let offset = (span.start as usize).min(src.len());
    let before = &src[..offset];
    let line_number = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    // `lines` strips the trailing CR, so CRLF sources do not print a stray carriage return.
    let line = src[line_start..].lines().next().unwrap_or_default();
    let column = offset - line_start;
    // A span can cover a whole definition, line terminator included, so clamp the caret to
    // what is actually on this line rather than running past its end.
    let width = (span.end.saturating_sub(span.start) as usize)
        .min(line.len().saturating_sub(column))
        .max(1);
    format!(
        "line {line_number}, column {}: {message}\n  {line}\n  {}{}",
        column + 1,
        " ".repeat(column),
        "^".repeat(width)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_without_base_suggests_the_workaround_for_core_rules() {
        let plain = CheckError::IncrementalWithoutBase {
            name: "foo".into(),
            shadows_core: false,
            span: Span::default(),
        };
        let core = CheckError::IncrementalWithoutBase {
            name: "DIGIT".into(),
            shadows_core: true,
            span: Span::default(),
        };
        assert!(!plain.to_string().contains("shadow"));
        assert!(core.to_string().contains("DIGIT = <core definition> / …"));
    }

    #[test]
    fn parse_error_span_is_reported_for_every_variant() {
        let span = Span::new(3, 7);
        for error in [
            ParseError::NonAscii { span },
            ParseError::ExpectedCrlf { span },
            ParseError::NumberTooLarge { span },
            ParseError::Expected { what: "=", span },
            ParseError::UnexpectedEof { span },
        ] {
            assert_eq!(error.span(), span);
            assert!(!error.to_string().is_empty());
        }
    }

    // -- CheckErrors ---------------------------------------------------------------------

    fn check_errors(src: &str) -> CheckErrors {
        crate::ast::Grammar::parse(src)
            .expect("parses")
            .check()
            .expect_err("should not check")
    }

    #[test]
    fn check_errors_is_a_std_error() {
        // The reason the newtype exists: `Vec` is foreign, so it can never implement `Error`,
        // and every caller had to handle this one error specially instead of writing `?`.
        fn takes_std_error(_: &dyn std::error::Error) {}
        takes_std_error(&check_errors("start = missing\r\n"));
    }

    #[test]
    fn check_errors_behaves_like_a_slice() {
        let errors = check_errors("start = a b c\r\n");
        assert_eq!(errors.len(), 3);
        assert!(!errors.is_empty());
        assert_eq!(errors.iter().count(), 3);
        assert!(matches!(errors[0], CheckError::UndefinedRule { .. }));
        assert_eq!(errors.as_slice().len(), 3);
    }

    #[test]
    fn check_errors_iterates_by_value_and_by_reference() {
        let errors = check_errors("start = a b\r\n");
        assert_eq!((&errors).into_iter().count(), 2);
        assert_eq!(errors.into_iter().count(), 2);
    }

    #[test]
    fn check_errors_converts_both_ways() {
        let errors = check_errors("start = a b\r\n");
        let as_vec: Vec<CheckError> = errors.clone().into();
        assert_eq!(as_vec.len(), 2);
        assert_eq!(CheckErrors::from(as_vec.clone()), errors);
        assert_eq!(errors.into_vec(), as_vec);
    }

    #[test]
    fn one_error_displays_as_itself() {
        let errors = check_errors("start = missing\r\n");
        assert_eq!(errors.to_string(), errors[0].to_string());
        assert!(!errors.to_string().contains("structural errors"));
    }

    #[test]
    fn several_errors_are_counted_then_listed() {
        let errors = check_errors("start = a b c\r\n");
        let shown = errors.to_string();
        assert!(shown.starts_with("3 structural errors:"), "{shown}");
        for error in &errors {
            assert!(shown.contains(&error.to_string()), "{shown}");
        }
    }

    #[test]
    fn render_shows_every_error_against_the_source() {
        // A grammar with four undefined references should take one round of fixing, not four.
        let src = "start = a b c d\r\n";
        let rendered = check_errors(src).render(src);

        assert_eq!(rendered.matches("undefined rule").count(), 4);
        assert!(rendered.contains('^'), "with carets: {rendered}");
        assert!(rendered.ends_with("4 structural errors"), "{rendered}");
    }

    #[test]
    fn render_says_error_in_the_singular() {
        let src = "start = missing\r\n";
        assert!(
            check_errors(src)
                .render(src)
                .ends_with("1 structural error")
        );
    }
}
