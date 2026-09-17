//! The semantic type model printed back as datom source.
//!
//! [`crate::lower`] resolves names against the declarations before them, so a
//! file only re-parses if every type appears after the ones it references.
//! Declaration order already satisfies that, so [`render_types`] keeps it.

use crate::types::TypeTable;

/// Render `table`'s declarations as a datom source file.
///
/// One declaration per type, blank-line separated, in declaration order.
pub fn render_types(table: &TypeTable) -> String {
    let declarations: Vec<String> = table
        .iter()
        .map(|id| table.get(id).display(table).to_string())
        .collect();

    if declarations.is_empty() {
        return String::new();
    }

    format!("{}\n", declarations.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOUR: &str = include_str!("../samples/tour.datom");
    const RENDERED_TOUR: &str = include_str!("../samples/tour.rendered.datom");

    /// Lower `source`, render it back, and lower that.
    fn round_trip(source: &str) -> (TypeTable, TypeTable) {
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
    fn a_recursive_type_round_trips() {
        let (a, b) = round_trip("type Node(value: number, next: Node)\ntype List(head: Node)");
        assert_eq!(a, b);
    }

    #[test]
    fn nothing_renders_as_nothing() {
        assert_eq!(render_types(&TypeTable::new()), "");
    }
}
