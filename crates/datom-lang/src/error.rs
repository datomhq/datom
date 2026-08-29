use std::{error::Error, fmt::Display};

use crate::scanner::TokenKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompileError {
    Scan(ScanError),
    Parse(ParseError),
}

impl Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scan(err) => write!(f, "{}", err),
            Self::Parse(err) => write!(f, "{}", err),
        }
    }
}

impl Error for CompileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scan(err) => Some(err),
            Self::Parse(err) => Some(err),
        }
    }
}

impl From<ScanError> for CompileError {
    fn from(value: ScanError) -> Self {
        CompileError::Scan(value)
    }
}

impl From<ParseError> for CompileError {
    fn from(value: ParseError) -> Self {
        CompileError::Parse(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanError {
    UnexpectedChar,
    UnterminatedString,
}

impl Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedChar => write!(f, "unexpected character"),
            Self::UnterminatedString => write!(f, "unterminated string"),
        }
    }
}

impl Error for ScanError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseError {
    Expected(Vec<TokenKind>, Option<TokenKind>),
}

impl Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Expected(expected, actual) => {
                f.write_str("expected ")?;

                for (i, kind) in expected.iter().enumerate() {
                    let separator = match i {
                        0 => "",
                        i if i + 1 == expected.len() => " or ",
                        _ => ", ",
                    };

                    f.write_str(separator)?;
                    write!(f, "{kind}")?;
                }

                match actual {
                    Some(actual) => write!(f, ", found {actual}"),
                    None => f.write_str(", found nothing"),
                }
            }
        }
    }
}

impl Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Primitive, scanner::Keyword};

    #[test]
    fn a_single_expected_kind_reads_as_a_sentence() {
        let err = ParseError::Expected(vec![TokenKind::Identifier], Some(TokenKind::LeftCurly));
        assert_eq!(err.to_string(), "expected an identifier, found `{`");
    }

    #[test]
    fn several_expected_kinds_are_joined_by_or() {
        let err = ParseError::Expected(
            vec![
                TokenKind::LeftParen,
                TokenKind::LeftCurly,
                TokenKind::Equals,
            ],
            Some(TokenKind::Keyword(Keyword::Primitive(Primitive::Number))),
        );
        assert_eq!(err.to_string(), "expected `(`, `{` or `=`, found `number`");
    }

    #[test]
    fn running_out_of_tokens_is_reported_as_eof() {
        let err = ParseError::Expected(vec![TokenKind::Semicolon], Some(TokenKind::Eof));
        assert_eq!(err.to_string(), "expected `;`, found <EOF>");
    }

    #[test]
    fn a_missing_actual_token_is_reported_as_nothing() {
        let err = ParseError::Expected(vec![TokenKind::Semicolon], None);
        assert_eq!(err.to_string(), "expected `;`, found nothing");
    }
}
