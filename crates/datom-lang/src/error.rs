use std::{error::Error, fmt::Display};

use crate::scanner::TokenKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompileError {
    Scan(ScanError),
    Parse(ParseError),
    Lower(LowerError),
}

impl Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scan(err) => write!(f, "{}", err),
            Self::Parse(err) => write!(f, "{}", err),
            Self::Lower(err) => write!(f, "{}", err),
        }
    }
}

impl Error for CompileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scan(err) => Some(err),
            Self::Parse(err) => Some(err),
            Self::Lower(err) => Some(err),
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

impl From<LowerError> for CompileError {
    fn from(value: LowerError) -> Self {
        CompileError::Lower(value)
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
            Self::UnexpectedChar => write!(f, "Unexpected character"),
            Self::UnterminatedString => write!(f, "Unterminated string"),
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
                f.write_str("Expected ")?;

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

/// A failure to turn a well-formed syntax tree into the semantic type model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LowerError {
    /// A type name no declaration in scope introduces.
    UnknownType(String),
    /// A second declaration claiming a name already taken.
    DuplicateType(String),
    /// A field name the record already has.
    DuplicateField(String),
}

impl Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownType(name) => write!(f, "Unknown type `{name}`"),
            Self::DuplicateType(name) => write!(f, "Duplicate type `{name}`"),
            Self::DuplicateField(name) => write!(f, "Duplicate field `{name}`"),
        }
    }
}

impl Error for LowerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{scanner::Keyword, types::Primitive};

    #[test]
    fn a_single_expected_kind_reads_as_a_sentence() {
        let err = ParseError::Expected(vec![TokenKind::Identifier], Some(TokenKind::LeftCurly));
        assert_eq!(err.to_string(), "Expected an identifier, found `{`");
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
        assert_eq!(err.to_string(), "Expected `(`, `{` or `=`, found `number`");
    }

    #[test]
    fn running_out_of_tokens_is_reported_as_eof() {
        let err = ParseError::Expected(vec![TokenKind::Semicolon], Some(TokenKind::Eof));
        assert_eq!(err.to_string(), "Expected `;`, found <EOF>");
    }

    #[test]
    fn a_missing_actual_token_is_reported_as_nothing() {
        let err = ParseError::Expected(vec![TokenKind::Semicolon], None);
        assert_eq!(err.to_string(), "Expected `;`, found nothing");
    }
}
