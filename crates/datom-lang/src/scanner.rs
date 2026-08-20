use std::{fmt::Display, iter::Peekable, range::Range, str::Chars};

use crate::{
    Primitive,
    diagnostics::Diagnostics,
    error::{CompileError, ScanError},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Equals,
    Bar,
    Semicolon,
    LeftParen,
    RightParen,
    LeftCurly,
    RightCurly,
    Comma,
    Colon,
    Keyword(Keyword),
    Identifier,
    Eof,
}

impl Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let repr = match self {
            TokenKind::Equals => "`=`",
            TokenKind::Bar => "`|`",
            TokenKind::Semicolon => "`;`",
            TokenKind::LeftParen => "`(`",
            TokenKind::RightParen => "`)`",
            TokenKind::LeftCurly => "`{`",
            TokenKind::RightCurly => "`}`",
            TokenKind::Comma => "`,`",
            TokenKind::Colon => "`:`",
            TokenKind::Keyword(Keyword::Type) => "`type`",
            TokenKind::Keyword(Keyword::Primitive(primitive)) => return write!(f, "`{primitive}`"),
            TokenKind::Identifier => "an identifier",
            TokenKind::Eof => "<EOF>",
        };

        write!(f, "{repr}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keyword {
    Type,
    Primitive(Primitive),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Token {
    /// The kind this token is.
    pub kind: TokenKind,
    /// The range of the source string where this token was found.
    pub range: Range<usize>,
}

impl Token {
    fn new(kind: TokenKind, start: usize, end: usize) -> Self {
        Self {
            kind,
            range: Range { start, end },
        }
    }

    pub(crate) fn lexeme<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range]
    }

    #[cfg(test)]
    fn assert_equals(&self, source: &str, kind: TokenKind, lexeme: &str) {
        assert_eq!(self.kind, kind);
        assert_eq!(self.lexeme(source), lexeme);
    }
}

pub(crate) fn scan(
    source: &str,
    diagnostics: &Diagnostics,
) -> impl Iterator<Item = Result<Token, CompileError>> {
    Scanner::new(source, diagnostics)
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
enum ScannerState {
    /// The scanner has been initialized but scanning has not comenced yet.
    #[default]
    Start,
    /// The scanner is mid-input and scanning characters.
    Scanning,
    /// The scanner has processed all characters and has yielded the Eof token.
    Exhausted,
    /// The scanner has encountered an unrecoverable error. Further calls to the iterator will yield None.
    Failed,
}

struct Scanner<'s, 'd> {
    source: &'s str,
    chars: Peekable<Chars<'s>>,
    diag: &'d Diagnostics,
    offset: usize,
    state: ScannerState,
}

impl<'s, 'd> Scanner<'s, 'd> {
    fn new(source: &'s str, diagnostics: &'d Diagnostics) -> Self {
        Self {
            source,
            chars: source.chars().peekable(),
            diag: diagnostics,
            offset: 0,
            state: ScannerState::default(),
        }
    }

    fn advance(&mut self) -> Option<char> {
        // on the first call to advance, don't increment offset
        // this is because offset starts at 0 but chars starts _before_ the first char (i.e. index -1)
        // and since offset is a usize, it can't store -1
        if self.state == ScannerState::Start {
            self.state = ScannerState::Scanning;
        } else {
            self.offset += 1;
        }

        self.chars.next()
    }

    fn ident_or_keyword(&mut self) -> Token {
        let start = self.offset;

        // whatever char stops the identifier is handled by `next()` as the start of a new token
        while self.chars.peek().is_some_and(|&c| is_ident_char(c)) {
            self.advance();
        }

        let end = self.offset + 1;
        let ident = &self.source[start..end];

        let kind = match ident {
            "type" => TokenKind::Keyword(Keyword::Type),
            "string" => TokenKind::Keyword(Keyword::Primitive(Primitive::String)),
            "number" => TokenKind::Keyword(Keyword::Primitive(Primitive::Number)),
            "bool" => TokenKind::Keyword(Keyword::Primitive(Primitive::Bool)),
            "datetime" => TokenKind::Keyword(Keyword::Primitive(Primitive::DateTime)),
            _ => TokenKind::Identifier,
        };

        Token::new(kind, start, end)
    }
}

impl<'s, 'd> Iterator for Scanner<'s, 'd> {
    type Item = Result<Token, CompileError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            // if the scanner has failed or already exhausted the chars, always yield none
            ScannerState::Failed | ScannerState::Exhausted => None,

            // otherwise, keep scanning
            ScannerState::Start | ScannerState::Scanning => {
                while let Some(char) = self.advance() {
                    let token = match char {
                        '=' => Token::new(TokenKind::Equals, self.offset, self.offset + 1),
                        '|' => Token::new(TokenKind::Bar, self.offset, self.offset + 1),
                        ';' => Token::new(TokenKind::Semicolon, self.offset, self.offset + 1),
                        '(' => Token::new(TokenKind::LeftParen, self.offset, self.offset + 1),
                        ')' => Token::new(TokenKind::RightParen, self.offset, self.offset + 1),
                        '{' => Token::new(TokenKind::LeftCurly, self.offset, self.offset + 1),
                        '}' => Token::new(TokenKind::RightCurly, self.offset, self.offset + 1),
                        ',' => Token::new(TokenKind::Comma, self.offset, self.offset + 1),
                        ':' => Token::new(TokenKind::Colon, self.offset, self.offset + 1),
                        _ if char.is_alphabetic() => self.ident_or_keyword(),
                        _ if char.is_whitespace() => continue,
                        _ => {
                            self.diag.error(
                                format!("Unexpected character '{}'", char),
                                self.offset,
                                self.offset + 1,
                            );

                            self.state = ScannerState::Failed;
                            return Some(Err(ScanError::UnexpectedChar.into()));
                        }
                    };

                    return Some(Ok(token));
                }

                self.state = ScannerState::Exhausted;
                Some(Ok(Token::new(TokenKind::Eof, self.offset, self.offset)))
            }
        }
    }
}

/// Whether `c` can appear in an identifier after the first character
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_tokens(
        source: &str,
        iter: impl Iterator<Item = Result<Token, CompileError>>,
        expected: &[(TokenKind, &'static str)],
    ) {
        let actual_tokens = iter.collect::<Vec<Result<Token, CompileError>>>();
        assert_eq!(actual_tokens.len(), expected.len());

        for (actual, expected) in actual_tokens.into_iter().zip(expected.iter()) {
            assert!(actual.is_ok());
            actual
                .unwrap()
                .assert_equals(source, expected.0, expected.1);
        }
    }

    // This is just for debugging test failures
    #[allow(dead_code)]
    fn inspect_tokens(source: &str, iter: impl Iterator<Item = Result<Token, CompileError>>) {
        // eagerly collect items to separate debug logging inside the scanner
        let tokens = iter.collect::<Vec<Result<Token, CompileError>>>();

        for token in tokens {
            match token {
                Ok(token) => {
                    eprintln!("kind: {:?}, lexeme: '{}'", token.kind, token.lexeme(source))
                }
                Err(err) => eprintln!("Err({:?})", err),
            }
        }
    }

    #[test]
    fn empty_source() {
        let source = "";
        let diagnostics = Diagnostics::new();
        let iter = scan(source, &diagnostics);
        assert_tokens(source, iter, &[(TokenKind::Eof, "")])
    }

    #[test]
    fn punctuation() {
        let source = "(){}:,|=;";
        let diagnostics = Diagnostics::new();
        let iter = scan(source, &diagnostics);
        assert_tokens(
            source,
            iter,
            &[
                (TokenKind::LeftParen, "("),
                (TokenKind::RightParen, ")"),
                (TokenKind::LeftCurly, "{"),
                (TokenKind::RightCurly, "}"),
                (TokenKind::Colon, ":"),
                (TokenKind::Comma, ","),
                (TokenKind::Bar, "|"),
                (TokenKind::Equals, "="),
                (TokenKind::Semicolon, ";"),
                (TokenKind::Eof, ""),
            ],
        );
    }

    #[test]
    fn keywords() {
        let source = "type string number bool datetime";
        let diagnostics = Diagnostics::new();
        let iter = scan(source, &diagnostics);
        assert_tokens(
            source,
            iter,
            &[
                (TokenKind::Keyword(Keyword::Type), "type"),
                (
                    TokenKind::Keyword(Keyword::Primitive(Primitive::String)),
                    "string",
                ),
                (
                    TokenKind::Keyword(Keyword::Primitive(Primitive::Number)),
                    "number",
                ),
                (
                    TokenKind::Keyword(Keyword::Primitive(Primitive::Bool)),
                    "bool",
                ),
                (
                    TokenKind::Keyword(Keyword::Primitive(Primitive::DateTime)),
                    "datetime",
                ),
                (TokenKind::Eof, ""),
            ],
        );
    }

    #[test]
    fn identifiers_end_at_punctuation() {
        let source = "type Person{name:string}";
        let diagnostics = Diagnostics::new();
        let iter = scan(source, &diagnostics);
        assert_tokens(
            source,
            iter,
            &[
                (TokenKind::Keyword(Keyword::Type), "type"),
                (TokenKind::Identifier, "Person"),
                (TokenKind::LeftCurly, "{"),
                (TokenKind::Identifier, "name"),
                (TokenKind::Colon, ":"),
                (
                    TokenKind::Keyword(Keyword::Primitive(Primitive::String)),
                    "string",
                ),
                (TokenKind::RightCurly, "}"),
                (TokenKind::Eof, ""),
            ],
        );
    }

    #[test]
    fn full_multiline_type() {
        let source = r#"type Person(
            id: number,
            name: string,
        )"#;
        let diagnostics = Diagnostics::new();
        let iter = scan(source, &diagnostics);
        assert_tokens(
            source,
            iter,
            &[
                (TokenKind::Keyword(Keyword::Type), "type"),
                (TokenKind::Identifier, "Person"),
                (TokenKind::LeftParen, "("),
                (TokenKind::Identifier, "id"),
                (TokenKind::Colon, ":"),
                (
                    TokenKind::Keyword(Keyword::Primitive(Primitive::Number)),
                    "number",
                ),
                (TokenKind::Comma, ","),
                (TokenKind::Identifier, "name"),
                (TokenKind::Colon, ":"),
                (
                    TokenKind::Keyword(Keyword::Primitive(Primitive::String)),
                    "string",
                ),
                (TokenKind::Comma, ","),
                (TokenKind::RightParen, ")"),
                (TokenKind::Eof, ""),
            ],
        );
    }

    #[test]
    fn identifier_character_classes() {
        let source = "lowercase UpperCase snake_case numb3rs Ev3ry_Thing x happy_trails_";
        let diagnostics = Diagnostics::new();
        let iter = scan(source, &diagnostics);
        assert_tokens(
            source,
            iter,
            &[
                (TokenKind::Identifier, "lowercase"),
                (TokenKind::Identifier, "UpperCase"),
                (TokenKind::Identifier, "snake_case"),
                (TokenKind::Identifier, "numb3rs"),
                (TokenKind::Identifier, "Ev3ry_Thing"),
                (TokenKind::Identifier, "x"),
                (TokenKind::Identifier, "happy_trails_"),
                // TODO: design decisions + non-ASCII:
                // (TokenKind::Identifier, "_lead_poisoning"),
                // (TokenKind::Identifier, "99luftballons"),
                // (TokenKind::Identifier, "café_ouioui"),
                (TokenKind::Eof, ""),
            ],
        );
    }

    #[test]
    fn unexpected_char_emits_diagnostic_and_fails() {
        let source = "type Pers#n{}";
        let diagnostics = Diagnostics::new();
        let iter = scan(source, &diagnostics);

        // consume the entire iter to force scanning
        let tokens = iter.collect::<Vec<_>>();

        // assert the iter yielded an Err variant for the unexpected char
        assert!(tokens[2].is_err());

        // make sure the error was tracked
        assert!(!diagnostics.is_ok());

        let rendered = diagnostics.render(source);

        // has the correct line:col coordinate
        assert!(rendered.contains("[1:9]"));

        // indicates it's an error severity
        assert!(rendered.contains("error"));

        // calls out the problematic character
        assert!(rendered.contains("'#'"));
    }

    #[test]
    fn trailing_whitespace_reaches_eof() {
        let source = "bool ";
        let diagnostics = Diagnostics::new();
        let iter = scan(source, &diagnostics);
        assert_tokens(
            source,
            iter,
            &[
                (
                    TokenKind::Keyword(Keyword::Primitive(Primitive::Bool)),
                    "bool",
                ),
                (TokenKind::Eof, ""),
            ],
        );
    }
}
