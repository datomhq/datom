use crate::{diagnostics::Diagnostics, error::CompileError, parser::Program};

pub(crate) struct TypedProgram {}

pub(crate) fn typecheck(
    _source: &str,
    _diagnostics: &Diagnostics,
    _program: Program,
) -> Result<TypedProgram, CompileError> {
    Ok(TypedProgram {})
}
