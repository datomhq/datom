//! The syntax tree translated into the semantic type model.
//!
//! [`crate::parser`] produces a tree of tokens — a field's type is the
//! identifier `Address`, a range into the source. Nothing in the tree says
//! whether `Address` was ever declared or what it holds. This pass resolves
//! those names against the declarations around them and fills in the
//! [`TypeTable`] in [`crate::types`], where a field holds a reference to its type.

use std::{collections::HashMap, sync::Arc};

use crate::{
    diagnostics::Diagnostics,
    error::{CompileError, LowerError},
    parser::{Program, Statement, TypeFields, TypeName, TypeStatement},
    scanner::{Keyword, Token, TokenKind},
    types::{Fields, Sum, Type, TypeRef, TypeTable},
};

/// Lower every type declaration in `program`, in declaration order.
pub(crate) fn lower(
    source: &str,
    program: &Program,
    diagnostics: &Diagnostics,
) -> Result<TypeTable, CompileError> {
    Lowering {
        source,
        diag: diagnostics,
        scope: HashMap::new(),
        table: TypeTable::new(),
    }
    .run(program)
}

struct Lowering<'s, 'd> {
    source: &'s str,
    diag: &'d Diagnostics,
    /// Every type declared so far by name.
    scope: HashMap<&'s str, Arc<Type>>,
    table: TypeTable,
}

impl<'s> Lowering<'s, '_> {
    fn run(mut self, program: &Program) -> Result<TypeTable, CompileError> {
        for statement in &program.statements {
            let Statement::Type(declaration) = statement else {
                continue;
            };

            self.type_statement(declaration)?;
        }

        Ok(self.table.finish())
    }

    /// Lower a declaration into the type its name reserved.
    fn type_statement(&mut self, declaration: &TypeStatement) -> Result<(), CompileError> {
        let (ty, sum) = match declaration {
            TypeStatement::Single(constructor) => {
                let ty = self.declare(&constructor.name)?;
                let fields = self.fields(&constructor.fields)?;

                (ty, Sum::Single(fields))
            }

            TypeStatement::Variadic((token, constructors)) => {
                let ty = self.declare(token)?;

                let mut variants = Vec::with_capacity(constructors.len());
                for constructor in constructors {
                    variants.push((
                        constructor.name.lexeme(self.source).to_string(),
                        self.fields(&constructor.fields)?,
                    ));
                }

                (ty, Sum::Variadic(variants))
            }

            TypeStatement::InlineVariadic((token, names)) => {
                let ty = self.declare(token)?;

                let mut variants = Vec::with_capacity(names.len());
                for type_name in names {
                    variants.push(self.type_name(type_name)?);
                }

                (ty, Sum::InlineVariadic(variants))
            }
        };

        self.table.define(&ty, sum);
        Ok(())
    }

    /// Bring the name `token` introduces into scope before its body is
    /// lowered, so the body can name it. Rejects a name already declared.
    fn declare(&mut self, token: &Token) -> Result<Arc<Type>, CompileError> {
        let name = token.lexeme(self.source);

        if self.scope.contains_key(name) {
            return Err(self.report(LowerError::DuplicateType(name.to_string()), token));
        }

        let ty = self.table.declare(name);
        self.scope.insert(name, Arc::clone(&ty));
        Ok(ty)
    }

    /// Lower a constructor's fields.
    fn fields(&mut self, fields: &TypeFields) -> Result<Fields, CompileError> {
        let mut lowered = Fields::with_capacity(fields.len());

        for field in fields {
            let name = field.name.lexeme(self.source);
            let ty = self.type_name(&field.ty)?;

            if lowered.insert(name.to_string(), ty).is_some() {
                return Err(self.report(LowerError::DuplicateField(name.to_string()), &field.name));
            }
        }

        Ok(lowered)
    }

    /// Resolve a written type name to a reference to the type it denotes.
    fn type_name(&mut self, name: &TypeName) -> Result<TypeRef, CompileError> {
        match name {
            TypeName::Concrete(token) => match token.kind {
                TokenKind::Keyword(Keyword::Primitive(primitive)) => {
                    Ok(TypeRef::new(&self.table.primitive(primitive)))
                }
                // Anything else in this position is written as a name, and
                // has to have been declared under it.
                _ => self.declared(token),
            },

            TypeName::Generic { collection, over } => {
                let TokenKind::Keyword(Keyword::Collection(kind)) = collection.kind else {
                    // The parser only builds a generic from a collection
                    // keyword, so this is unreachable through it — reported
                    // rather than asserted so a future production cannot turn
                    // it into a panic.
                    return Err(self.report(
                        LowerError::UnknownType(collection.lexeme(self.source).to_string()),
                        collection,
                    ));
                };

                let inner = self.type_name(over)?;
                Ok(TypeRef::new(&self.table.collection(kind, inner)))
            }
        }
    }

    /// The type `token` names, which a declaration must have introduced.
    ///
    /// A type still being lowered has no body yet, so naming it from inside
    /// that body yields a weak reference rather than an `Arc` cycle.
    fn declared(&self, token: &Token) -> Result<TypeRef, CompileError> {
        let name = token.lexeme(self.source);

        match self.scope.get(name) {
            Some(ty) => Ok(TypeRef::new(ty)),
            None => Err(self.report(LowerError::UnknownType(name.to_string()), token)),
        }
    }

    /// Records `error` against `token`'s range.
    fn report(&self, error: LowerError, token: &Token) -> CompileError {
        // TODO: teach Diagnostics::error to take a range so callers holding one
        // don't have to split it into start/end (same at parser.rs `report`).
        self.diag
            .error(error.to_string(), token.range.start, token.range.end);
        error.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        parser, scanner,
        types::{Collection, Primitive, TypeDetails},
    };

    /// Scan, parse and lower `source`.
    fn lowered(source: &str) -> Result<TypeTable, String> {
        let diag = Diagnostics::new();
        let tokens = scanner::scan(source, &diag);

        parser::parse(source, &diag, tokens)
            .and_then(|program| lower(source, &program, &diag))
            .map_err(|_| diag.render(source))
    }

    /// The type declared under `name`, or a panic.
    fn nominal(table: &TypeTable, name: &str) -> Arc<Type> {
        table
            .iter()
            .find(|ty| ty.name == name)
            .cloned()
            .unwrap_or_else(|| panic!("no type named `{name}`"))
    }

    /// The single sum's fields, or a panic naming what was found instead.
    fn single(ty: &Type) -> &Fields {
        match ty.details() {
            TypeDetails::Sum(Sum::Single(fields)) => fields,
            other => panic!("expected a single sum, found {other:?}"),
        }
    }

    /// A variadic sum's named variant, or a panic naming what was found instead.
    fn variant<'t>(ty: &'t Type, name: &str) -> &'t Fields {
        match ty.details() {
            TypeDetails::Sum(Sum::Variadic(variants)) => variants
                .iter()
                .find(|(variant, _)| variant == name)
                .map(|(_, fields)| fields)
                .unwrap_or_else(|| panic!("no variant named `{name}`")),
            other => panic!("expected a variadic sum, found {other:?}"),
        }
    }

    #[test]
    fn the_tour_lowers_every_declaration_in_order() {
        let table = lowered(include_str!("../samples/tour.datom")).expect("the tour must lower");

        let names: Vec<&str> = table.iter().map(|ty| ty.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Tag",
                "Money",
                "Address",
                "Customer",
                "Product",
                "OrderStatus",
                "LineItem",
                "Order",
                "Grid",
                "Sku",
                "Id",
                "Party",
                "Anything",
            ]
        );
    }

    #[test]
    fn expression_statements_lower_to_nothing() {
        let table = lowered("42;\n\"hello, datom\";\ntrue;").expect("expressions parse");

        assert_eq!(table.iter().count(), 0);
    }

    #[test]
    fn a_field_holds_the_type_it_names_not_the_name() {
        let table = lowered("type Address(city: string)\ntype Person(home: Address)").unwrap();

        assert_eq!(
            single(&nominal(&table, "Person"))["home"].get(),
            nominal(&table, "Address")
        );
    }

    #[test]
    fn a_generic_lowers_to_a_collection_over_its_element() {
        let table = lowered("type Grid(cells: list<list<number>>)").unwrap();
        let cells = single(&nominal(&table, "Grid"))["cells"].get();

        let TypeDetails::Collection(Collection::List, inner) = cells.details() else {
            panic!("expected a list, found {:?}", cells.details());
        };

        let inner = inner.get();
        let TypeDetails::Collection(Collection::List, element) = inner.details() else {
            panic!("expected a list, found {:?}", inner.details());
        };

        assert_eq!(element.get(), table.primitive(Primitive::Number));
    }

    #[test]
    fn a_name_no_declaration_introduces_is_an_error() {
        let err = lowered("type A(x: Missing)").unwrap_err();
        assert_eq!(err, "[1:11] error: Unknown type `Missing`\n");
    }

    #[test]
    fn a_second_declaration_of_a_name_is_an_error() {
        let err = lowered("type A(x: bool)\ntype A(y: bool)").unwrap_err();
        assert_eq!(err, "[2:6] error: Duplicate type `A`\n");
    }

    #[test]
    fn a_forward_reference_is_an_error() {
        let err = lowered("type Person(home: Address)\ntype Address(city: string)").unwrap_err();
        assert_eq!(err, "[1:19] error: Unknown type `Address`\n");
    }

    #[test]
    fn a_type_can_reference_itself() {
        let table = lowered("type Category(name: string, children: list<Category>)").unwrap();

        assert_eq!(
            nominal(&table, "Category").to_string(),
            "type Category(children: list<Category>, name: string)"
        );
    }

    #[test]
    fn a_variant_can_reference_its_own_type() {
        let table =
            lowered("type Tree { Leaf(value: number), Branch(left: Tree, right: Tree) }").unwrap();

        assert_eq!(
            nominal(&table, "Tree").to_string(),
            "type Tree { Leaf(value: number), Branch(left: Tree, right: Tree) }"
        );
    }

    #[test]
    fn an_inline_variadic_can_reference_itself() {
        let table = lowered("type Json = string | number | list<Json>;").unwrap();

        assert_eq!(
            nominal(&table, "Json").to_string(),
            "type Json = string | number | list<Json>;"
        );
    }

    #[test]
    fn a_self_reference_holds_the_type_itself() {
        let table = lowered("type Node(value: number, next: Node)").unwrap();
        let node = nominal(&table, "Node");

        assert_eq!(single(&node)["next"].get(), node);
    }

    #[test]
    fn a_self_reference_does_not_keep_its_type_alive() {
        let table = lowered("type Category(name: string, children: list<Category>)").unwrap();
        let category = Arc::downgrade(&nominal(&table, "Category"));

        drop(table);

        assert!(category.upgrade().is_none());
    }

    #[test]
    fn a_variant_self_reference_holds_the_whole_variadic_type() {
        let table =
            lowered("type Tree { Leaf(value: number), Branch(left: Tree, right: Tree) }").unwrap();

        let tree = nominal(&table, "Tree");
        let branch = variant(&tree, "Branch");

        assert_eq!(branch["left"].get(), tree);
        assert_eq!(branch["right"].get(), tree);
    }

    #[test]
    fn a_record_cannot_repeat_a_field_name() {
        let err = lowered("type A(x: bool, x: number)").unwrap_err();
        assert_eq!(err, "[1:17] error: Duplicate field `x`\n");
    }
}
