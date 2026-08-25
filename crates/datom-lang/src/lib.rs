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
    Number,
    String,
    Bool,
    DateTime,
}

impl Display for Primitive {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let repr = match self {
            Self::Number => "number",
            Self::String => "string",
            Self::Bool => "bool",
            Self::DateTime => "datetime",
        };

        write!(f, "{repr}")
    }
}

/// A collection type within the datom type system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Collection {
    List,
    Map,
    Set,
}

impl Display for Collection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let repr = match self {
            Self::List => "list",
            Self::Map => "map",
            Self::Set => "set",
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

#[derive(Debug, Clone)]
pub(crate) struct CollectionDetails {
    kind: Collection,
    generic: Box<Type>,
}

impl Display for CollectionDetails {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}<{}>", self.kind, self.generic)
    }
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

    /// A single sum type.
    pub(crate) fn single(name: &str, fields: Fields) -> Self {
        Self::of(name, Sum::Single(fields))
    }

    /// A variadic sum type — several named variants, each with its own fields.
    pub(crate) fn variadic(name: &str, variants: Vec<(String, Fields)>) -> Self {
        Self::of(name, Sum::Variadic(variants))
    }

    /// An inline variadic sum type — several variants, each an existing type.
    pub(crate) fn inline_variadic(name: &str, variants: Vec<Type>) -> Self {
        Self::of(name, Sum::InlineVariadic(variants))
    }

    pub(crate) fn collection(kind: Collection, generic: Type) -> Self {
        let details = CollectionDetails {
            kind,
            generic: Box::new(generic),
        };

        Self {
            name: details.to_string(),
            details: TypeDetails::Collection(details),
        }
    }

    fn of(name: &str, sum: Sum) -> Self {
        Self {
            name: name.to_string(),
            details: TypeDetails::Sum(sum),
        }
    }
}

/// Prints a type as the declaration that introduces it — `type Person(name: string)`.
impl Display for Type {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.details {
            TypeDetails::Primitive(_) => f.write_str(&self.name),
            TypeDetails::Sum(sum) => write!(f, "type {}{sum}", self.name),
            TypeDetails::Collection(collection) => write!(f, "{collection}"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum TypeDetails {
    Primitive(Primitive),
    Sum(Sum),
    Collection(CollectionDetails),
}

impl Display for TypeDetails {
    /// Pre-order DFS traversal of the type tree.
    /// Each node writes its own fields before descending.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primitive(primitive) => write!(f, "{primitive}"),
            Self::Sum(sum) => write!(f, "{sum}"),
            Self::Collection(collection) => write!(f, "{collection}"),
        }
    }
}

impl Display for Sum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            // e.g., `(id: number, name: string)`
            Self::Single(fields) => write_fields(f, fields),

            // e.g., `{ Employee(name: string), Robot(id: number) }`
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

            // e.g., ` = Person | Robot`
            Self::InlineVariadic(tys) => {
                f.write_str(" = ")?;
                for (i, ty) in tys.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    f.write_str(&ty.name)?;
                }
                f.write_str(";")?;
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
    use std::vec;

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
        let cell = Type::single(
            "Cell",
            fields([
                ("nucleus", Type::primitive(Primitive::Bool)),
                ("wall", Type::primitive(Primitive::Bool)),
            ]),
        );
        assert_eq!(cell.to_string(), "type Cell(nucleus: bool, wall: bool)");
    }

    #[test]
    fn a_variadic_sum_prints_every_variant() {
        let person = Type::variadic(
            "Person",
            vec![
                (
                    String::from("Student"),
                    fields([("id", Type::primitive(Primitive::Number))]),
                ),
                (
                    String::from("Professor"),
                    fields([("tenured", Type::primitive(Primitive::Bool))]),
                ),
            ],
        );

        assert_eq!(
            person.to_string(),
            "type Person { Student(id: number), Professor(tenured: bool) }"
        );
    }

    #[test]
    fn nested_sums_recurse_down_to_their_primitives() {
        let address = Type::single(
            "Address",
            fields([("city", Type::primitive(Primitive::String))]),
        );

        let person = Type::single(
            "Person",
            fields([
                ("home", address.clone()),
                ("id", Type::primitive(Primitive::Number)),
            ]),
        );

        assert_eq!(
            format!("{address}\n{person}"),
            "type Address(city: string)\ntype Person(home: Address, id: number)"
        );
    }

    #[test]
    fn a_variant_may_nest_a_sum_too() {
        let major = Type::variadic(
            "Major",
            vec![
                (String::from("Undeclared"), fields([])),
                (
                    String::from("Declared"),
                    fields([("name", Type::primitive(Primitive::String))]),
                ),
            ],
        );

        let student = Type::single("Student", fields([("major", major.clone())]));

        assert_eq!(
            format!("{major}\n{student}"),
            "type Major { Undeclared(), Declared(name: string) }\ntype Student(major: Major)"
        );
    }

    #[test]
    fn an_inline_variadic_prints_references() {
        let person = Type::single(
            "Person",
            fields([("name", Type::primitive(Primitive::String))]),
        );

        let robot = Type::single(
            "Robot",
            fields([("id", Type::primitive(Primitive::Number))]),
        );

        let employee = Type::inline_variadic("Employee", vec![person, robot]);

        assert_eq!(employee.to_string(), "type Employee = Person | Robot;");
    }

    #[test]
    fn an_inline_variadic_may_mix_primitives_and_singles() {
        let badge = Type::single(
            "Badge",
            fields([("serial", Type::primitive(Primitive::Number))]),
        );

        let id = Type::inline_variadic(
            "Id",
            vec![
                Type::primitive(Primitive::String),
                Type::primitive(Primitive::Number),
                badge,
            ],
        );

        assert_eq!(id.to_string(), "type Id = string | number | Badge;");
    }

    #[test]
    fn fields_print_in_a_stable_order() {
        let ty = Type::single(
            "Zoo",
            fields([
                ("zebra", Type::primitive(Primitive::Bool)),
                ("apple", Type::primitive(Primitive::Bool)),
                ("middle", Type::primitive(Primitive::Number)),
            ]),
        );

        assert_eq!(
            ty.to_string(),
            "type Zoo(apple: bool, middle: number, zebra: bool)"
        );
    }

    #[test]
    fn collections() {
        let ty = Type::collection(Collection::List, Type::primitive(Primitive::Number));
        assert_eq!(ty.to_string(), "list<number>");
    }

    #[test]
    fn nested_collections_recurse() {
        let inner = Type::collection(Collection::List, Type::primitive(Primitive::Number));
        let ty = Type::collection(Collection::List, inner);

        assert_eq!(ty.to_string(), "list<list<number>>");
    }

    #[test]
    fn collections_as_fields() {
        let collection = Type::collection(Collection::List, Type::primitive(Primitive::Bool));
        let ty = Type::single("Arena", fields([("items", collection)]));

        assert_eq!(ty.to_string(), "type Arena(items: list<bool>)")
    }
}
