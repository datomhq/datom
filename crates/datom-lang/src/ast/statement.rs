use crate::{
    ast::{TypedExpr, UntypedExpr},
    src_span::SrcSpan,
};

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct ExprStmt<ExprT> {
    expr: ExprT,
}

pub(crate) type UntypedExprStmt = ExprStmt<UntypedExpr>;
pub(crate) type TypedExprStmt = ExprStmt<TypedExpr>;

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Assignment<ExprT> {
    location: SrcSpan,
    variable: String,
    value: ExprT,
}

pub(crate) type UntypedAssignment = Assignment<UntypedExpr>;
pub(crate) type TypedAssignment = Assignment<TypedExpr>;

impl<ExprT> Assignment<ExprT> {
    pub fn new(location: SrcSpan, variable: String, value: ExprT) -> Self {
        Self {
            location,
            variable,
            value,
        }
    }

    pub fn lexeme<'src>(&self, source: &'src str) -> &'src str {
        return self.location.lexeme(source);
    }
}

