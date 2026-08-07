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
}

impl Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedChar => write!(f, "unexpected character"),
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
                // TODO: clean up this formatting to not use Debug
                write!(f, "Expected one of '{:?}', got '{:?}'", expected, actual)
            }
        }
    }
}

impl Error for ParseError {}
