use crate::src_span::SrcSpan;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum BinOp {
    Plus,
    Minus,
    Times,
    Divide,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum UnaryOp {
    Not,
    Negative,
}

/// Borrowed from Glean's internals, this is a thin wrapper over an f64 that
/// disallows NaN so we can implement Eq.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) struct NumberLiteral(f64);

impl Eq for NumberLiteral {}

impl NumberLiteral {
    /// Parse from a string, returning `None` if the string
    /// is not a valid f64 or the float is `NaN``
    pub fn parse(value: &str) -> Option<Self> {
        value
            .replace("_", "")
            .parse::<f64>()
            .ok()
            .filter(|float| !float.is_nan())
            .map(NumberLiteral)
    }

    pub fn value(&self) -> f64 {
        self.0
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum UntypedExpr {
    Number {
        location: SrcSpan,
        value: NumberLiteral,
    },
    String {
        location: SrcSpan,
        value: String,
    },
    Bool {
        location: SrcSpan,
        value: bool,
    },
    BinOp {
        location: SrcSpan,
        operator: BinOp,
        left: Box<Self>,
        right: Box<Self>,
    },
    UnaryOp {
        location: SrcSpan,
        operator: UnaryOp,
        right: Box<Self>,
    },
}

impl UntypedExpr {
    pub(crate) fn location(&self) -> &SrcSpan {
        match self {
            UntypedExpr::Number { location, .. }
            | UntypedExpr::String { location, .. }
            | UntypedExpr::Bool { location, .. }
            | UntypedExpr::BinOp { location, .. }
            | UntypedExpr::UnaryOp { location, .. } => location,
        }
    }
}
