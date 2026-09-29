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

#[derive(Debug, PartialEq, Clone)]
pub(crate) enum UntypedExpr {
    Number {
        location: SrcSpan,
        value: f64,
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
