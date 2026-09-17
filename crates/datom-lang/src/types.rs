//! The datom type system.

use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
};

/// A primitive type within the datom type system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
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
pub enum Collection {
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

/// An opaque id for one type in a [`TypeTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(u32);

impl TypeId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// The primitives, in the order every table seeds them.
const PRIMITIVES: [Primitive; 4] = [
    Primitive::Number,
    Primitive::String,
    Primitive::Bool,
    Primitive::DateTime,
];

/// Every type a program mentions, addressed by [`TypeId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeTable {
    /// Every type: primitives, collections, declarations.
    types: Vec<Type>,
    /// What the source declared, in order.
    declared: Vec<TypeId>,
}

impl Default for TypeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeTable {
    /// A table holding nothing but the primitives.
    pub fn new() -> Self {
        let types = PRIMITIVES
            .into_iter()
            .map(|primitive| Type {
                name: primitive.to_string(),
                details: Some(TypeDetails::Primitive(primitive)),
            })
            .collect();

        Self {
            types,
            declared: Vec::new(),
        }
    }

    /// The id of a primitive, seeded before anything else is in the table.
    pub fn primitive(&self, primitive: Primitive) -> TypeId {
        let index = match primitive {
            Primitive::Number => 0,
            Primitive::String => 1,
            Primitive::Bool => 2,
            Primitive::DateTime => 3,
        };

        TypeId(index)
    }

    /// The id of `kind` over `inner`, interned.
    pub fn collection(&mut self, kind: Collection, inner: TypeId) -> TypeId {
        if let Some(id) = self.interned(kind, inner) {
            return id;
        }

        let name = format!("{kind}<{}>", self.name_of(inner));
        self.push(Type {
            name,
            details: Some(TypeDetails::Collection(kind, inner)),
        })
    }

    /// Take an id for a declared type.
    pub fn declare(&mut self, name: &str) -> TypeId {
        let id = self.push(Type {
            name: name.to_string(),
            details: None,
        });

        self.declared.push(id);
        id
    }

    /// Give a declared type the body it was reserved for.
    pub fn define(&mut self, id: TypeId, sum: Sum) {
        self.types[id.index()].details = Some(TypeDetails::Sum(sum));
    }

    /// Every declared type has a body; panics naming the first that does not.
    pub fn finish(self) -> Self {
        for id in self.iter() {
            let entry = &self.types[id.index()];
            assert!(
                entry.details.is_some(),
                "`{}` was declared but never defined",
                entry.name
            );
        }

        self
    }

    /// The type `id` addresses.
    pub fn get(&self, id: TypeId) -> &Type {
        &self.types[id.index()]
    }

    /// The name `id` is written under — `number`, `list<Category>`, `Person`.
    pub fn name_of(&self, id: TypeId) -> &str {
        &self.types[id.index()].name
    }

    /// The types the source declared, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.declared.iter().copied()
    }

    /// The id already holding `kind` over `inner`, if some field asked first.
    fn interned(&self, kind: Collection, inner: TypeId) -> Option<TypeId> {
        self.types
            .iter()
            .position(|ty| matches!(ty.details, Some(TypeDetails::Collection(k, i)) if k == kind && i == inner))
            .map(|index| TypeId(index as u32))
    }

    fn push(&mut self, ty: Type) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        self.types.push(ty);
        id
    }
}

/// The map of fields and their types for a datom sum type.
pub type Fields = HashMap<String, TypeId>;

/// A sum type within the datom type system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sum {
    /// A single sum type has only a single variant, implicitly named the same as the overall type.
    Single(Fields),
    /// A variadic sum type has multiple variants, each with a name and different fields.
    Variadic(Vec<(String, Fields)>),
    /// An inline variadic sum has type has multiple variants, each its own field.
    InlineVariadic(Vec<TypeId>),
}

impl Sum {
    /// Print the body of a declaration — the fields, variants or alternatives
    /// that follow its name.
    pub fn display<'a>(&'a self, table: &'a TypeTable) -> impl Display + 'a {
        Displayed { value: self, table }
    }
}

/// A type within the datom type system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    pub name: String,
    details: Option<TypeDetails>,
}

impl Type {
    /// What the type is.
    pub fn details(&self) -> &TypeDetails {
        self.details
            .as_ref()
            .unwrap_or_else(|| panic!("`{}` was declared but never defined", self.name))
    }

    /// Print the declaration that introduces this type — `type Person(name: string)`.
    pub fn display<'a>(&'a self, table: &'a TypeTable) -> impl Display + 'a {
        Displayed { value: self, table }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDetails {
    Primitive(Primitive),
    Sum(Sum),
    Collection(Collection, TypeId),
}

impl TypeDetails {
    /// Print what the type is, without the declaration around it.
    pub fn display<'a>(&'a self, table: &'a TypeTable) -> impl Display + 'a {
        Displayed { value: self, table }
    }
}

/// Something printable paired with the table its ids address.
struct Displayed<'a, T> {
    value: &'a T,
    table: &'a TypeTable,
}

/// Prints a type as the declaration that introduces it — `type Person(name: string)`.
impl Display for Displayed<'_, Type> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let details = self.value.details();

        match details {
            TypeDetails::Primitive(_) => f.write_str(&self.value.name),
            TypeDetails::Sum(sum) => {
                write!(f, "type {}{}", self.value.name, sum.display(self.table))
            }
            TypeDetails::Collection(..) => details.display(self.table).fmt(f),
        }
    }
}

impl Display for Displayed<'_, TypeDetails> {
    /// Pre-order DFS traversal of the type tree.
    /// Each node writes its own fields before descending.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.value {
            TypeDetails::Primitive(primitive) => write!(f, "{primitive}"),
            TypeDetails::Sum(sum) => sum.display(self.table).fmt(f),
            TypeDetails::Collection(kind, inner) => {
                write!(f, "{kind}<{}>", self.table.name_of(*inner))
            }
        }
    }
}

impl Display for Displayed<'_, Sum> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.value {
            // e.g., `(id: number, name: string)`
            Sum::Single(fields) => write_fields(f, fields, self.table),

            // e.g., `{ Employee(name: string), Robot(id: number) }`
            Sum::Variadic(variants) => {
                f.write_str(" { ")?;
                for (i, (name, fields)) in variants.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(name)?;
                    write_fields(f, fields, self.table)?;
                }
                f.write_str(" }")
            }

            // e.g., ` = Person | Robot`
            Sum::InlineVariadic(ids) => {
                f.write_str(" = ")?;
                for (i, id) in ids.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    f.write_str(self.table.name_of(*id))?;
                }
                f.write_str(";")?;
                Ok(())
            }
        }
    }
}

/// Writes a parenthesised list of fields.
fn write_fields(f: &mut Formatter<'_>, fields: &Fields, table: &TypeTable) -> fmt::Result {
    let mut names: Vec<&str> = fields.keys().map(String::as_str).collect();
    names.sort_unstable();

    f.write_str("(")?;
    for (i, name) in names.into_iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{name}: {}", table.name_of(fields[name]))?;
    }
    f.write_str(")")
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

    #[test]
    fn collections_print_sums() {
        let address = Type::single(
            "Address",
            fields([("city", Type::primitive(Primitive::String))]),
        );

        let ty = Type::collection(Collection::List, address);

        assert_eq!(ty.to_string(), "list<Address>");
    }

    #[test]
    fn fields_compare_without_regard_to_insertion_order() {
        let one = Type::single(
            "Zoo",
            fields([
                ("zebra", Type::primitive(Primitive::Bool)),
                ("apple", Type::primitive(Primitive::Bool)),
            ]),
        );

        let other = Type::single(
            "Zoo",
            fields([
                ("apple", Type::primitive(Primitive::Bool)),
                ("zebra", Type::primitive(Primitive::Bool)),
            ]),
        );

        assert_eq!(one, other);
    }

    #[test]
    fn variant_order_is_significant() {
        let variants = |first: &str, second: &str| {
            vec![
                (String::from(first), fields([])),
                (String::from(second), fields([])),
            ]
        };

        let one = Type::variadic("Major", variants("Undeclared", "Declared"));
        let other = Type::variadic("Major", variants("Declared", "Undeclared"));

        assert_ne!(one, other);
    }

    #[test]
    fn every_primitive_is_seeded_at_the_id_it_answers_with() {
        let table = TypeTable::new();

        for primitive in PRIMITIVES {
            assert_eq!(
                table.get(table.primitive(primitive)).details(),
                &TypeDetails::Primitive(primitive)
            );
        }
    }
}
