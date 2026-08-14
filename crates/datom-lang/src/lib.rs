#![allow(dead_code)]

use std::collections::HashMap;

pub(crate) mod diagnostics;
pub(crate) mod error;
pub(crate) mod parser;
pub(crate) mod scanner;

/// A primitive type within the datom type system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Primitive {
    U32,
    I32,
    F32,
    F64,
    String,
    Bool,
    DateTime,
}

/// The map of fields and their types for a datom sum type.
pub(crate) type Fields = HashMap<String, Type>;

/// A sum type within the datom type system.
#[derive(Debug, Clone)]
pub(crate) enum Sum {
    /// A single sum type has only a single variant, implicitly named the same as the overall type.
    Single(Fields),
    /// A variadic sum type has multiple variants, each with a name and different fields.
    Variadic(Vec<(String, Fields)>),
    /// An inline variadic sum has type has multiple variants, each its own field.
    InlineVariadic(Vec<Type>),
}

/// A type within the datom type system.
#[derive(Debug, Clone)]
pub(crate) struct Type {
    pub name: String,
    pub details: TypeDetails,
}

#[derive(Debug, Clone)]
pub(crate) enum TypeDetails {
    Primitive(Primitive),
    Sum(Sum),
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
