#![allow(dead_code)]

use std::fmt::{self, Display, Formatter};

pub(crate) mod diagnostics;
pub(crate) mod error;
pub(crate) mod parser;
pub(crate) mod scanner;
pub(crate) mod tree;
pub(crate) mod types;

/// The diagnostics from a compilation that failed, one per line.
///
/// Compilation aborts at the first token it cannot use, so this is everything
/// reported up to and including the error that stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileFailure {
    diagnostics: String,
}

impl Display for CompileFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.diagnostics.trim_end())
    }
}

impl std::error::Error for CompileFailure {}

/// Parse `source` and render its syntax tree.
///
/// Unlike [`compile`], a failure carries the
/// diagnostics collected along the way instead of dropping them.
pub fn parse(source: &str) -> Result<String, CompileFailure> {
    let diag = diagnostics::Diagnostics::new();
    let tokens = scanner::scan(source, &diag);

    match parser::parse(source, &diag, tokens) {
        Ok(program) => Ok(tree::render(source, &program)),
        Err(_) => Err(CompileFailure {
            diagnostics: diag.render(source),
        }),
    }
}

// The final signature will not return the parser::Program AST; this is just done as a stopgap for now.
#[allow(private_interfaces)]
/// Compile the source code into an executable representation.
pub fn compile(source: &str) -> Result<parser::Program, error::CompileError> {
    let diag = diagnostics::Diagnostics::new();
    let tokens = scanner::scan(source, &diag);
    parser::parse(source, &diag, tokens)

    // as more stages accumulate, you might have:
    // if !diag.is_ok() { ... }
    // to stop compilation once errors appear, for example
}
