//! The syntax tree translated into the semantic type model.
//!
//! [`crate::parser`] produces a tree of tokens — a field's type is the
//! identifier `Address`, a range into the source. Nothing in the tree says
//! whether `Address` was ever declared or what it holds. This pass resolves
//! those names against the declarations around them and builds the [`Type`]
//! values in [`crate::types`], where a field holds the type itself.

use std::collections::HashMap;

use crate::{
    diagnostics::Diagnostics,
    error::{CompileError, LowerError},
    parser::{Program, Statement, TypeFields, TypeName, TypeStatement},
    scanner::{Keyword, Token, TokenKind},
    types::{Fields, Type},
};

/// Lower every type declaration in `program`, in declaration order.
pub(crate) fn lower(
    source: &str,
    program: &Program,
    diagnostics: &Diagnostics,
) -> Result<Vec<Type>, CompileError> {
    Lowering {
        source,
        diag: diagnostics,
        scope: HashMap::new(),
    }
    .run(program)
}

struct Lowering<'s, 'd> {
    source: &'s str,
    diag: &'d Diagnostics,
    /// Every type declared so far by name.
    scope: HashMap<String, Type>,
}

impl Lowering<'_, '_> {
    fn run(&mut self, program: &Program) -> Result<Vec<Type>, CompileError> {
        let mut types = Vec::new();

        for statement in &program.statements {
            let Statement::Type(declaration) = statement else { continue };

            let ty = self.type_statement(declaration)?;
            self.scope.insert(ty.name.clone(), ty.clone());
            types.push(ty);
        }

        Ok(types)
    }

    fn type_statement(&self, declaration: &TypeStatement) -> Result<Type, CompileError> {
        match declaration {
            TypeStatement::Single(constructor) => {
                let name = self.declare(&constructor.name)?;
                Ok(Type::single(&name, self.fields(&constructor.fields)?))
            }

            TypeStatement::Variadic((token, constructors)) => {
                let name = self.declare(token)?;

                let mut variants = Vec::with_capacity(constructors.len());
                for constructor in constructors {
                    variants.push((
                        constructor.name.lexeme(self.source).to_string(),
                        self.fields(&constructor.fields)?,
                    ));
                }

                Ok(Type::variadic(&name, variants))
            }

            TypeStatement::InlineVariadic((token, names)) => {
                let name = self.declare(token)?;

                let mut variants = Vec::with_capacity(names.len());
                for type_name in names {
                    variants.push(self.type_name(type_name)?);
                }

                Ok(Type::inline_variadic(&name, variants))
            }
        }
    }

    /// The name `token` introduces, rejecting one already declared.
    fn declare(&self, token: &Token) -> Result<String, CompileError> {
        let name = token.lexeme(self.source);

        if self.scope.contains_key(name) {
            return Err(self.report(LowerError::DuplicateType(name.to_string()), token));
        }

        Ok(name.to_string())
    }

    /// Lower a constructor's fields.
    fn fields(&self, fields: &TypeFields) -> Result<Fields, CompileError> {
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

    /// Resolve a written type name to the type it denotes.
    fn type_name(&self, name: &TypeName) -> Result<Type, CompileError> {
        match name {
            TypeName::Concrete(token) => match token.kind {
                TokenKind::Keyword(Keyword::Primitive(primitive)) => Ok(Type::primitive(primitive)),
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

                Ok(Type::collection(kind, self.type_name(over)?))
            }
        }
    }

    /// The type `token` names, which a declaration must have introduced.
    fn declared(&self, token: &Token) -> Result<Type, CompileError> {
        let name = token.lexeme(self.source);

        match self.scope.get(name) {
            Some(ty) => Ok(ty.clone()),
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
        types::{Collection, Primitive, Sum, TypeDetails},
    };

    /// Scan, parse and lower `source`.
    fn lowered(source: &str) -> Result<Vec<Type>, String> {
        let diag = Diagnostics::new();
        let tokens = scanner::scan(source, &diag);

        parser::parse(source, &diag, tokens)
            .and_then(|program| lower(source, &program, &diag))
            .map_err(|_| diag.render(source))
    }

    /// The single sum's fields, or a panic naming what was found instead.
    fn single(ty: &Type) -> &Fields {
        match &ty.details {
            TypeDetails::Sum(Sum::Single(fields)) => fields,
            other => panic!("expected a single sum, found {other:?}"),
        }
    }

    #[test]
    fn the_tour_lowers_every_declaration_in_order() {
        let types = lowered(include_str!("../samples/tour.datom")).expect("the tour must lower");

        let names: Vec<&str> = types.iter().map(|ty| ty.name.as_str()).collect();
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
        let types = lowered("42;\n\"hello, datom\";\ntrue;").expect("expressions parse");
        assert!(types.is_empty(), "{types:?}");
    }

    #[test]
    fn a_field_holds_the_type_it_names_not_the_name() {
        let types = lowered("type Address(city: string)\ntype Person(home: Address)").unwrap();

        assert_eq!(single(&types[1])["home"], types[0]);
    }

    #[test]
    fn a_generic_lowers_to_a_collection_over_its_element() {
        let types = lowered("type Grid(cells: list<list<number>>)").unwrap();

        let number = Type::primitive(Primitive::Number);
        let inner = Type::collection(Collection::List, number);
        assert_eq!(
            single(&types[0])["cells"],
            Type::collection(Collection::List, inner)
        );
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
    fn a_type_cannot_reference_itself() {
        let err = lowered("type Node(next: Node)").unwrap_err();
        // TODO: come back to this on recursive types
        //       a field will need to hold a reference to the type
        assert_eq!(err, "[1:17] error: Unknown type `Node`\n");
    }

    #[test]
    fn a_record_cannot_repeat_a_field_name() {
        let err = lowered("type A(x: bool, x: number)").unwrap_err();
        assert_eq!(err, "[1:17] error: Duplicate field `x`\n");
    }
}
