#![allow(dead_code)]

use std::fmt::{self, Display, Formatter};

pub(crate) mod diagnostics;
pub(crate) mod error;
pub(crate) mod parser;
pub(crate) mod scanner;
pub(crate) mod tree;
pub(crate) mod types;

/// A parsed source file, rendered for reading.
///
/// The AST itself stays internal while the compiler's shape is in flux, so what
/// crosses the crate boundary is the rendering rather than the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The syntax tree as an indented outline, one node per line.
    pub tree: String,
    /// Diagnostics raised on the way, one per line; empty when there were none.
    pub diagnostics: String,
}

/// A fatal error that stopped compilation, already rendered for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileFailure {
    message: String,
    diagnostics: String,
}

impl CompileFailure {
    /// Diagnostics raised before compilation stopped, one per line.
    ///
    /// A parse error aborts at the first token it cannot use, so anything the
    /// scanner reported on the way here is still worth showing.
    pub fn diagnostics(&self) -> &str {
        &self.diagnostics
    }
}

impl Display for CompileFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CompileFailure {}

/// Parse `source` and render its syntax tree.
///
/// This is what `datom parse` prints. Unlike [`compile`], it reports the
/// diagnostics collected along the way instead of dropping them.
pub fn parse(source: &str) -> Result<Parsed, CompileFailure> {
    let diag = diagnostics::Diagnostics::new();
    let tokens = scanner::scan(source, &diag);

    match parser::parse(source, &diag, tokens) {
        Ok(program) => Ok(Parsed {
            tree: tree::render(source, &program),
            diagnostics: diag.render(source),
        }),
        Err(err) => Err(CompileFailure {
            message: err.to_string(),
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
