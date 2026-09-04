//! Renders a parsed [`Program`] as an indented outline.
//!
//! This is the shape `datom parse` prints.

use crate::parser::{
    Expr, Program, Statement, TypeConstructor, TypeFields, TypeName, TypeStatement,
};

/// Renders `program` as an indented tree, ending in a newline.
pub(crate) fn render(source: &str, program: &Program) -> String {
    let mut out = String::from("program\n");
    let last = program.statements.len().saturating_sub(1);

    for (i, statement) in program.statements.iter().enumerate() {
        match statement {
            Statement::Type(ty) => type_statement(&mut out, source, "", i == last, ty),
            Statement::Expr(expr) => expression(&mut out, source, "", i == last, expr),
        }
    }

    out
}

/// Formats a type name the way it was written — `string`, or `list<set<string>>`.
pub(crate) fn type_name(source: &str, ty: &TypeName) -> String {
    match ty {
        TypeName::Concrete(token) => String::from(token.lexeme(source)),
        TypeName::Generic { collection, over } => {
            format!("{}<{}>", collection.lexeme(source), type_name(source, over))
        }
    }
}

/// Writes one node under `prefix` and returns the prefix its children sit at.
fn branch(out: &mut String, prefix: &str, last: bool, label: &str) -> String {
    out.push_str(prefix);
    out.push_str(if last { "└─ " } else { "├─ " });
    out.push_str(label);
    out.push('\n');

    format!("{prefix}{}", if last { "   " } else { "│  " })
}

fn type_statement(
    out: &mut String,
    source: &str,
    prefix: &str,
    last: bool,
    statement: &TypeStatement,
) {
    match statement {
        TypeStatement::Single(ctor) => {
            let name = ctor.name.lexeme(source);
            let child = branch(out, prefix, last, &format!("single type `{name}`"));
            fields(out, source, &child, &ctor.fields);
        }

        TypeStatement::Variadic((name, ctors)) => {
            let name = name.lexeme(source);
            let child = branch(out, prefix, last, &format!("variadic type `{name}`"));
            let last_ctor = ctors.len().saturating_sub(1);

            for (i, ctor) in ctors.iter().enumerate() {
                constructor(out, source, &child, i == last_ctor, ctor);
            }
        }

        TypeStatement::InlineVariadic((name, tys)) => {
            let name = name.lexeme(source);
            let child = branch(out, prefix, last, &format!("inline variadic type `{name}`"));
            let last_ty = tys.len().saturating_sub(1);

            for (i, ty) in tys.iter().enumerate() {
                let variant = format!("variant `{}`", type_name(source, ty));
                branch(out, &child, i == last_ty, &variant);
            }
        }
    }
}

fn constructor(out: &mut String, source: &str, prefix: &str, last: bool, ctor: &TypeConstructor) {
    let variant = format!("variant `{}`", ctor.name.lexeme(source));
    let child = branch(out, prefix, last, &variant);
    fields(out, source, &child, &ctor.fields);
}

fn fields(out: &mut String, source: &str, prefix: &str, fields: &TypeFields) {
    let last = fields.len().saturating_sub(1);

    for (i, field) in fields.iter().enumerate() {
        let label = format!(
            "field `{}`: {}",
            field.name.lexeme(source),
            type_name(source, &field.ty)
        );

        branch(out, prefix, i == last, &label);
    }
}

fn expression(out: &mut String, source: &str, prefix: &str, last: bool, expr: &Expr) {
    let (kind, token) = match expr {
        Expr::Number(token) => ("number", token),
        Expr::String(token) => ("string", token),
        Expr::Bool(token) => ("bool", token),
    };

    let label = format!("{kind} literal `{}`", token.lexeme(source));
    branch(out, prefix, last, &label);
}

#[cfg(test)]
mod tests {
    use crate::{diagnostics::Diagnostics, parser, scanner};

    /// Renders `source`, asserting it parsed cleanly first.
    fn tree(source: &str) -> String {
        let diagnostics = Diagnostics::new();
        let tokens = scanner::scan(source, &diagnostics);
        let program = parser::parse(source, &diagnostics, tokens).expect("source should parse");

        super::render(source, &program)
    }

    #[test]
    fn a_single_type_lists_its_fields() {
        assert_eq!(
            tree("type Person(home: Address, id: number)"),
            "\
program
└─ single type `Person`
   ├─ field `home`: Address
   └─ field `id`: number
"
        );
    }

    #[test]
    fn a_variadic_type_nests_fields_under_their_variant() {
        assert_eq!(
            tree("type Person { Student(id: number), Professor(tenured: bool) }"),
            "\
program
└─ variadic type `Person`
   ├─ variant `Student`
   │  └─ field `id`: number
   └─ variant `Professor`
      └─ field `tenured`: bool
"
        );
    }

    #[test]
    fn an_inline_variadic_type_lists_the_types_it_unions() {
        assert_eq!(
            tree("type Id = string | number | Badge;"),
            "\
program
└─ inline variadic type `Id`
   ├─ variant `string`
   ├─ variant `number`
   └─ variant `Badge`
"
        );
    }

    #[test]
    fn a_collection_field_keeps_its_generic_inline() {
        assert_eq!(
            tree("type Grid(cells: list<list<number>>, tags: set<string>)"),
            "\
program
└─ single type `Grid`
   ├─ field `cells`: list<list<number>>
   └─ field `tags`: set<string>
"
        );
    }

    #[test]
    fn a_collection_variant_keeps_its_generic_inline() {
        assert_eq!(
            tree("type Bag = list<number> | map<string>;"),
            "\
program
└─ inline variadic type `Bag`
   ├─ variant `list<number>`
   └─ variant `map<string>`
"
        );
    }

    #[test]
    fn literal_statements_render_their_kind_and_lexeme() {
        assert_eq!(
            tree("1_000.23; \"hello\"; true;"),
            "\
program
├─ number literal `1_000.23`
├─ string literal `\"hello\"`
└─ bool literal `true`
"
        );
    }

    /// The trunk has to run past a statement's whole subtree to reach the next
    /// one, which only shows up once something follows it.
    #[test]
    fn an_earlier_statement_keeps_the_trunk_running_past_its_fields() {
        assert_eq!(
            tree("type Address(city: string) type Person(home: Address)"),
            "\
program
├─ single type `Address`
│  └─ field `city`: string
└─ single type `Person`
   └─ field `home`: Address
"
        );
    }

    #[test]
    fn an_empty_program_renders_only_its_root() {
        assert_eq!(tree(""), "program\n");
    }
}
