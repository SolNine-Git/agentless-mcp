//! Source lines of Python code: the lines on which a counted token starts.

use std::collections::BTreeSet;
use std::fmt;

// Fixed values, not tuning: tokenize.json pins each one (99/100 indents, 200/201 parens, 149/150
// f-strings, format-spec depth 2/3, and a tab against four spaces for the tab size).
const MAX_INDENT: usize = 100;
const MAX_LEVEL: usize = 200;
const MAX_FSTRING_LEVEL: usize = 150;
const MAX_EXPR_NESTING: i32 = 3;
const TAB_SIZE: usize = 8;
const UNEXPECTED_EOF: &str = "unexpected EOF in multi-line statement";
const FORMAT_SPEC_NEWLINE: &str =
    "f-string: newlines are not allowed in format specifiers for single quoted f-strings";

/// The class of a tokenize failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenizeErrorKind {
    /// A lexical error, reported as `TokenError`.
    Token,
    /// An indentation error, reported as `IndentationError`.
    Indentation,
    /// Inconsistent tabs and spaces in indentation, reported as `TabError`.
    Tab,
}

impl TokenizeErrorKind {
    /// Returns the class name: `TokenError`, `IndentationError` or `TabError`.
    #[must_use]
    pub fn class_name(self) -> &'static str {
        match self {
            Self::Token => "TokenError",
            Self::Indentation => "IndentationError",
            Self::Tab => "TabError",
        }
    }
}

/// A tokenize failure: its class and its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenizeError {
    /// The class of the failure.
    pub kind: TokenizeErrorKind,
    /// The message, without the class name.
    pub message: String,
}

impl fmt::Display for TokenizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.class_name(), self.message)
    }
}

impl std::error::Error for TokenizeError {}

// The line numbers on which a counted token of `source` starts, or the failure to tokenize it.
pub(crate) fn python_sloc(source: &str) -> Result<BTreeSet<u32>, TokenizeError> {
    Scanner::new(source).run()
}

#[derive(Clone, Copy)]
struct FString {
    quote: char,
    size: usize,
    raw: bool,
    in_literal: bool,
    brackets: i32,
    // The depth of the open replacement field, from 0; -1 in the top-level literal text.
    fields: i32,
    format_spec: bool,
}

impl FString {
    fn in_format_spec(self) -> bool {
        self.format_spec && self.fields >= 0
    }
}

#[derive(Clone, Copy)]
struct Prefix {
    len: usize,
    quote: char,
    fstring: bool,
    raw: bool,
}

struct Scanner {
    chars: Vec<char>,
    cut_at_null: bool,
    pos: usize,
    line: u32,
    level: usize,
    at_line_start: bool,
    indents: Vec<(usize, usize)>,
    fstrings: Vec<FString>,
    lines: Vec<u32>,
}

impl Scanner {
    fn new(source: &str) -> Self {
        let (chars, cut_at_null) = scan_buffer(source);
        Self {
            chars,
            cut_at_null,
            pos: 0,
            line: 1,
            level: 0,
            at_line_start: true,
            indents: vec![(0, 0)],
            fstrings: Vec::new(),
            lines: Vec::new(),
        }
    }

    fn run(mut self) -> Result<BTreeSet<u32>, TokenizeError> {
        while self.next_token()? {}
        Ok(self.lines.into_iter().collect())
    }

    fn next_token(&mut self) -> Result<bool, TokenizeError> {
        match self.fstrings.last() {
            Some(&fstring) if fstring.in_literal => self.fstring_part(fstring).map(|()| true),
            _ => self.normal_token(),
        }
    }

    fn normal_token(&mut self) -> Result<bool, TokenizeError> {
        if self.at_line_start {
            self.at_line_start = false;
            self.indentation()?;
        }
        self.skip_spaces();
        let Some(c) = self.peek() else {
            return self.end_of_input().map(|()| false);
        };
        self.token(c)?;
        Ok(true)
    }

    fn token(&mut self, c: char) -> Result<(), TokenizeError> {
        match c {
            '#' => {
                self.comment();
                Ok(())
            }
            '\n' => {
                self.newline();
                Ok(())
            }
            '\\' => self.continuation(),
            '\'' | '"' => self.string(c),
            '0'..='9' => self.number(),
            _ if is_name_start(c) => self.name_or_string(),
            _ => self.punctuation(c),
        }
    }

    fn indentation(&mut self) -> Result<(), TokenizeError> {
        let (col, alt) = self.indent_columns()?;
        if matches!(self.peek(), Some('#' | '\n')) || self.level > 0 {
            return Ok(());
        }
        self.check_indent(col, alt)
    }

    fn indent_columns(&mut self) -> Result<(usize, usize), TokenizeError> {
        let (mut col, mut alt, mut continued_at) = (0, 0, 0);
        loop {
            let c = self.peek();
            // The first backslash past column 0 fixes the indentation of the logical line.
            if c == Some('\\') && continued_at == 0 {
                continued_at = col;
            }
            match c {
                Some(' ') => (col, alt) = (col + 1, alt + 1),
                Some('\t') => (col, alt) = ((col / TAB_SIZE + 1) * TAB_SIZE, alt + 1),
                Some('\u{c}') => (col, alt) = (0, 0),
                Some('\\') => {
                    self.continuation()?;
                    continue;
                }
                _ => break,
            }
            self.bump();
        }
        if continued_at == 0 {
            Ok((col, alt))
        } else {
            Ok((continued_at, continued_at))
        }
    }

    fn check_indent(&mut self, col: usize, alt: usize) -> Result<(), TokenizeError> {
        let (top_col, top_alt) = self.indent_top();
        if col > top_col {
            if self.indents.len() >= MAX_INDENT {
                return Err(indentation_error("too many levels of indentation"));
            }
            if alt <= top_alt {
                return Err(tab_error());
            }
            self.indents.push((col, alt));
            return Ok(());
        }
        while self.indents.len() > 1 && col < self.indent_top().0 {
            self.indents.pop();
        }
        let (top_col, top_alt) = self.indent_top();
        if col != top_col {
            return Err(indentation_error(
                "unindent does not match any outer indentation level",
            ));
        }
        if alt != top_alt {
            return Err(tab_error());
        }
        Ok(())
    }

    fn indent_top(&self) -> (usize, usize) {
        *self
            .indents
            .last()
            .expect("the indent stack never pops its first entry")
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\u{c}')) {
            self.bump();
        }
    }

    fn comment(&mut self) {
        self.skip_while(|c| c != '\n');
    }

    fn newline(&mut self) {
        self.bump();
        self.at_line_start = true;
    }

    fn continuation(&mut self) -> Result<(), TokenizeError> {
        self.bump();
        if self.peek() != Some('\n') {
            return Err(token_error(
                "unexpected character after line continuation character",
            ));
        }
        self.bump();
        if self.peek().is_none() {
            return Err(self.eof_error(token_error(UNEXPECTED_EOF)));
        }
        Ok(())
    }

    fn end_of_input(&self) -> Result<(), TokenizeError> {
        if self.level > 0 || self.cut_at_null {
            return Err(self.eof_error(token_error(UNEXPECTED_EOF)));
        }
        Ok(())
    }

    fn name_or_string(&mut self) -> Result<(), TokenizeError> {
        self.count();
        match self.string_prefix() {
            Some(prefix) if prefix.fstring => self.fstring_start(prefix),
            Some(prefix) => {
                self.skip(prefix.len);
                self.string_body(prefix.quote)
            }
            None => {
                self.skip_while(is_name_char);
                Ok(())
            }
        }
    }

    fn string_prefix(&self) -> Option<Prefix> {
        let len = (1..=2).find(|&offset| matches!(self.peek_at(offset), Some('\'' | '"')))?;
        let letters: String = self
            .chars
            .get(self.pos..self.pos + len)?
            .iter()
            .map(char::to_ascii_lowercase)
            .collect();
        if !matches!(
            letters.as_str(),
            "b" | "r" | "u" | "f" | "br" | "rb" | "fr" | "rf"
        ) {
            return None;
        }
        Some(Prefix {
            len,
            quote: self.peek_at(len)?,
            fstring: letters.contains('f'),
            raw: letters.contains('r'),
        })
    }

    fn string(&mut self, quote: char) -> Result<(), TokenizeError> {
        self.count();
        self.string_body(quote)
    }

    fn string_body(&mut self, quote: char) -> Result<(), TokenizeError> {
        let size = self.open_quote(quote);
        let mut escaped_quote = false;
        while !self.at_quotes(quote, size) {
            match self.peek() {
                None => {
                    let error = self.unterminated_string(quote, size, escaped_quote);
                    return Err(self.eof_error(error));
                }
                Some('\n') if size == 1 => {
                    return Err(self.unterminated_string(quote, size, escaped_quote));
                }
                Some('\\') => escaped_quote |= self.skip_escape(quote),
                Some(_) => self.bump(),
            }
        }
        self.skip(size);
        Ok(())
    }

    fn open_quote(&mut self, quote: char) -> usize {
        let size = if self.at_quotes(quote, 3) { 3 } else { 1 };
        self.skip(size);
        size
    }

    fn skip_escape(&mut self, quote: char) -> bool {
        self.bump();
        let escaped = self.peek();
        self.bump();
        escaped == Some(quote)
    }

    fn unterminated_string(&self, quote: char, size: usize, escaped_quote: bool) -> TokenizeError {
        if self
            .fstrings
            .last()
            .is_some_and(|fstring| fstring.quote == quote && fstring.size == size)
        {
            return token_error("f-string: expecting '}'");
        }
        if size == 3 {
            return token_error("EOF in multi-line string");
        }
        let hint = if escaped_quote {
            "; perhaps you escaped the end quote?"
        } else {
            ""
        };
        token_error(format!(
            "unterminated string literal (detected at line {}){hint}",
            self.detected_line()
        ))
    }

    fn number(&mut self) -> Result<(), TokenizeError> {
        self.count();
        if self.peek() == Some('0')
            && matches!(self.peek_at(1), Some('x' | 'X' | 'o' | 'O' | 'b' | 'B'))
        {
            return self.radix_number();
        }
        self.decimal_digits()?;
        if self.peek() == Some('.') {
            self.bump();
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.decimal_digits()?;
            }
        }
        self.exponent()?;
        if matches!(self.peek(), Some('j' | 'J')) {
            self.bump();
        }
        Ok(())
    }

    fn decimal_digits(&mut self) -> Result<(), TokenizeError> {
        loop {
            self.skip_while(|c| c.is_ascii_digit());
            if self.peek() != Some('_') {
                return Ok(());
            }
            self.bump();
            if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
                return Err(token_error("invalid decimal literal"));
            }
        }
    }

    fn exponent(&mut self) -> Result<(), TokenizeError> {
        if !matches!(self.peek(), Some('e' | 'E')) {
            return Ok(());
        }
        let sign = usize::from(matches!(self.peek_at(1), Some('+' | '-')));
        if !self.peek_at(1 + sign).is_some_and(|c| c.is_ascii_digit()) {
            // Without a sign, the `e` starts the next token.
            if sign == 1 {
                return Err(token_error("invalid decimal literal"));
            }
            return Ok(());
        }
        self.skip(1 + sign);
        self.decimal_digits()
    }

    fn radix_number(&mut self) -> Result<(), TokenizeError> {
        self.bump();
        let (name, is_digit) = radix(self.peek());
        self.bump();
        loop {
            if self.peek() == Some('_') {
                self.bump();
            }
            if !self.peek().is_some_and(is_digit) {
                return Err(radix_error(name, self.peek()));
            }
            self.skip_while(is_digit);
            if self.peek() != Some('_') {
                break;
            }
        }
        if self.peek().is_some_and(|c| c.is_ascii_digit()) {
            return Err(radix_error(name, self.peek()));
        }
        Ok(())
    }

    fn punctuation(&mut self, c: char) -> Result<(), TokenizeError> {
        if c == ':' && self.starts_format_spec() {
            self.count();
            self.bump();
            let top = self.top_fstring();
            top.in_literal = true;
            top.format_spec = true;
            return Ok(());
        }
        match c {
            '(' | '[' | '{' => self.open_bracket()?,
            ')' | ']' | '}' => self.close_bracket(c)?,
            _ if c.is_ascii_control() => {
                return Err(token_error(format!(
                    "invalid non-printable character U+{:04X}",
                    u32::from(c)
                )));
            }
            _ => {}
        }
        self.count();
        self.bump();
        Ok(())
    }

    fn starts_format_spec(&self) -> bool {
        self.fstrings
            .last()
            .is_some_and(|fstring| fstring.fields >= 0 && fstring.brackets - 1 == fstring.fields)
    }

    fn open_bracket(&mut self) -> Result<(), TokenizeError> {
        if self.level >= MAX_LEVEL {
            return Err(token_error("too many nested parentheses"));
        }
        self.level += 1;
        if let Some(top) = self.fstrings.last_mut() {
            top.brackets += 1;
        }
        Ok(())
    }

    fn close_bracket(&mut self, c: char) -> Result<(), TokenizeError> {
        self.level = self.level.saturating_sub(1);
        let Some(top) = self.fstrings.last_mut() else {
            return Ok(());
        };
        if c == '}' && top.brackets == 0 {
            return Err(token_error("f-string: single '}' is not allowed"));
        }
        top.brackets -= 1;
        if top.brackets < 0 {
            return Err(token_error(format!("f-string: unmatched '{c}'")));
        }
        if c == '}' && top.brackets == top.fields {
            top.fields -= 1;
            top.in_literal = true;
            top.format_spec = false;
        }
        Ok(())
    }

    fn fstring_start(&mut self, prefix: Prefix) -> Result<(), TokenizeError> {
        self.skip(prefix.len);
        let size = self.open_quote(prefix.quote);
        if self.fstrings.len() + 1 >= MAX_FSTRING_LEVEL {
            return Err(token_error("too many nested f-strings"));
        }
        self.fstrings.push(FString {
            quote: prefix.quote,
            size,
            raw: prefix.raw,
            in_literal: true,
            brackets: 0,
            fields: -1,
            format_spec: false,
        });
        Ok(())
    }

    fn fstring_part(&mut self, fstring: FString) -> Result<(), TokenizeError> {
        if self.peek() == Some('{') && self.peek_at(1) != Some('{') {
            return self.open_field();
        }
        if self.at_quotes(fstring.quote, fstring.size) {
            self.count();
            self.skip(fstring.size);
            self.fstrings.pop();
            return Ok(());
        }
        self.fstring_middle(fstring)
    }

    fn open_field(&mut self) -> Result<(), TokenizeError> {
        let top = self.top_fstring();
        top.fields += 1;
        top.in_literal = false;
        if top.fields >= MAX_EXPR_NESTING {
            return Err(token_error("f-string: expressions nested too deeply"));
        }
        Ok(())
    }

    fn fstring_middle(&mut self, fstring: FString) -> Result<(), TokenizeError> {
        let (start, line) = (self.pos, self.line);
        let mut named_escape = false;
        while !self.at_quotes(fstring.quote, fstring.size) {
            if self.middle_step(fstring, &mut named_escape)? {
                break;
            }
        }
        if self.pos > start {
            self.count_line(line);
        }
        Ok(())
    }

    fn middle_step(
        &mut self,
        fstring: FString,
        named_escape: &mut bool,
    ) -> Result<bool, TokenizeError> {
        match self.peek() {
            None => Err(self.eof_error(self.unterminated_fstring(fstring.size))),
            Some('\n') if fstring.size == 1 => Err(self.middle_newline(fstring)),
            Some('{') => self.middle_open(fstring).map(|()| true),
            Some('}') => {
                self.middle_close(fstring, *named_escape);
                Ok(true)
            }
            Some('\\') => {
                *named_escape |= self.middle_escape(fstring.raw);
                Ok(false)
            }
            Some(_) => {
                self.bump();
                Ok(false)
            }
        }
    }

    fn middle_open(&mut self, fstring: FString) -> Result<(), TokenizeError> {
        if self.peek_at(1) == Some('{') && !fstring.in_format_spec() {
            self.skip(2);
            return Ok(());
        }
        self.top_fstring().format_spec = false;
        self.open_field()
    }

    fn middle_close(&mut self, fstring: FString, named_escape: bool) {
        if named_escape {
            self.bump();
        } else if self.peek_at(1) == Some('}') && !fstring.in_format_spec() && fstring.brackets == 0
        {
            self.skip(2);
        } else {
            let top = self.top_fstring();
            top.in_literal = false;
            top.format_spec = false;
        }
    }

    fn middle_escape(&mut self, raw: bool) -> bool {
        self.bump();
        match self.peek() {
            Some('{' | '}') => false,
            Some('N') if !raw => {
                self.bump();
                let named = self.peek() == Some('{');
                if named {
                    self.bump();
                }
                named
            }
            _ => {
                self.bump();
                false
            }
        }
    }

    fn middle_newline(&self, fstring: FString) -> TokenizeError {
        if fstring.in_format_spec() {
            return token_error(FORMAT_SPEC_NEWLINE);
        }
        self.unterminated_fstring(fstring.size)
    }

    fn unterminated_fstring(&self, size: usize) -> TokenizeError {
        let triple = if size == 3 { "triple-quoted " } else { "" };
        token_error(format!(
            "unterminated {triple}f-string literal (detected at line {})",
            self.detected_line()
        ))
    }

    fn top_fstring(&mut self) -> &mut FString {
        self.fstrings
            .last_mut()
            .expect("the f-string paths run only while an f-string is open")
    }

    fn at_quotes(&self, quote: char, size: usize) -> bool {
        (0..size).all(|offset| self.peek_at(offset) == Some(quote))
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.line += u32::from(c == '\n');
            self.pos += 1;
        }
    }

    fn skip(&mut self, count: usize) {
        for _ in 0..count {
            self.bump();
        }
    }

    fn skip_while(&mut self, accept: impl Fn(char) -> bool) {
        while self.peek().is_some_and(&accept) {
            self.bump();
        }
    }

    fn count(&mut self) {
        self.count_line(self.line);
    }

    // Most repeats are of the last line. The set in run() removes the others.
    fn count_line(&mut self, line: u32) {
        if self.lines.last() != Some(&line) {
            self.lines.push(line);
        }
    }

    fn detected_line(&self) -> u32 {
        // At the end of input the scanner has passed the last line feed.
        self.line - u32::from(self.peek().is_none())
    }

    fn eof_error(&self, error: TokenizeError) -> TokenizeError {
        if self.cut_at_null {
            return token_error("source code cannot contain null bytes");
        }
        error
    }
}

fn scan_buffer(source: &str) -> (Vec<char>, bool) {
    let mut chars = Vec::with_capacity(source.len() + 1);
    for piece in text::splitlines(source, true) {
        // A line with a null byte stops the scan, which fails when it reaches the cut.
        if piece.contains('\0') {
            return (chars, true);
        }
        // Every line ends in one line feed; a carriage return before it changes no token.
        let body = piece.strip_suffix('\n').unwrap_or(piece);
        chars.extend(body.strip_suffix('\r').unwrap_or(body).chars());
        chars.push('\n');
    }
    (chars, false)
}

fn radix(marker: Option<char>) -> (&'static str, fn(char) -> bool) {
    match marker {
        Some('x' | 'X') => ("hexadecimal", is_hex_digit),
        Some('o' | 'O') => ("octal", is_octal_digit),
        _ => ("binary", is_binary_digit),
    }
}

fn radix_error(name: &str, found: Option<char>) -> TokenizeError {
    match found {
        Some(c) if c.is_ascii_digit() => {
            token_error(format!("invalid digit '{c}' in {name} literal"))
        }
        _ => token_error(format!("invalid {name} literal")),
    }
}

fn is_hex_digit(c: char) -> bool {
    c.is_ascii_hexdigit()
}

fn is_octal_digit(c: char) -> bool {
    matches!(c, '0'..='7')
}

fn is_binary_digit(c: char) -> bool {
    matches!(c, '0' | '1')
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit()
}

fn token_error(message: impl Into<String>) -> TokenizeError {
    TokenizeError {
        kind: TokenizeErrorKind::Token,
        message: message.into(),
    }
}

fn indentation_error(message: &str) -> TokenizeError {
    TokenizeError {
        kind: TokenizeErrorKind::Indentation,
        message: message.to_owned(),
    }
}

fn tab_error() -> TokenizeError {
    TokenizeError {
        kind: TokenizeErrorKind::Tab,
        message: "inconsistent use of tabs and spaces in indentation".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use text::testing::cases;

    #[test]
    fn fixture_cases() {
        let lines: Vec<(String, BTreeSet<u32>)> =
            cases(env!("CARGO_MANIFEST_DIR"), "tokenize", "lines");
        for (source, expected) in lines {
            assert_eq!(python_sloc(&source), Ok(expected), "source: {source:?}");
        }
        let errors: Vec<(String, String, String)> =
            cases(env!("CARGO_MANIFEST_DIR"), "tokenize", "errors");
        for (source, class_name, message) in errors {
            let err = python_sloc(&source).expect_err(&source);
            let got = (err.kind.class_name().to_owned(), err.message);
            assert_eq!(got, (class_name, message), "source: {source:?}");
        }
    }

    #[test]
    fn nested_brackets_199() {
        let source = format!("{}\n", "(".repeat(199) + &")".repeat(199));
        let got = python_sloc(&source).expect("199 nested brackets should not error");
        assert_eq!(got, [1].into_iter().collect());
    }

    #[test]
    fn nested_brackets_201() {
        let source = format!("{}\n", "(".repeat(201));
        let err = python_sloc(&source).expect_err("201 nested brackets should error");
        assert_eq!(err.kind, TokenizeErrorKind::Token);
        assert_eq!(err.message, "too many nested parentheses");
    }

    #[test]
    fn too_many_indent_levels() {
        let err = python_sloc(&nested_ifs(101)).expect_err("101 indent levels should error");
        assert_eq!(err.kind, TokenizeErrorKind::Indentation);
        assert_eq!(err.message, "too many levels of indentation");
    }

    #[test]
    fn indent_levels_at_the_limit() {
        let got = python_sloc(&nested_ifs(99)).expect("99 indent levels should not error");
        assert_eq!(got, (1..=100).collect());
        let err = python_sloc(&nested_ifs(100)).expect_err("100 indent levels should error");
        assert_eq!(err.message, "too many levels of indentation");
    }

    #[test]
    fn display_writes_class_and_message() {
        let err = python_sloc("'''abc").expect_err("an open triple quote should error");
        assert_eq!(err.to_string(), "TokenError: EOF in multi-line string");
    }

    #[test]
    fn nested_brackets_200() {
        let source = format!("{}{}\n", "(".repeat(200), ")".repeat(200));
        let got = python_sloc(&source).expect("200 nested brackets should not error");
        assert_eq!(got, [1].into_iter().collect());
    }

    #[test]
    fn fstring_nesting_limit() {
        let err = python_sloc(&format!("{}\n", "f'{".repeat(149))).expect_err("open f-strings");
        assert_eq!(err.message, "unexpected EOF in multi-line statement");
        let err = python_sloc(&format!("{}\n", "f'{".repeat(150))).expect_err("150 f-strings");
        assert_eq!(err.message, "too many nested f-strings");
    }

    fn nested_ifs(levels: usize) -> String {
        let mut source = String::from("if a:\n");
        for i in 0..levels {
            source.push_str(&" ".repeat(i + 1));
            source.push_str("if a");
            source.push_str(&i.to_string());
            source.push_str(":\n");
        }
        source
    }
}
