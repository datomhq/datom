//! The semantic type model printed back as datom source.
//!
//! [`crate::lower`] resolves names against the declarations before them, so a
//! file only re-parses if every type appears after the ones it references.
//! [`render_types`] keeps that order; a field holds the whole nested [`Type`].

use std::collections::HashSet;

use crate::types::{Fields, Sum, Type, TypeDetails};

/// Render `types` as a datom source file.
///
/// One declaration per type, blank-line separated, ordered so that the output
/// lowers back to the types it was rendered from. Types reached through fields
/// are declared too, once each, whether or not they appear in `types`.
#[allow(private_interfaces)]
pub fn render_types(types: &[Type]) -> String {
    let mut program = Program::default();

    for ty in types {
        program.push(ty);
    }

    program.finish()
}

/// Declarations collected in an order that lowers.
#[derive(Default)]
struct Program {
    declared: HashSet<String>,
    declarations: Vec<String>,
}

impl Program {
    /// Declare everything `ty` depends on, then `ty` itself.
    fn push(&mut self, ty: &Type) {
        match &ty.details {
            TypeDetails::Primitive(_) => {}
            TypeDetails::Collection(collection) => self.push(collection.generic()),

            TypeDetails::Sum(sum) => {
                if self.declared.contains(&ty.name) {
                    return;
                }

                self.dependencies(sum);
                self.declared.insert(ty.name.clone());
                self.declarations.push(ty.to_string());
            }
        }
    }

    /// Walk the types `sum` refers to.
    fn dependencies(&mut self, sum: &Sum) {
        match sum {
            Sum::Single(fields) => self.fields(fields),

            Sum::Variadic(variants) => {
                for (_, fields) in variants {
                    self.fields(fields);
                }
            }

            Sum::InlineVariadic(variants) => {
                for variant in variants {
                    self.push(variant);
                }
            }
        }
    }

    /// Walk a constructor's field types alphabetically.
    fn fields(&mut self, fields: &Fields) {
        let mut names: Vec<&String> = fields.keys().collect();
        names.sort_unstable();

        for name in names {
            self.push(&fields[name]);
        }
    }

    fn finish(self) -> String {
        if self.declarations.is_empty() {
            return String::new();
        }

        format!("{}\n", self.declarations.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Primitive;

    const TOUR: &str = include_str!("../samples/tour.datom");
    const RENDERED_TOUR: &str = include_str!("../samples/tour.rendered.datom");

    /// Lower `source`, render it back, and lower that.
    fn round_trip(source: &str) -> (Vec<Type>, Vec<Type>) {
        let lowered = crate::types(source).expect("the source must lower");
        let rendered = render_types(&lowered);

        let relowered = crate::types(&rendered)
            .unwrap_or_else(|failure| panic!("rendered output must lower:\n{rendered}\n{failure}"));

        (lowered, relowered)
    }

    #[test]
    fn the_tour_round_trips() {
        let (a, b) = round_trip(TOUR);
        assert_eq!(a, b);
    }

    #[test]
    fn the_tour_renders_as_source() {
        let types = crate::types(TOUR).expect("the tour must lower");
        assert_eq!(render_types(&types), RENDERED_TOUR);
    }

    #[test]
    fn a_type_reached_only_through_a_field_is_declared_too() {
        let address = Type::single(
            "Address",
            Fields::from([(String::from("city"), Type::primitive(Primitive::String))]),
        );

        let person = Type::single("Person", Fields::from([(String::from("home"), address)]));

        assert_eq!(
            render_types(&[person]),
            "type Address(city: string)\n\ntype Person(home: Address)\n"
        );
    }

    #[test]
    fn nothing_renders_as_nothing() {
        assert_eq!(render_types(&[]), "");
    }
}
