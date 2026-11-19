use std::sync::Arc;

use crate::{Type, ast::NumberLiteral, src_span::SrcSpan};

#[derive(Debug, Clone)]
pub enum TypedExpr {
    Number {
        location: SrcSpan,
        value: NumberLiteral,
        ty: Arc<Type>,
    },
}
