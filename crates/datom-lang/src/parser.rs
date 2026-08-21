use std::iter::Peekable;

use crate::{
    Collection, Primitive,
    diagnostics::Diagnostics,
    error::{CompileError, ParseError},
    scanner::{Keyword, Token, TokenKind},
};

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Program {
    statements: Vec<Statement>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum Statement {
    Type(TypeStatement),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct TypeConstructor {
    name: Token,
    fields: TypeFields,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum TypeStatement {
    Single(TypeConstructor),
    // variadic carries both the name of the type, plus 1+ named variants
    Variadic((Token, Vec<TypeConstructor>)),
    InlineVariadic((Token, Vec<TypeName>)),
}

pub(crate) type TypeFields = Vec<TypeField>;

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct TypeField {
    name: Token,
    ty: TypeName,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum TypeName {
    /// Non-generic type names like identifiers and primitives.
    Concrete(Token),

    /// Generic type names, currently just collections.
    Generic {
        collection: Token,
        over: Box<TypeName>,
    },
}

pub(crate) type Generic = TypeName;

pub(crate) fn parse(
    source: &str,
    diagnostics: &Diagnostics,
    tokens: impl Iterator<Item = Result<Token, CompileError>>,
) -> Result<Program, CompileError> {
    Parser::new(source, diagnostics, tokens).parse()
}

struct Parser<'s, 'd, T>
where
    T: Iterator<Item = Result<Token, CompileError>>,
{
    source: &'s str,
    tokens: Peekable<T>,
    diag: &'d Diagnostics,
}

impl<'s, 'd, T> Parser<'s, 'd, T>
where
    T: Iterator<Item = Result<Token, CompileError>>,
{
    fn new(source: &'s str, diagnostics: &'d Diagnostics, tokens: T) -> Self {
        Self {
            source,
            tokens: tokens.peekable(),
            diag: diagnostics,
        }
    }

    fn parse(&mut self) -> Result<Program, CompileError> {
        self.program()
    }

    // MARK: - Recursive descent productions

    fn program(&mut self) -> Result<Program, CompileError> {
        let mut statements = Vec::new();

        while self.tokens.peek().is_some() {
            if self.is_next(TokenKind::Eof) {
                self.advance_unchecked()?;
                break;
            }

            statements.push(self.statement()?);
        }

        Ok(Program { statements })
    }

    fn statement(&mut self) -> Result<Statement, CompileError> {
        if self.is_next(TokenKind::Keyword(Keyword::Type)) {
            self.advance_unchecked()?;
            let stmt = self.type_statement()?;
            Ok(Statement::Type(stmt))
        } else {
            let actual = match self.tokens.next() {
                Some(result) => Some(result?.kind),
                None => None,
            };

            Err(ParseError::Expected(vec![TokenKind::Keyword(Keyword::Type)], actual).into())
        }
    }

    fn type_statement(&mut self) -> Result<TypeStatement, CompileError> {
        let ident = self.expect(TokenKind::Identifier)?;

        if self.is_next(TokenKind::LeftParen) {
            let _left_paren = self.advance_unchecked()?;
            let fields = self.type_fields()?;
            let _right_paren = self.expect(TokenKind::RightParen)?;

            Ok(TypeStatement::Single(TypeConstructor {
                name: ident,
                fields,
            }))
        } else if self.is_next(TokenKind::LeftCurly) {
            let _left_curly = self.advance_unchecked()?;

            let mut constructors = vec![self.type_constructor()?];

            while self.is_next(TokenKind::Comma) {
                let _comma = self.advance_unchecked();

                if self.is_next(TokenKind::Identifier) {
                    constructors.push(self.type_constructor()?);
                } else {
                    break;
                }
            }

            let _right_curly = self.expect(TokenKind::RightCurly)?;

            Ok(TypeStatement::Variadic((ident, constructors)))
        } else if self.is_next(TokenKind::Equals) {
            let _equals = self.advance_unchecked()?;
            let mut tys = vec![self.type_name()?];

            while self.is_next(TokenKind::Bar) {
                let _bar = self.advance_unchecked();
                tys.push(self.type_name()?);
            }

            let _semi = self.expect(TokenKind::Semicolon)?;
            Ok(TypeStatement::InlineVariadic((ident, tys)))
        } else {
            let actual = match self.tokens.next() {
                Some(token) => match token {
                    Ok(token) => Some(token.kind),
                    Err(_) => None,
                },
                None => None,
            };

            Err(
                ParseError::Expected(vec![TokenKind::LeftParen, TokenKind::LeftCurly], actual)
                    .into(),
            )
        }
    }

    fn type_name(&mut self) -> Result<TypeName, CompileError> {
        if self.is_any_next(&[
            TokenKind::Keyword(Keyword::Collection(Collection::List)),
            TokenKind::Keyword(Keyword::Collection(Collection::Map)),
            TokenKind::Keyword(Keyword::Collection(Collection::Set)),
        ]) {
            let collection = self.advance_unchecked()?;
            let generic = self.generic()?;

            Ok(TypeName::Generic {
                collection,
                over: Box::new(generic),
            })
        } else {
            let concrete = self.expect_any(&[
                // user-named type
                TokenKind::Identifier,
                TokenKind::Keyword(Keyword::Primitive(Primitive::Bool)),
                TokenKind::Keyword(Keyword::Primitive(Primitive::DateTime)),
                TokenKind::Keyword(Keyword::Primitive(Primitive::Number)),
                TokenKind::Keyword(Keyword::Primitive(Primitive::String)),
            ])?;

            Ok(TypeName::Concrete(concrete))
        }
    }

    fn generic(&mut self) -> Result<Generic, CompileError> {
        let _left_angle = self.expect(TokenKind::LeftAngle)?;
        let type_name = self.type_name()?;
        let _right_angle = self.expect(TokenKind::RightAngle)?;

        Ok(type_name)
    }

    fn type_constructor(&mut self) -> Result<TypeConstructor, CompileError> {
        let ident = self.expect(TokenKind::Identifier)?;
        let _left_paren = self.advance_unchecked()?;
        let fields = self.type_fields()?;
        let _right_paren = self.expect(TokenKind::RightParen)?;

        Ok(TypeConstructor {
            name: ident,
            fields,
        })
    }

    fn type_fields(&mut self) -> Result<TypeFields, CompileError> {
        let mut fields = vec![self.type_field()?];

        while self.is_next(TokenKind::Comma) {
            let _comma = self.advance_unchecked()?;

            if self.is_next(TokenKind::Identifier) {
                fields.push(self.type_field()?);
            } else {
                break;
            }
        }

        Ok(fields)
    }

    fn type_field(&mut self) -> Result<TypeField, CompileError> {
        let name = self.expect(TokenKind::Identifier)?;
        let _colon = self.expect(TokenKind::Colon)?;
        let ty = self.type_name()?;

        Ok(TypeField { name, ty })
    }

    // MARK: - Helpers

    fn is_next(&mut self, kind: TokenKind) -> bool {
        if let Some(token) = self.tokens.peek()
            && token.clone().is_ok_and(|t| t.kind == kind)
        {
            true
        } else {
            false
        }
    }

    fn is_any_next(&mut self, kinds: &[TokenKind]) -> bool {
        for kind in kinds {
            if self.is_next(*kind) {
                return true;
            }
        }

        false
    }

    /// Advance the iterator, unwrapping the Option and returning the contained Result.
    /// Should only be called after peeking, such as with `is_next`.
    fn advance_unchecked(&mut self) -> Result<Token, CompileError> {
        self.tokens.next().unwrap()
    }

    /// Advance the iterator only if the peeked token matches the specified kind.
    fn advance_if(&mut self, kind: TokenKind) -> Result<Option<Token>, CompileError> {
        if self.is_next(kind) {
            Ok(Some(self.advance_unchecked()?))
        } else {
            Ok(None)
        }
    }

    /// Advance the iterator and confirm the received token is of the expected kind. Returns
    /// a ParseError if the token is not the right kind.
    fn expect(&mut self, kind: TokenKind) -> Result<Token, CompileError> {
        if let Some(next_token) = self.tokens.next() {
            let next_token = next_token?;

            if next_token.kind == kind {
                Ok(next_token)
            } else {
                Err(ParseError::Expected(vec![kind], Some(next_token.kind)).into())
            }
        } else {
            Err(ParseError::Expected(vec![kind], Some(TokenKind::Eof)).into())
        }
    }

    fn expect_any(&mut self, kinds: &[TokenKind]) -> Result<Token, CompileError> {
        if let Some(next_token) = self.tokens.next() {
            let next_token = next_token?;

            for expected in kinds {
                if next_token.kind == *expected {
                    return Ok(next_token);
                }
            }

            Err(ParseError::Expected(Vec::from(kinds), Some(next_token.kind)).into())
        } else {
            Err(ParseError::Expected(Vec::from(kinds), Some(TokenKind::Eof)).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Debug, PartialEq, Eq)]
    enum NodeKind {
        TypeStatement,
        SingleTypeStatement,
        VariadicTypeStatement,
        InlineVariadicTypeStatement,
        TypeConstructor,
        TypeField,
        TypeName,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Node {
        kind: NodeKind,
        lexeme: String,
    }

    trait IntoNode<'src> {
        fn into_node(self, source: &'src str) -> Node;
    }

    impl<'src> IntoNode<'src> for TypeField {
        fn into_node(self, source: &'src str) -> Node {
            Node {
                kind: NodeKind::TypeField,
                lexeme: format!(
                    "{}: {}",
                    self.name.lexeme(source),
                    format_type_name(source, self.ty)
                ),
            }
        }
    }

    fn format_type_name(source: &str, type_name: TypeName) -> String {
        match type_name {
            TypeName::Concrete(token) => token.lexeme(source).to_string(),
            TypeName::Generic { collection, over } => format!(
                "{}<{}>",
                collection.lexeme(source),
                format_type_name(source, *over)
            ),
        }
    }

    // NF: I am not in love with the format for the nodes or how this works, but writing
    // the assertions against a Vec<Node> is **much** nicer than writing a raw expected AST.
    fn bfs(source: &str, program: &Program) -> Vec<Node> {
        let mut out = Vec::new();

        // seed the queue with all the statements
        let mut queue = VecDeque::new();
        for stmt in program.statements.clone() {
            queue.push_back(stmt);
        }

        while !queue.is_empty() {
            let item = queue.pop_front().unwrap();

            match item {
                Statement::Type(ty) => {
                    out.push(Node {
                        kind: NodeKind::TypeStatement,
                        lexeme: String::new(),
                    });

                    match ty {
                        TypeStatement::Single(ctor) => {
                            out.push(Node {
                                kind: NodeKind::SingleTypeStatement,
                                lexeme: String::from(ctor.name.lexeme(source)),
                            });

                            for field in ctor.fields {
                                out.push(field.into_node(source));
                            }
                        }
                        TypeStatement::Variadic((name, ctors)) => {
                            out.push(Node {
                                kind: NodeKind::VariadicTypeStatement,
                                lexeme: String::from(name.lexeme(source)),
                            });

                            for ctor in ctors {
                                out.push(Node {
                                    kind: NodeKind::TypeConstructor,
                                    lexeme: String::from(ctor.name.lexeme(source)),
                                });

                                for field in ctor.fields {
                                    out.push(field.into_node(source));
                                }
                            }
                        }
                        TypeStatement::InlineVariadic((name, tys)) => {
                            out.push(Node {
                                kind: NodeKind::InlineVariadicTypeStatement,
                                lexeme: String::from(name.lexeme(source)),
                            });

                            for ty in tys {
                                out.push(Node {
                                    kind: NodeKind::TypeName,
                                    lexeme: format_type_name(source, ty),
                                })
                            }
                        }
                    }
                }
            }
        }

        out
    }

    fn assert_nodes(source: &str, program: &Program, nodes: &[Node]) {
        let actual = bfs(source, program);

        assert_eq!(actual.len(), nodes.len());

        for (actual, expected) in actual.iter().zip(nodes) {
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn single_type() {
        let source = "type Person(id: number)";

        let diagnostics = Diagnostics::new();
        let tokens = crate::scanner::scan(source, &diagnostics);
        let program = parse(source, &diagnostics, tokens);

        assert!(program.is_ok());
        assert_nodes(
            source,
            &program.unwrap(),
            &[
                Node {
                    kind: NodeKind::TypeStatement,
                    lexeme: String::new(),
                },
                Node {
                    kind: NodeKind::SingleTypeStatement,
                    lexeme: String::from("Person"),
                },
                Node {
                    kind: NodeKind::TypeField,
                    lexeme: String::from("id: number"),
                },
            ],
        );
    }

    #[test]
    fn single_type_trailing_field_comma() {
        let source = "type Person(id: number,)";

        let diagnostics = Diagnostics::new();
        let tokens = crate::scanner::scan(source, &diagnostics);
        let program = parse(source, &diagnostics, tokens);

        assert!(program.is_ok());
        assert_nodes(
            source,
            &program.unwrap(),
            &[
                Node {
                    kind: NodeKind::TypeStatement,
                    lexeme: String::new(),
                },
                Node {
                    kind: NodeKind::SingleTypeStatement,
                    lexeme: String::from("Person"),
                },
                Node {
                    kind: NodeKind::TypeField,
                    lexeme: String::from("id: number"),
                },
            ],
        );
    }

    #[test]
    fn variadic_type() {
        let source = "type Person { Student(id: number, major: string), Professor(id: number), }";

        let diagnostics = Diagnostics::new();
        let tokens = crate::scanner::scan(source, &diagnostics);
        let program = parse(source, &diagnostics, tokens);

        assert!(program.is_ok());
        assert_nodes(
            source,
            &program.unwrap(),
            &[
                Node {
                    kind: NodeKind::TypeStatement,
                    lexeme: String::new(),
                },
                Node {
                    kind: NodeKind::VariadicTypeStatement,
                    lexeme: String::from("Person"),
                },
                Node {
                    kind: NodeKind::TypeConstructor,
                    lexeme: String::from("Student"),
                },
                Node {
                    kind: NodeKind::TypeField,
                    lexeme: String::from("id: number"),
                },
                Node {
                    kind: NodeKind::TypeField,
                    lexeme: String::from("major: string"),
                },
                Node {
                    kind: NodeKind::TypeConstructor,
                    lexeme: String::from("Professor"),
                },
                Node {
                    kind: NodeKind::TypeField,
                    lexeme: String::from("id: number"),
                },
            ],
        );
    }

    #[test]
    fn a_field_names_the_type_it_references() {
        let source = "type Person(home: Address, id: number)";

        let diagnostics = Diagnostics::new();
        let tokens = crate::scanner::scan(source, &diagnostics);
        let program = parse(source, &diagnostics, tokens);

        assert!(program.is_ok());
        assert_nodes(
            source,
            &program.unwrap(),
            &[
                Node {
                    kind: NodeKind::TypeStatement,
                    lexeme: String::new(),
                },
                Node {
                    kind: NodeKind::SingleTypeStatement,
                    lexeme: String::from("Person"),
                },
                Node {
                    kind: NodeKind::TypeField,
                    lexeme: String::from("home: Address"),
                },
                Node {
                    kind: NodeKind::TypeField,
                    lexeme: String::from("id: number"),
                },
            ],
        );
    }

    #[test]
    fn inline_variadic_type() {
        let source = "type Id = string | number;";

        let diagnostics = Diagnostics::new();
        let tokens = crate::scanner::scan(source, &diagnostics);
        let program = parse(source, &diagnostics, tokens);

        assert!(program.is_ok());
        assert!(diagnostics.is_ok());
        assert_nodes(
            source,
            &program.unwrap(),
            &[
                Node {
                    kind: NodeKind::TypeStatement,
                    lexeme: String::new(),
                },
                Node {
                    kind: NodeKind::InlineVariadicTypeStatement,
                    lexeme: String::from("Id"),
                },
                Node {
                    kind: NodeKind::TypeName,
                    lexeme: String::from("string"),
                },
                Node {
                    kind: NodeKind::TypeName,
                    lexeme: String::from("number"),
                },
            ],
        );
    }

    #[test]
    fn collections() {
        let source = "type X = list<number> | map<string> | set<datetime>;";

        let diagnostics = Diagnostics::new();
        let tokens = crate::scanner::scan(source, &diagnostics);
        let program = parse(source, &diagnostics, tokens);

        assert!(program.is_ok());
        assert!(diagnostics.is_ok());
        assert_nodes(
            source,
            &program.unwrap(),
            &[
                Node {
                    kind: NodeKind::TypeStatement,
                    lexeme: String::new(),
                },
                Node {
                    kind: NodeKind::InlineVariadicTypeStatement,
                    lexeme: String::from("X"),
                },
                Node {
                    kind: NodeKind::TypeName,
                    lexeme: String::from("list<number>"),
                },
                Node {
                    kind: NodeKind::TypeName,
                    lexeme: String::from("map<string>"),
                },
                Node {
                    kind: NodeKind::TypeName,
                    lexeme: String::from("set<datetime>"),
                },
            ],
        )
    }
}
