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
        let details = TypeDetails::Collection(kind, inner);

        if let Some(index) = self
            .types
            .iter()
            .position(|ty| ty.details.as_ref() == Some(&details))
        {
            return TypeId(index as u32);
        }

        let name = format!("{kind}<{}>", self.name_of(inner));
        self.push(Type {
            name,
            details: Some(details),
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
            self.get(id).details();
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
        Displayed { ty: self, table }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDetails {
    Primitive(Primitive),
    Sum(Sum),
    Collection(Collection, TypeId),
}

/// A type paired with the table its ids address.
struct Displayed<'a> {
    ty: &'a Type,
    table: &'a TypeTable,
}

/// Prints a type as the declaration that introduces it — `type Person(name: string)`.
impl Display for Displayed<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let TypeDetails::Sum(sum) = self.ty.details() else {
            // A primitive or a collection is written as its own name.
            return f.write_str(&self.ty.name);
        };

        write!(f, "type {}", self.ty.name)?;

        match sum {
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

            // e.g., ` = Person | Robot;`
            Sum::InlineVariadic(ids) => {
                f.write_str(" = ")?;
                for (i, id) in ids.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    f.write_str(self.table.name_of(*id))?;
                }
                f.write_str(";")
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
    use super::*;

    /// Build a [`Fields`] map from `(name, id)` pairs.
    fn fields<const N: usize>(entries: [(&str, TypeId); N]) -> Fields {
        entries
            .into_iter()
            .map(|(name, id)| (String::from(name), id))
            .collect()
    }

    /// Declare `name` and give it `sum` in one step.
    fn ty(table: &mut TypeTable, name: &str, sum: Sum) -> TypeId {
        let id = table.declare(name);
        table.define(id, sum);
        id
    }

    /// How `id` prints as a declaration.
    fn rendered(table: &TypeTable, id: TypeId) -> String {
        table.get(id).display(table).to_string()
    }

    // Printing: one case per `Sum` shape. The exhaustive coverage lives in
    // `render::tests::the_tour_renders_as_source`, against a golden file.

    #[test]
    fn a_single_sum_prints_its_fields() {
        let mut table = TypeTable::new();
        let boolean = table.primitive(Primitive::Bool);

        let cell = ty(
            &mut table,
            "Cell",
            Sum::Single(fields([("nucleus", boolean), ("wall", boolean)])),
        );

        assert_eq!(
            rendered(&table, cell),
            "type Cell(nucleus: bool, wall: bool)"
        );
    }

    #[test]
    fn a_variadic_sum_prints_every_variant() {
        let mut table = TypeTable::new();
        let number = table.primitive(Primitive::Number);
        let boolean = table.primitive(Primitive::Bool);

        let person = ty(
            &mut table,
            "Person",
            Sum::Variadic(vec![
                (String::from("Student"), fields([("id", number)])),
                (String::from("Professor"), fields([("tenured", boolean)])),
            ]),
        );

        assert_eq!(
            rendered(&table, person),
            "type Person { Student(id: number), Professor(tenured: bool) }"
        );
    }

    #[test]
    fn an_inline_variadic_prints_the_names_it_unions() {
        let mut table = TypeTable::new();
        let string = table.primitive(Primitive::String);
        let number = table.primitive(Primitive::Number);

        let person = ty(
            &mut table,
            "Person",
            Sum::Single(fields([("name", string)])),
        );
        let robot = ty(&mut table, "Robot", Sum::Single(fields([("id", number)])));
        let employee = ty(
            &mut table,
            "Employee",
            Sum::InlineVariadic(vec![person, robot]),
        );

        assert_eq!(
            rendered(&table, employee),
            "type Employee = Person | Robot;"
        );
    }

    #[test]
    fn nested_collections_recurse() {
        let mut table = TypeTable::new();
        let number = table.primitive(Primitive::Number);

        let inner = table.collection(Collection::List, number);
        let outer = table.collection(Collection::List, inner);

        assert_eq!(rendered(&table, outer), "list<list<number>>");
    }

    #[test]
    fn fields_print_in_a_stable_order() {
        let mut table = TypeTable::new();
        let boolean = table.primitive(Primitive::Bool);
        let number = table.primitive(Primitive::Number);

        let zoo = ty(
            &mut table,
            "Zoo",
            Sum::Single(fields([
                ("zebra", boolean),
                ("apple", boolean),
                ("middle", number),
            ])),
        );

        assert_eq!(
            rendered(&table, zoo),
            "type Zoo(apple: bool, middle: number, zebra: bool)"
        );
    }

    // The table itself, which the golden file cannot reach.

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

    #[test]
    fn a_collection_interns_on_its_kind_and_element() {
        let mut table = TypeTable::new();
        let number = table.primitive(Primitive::Number);
        let string = table.primitive(Primitive::String);

        let list = table.collection(Collection::List, number);

        assert_eq!(table.collection(Collection::List, number), list);
        assert_ne!(table.collection(Collection::Set, number), list);
        assert_ne!(table.collection(Collection::List, string), list);
    }

    #[test]
    fn a_declaration_never_interns() {
        let mut table = TypeTable::new();
        let boolean = table.primitive(Primitive::Bool);

        let a = ty(&mut table, "A", Sum::Single(fields([("x", boolean)])));
        let b = ty(&mut table, "B", Sum::Single(fields([("x", boolean)])));

        // Same body, different types: equality is nominal.
        assert_ne!(a, b);
        // Even the same name takes a fresh id — the scope, not the table,
        // is what rejects a duplicate declaration.
        assert_ne!(table.declare("A"), a);
    }

    #[test]
    fn only_declared_types_are_iterated() {
        let mut table = TypeTable::new();
        let number = table.primitive(Primitive::Number);
        let cells = table.collection(Collection::List, number);

        ty(&mut table, "Grid", Sum::Single(fields([("cells", cells)])));

        // Primitives and `list<number>` are in the table, but nothing
        // declared them, so nothing may render them as declarations.
        let names: Vec<&str> = table.iter().map(|id| table.name_of(id)).collect();
        assert_eq!(names, ["Grid"]);
    }

    #[test]
    fn a_name_is_readable_before_its_body_arrives() {
        let mut table = TypeTable::new();
        let category = table.declare("Category");

        // Interning `list<Category>` while `Category` is still being lowered
        // needs its name already readable.
        let children = table.collection(Collection::List, category);
        assert_eq!(table.name_of(children), "list<Category>");

        let string = table.primitive(Primitive::String);
        table.define(
            category,
            Sum::Single(fields([("name", string), ("children", children)])),
        );

        assert_eq!(
            rendered(&table, category),
            "type Category(children: list<Category>, name: string)"
        );
    }

    #[test]
    #[should_panic(expected = "`Ghost` was declared but never defined")]
    fn finishing_with_a_declaration_that_has_no_body_panics() {
        let mut table = TypeTable::new();
        table.declare("Ghost");

        table.finish();
    }
}
