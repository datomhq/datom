#![allow(dead_code)]

use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
};

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

impl Display for Primitive {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let repr = match self {
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::String => "string",
            Self::Bool => "bool",
            Self::DateTime => "datetime",
        };

        write!(f, "{repr}")
    }
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

impl Display for Type {
    /// Pre-order DFS traversal of the type tree.
    /// Each node writes its own fields before descending.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primitive(primitive) => write!(f, "{primitive}"),
            Self::Sum(sum) => write!(f, "{sum}"),
        }
    }
}

impl Display for Sum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            // e.g., `(id: u32, name: string)`
            Self::Single(fields) => write_fields(f, fields),

            // e.g., `{ Employee(name: string), Robot(id: u32) }`
            Self::Variadic(variants) => {
                f.write_str("{ ")?;
                for (i, (name, fields)) in variants.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(name)?;
                    write_fields(f, fields)?;
                }
                f.write_str(" }")
            }
        }
    }
}

/// Writes a parenthesised list of fields.
fn write_fields(f: &mut Formatter<'_>, fields: &Fields) -> fmt::Result {
    let mut names: Vec<&str> = fields.keys().map(String::as_str).collect();
    names.sort_unstable();

    f.write_str("(")?;
    for (i, name) in names.into_iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{name}: {}", fields[name])?;
    }
    f.write_str(")")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a [`Fields`] map from `(name, type)` pairs.
    fn fields<const N: usize>(entries: [(&str, Type); N]) -> Fields {
        entries
            .into_iter()
            .map(|(name, ty)| (String::from(name), Box::new(ty)))
            .collect()
    }

    #[test]
    fn a_primitive_type_prints_as_the_primitive() {
        assert_eq!(Type::Primitive(Primitive::DateTime).to_string(), "datetime");
    }

    #[test]
    fn a_single_sum_prints_its_fields() {
        let cell = Type::Sum(Sum::Single(fields([
            ("nucleus", Type::Primitive(Primitive::Bool)),
            ("wall", Type::Primitive(Primitive::Bool)),
        ])));

        assert_eq!(cell.to_string(), "(nucleus: bool, wall: bool)");
    }

    #[test]
    fn a_variadic_sum_prints_every_variant() {
        let person = Type::Sum(Sum::Variadic(vec![
            (
                String::from("Student"),
                fields([("id", Type::Primitive(Primitive::U32))]),
            ),
            (
                String::from("Professor"),
                fields([("tenured", Type::Primitive(Primitive::Bool))]),
            ),
        ]));

        assert_eq!(
            person.to_string(),
            "{ Student(id: u32), Professor(tenured: bool) }"
        );
    }

    #[test]
    fn nested_sums_recurse_down_to_their_primitives() {
        let address = Type::Sum(Sum::Single(fields([("city", Type::Primitive(Primitive::String))])));
        let person = Type::Sum(Sum::Single(fields([
            ("home", address),
            ("id", Type::Primitive(Primitive::U32)),
        ])));

        assert_eq!(person.to_string(), "(home: (city: string), id: u32)");
    }

    #[test]
    fn a_variant_may_nest_a_sum_too() {
        let major = Type::Sum(Sum::Variadic(vec![
            (String::from("Undeclared"), fields([])),
            (
                String::from("Declared"),
                fields([("name", Type::Primitive(Primitive::String))]),
            ),
        ]));
        let student = Type::Sum(Sum::Single(fields([("major", major)])));

        assert_eq!(
            student.to_string(),
            "(major: { Undeclared(), Declared(name: string) })"
        );
    }

    #[test]
    fn fields_print_in_a_stable_order() {
        let ty = Type::Sum(Sum::Single(fields([
            ("zebra", Type::Primitive(Primitive::Bool)),
            ("apple", Type::Primitive(Primitive::Bool)),
            ("middle", Type::Primitive(Primitive::F32)),
        ])));

        assert_eq!(ty.to_string(), "(apple: bool, middle: f32, zebra: bool)");
    }
}
