//! The datom type system.

use std::{
    collections::HashMap,
    fmt::{self, Debug, Display, Formatter},
    sync::{Arc, OnceLock, Weak},
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

/// A reference from one type to another.
///
/// Weak when it points at a type that is self-referencing.
#[derive(Clone)]
pub enum TypeRef {
    Strong(Arc<Type>),
    Weak(Weak<Type>),
}

impl TypeRef {
    /// A reference to `ty`; weak while `ty` has no body yet.
    pub fn new(ty: &Arc<Type>) -> Self {
        if ty.details.get().is_some() {
            Self::Strong(Arc::clone(ty))
        } else {
            Self::Weak(Arc::downgrade(ty))
        }
    }

    /// The type referred to; `None` if a weak reference outlived it.
    pub fn get(&self) -> Option<Arc<Type>> {
        match self {
            Self::Strong(ty) => Some(Arc::clone(ty)),
            Self::Weak(ty) => ty.upgrade(),
        }
    }

    fn as_ptr(&self) -> *const Type {
        match self {
            Self::Strong(ty) => Arc::as_ptr(ty),
            Self::Weak(ty) => ty.as_ptr(),
        }
    }
}

/// Nominal: two references are equal when they point at the same type.
impl PartialEq for TypeRef {
    fn eq(&self, other: &Self) -> bool {
        self.as_ptr() == other.as_ptr()
    }
}

impl Eq for TypeRef {}

/// Prints the name only, so a recursive type does not recurse.
impl Debug for TypeRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Strong(ty) => f.write_str(&ty.name),
            Self::Weak(ty) => match ty.upgrade() {
                Some(ty) => f.write_str(&ty.name),
                None => f.write_str("<dropped>"),
            },
        }
    }
}

/// The primitives, in the order every table seeds them.
const PRIMITIVES: [Primitive; 4] = [
    Primitive::Number,
    Primitive::String,
    Primitive::Bool,
    Primitive::DateTime,
];

/// Every type a program mentions.
#[derive(Debug, Clone)]
pub struct TypeTable {
    /// Every type: primitives, collections, declarations.
    types: Vec<Arc<Type>>,
    /// What the source declared, in order.
    declared: Vec<Arc<Type>>,
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
            .map(|primitive| {
                Arc::new(Type {
                    name: primitive.to_string(),
                    details: OnceLock::from(TypeDetails::Primitive(primitive)),
                })
            })
            .collect();

        Self {
            types,
            declared: Vec::new(),
        }
    }

    /// A primitive, seeded before anything else is in the table.
    pub fn primitive(&self, primitive: Primitive) -> Arc<Type> {
        let index = match primitive {
            Primitive::Number => 0,
            Primitive::String => 1,
            Primitive::Bool => 2,
            Primitive::DateTime => 3,
        };

        Arc::clone(&self.types[index])
    }

    /// `kind` over `inner`, interned.
    pub fn collection(&mut self, kind: Collection, inner: TypeRef) -> Arc<Type> {
        let name = format!("{kind}<{inner:?}>");
        let details = TypeDetails::Collection(kind, inner);

        if let Some(ty) = self
            .types
            .iter()
            .find(|ty| ty.details.get() == Some(&details))
        {
            return Arc::clone(ty);
        }

        self.push(Type {
            name,
            details: OnceLock::from(details),
        })
    }

    /// Reserve a declared type, its body to follow.
    pub fn declare(&mut self, name: &str) -> Arc<Type> {
        let ty = self.push(Type {
            name: name.to_string(),
            details: OnceLock::new(),
        });

        self.declared.push(Arc::clone(&ty));
        ty
    }

    /// Give a declared type the body it was reserved for.
    pub fn define(&mut self, ty: &Arc<Type>, sum: Sum) {
        if ty.details.set(TypeDetails::Sum(sum)).is_err() {
            panic!("`{}` was defined twice", ty.name);
        }
    }

    /// Every declared type has a body; panics naming the first that does not.
    pub fn finish(self) -> Self {
        for ty in self.iter() {
            ty.details();
        }

        self
    }

    /// The types the source declared, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Type>> + '_ {
        self.declared.iter()
    }

    fn push(&mut self, ty: Type) -> Arc<Type> {
        let ty = Arc::new(ty);
        self.types.push(Arc::clone(&ty));
        ty
    }

    /// Where the type at `ptr` sits in this table.
    fn position(&self, ptr: *const Type) -> Option<usize> {
        self.types.iter().position(|ty| Arc::as_ptr(ty) == ptr)
    }

    fn same_ref(&self, other: &Self, a: &TypeRef, b: &TypeRef) -> bool {
        self.position(a.as_ptr()) == other.position(b.as_ptr())
    }

    fn same_fields(&self, other: &Self, a: &Fields, b: &Fields) -> bool {
        a.len() == b.len()
            && a.iter()
                .all(|(name, a)| b.get(name).is_some_and(|b| self.same_ref(other, a, b)))
    }

    fn same_details(&self, other: &Self, a: Option<&TypeDetails>, b: Option<&TypeDetails>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(TypeDetails::Primitive(a)), Some(TypeDetails::Primitive(b))) => a == b,
            (
                Some(TypeDetails::Collection(a_kind, a)),
                Some(TypeDetails::Collection(b_kind, b)),
            ) => a_kind == b_kind && self.same_ref(other, a, b),
            (Some(TypeDetails::Sum(a)), Some(TypeDetails::Sum(b))) => match (a, b) {
                (Sum::Single(a), Sum::Single(b)) => self.same_fields(other, a, b),
                (Sum::Variadic(a), Sum::Variadic(b)) => {
                    a.len() == b.len()
                        && a.iter().zip(b).all(|((a_name, a), (b_name, b))| {
                            a_name == b_name && self.same_fields(other, a, b)
                        })
                }
                (Sum::InlineVariadic(a), Sum::InlineVariadic(b)) => {
                    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| self.same_ref(other, a, b))
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// Positional: two tables are equal when they hold the same types in the same
/// order, referring to one another at the same positions.
impl PartialEq for TypeTable {
    fn eq(&self, other: &Self) -> bool {
        self.types.len() == other.types.len()
            && self.declared.len() == other.declared.len()
            && self
                .declared
                .iter()
                .zip(&other.declared)
                .all(|(a, b)| self.position(Arc::as_ptr(a)) == other.position(Arc::as_ptr(b)))
            && self.types.iter().zip(&other.types).all(|(a, b)| {
                a.name == b.name && self.same_details(other, a.details.get(), b.details.get())
            })
    }
}

impl Eq for TypeTable {}

/// The map of fields and their types for a datom sum type.
pub type Fields = HashMap<String, TypeRef>;

/// A sum type within the datom type system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sum {
    /// A single sum type has only a single variant, implicitly named the same as the overall type.
    Single(Fields),
    /// A variadic sum type has multiple variants, each with a name and different fields.
    Variadic(Vec<(String, Fields)>),
    /// An inline variadic sum has type has multiple variants, each its own field.
    InlineVariadic(Vec<TypeRef>),
}

/// A type within the datom type system.
#[derive(Debug)]
pub struct Type {
    pub name: String,
    details: OnceLock<TypeDetails>,
}

impl Type {
    /// What the type is.
    pub fn details(&self) -> &TypeDetails {
        self.details
            .get()
            .unwrap_or_else(|| panic!("`{}` was declared but never defined", self.name))
    }
}

/// Nominal: a type is equal only to itself.
impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Type {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDetails {
    Primitive(Primitive),
    Sum(Sum),
    Collection(Collection, TypeRef),
}

/// Prints a type as the declaration that introduces it — `type Person(name: string)`.
impl Display for Type {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let TypeDetails::Sum(sum) = self.details() else {
            // A primitive or a collection is written as its own name.
            return f.write_str(&self.name);
        };

        write!(f, "type {}", self.name)?;

        match sum {
            // e.g., `(id: number, name: string)`
            Sum::Single(fields) => write_fields(f, fields),

            // e.g., `{ Employee(name: string), Robot(id: number) }`
            Sum::Variadic(variants) => {
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

            // e.g., ` = Person | Robot;`
            Sum::InlineVariadic(members) => {
                f.write_str(" = ")?;
                for (i, member) in members.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{member:?}")?;
                }
                f.write_str(";")
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
        write!(f, "{name}: {:?}", fields[name])?;
    }
    f.write_str(")")
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a [`Fields`] map from `(name, type)` pairs.
    fn fields<const N: usize>(entries: [(&str, &Arc<Type>); N]) -> Fields {
        entries
            .into_iter()
            .map(|(name, ty)| (String::from(name), TypeRef::new(ty)))
            .collect()
    }

    /// Declare `name` and give it `sum` in one step.
    fn ty(table: &mut TypeTable, name: &str, sum: Sum) -> Arc<Type> {
        let ty = table.declare(name);
        table.define(&ty, sum);
        ty
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
            Sum::Single(fields([("nucleus", &boolean), ("wall", &boolean)])),
        );

        assert_eq!(cell.to_string(), "type Cell(nucleus: bool, wall: bool)");
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
                (String::from("Student"), fields([("id", &number)])),
                (String::from("Professor"), fields([("tenured", &boolean)])),
            ]),
        );

        assert_eq!(
            person.to_string(),
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
            Sum::Single(fields([("name", &string)])),
        );
        let robot = ty(&mut table, "Robot", Sum::Single(fields([("id", &number)])));
        let employee = ty(
            &mut table,
            "Employee",
            Sum::InlineVariadic(vec![TypeRef::new(&person), TypeRef::new(&robot)]),
        );

        assert_eq!(employee.to_string(), "type Employee = Person | Robot;");
    }

    #[test]
    fn nested_collections_recurse() {
        let mut table = TypeTable::new();
        let number = table.primitive(Primitive::Number);

        let inner = table.collection(Collection::List, TypeRef::new(&number));
        let outer = table.collection(Collection::List, TypeRef::new(&inner));

        assert_eq!(outer.to_string(), "list<list<number>>");
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
                ("zebra", &boolean),
                ("apple", &boolean),
                ("middle", &number),
            ])),
        );

        assert_eq!(
            zoo.to_string(),
            "type Zoo(apple: bool, middle: number, zebra: bool)"
        );
    }

    // The table itself, which the golden file cannot reach.

    #[test]
    fn every_primitive_is_seeded_as_itself() {
        let table = TypeTable::new();

        for primitive in PRIMITIVES {
            assert_eq!(
                table.primitive(primitive).details(),
                &TypeDetails::Primitive(primitive)
            );
        }
    }

    #[test]
    fn a_collection_interns_on_its_kind_and_element() {
        let mut table = TypeTable::new();
        let number = table.primitive(Primitive::Number);
        let string = table.primitive(Primitive::String);

        let list = table.collection(Collection::List, TypeRef::new(&number));

        assert_eq!(
            table.collection(Collection::List, TypeRef::new(&number)),
            list
        );
        assert_ne!(
            table.collection(Collection::Set, TypeRef::new(&number)),
            list
        );
        assert_ne!(
            table.collection(Collection::List, TypeRef::new(&string)),
            list
        );
    }

    #[test]
    fn a_declaration_never_interns() {
        let mut table = TypeTable::new();
        let boolean = table.primitive(Primitive::Bool);

        let a = ty(&mut table, "A", Sum::Single(fields([("x", &boolean)])));
        let b = ty(&mut table, "B", Sum::Single(fields([("x", &boolean)])));

        // Same body, different types: equality is nominal.
        assert_ne!(a, b);
        // Even the same name takes a fresh type — the scope, not the table,
        // is what rejects a duplicate declaration.
        assert_ne!(table.declare("A"), a);
    }

    #[test]
    fn only_declared_types_are_iterated() {
        let mut table = TypeTable::new();
        let number = table.primitive(Primitive::Number);
        let cells = table.collection(Collection::List, TypeRef::new(&number));

        ty(&mut table, "Grid", Sum::Single(fields([("cells", &cells)])));

        // Primitives and `list<number>` are in the table, but nothing
        // declared them, so nothing may render them as declarations.
        let names: Vec<&str> = table.iter().map(|ty| ty.name.as_str()).collect();
        assert_eq!(names, ["Grid"]);
    }

    #[test]
    fn a_name_is_readable_before_its_body_arrives() {
        let mut table = TypeTable::new();
        let category = table.declare("Category");

        // Interning `list<Category>` while `Category` is still being lowered
        // needs its name already readable.
        let children = table.collection(Collection::List, TypeRef::new(&category));
        assert_eq!(children.name, "list<Category>");

        let string = table.primitive(Primitive::String);
        table.define(
            &category,
            Sum::Single(fields([("name", &string), ("children", &children)])),
        );

        assert_eq!(
            category.to_string(),
            "type Category(children: list<Category>, name: string)"
        );
    }

    #[test]
    fn a_reference_is_weak_only_while_its_target_has_no_body() {
        let mut table = TypeTable::new();
        let category = table.declare("Category");

        assert!(matches!(TypeRef::new(&category), TypeRef::Weak(_)));

        table.define(&category, Sum::Single(Fields::new()));

        assert!(matches!(TypeRef::new(&category), TypeRef::Strong(_)));
    }

    #[test]
    #[should_panic(expected = "`Ghost` was declared but never defined")]
    fn finishing_with_a_declaration_that_has_no_body_panics() {
        let mut table = TypeTable::new();
        table.declare("Ghost");

        table.finish();
    }
}
