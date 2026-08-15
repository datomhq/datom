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

impl Type {
    /// A primitive type, named for the primitive itself.
    pub(crate) fn primitive(primitive: Primitive) -> Self {
        Self {
            name: primitive.to_string(),
            details: TypeDetails::Primitive(primitive),
        }
    }
}

/// Prints a type as the declaration that introduces it — `type Person(name: string)`.
impl Display for Type {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.details {
            TypeDetails::Primitive(_) => f.write_str(&self.name),
            TypeDetails::Sum(sum) => write!(f, "type {}{sum}", self.name),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum TypeDetails {
    Primitive(Primitive),
    Sum(Sum),
}

impl Display for TypeDetails {
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
                f.write_str(" { ")?;
                for (i, (name, fields)) in variants.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(name)?;
                    write_fields(f, fields)?;
                }
                f.write_str(" }")
            }

            // e.g., `Employee | Robot`
            Self::InlineVariadic(tys) => {
                for (i, ty) in tys.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    f.write_str(&ty.name)?;
                }
                Ok(())
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
        write!(f, "{name}: {}", fields[name].name)?;
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
            .map(|(name, ty)| (String::from(name), ty))
            .collect()
    }

    #[test]
    fn a_primitive_type_prints_as_the_primitive() {
        assert_eq!(
            TypeDetails::Primitive(Primitive::DateTime).to_string(),
            "datetime"
        );
    }

    #[test]
    fn a_single_sum_prints_its_fields() {
        let cell = Type {
            name: String::from("Cell"),
            details: TypeDetails::Sum(Sum::Single(fields([
                ("nucleus", Type::primitive(Primitive::Bool)),
                ("wall", Type::primitive(Primitive::Bool)),
            ]))),
        };
        assert_eq!(cell.to_string(), "type Cell(nucleus: bool, wall: bool)");
    }

    #[test]
    fn a_variadic_sum_prints_every_variant() {
        let person = Type {
            name: String::from("Person"),
            details: TypeDetails::Sum(Sum::Variadic(vec![
                (
                    String::from("Student"),
                    fields([("id", Type::primitive(Primitive::U32))]),
                ),
                (
                    String::from("Professor"),
                    fields([("tenured", Type::primitive(Primitive::Bool))]),
                ),
            ])),
        };

        assert_eq!(
            person.to_string(),
            "type Person { Student(id: u32), Professor(tenured: bool) }"
        );
    }

    #[test]
    fn nested_sums_recurse_down_to_their_primitives() {
        let address = Type {
            name: String::from("Address"),
            details: TypeDetails::Sum(Sum::Single(fields([(
                "city",
                Type::primitive(Primitive::String),
            )]))),
        };

        let person = Type {
            name: String::from("Person"),
            details: TypeDetails::Sum(Sum::Single(fields([
                ("home", address.clone()),
                ("id", Type::primitive(Primitive::U32)),
            ]))),
        };

        assert_eq!(
            format!("{address}\n{person}"),
            "type Address(city: string)\ntype Person(home: Address, id: u32)"
        );
    }

    #[test]
    fn a_variant_may_nest_a_sum_too() {
        let major = Type {
            name: String::from("Major"),
            details: TypeDetails::Sum(Sum::Variadic(vec![
                (String::from("Undeclared"), fields([])),
                (
                    String::from("Declared"),
                    fields([("name", Type::primitive(Primitive::String))]),
                ),
            ])),
        };

        let student = Type {
            name: String::from("Student"),
            details: TypeDetails::Sum(Sum::Single(fields([("major", major.clone())]))),
        };

        assert_eq!(
            format!("{major}\n{student}"),
            "type Major { Undeclared(), Declared(name: string) }\ntype Student(major: Major)"
        );
    }

    #[test]
    fn fields_print_in_a_stable_order() {
        let ty = Type {
            name: String::from("Zoo"),
            details: TypeDetails::Sum(Sum::Single(fields([
                ("zebra", Type::primitive(Primitive::Bool)),
                ("apple", Type::primitive(Primitive::Bool)),
                ("middle", Type::primitive(Primitive::F32)),
            ]))),
        };

        assert_eq!(
            ty.to_string(),
            "type Zoo(apple: bool, middle: f32, zebra: bool)"
        );
    }
}
