//! Grammar text to [`Grammar`]: hand-written recursive descent.
//!
//! Syntax only: `=/` is still separate from `=`, names are unresolved, and there are no node
//! ids — merging, resolution and ids are `check`'s job (SCOPE.md 6.7, D32). The local rewrites
//! that need no knowledge of other rules *are* applied here, on the way out of each rule body;
//! see [`canonicalize`].
//!
//! # The grammar implemented
//!
//! RFC 5234 4, with both verified errata and the RFC 7405 amendment. Every production below has
//! exactly one function, named for it, so that the invariant "the parser accepts exactly the
//! language of the canonical self-grammar" (SCOPE.md 4.2, D35) can be audited by reading this
//! file against the fixture.
//!
//! ```text
//! rulelist       =  1*( rule / (*WSP c-nl) )          ; Erratum 3076
//! rule           =  rulename defined-as elements c-nl
//! rulename       =  ALPHA *(ALPHA / DIGIT / "-")
//! defined-as     =  *c-wsp ("=" / "=/") *c-wsp
//! elements       =  alternation *WSP                  ; Erratum 2968
//! c-wsp          =  WSP / (c-nl WSP)
//! c-nl           =  comment / CRLF
//! comment        =  ";" *(WSP / VCHAR) CRLF
//! alternation    =  concatenation *(*c-wsp "/" *c-wsp concatenation)
//! concatenation  =  repetition *(1*c-wsp repetition)
//! repetition     =  [repeat] element
//! repeat         =  1*DIGIT / (*DIGIT "*" *DIGIT)
//! element        =  rulename / group / option / char-val / num-val / prose-val
//! group          =  "(" *c-wsp alternation *c-wsp ")"
//! option         =  "[" *c-wsp alternation *c-wsp "]"
//! char-val       =  case-insensitive-string / case-sensitive-string   ; RFC 7405 2.2
//! num-val        =  "%" (bin-val / dec-val / hex-val)
//! prose-val      =  "<" *(%x20-3D / %x3F-7E) ">"
//! ```
//!
//! Both errata remove an *ambiguity* rather than changing the language, so they do not alter
//! what is accepted — but they decide how a deterministic parser must be shaped, which is why
//! they are implemented rather than merely noted.
//!
//! # Two places this is deliberately not lenient
//!
//! A rule must end with a line ending: `rule` ends in `c-nl`, so a file whose last line has no
//! terminator is rejected. Accepting it would accept something the self-grammar does not, and
//! D35's invariant is what M2 and M3 test in both directions.
//!
//! Line endings are the one documented leniency (SCOPE.md 4.2): CRLF, LF and CR are all
//! accepted unless [`ParseOptions::strict_crlf`] is set.

use crate::ast::{
    CharVal, DefinedAs, Definition, Element, Grammar, Ignored, NumVal, ParseOptions, Repeat,
    RuleName, Span,
};
use crate::error::ParseError;

const HTAB: u8 = 0x09;
const LF: u8 = 0x0A;
const CR: u8 = 0x0D;
const SP: u8 = 0x20;
const DQUOTE: u8 = 0x22;

const fn is_wsp(b: u8) -> bool {
    b == SP || b == HTAB
}

const fn is_vchar(b: u8) -> bool {
    b >= 0x21 && b <= 0x7E
}

const fn is_alpha(b: u8) -> bool {
    b.is_ascii_alphabetic()
}

const fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

/// Whether a byte can begin a `repetition`, i.e. a `repeat` or an `element`.
///
/// Used to decide whether a run of `c-wsp` separates two elements of a concatenation or merely
/// precedes something that ends it (`/`, `)`, `]`, a line ending). Deciding by lookahead rather
/// than by trying and backtracking keeps a genuine error inside the next element reportable,
/// instead of being swallowed as "the concatenation ended here".
const fn starts_repetition(b: u8) -> bool {
    is_alpha(b) || is_digit(b) || matches!(b, b'(' | b'[' | DQUOTE | b'%' | b'<' | b'*')
}

/// Saturating byte offset, so a pathologically large input cannot wrap a span.
fn offset(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

impl Grammar {
    /// Parses ABNF grammar text.
    ///
    /// The RFC 5234 Appendix B core rules are always implicit and need not be defined; a
    /// grammar may still define them itself, which shadows them (SCOPE.md 4.1).
    ///
    /// Succeeds for any syntactically valid grammar, including one that references undefined
    /// rules or defines the same rule twice — those are structural problems, reported by
    /// `check` rather than here (D32).
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] if the text is not valid ABNF, contains a non-ASCII byte, or
    /// contains a numeric value or repetition bound that does not fit in 64 bits.
    pub fn parse(src: &str) -> Result<Self, ParseError> {
        Self::parse_with(src, ParseOptions::default())
    }

    /// Parses ABNF grammar text with explicit syntactic options.
    ///
    /// # Errors
    ///
    /// As [`Grammar::parse`], plus [`ParseError::ExpectedCrlf`] when
    /// [`ParseOptions::strict_crlf`] is set and a bare LF or CR appears.
    pub fn parse_with(src: &str, opts: ParseOptions) -> Result<Self, ParseError> {
        Parser::new(src, opts)?.rulelist()
    }
}

/// A cursor over ASCII grammar text.
struct Parser<'a> {
    /// The source. Known to be ASCII: [`Parser::new`] rejects anything else up front, so every
    /// function below may treat one byte as one character.
    src: &'a [u8],
    pos: usize,
    opts: ParseOptions,
}

impl<'a> Parser<'a> {
    /// Creates a parser, applying the ASCII gate (D35).
    fn new(src: &'a str, opts: ParseOptions) -> Result<Self, ParseError> {
        if let Some((index, ch)) = src.char_indices().find(|(_, ch)| !ch.is_ascii()) {
            return Err(ParseError::NonAscii {
                span: Span::new(offset(index), offset(index + ch.len_utf8())),
            });
        }
        Ok(Self {
            src: src.as_bytes(),
            pos: 0,
            opts,
        })
    }

    // -- cursor ----------------------------------------------------------------------------

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.src.get(self.pos + ahead).copied()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// The text consumed since `start`. Always valid UTF-8, because the input is ASCII.
    fn text_from(&self, start: usize) -> &'a str {
        core::str::from_utf8(&self.src[start..self.pos]).expect("input is ASCII")
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(offset(start), offset(self.pos))
    }

    /// A zero-width span at the cursor, for errors that point *between* two bytes.
    fn point(&self) -> Span {
        Span::new(offset(self.pos), offset(self.pos))
    }

    fn expected(&self, what: &'static str) -> ParseError {
        if self.at_end() {
            ParseError::UnexpectedEof { span: self.point() }
        } else {
            ParseError::Expected {
                what,
                span: self.point(),
            }
        }
    }

    // -- whitespace, comments and line endings ---------------------------------------------

    /// Consumes CRLF, or a bare LF or CR unless `strict_crlf` is set (SCOPE.md 4.2).
    fn try_line_ending(&mut self) -> bool {
        match (self.peek(), self.peek_at(1)) {
            (Some(CR), Some(LF)) => {
                self.pos += 2;
                true
            }
            (Some(CR | LF), _) if !self.opts.strict_crlf => {
                self.pos += 1;
                true
            }
            _ => false,
        }
    }

    fn line_ending(&mut self) -> Result<(), ParseError> {
        if self.try_line_ending() {
            return Ok(());
        }
        // Distinguish "there is a line ending here, of the wrong kind" from "there is no line
        // ending here", so that a CRLF-only grammar fed LF text says so.
        if self.opts.strict_crlf && matches!(self.peek(), Some(CR | LF)) {
            return Err(ParseError::ExpectedCrlf { span: self.point() });
        }
        Err(self.expected("end of line"))
    }

    /// `comment = ";" *(WSP / VCHAR) CRLF`
    fn comment(&mut self) -> Result<(), ParseError> {
        debug_assert_eq!(self.peek(), Some(b';'));
        self.pos += 1;
        while let Some(byte) = self.peek() {
            if is_wsp(byte) || is_vchar(byte) {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.line_ending()
    }

    /// `c-nl = comment / CRLF`
    fn c_nl(&mut self) -> Result<(), ParseError> {
        if self.peek() == Some(b';') {
            self.comment()
        } else {
            self.line_ending()
        }
    }

    fn at_c_nl(&self) -> bool {
        matches!(self.peek(), Some(b';' | CR | LF))
    }

    /// `c-wsp = WSP / (c-nl WSP)`
    ///
    /// The second alternative is the whole of ABNF's line-continuation rule: a line break
    /// counts as whitespace only when the next line begins with whitespace.
    fn c_wsp(&mut self) -> bool {
        if matches!(self.peek(), Some(byte) if is_wsp(byte)) {
            self.pos += 1;
            return true;
        }
        let save = self.pos;
        if self.at_c_nl()
            && self.c_nl().is_ok()
            && matches!(self.peek(), Some(byte) if is_wsp(byte))
        {
            self.pos += 1;
            return true;
        }
        self.pos = save;
        false
    }

    fn star_c_wsp(&mut self) {
        while self.c_wsp() {}
    }

    fn star_wsp(&mut self) {
        while matches!(self.peek(), Some(byte) if is_wsp(byte)) {
            self.pos += 1;
        }
    }

    // -- productions -----------------------------------------------------------------------

    /// `rulelist = 1*( rule / (*WSP c-nl) )` (Erratum 3076)
    fn rulelist(mut self) -> Result<Grammar, ParseError> {
        let mut defs = Vec::new();
        let mut iterations = 0_usize;

        while !self.at_end() {
            let before = self.pos;
            match self.peek() {
                // A rule can only begin with a rule name, and a rule name with ALPHA, so the
                // two alternatives never overlap and no backtracking is needed here.
                Some(byte) if is_alpha(byte) => defs.push(self.rule()?),
                Some(byte) if is_wsp(byte) || byte == b';' || byte == CR || byte == LF => {
                    self.star_wsp();
                    self.c_nl()?;
                }
                _ => return Err(self.expected("a rule name")),
            }
            debug_assert!(self.pos > before, "rulelist iteration consumed nothing");
            iterations += 1;
        }

        if iterations == 0 {
            return Err(self.expected("at least one rule"));
        }
        Ok(Grammar::new(defs, self.opts))
    }

    /// `rule = rulename defined-as elements c-nl`
    fn rule(&mut self) -> Result<Definition, ParseError> {
        let start = self.pos;
        let name = self.rulename()?;
        let defined_as = self.defined_as()?;
        let body = canonicalize(self.elements()?);
        self.c_nl()?;
        Ok(Definition {
            name,
            defined_as,
            body,
            span: Ignored(self.span_from(start)),
        })
    }

    /// `rulename = ALPHA *(ALPHA / DIGIT / "-")`
    fn rulename(&mut self) -> Result<RuleName, ParseError> {
        let start = self.pos;
        match self.peek() {
            Some(byte) if is_alpha(byte) => self.pos += 1,
            _ => return Err(self.expected("a rule name")),
        }
        while let Some(byte) = self.peek() {
            if is_alpha(byte) || is_digit(byte) || byte == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(RuleName::new(self.text_from(start)))
    }

    /// `defined-as = *c-wsp ("=" / "=/") *c-wsp`
    fn defined_as(&mut self) -> Result<DefinedAs, ParseError> {
        self.star_c_wsp();
        if !self.eat(b'=') {
            return Err(self.expected("`=` or `=/`"));
        }
        // `=` and `=/` overlap, so take the longer one; an alternation can never begin with
        // `/`, which is what makes that unambiguous.
        let defined_as = if self.eat(b'/') {
            DefinedAs::Incremental
        } else {
            DefinedAs::Base
        };
        self.star_c_wsp();
        Ok(defined_as)
    }

    /// `elements = alternation *WSP` (Erratum 2968)
    fn elements(&mut self) -> Result<Element, ParseError> {
        let alternation = self.alternation()?;
        self.star_wsp();
        Ok(alternation)
    }

    /// `alternation = concatenation *(*c-wsp "/" *c-wsp concatenation)`
    fn alternation(&mut self) -> Result<Element, ParseError> {
        let mut branches = vec![self.concatenation()?];
        loop {
            let save = self.pos;
            self.star_c_wsp();
            if !self.eat(b'/') {
                self.pos = save;
                break;
            }
            self.star_c_wsp();
            branches.push(self.concatenation()?);
        }
        Ok(unwrap_single(branches, Element::Alt))
    }

    /// `concatenation = repetition *(1*c-wsp repetition)`
    fn concatenation(&mut self) -> Result<Element, ParseError> {
        let mut items = vec![self.repetition()?];
        loop {
            let save = self.pos;
            if !self.c_wsp() {
                break;
            }
            self.star_c_wsp();
            match self.peek() {
                Some(byte) if starts_repetition(byte) => items.push(self.repetition()?),
                _ => {
                    self.pos = save;
                    break;
                }
            }
        }
        Ok(unwrap_single(items, Element::Concat))
    }

    /// `repetition = [repeat] element`
    fn repetition(&mut self) -> Result<Element, ParseError> {
        let repeat = self.repeat()?;
        let element = self.element()?;
        Ok(match repeat {
            Some(repeat) => Element::Repeat {
                repeat,
                body: Box::new(element),
            },
            None => element,
        })
    }

    /// `repeat = 1*DIGIT / (*DIGIT "*" *DIGIT)`
    fn repeat(&mut self) -> Result<Option<Repeat>, ParseError> {
        let start = self.pos;
        let min = self.try_number(10)?;
        if self.eat(b'*') {
            let max = self.try_number(10)?;
            return Ok(Some(Repeat {
                min: min.unwrap_or(0),
                max,
            }));
        }
        match min {
            Some(count) => Ok(Some(Repeat::exactly(count))),
            None => {
                self.pos = start;
                Ok(None)
            }
        }
    }

    /// `element = rulename / group / option / char-val / num-val / prose-val`
    fn element(&mut self) -> Result<Element, ParseError> {
        match self.peek() {
            Some(byte) if is_alpha(byte) => {
                let start = self.pos;
                let name = self.rulename()?;
                Ok(Element::RuleRef {
                    name,
                    span: Ignored(self.span_from(start)),
                })
            }
            Some(b'(') => self.group(),
            Some(b'[') => self.option(),
            Some(DQUOTE) => Ok(Element::CharVal(self.quoted_string(false)?)),
            Some(b'%') => self.percent(),
            Some(b'<') => self.prose_val(),
            _ => Err(self.expected("an element")),
        }
    }

    /// `group = "(" *c-wsp alternation *c-wsp ")"`
    ///
    /// A group is pure precedence: it contributes no node of its own, so `(a)` parses exactly
    /// as `a` does.
    fn group(&mut self) -> Result<Element, ParseError> {
        self.pos += 1;
        self.star_c_wsp();
        let inner = self.alternation()?;
        self.star_c_wsp();
        if !self.eat(b')') {
            return Err(self.expected("a closing `)`"));
        }
        Ok(inner)
    }

    /// `option = "[" *c-wsp alternation *c-wsp "]"`
    fn option(&mut self) -> Result<Element, ParseError> {
        self.pos += 1;
        self.star_c_wsp();
        let inner = self.alternation()?;
        self.star_c_wsp();
        if !self.eat(b']') {
            return Err(self.expected("a closing `]`"));
        }
        Ok(Element::Optional(Box::new(inner)))
    }

    /// `num-val` (RFC 5234) or a `%s` / `%i` prefixed `char-val` (RFC 7405 2.2).
    ///
    /// The self-grammar spells every one of these markers as a case-insensitive `char-val`
    /// (`"b"`, `"x"`, `"%s"`), so `%X41` and `%S"a"` are exactly as legal as `%x41` and
    /// `%s"a"`, and rejecting them would narrow the accepted language (D35).
    fn percent(&mut self) -> Result<Element, ParseError> {
        self.pos += 1;
        let marker = match self.peek() {
            Some(byte) => byte.to_ascii_lowercase(),
            None => return Err(self.expected("`b`, `d`, `x`, `s` or `i` after `%`")),
        };
        let radix = match marker {
            b'b' => 2,
            b'd' => 10,
            b'x' => 16,
            b's' | b'i' => {
                self.pos += 1;
                return Ok(Element::CharVal(self.quoted_string(marker == b's')?));
            }
            _ => return Err(self.expected("`b`, `d`, `x`, `s` or `i` after `%`")),
        };
        self.pos += 1;
        self.num_val(radix)
    }

    /// `bin-val` / `dec-val` / `hex-val`, which share the shape
    /// `1*DIGIT [ 1*("." 1*DIGIT) / ("-" 1*DIGIT) ]`.
    fn num_val(&mut self, radix: u32) -> Result<Element, ParseError> {
        let first = self.number(radix)?;
        if self.peek() == Some(b'.') {
            let mut values = vec![first];
            while self.eat(b'.') {
                values.push(self.number(radix)?);
            }
            return Ok(Element::NumVal(NumVal::Concat(values)));
        }
        if self.eat(b'-') {
            let hi = self.number(radix)?;
            return Ok(Element::NumVal(NumVal::Range { lo: first, hi }));
        }
        Ok(Element::NumVal(NumVal::Scalar(first)))
    }

    /// One or more digits in `radix`, as a `u64`.
    fn number(&mut self, radix: u32) -> Result<u64, ParseError> {
        let start = self.pos;
        let mut value: u64 = 0;
        let mut overflowed = false;

        while let Some(byte) = self.peek() {
            let Some(digit) = char::from(byte).to_digit(radix) else {
                break;
            };
            self.pos += 1;
            // Keep consuming after overflow, so the span covers the whole literal rather than
            // stopping at the digit that happened to tip it over.
            value = match value
                .checked_mul(u64::from(radix))
                .and_then(|scaled| scaled.checked_add(u64::from(digit)))
            {
                Some(next) => next,
                None => {
                    overflowed = true;
                    0
                }
            };
        }

        if self.pos == start {
            return Err(self.expected("a numeric value"));
        }
        if overflowed {
            return Err(ParseError::NumberTooLarge {
                span: self.span_from(start),
            });
        }
        Ok(value)
    }

    /// Zero or more digits, for the optional bounds of `repeat`.
    fn try_number(&mut self, radix: u32) -> Result<Option<u64>, ParseError> {
        match self.peek() {
            Some(byte) if char::from(byte).is_digit(radix) => self.number(radix).map(Some),
            _ => Ok(None),
        }
    }

    /// `quoted-string = DQUOTE *(%x20-21 / %x23-7E) DQUOTE`
    fn quoted_string(&mut self, case_sensitive: bool) -> Result<CharVal, ParseError> {
        if !self.eat(DQUOTE) {
            return Err(self.expected("a quoted string"));
        }
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if matches!(byte, 0x20..=0x21 | 0x23..=0x7E) {
                self.pos += 1;
            } else {
                break;
            }
        }
        let value = self.text_from(start).to_owned();
        if !self.eat(DQUOTE) {
            return Err(self.expected("a closing `\"`"));
        }
        Ok(CharVal {
            value,
            case_sensitive,
        })
    }

    /// `prose-val = "<" *(%x20-3D / %x3F-7E) ">"`
    fn prose_val(&mut self) -> Result<Element, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let text_start = self.pos;
        while let Some(byte) = self.peek() {
            if matches!(byte, 0x20..=0x3D | 0x3F..=0x7E) {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = self.text_from(text_start).to_owned();
        if !self.eat(b'>') {
            return Err(self.expected("a closing `>`"));
        }
        Ok(Element::ProseVal {
            text,
            span: Ignored(self.span_from(start)),
        })
    }
}

/// Returns the sole element, or wraps the several in `combine`.
///
/// An alternation of one concatenation is just that concatenation; building a one-branch node
/// and unwrapping it later would only give M1.2 more to undo.
/// Applies the local rewrites of SCOPE.md 6.7 to a rule body.
///
/// "Local" means needing no knowledge of any other rule, which is the test for whether a rewrite
/// belongs at the syntactic layer at all (D32). There are two kinds:
///
/// * **Redundant structure.** A group is pure precedence, so `a (b c)` parses to a concatenation
///   holding a concatenation, and `a / (b / c)` to an alternation holding an alternation. Both
///   are flattened, and a one-child alternation or concatenation is unwrapped. This is not
///   cosmetic: coverage units are `(alternation node, branch index)` pairs, so leaving the nested
///   shape would give `a / (b / c)` four units where `a / b / c` has three, for two spellings of
///   one language (D31).
/// * **Repetition spellings.** `*1a` becomes `[a]` and `1*1a` becomes `a`, so the three ways of
///   writing an optional element and the two ways of writing a bare one are indistinguishable
///   downstream.
///
/// One bottom-up pass reaches the fixpoint: a node is rewritten only after its children are
/// canonical, so an unwrapping that exposes a new nesting — `x (1*1(a b)) y` — is flattened by
/// the parent on the way back up.
fn canonicalize(element: Element) -> Element {
    match element {
        Element::Alt(branches) => {
            let mut flattened = Vec::with_capacity(branches.len());
            for branch in branches {
                match canonicalize(branch) {
                    Element::Alt(nested) => flattened.extend(nested),
                    other => flattened.push(other),
                }
            }
            unwrap_single(flattened, Element::Alt)
        }
        Element::Concat(items) => {
            let mut flattened = Vec::with_capacity(items.len());
            for item in items {
                match canonicalize(item) {
                    Element::Concat(nested) => flattened.extend(nested),
                    other => flattened.push(other),
                }
            }
            unwrap_single(flattened, Element::Concat)
        }
        Element::Repeat { repeat, body } => {
            let body = canonicalize(*body);
            match (repeat.min, repeat.max) {
                (0, Some(1)) => Element::Optional(Box::new(body)),
                (1, Some(1)) => body,
                _ => Element::Repeat {
                    repeat,
                    body: Box::new(body),
                },
            }
        }
        Element::Optional(body) => Element::Optional(Box::new(canonicalize(*body))),
        terminal => terminal,
    }
}

fn unwrap_single(mut items: Vec<Element>, combine: fn(Vec<Element>) -> Element) -> Element {
    if items.len() == 1 {
        items.pop().expect("length checked")
    } else {
        combine(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses, expecting success.
    fn parse(src: &str) -> Grammar {
        Grammar::parse(src).unwrap_or_else(|error| panic!("{src:?} should parse: {error}"))
    }

    /// The body of a one-rule grammar, for testing elements without the surrounding noise.
    fn body(elements: &str) -> Element {
        let grammar = parse(&format!("x = {elements}\r\n"));
        assert_eq!(grammar.definitions().len(), 1);
        grammar.definitions()[0].body.clone()
    }

    fn error(src: &str) -> ParseError {
        Grammar::parse(src).expect_err("should not parse")
    }

    fn ci(value: &str) -> Element {
        Element::CharVal(CharVal {
            value: value.into(),
            case_sensitive: false,
        })
    }

    fn cs(value: &str) -> Element {
        Element::CharVal(CharVal {
            value: value.into(),
            case_sensitive: true,
        })
    }

    fn rule_ref(name: &str) -> Element {
        Element::RuleRef {
            name: RuleName::new(name),
            span: Ignored(Span::default()),
        }
    }

    fn scalar(value: u64) -> Element {
        Element::NumVal(NumVal::Scalar(value))
    }

    fn repeat(min: u64, max: Option<u64>, body: Element) -> Element {
        Element::Repeat {
            repeat: Repeat { min, max },
            body: Box::new(body),
        }
    }

    // -- 4: one test per construct ----------------------------------------------------------

    #[test]
    fn rule_definition() {
        let grammar = parse("foo = \"a\"\r\n");
        let def = &grammar.definitions()[0];
        assert_eq!(def.name, RuleName::new("foo"));
        assert_eq!(def.defined_as, DefinedAs::Base);
        assert_eq!(def.body, ci("a"));
    }

    #[test]
    fn incremental_alternative_stays_separate() {
        // Merging is `check`'s job (D32); the syntactic layer keeps what was written.
        let grammar = parse("foo = \"a\"\r\nfoo =/ \"b\"\r\n");
        let kinds: Vec<_> = grammar.definitions().iter().map(|d| d.defined_as).collect();
        assert_eq!(kinds, vec![DefinedAs::Base, DefinedAs::Incremental]);
        assert_eq!(grammar.definitions()[1].name, RuleName::new("foo"));
    }

    #[test]
    fn rule_names_may_contain_digits_and_hyphens() {
        let grammar = parse("a1-b2 = \"x\"\r\n");
        assert_eq!(grammar.definitions()[0].name, RuleName::new("a1-b2"));
    }

    #[test]
    fn alternation() {
        assert_eq!(
            body("\"a\" / \"b\" / \"c\""),
            Element::Alt(vec![ci("a"), ci("b"), ci("c")])
        );
    }

    #[test]
    fn concatenation() {
        assert_eq!(body("\"a\" \"b\""), Element::Concat(vec![ci("a"), ci("b")]));
    }

    #[test]
    fn grouping_binds_tighter_than_alternation() {
        assert_eq!(
            body("\"a\" (\"b\" / \"c\")"),
            Element::Concat(vec![ci("a"), Element::Alt(vec![ci("b"), ci("c")])])
        );
        // A group contributes no node of its own.
        assert_eq!(body("(\"a\")"), ci("a"));
    }

    #[test]
    fn optional() {
        assert_eq!(body("[\"a\"]"), Element::Optional(Box::new(ci("a"))));
        assert_eq!(
            body("[\"a\" / \"b\"]"),
            Element::Optional(Box::new(Element::Alt(vec![ci("a"), ci("b")])))
        );
    }

    #[test]
    fn repetition_in_every_spelling() {
        assert_eq!(body("*\"a\""), repeat(0, None, ci("a")));
        assert_eq!(body("3\"a\""), repeat(3, Some(3), ci("a")));
        assert_eq!(body("2*5\"a\""), repeat(2, Some(5), ci("a")));
        assert_eq!(body("*3\"a\""), repeat(0, Some(3), ci("a")));
        assert_eq!(body("3*\"a\""), repeat(3, None, ci("a")));
        // Canonicalization collapses the redundant spellings on the way out of the parser
        // (SCOPE.md 6.7), so these two never reach the rest of the crate as repetitions.
        assert_eq!(body("*1\"a\""), Element::Optional(Box::new(ci("a"))));
        assert_eq!(body("1*1\"a\""), ci("a"));
    }

    #[test]
    fn quoted_strings() {
        assert_eq!(body("\"abc\""), ci("abc"));
        assert_eq!(body("%s\"abc\""), cs("abc"));
        assert_eq!(body("%i\"abc\""), ci("abc"));
        assert_eq!(body("\"\""), ci(""), "an empty char-val is legal");
    }

    #[test]
    fn numeric_values_in_every_radix() {
        assert_eq!(body("%x41"), scalar(0x41));
        assert_eq!(body("%d65"), scalar(65));
        assert_eq!(body("%b1000001"), scalar(0b100_0001));
    }

    #[test]
    fn numeric_range_and_concatenation() {
        assert_eq!(
            body("%x41-5A"),
            Element::NumVal(NumVal::Range { lo: 0x41, hi: 0x5A })
        );
        assert_eq!(
            body("%x41.42.43"),
            Element::NumVal(NumVal::Concat(vec![0x41, 0x42, 0x43]))
        );
    }

    #[test]
    fn radix_and_string_markers_are_case_insensitive() {
        // The self-grammar spells these as case-insensitive char-vals, so the uppercase forms
        // are part of the language (D35).
        assert_eq!(body("%X41"), scalar(0x41));
        assert_eq!(body("%D65"), scalar(65));
        assert_eq!(body("%B1000001"), scalar(0b100_0001));
        assert_eq!(body("%S\"a\""), cs("a"));
        assert_eq!(body("%I\"a\""), ci("a"));
        assert_eq!(body("%x4a"), scalar(0x4A), "lowercase HEXDIG is legal too");
    }

    #[test]
    fn prose_value() {
        assert_eq!(
            body("<any single character>"),
            Element::ProseVal {
                text: "any single character".into(),
                span: Ignored(Span::default()),
            }
        );
    }

    #[test]
    fn rule_reference() {
        assert_eq!(body("ALPHA"), rule_ref("ALPHA"));
    }

    #[test]
    fn comments_and_blank_lines() {
        let grammar = parse(
            "; leading comment\r\n\
             \r\n\
             foo = \"a\"  ; trailing comment\r\n\
             \r\n\
             bar = \"b\"\r\n",
        );
        assert_eq!(grammar.definitions().len(), 2);
        assert_eq!(grammar.definitions()[0].body, ci("a"));
        assert_eq!(grammar.definitions()[1].body, ci("b"));
    }

    #[test]
    fn line_continuation() {
        // A line break is whitespace only when the next line starts with whitespace.
        let continued = parse("foo = \"a\"\r\n      \"b\"\r\n");
        assert_eq!(continued.definitions().len(), 1);
        assert_eq!(
            continued.definitions()[0].body,
            Element::Concat(vec![ci("a"), ci("b")])
        );

        let separate = parse("foo = \"a\"\r\nbar = \"b\"\r\n");
        assert_eq!(separate.definitions().len(), 2);
    }

    #[test]
    fn continuation_across_an_alternation() {
        let grammar = parse("foo = \"a\"\r\n    / \"b\"\r\n");
        assert_eq!(
            grammar.definitions()[0].body,
            Element::Alt(vec![ci("a"), ci("b")])
        );
    }

    #[test]
    fn comment_inside_a_continued_rule() {
        let grammar = parse("foo = \"a\" ; why\r\n      \"b\"\r\n");
        assert_eq!(grammar.definitions().len(), 1);
        assert_eq!(
            grammar.definitions()[0].body,
            Element::Concat(vec![ci("a"), ci("b")])
        );
    }

    // -- the errata --------------------------------------------------------------------------

    #[test]
    fn erratum_3076_indented_comment_line_is_not_part_of_the_rule() {
        // The input from the erratum: without the fix, `rulelist` can attach the indented
        // comment to the rule or treat it as a list item, and both parse.
        let grammar = parse("X = Y\r\n ;Z\r\n");
        assert_eq!(grammar.definitions().len(), 1);
        assert_eq!(grammar.definitions()[0].body, rule_ref("Y"));
    }

    #[test]
    fn erratum_2968_elements_takes_only_wsp() {
        let grammar = parse("X = Y  \r\n");
        assert_eq!(grammar.definitions()[0].body, rule_ref("Y"));
    }

    // -- line endings ------------------------------------------------------------------------

    #[test]
    fn lf_and_cr_are_accepted_by_default() {
        assert_eq!(parse("foo = \"a\"\n").definitions().len(), 1);
        assert_eq!(parse("foo = \"a\"\r").definitions().len(), 1);
        assert_eq!(parse("foo = \"a\"\r\n").definitions().len(), 1);
    }

    #[test]
    fn strict_crlf_rejects_bare_lf() {
        let strict = ParseOptions { strict_crlf: true };
        assert!(Grammar::parse_with("foo = \"a\"\r\n", strict.clone()).is_ok());
        assert!(matches!(
            Grammar::parse_with("foo = \"a\"\n", strict.clone()),
            Err(ParseError::ExpectedCrlf { .. })
        ));
        assert!(matches!(
            Grammar::parse_with("foo = \"a\"\r", strict),
            Err(ParseError::ExpectedCrlf { .. })
        ));
    }

    #[test]
    fn a_rule_must_be_terminated() {
        // `rule` ends in `c-nl`, so the self-grammar rejects a missing final line ending and
        // so must this parser (D35). Documented rather than papered over.
        assert!(matches!(
            error("foo = \"a\""),
            ParseError::UnexpectedEof { .. }
        ));
    }

    // -- errors ------------------------------------------------------------------------------

    #[test]
    fn non_ascii_is_rejected_before_anything_else() {
        let err = error("foo = \"a\" ; naïve\r\n");
        let ParseError::NonAscii { span } = err else {
            panic!("expected NonAscii, got {err}");
        };
        // The span covers the whole multi-byte character, not just its first byte.
        assert_eq!(span.end - span.start, 2);
    }

    #[test]
    fn non_ascii_anywhere_is_rejected() {
        for src in [
            "fo\u{f6} = \"a\"\r\n",
            "foo = \"\u{e9}\"\r\n",
            "foo = <\u{2014}>\r\n",
        ] {
            assert!(matches!(error(src), ParseError::NonAscii { .. }), "{src:?}");
        }
    }

    #[test]
    fn numbers_beyond_64_bits_are_rejected() {
        assert!(matches!(
            error("foo = 1234567890123456789012345\"a\"\r\n"),
            ParseError::NumberTooLarge { .. }
        ));
        assert!(matches!(
            error("foo = %x1234567890123456789\r\n"),
            ParseError::NumberTooLarge { .. }
        ));
        // u64::MAX itself fits.
        assert_eq!(
            body("18446744073709551615\"a\""),
            repeat(u64::MAX, Some(u64::MAX), ci("a"))
        );
    }

    #[test]
    fn syntax_errors_are_reported_with_a_span() {
        for src in [
            "foo\r\n",          // no defined-as
            "foo = \r\n",       // no elements
            "foo = (\"a\"\r\n", // unclosed group
            "foo = [\"a\"\r\n", // unclosed option
            "foo = \"a\r\n",    // unclosed string
            "foo = <a\r\n",     // unclosed prose
            "foo = %q41\r\n",   // bad radix
            "foo = %x\r\n",     // no digits
            "= \"a\"\r\n",      // no rule name
            "",                 // empty input
        ] {
            let err = error(src);
            assert!(!err.to_string().is_empty(), "{src:?}");
        }
    }

    #[test]
    fn empty_input_is_not_a_grammar() {
        assert!(matches!(error(""), ParseError::UnexpectedEof { .. }));
    }

    #[test]
    fn a_grammar_of_only_blank_lines_has_no_rules() {
        // `1*( rule / (*WSP c-nl) )` is satisfied by the blank lines alone.
        assert_eq!(
            parse("\r\n   \r\n; just a comment\r\n").definitions().len(),
            0
        );
    }

    #[test]
    fn spans_cover_the_definition() {
        let src = "foo = \"a\"\r\nbar = \"b\"\r\n";
        let grammar = parse(src);
        let first = grammar.definitions()[0].span.0;
        assert_eq!(&src[first.range()], "foo = \"a\"\r\n");
    }
}
