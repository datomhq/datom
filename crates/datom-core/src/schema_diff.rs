//! Structural diffing of inferred schemas, for contract checks.
//!
//! Compares the schema stored in a `.types.datom` file against a freshly
//! inferred one and reports field-level drift. Nested record *names* are
//! ignored — the renderer may prefix or suffix them to keep declarations
//! unique — so only structure and field optionality are compared.

use std::fmt;

use crate::schema::{InferredType, RecordField, RecordType};
use crate::types_format::primitive_name;

/// One difference between a stored schema and a freshly inferred one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaChange {
    /// A field the stored schema does not have.
    Added {
        /// Dotted path of the field (`address.city`, `items[].sku`).
        path: String,
        /// Rendered type of the new field.
        ty: String,
    },

    /// A stored field missing from the fresh schema.
    Removed {
        /// Dotted path of the field.
        path: String,
        /// Rendered type of the removed field.
        ty: String,
    },

    /// A field whose type or optionality (rendered as `?`) changed.
    Changed {
        /// Dotted path of the field.
        path: String,
        /// Rendered stored type.
        from: String,
        /// Rendered fresh type.
        to: String,
    },
}

impl fmt::Display for SchemaChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaChange::Added { path, ty } => write!(f, "+ {path}: {ty}"),
            SchemaChange::Removed { path, ty } => write!(f, "- {path}: {ty}"),
            SchemaChange::Changed { path, from, to } => write!(f, "~ {path}: {from} -> {to}"),
        }
    }
}

/// Diff two schemas, reporting the changes that turn `stored` into `fresh`.
/// No changes means the contract is intact.
pub fn diff_schemas(stored: &InferredType, fresh: &InferredType) -> Vec<SchemaChange> {
    let mut changes = Vec::new();
    diff_type("", stored, fresh, &mut changes);
    changes
}

/// Diff two types observed at `path`. Records and arrays recurse to report
/// field-level changes; any other differing pair is one [`Changed`] entry.
///
/// [`Changed`]: SchemaChange::Changed
fn diff_type(path: &str, stored: &InferredType, fresh: &InferredType, out: &mut Vec<SchemaChange>) {
    match (stored, fresh) {
        (InferredType::Record(a), InferredType::Record(b)) => diff_records(path, a, b, out),
        (InferredType::Array(a), InferredType::Array(b)) => {
            diff_type(&format!("{path}[]"), a, b, out);
        }
        (a, b) if same_type(a, b) => {}
        (a, b) => out.push(SchemaChange::Changed {
            path: if path.is_empty() {
                "response".to_string()
            } else {
                path.to_string()
            },
            from: type_label(a),
            to: type_label(b),
        }),
    }
}

/// Diff two records field by field.
fn diff_records(path: &str, stored: &RecordType, fresh: &RecordType, out: &mut Vec<SchemaChange>) {
    for field in &stored.fields {
        match fresh.fields.iter().find(|f| f.name == field.name) {
            Some(fresh_field) => diff_field(path, field, fresh_field, out),
            None => out.push(SchemaChange::Removed {
                path: join(path, &field.name),
                ty: field_label(field),
            }),
        }
    }
    for field in &fresh.fields {
        if !stored.fields.iter().any(|f| f.name == field.name) {
            out.push(SchemaChange::Added {
                path: join(path, &field.name),
                ty: field_label(field),
            });
        }
    }
}

/// Diff one field present on both sides.
fn diff_field(path: &str, stored: &RecordField, fresh: &RecordField, out: &mut Vec<SchemaChange>) {
    let field_path = join(path, &stored.name);
    let same = same_type(&stored.ty, &fresh.ty);

    let recursable = matches!(
        (&stored.ty, &fresh.ty),
        (InferredType::Record(_), InferredType::Record(_))
            | (InferredType::Array(_), InferredType::Array(_))
    );
    if recursable {
        // Report an optionality flip here; structural drift recurses so it
        // is reported per nested field.
        if stored.optional != fresh.optional {
            out.push(SchemaChange::Changed {
                path: field_path.clone(),
                from: field_label(stored),
                to: field_label(fresh),
            });
        }
        if !same {
            diff_type(&field_path, &stored.ty, &fresh.ty, out);
        }
    } else if !same || stored.optional != fresh.optional {
        out.push(SchemaChange::Changed {
            path: field_path,
            from: field_label(stored),
            to: field_label(fresh),
        });
    }
}

/// Deep structural equality, ignoring record names (the renderer may have
/// rewritten them) but honoring field optionality.
fn same_type(a: &InferredType, b: &InferredType) -> bool {
    match (a, b) {
        (InferredType::Record(x), InferredType::Record(y)) => {
            x.fields.len() == y.fields.len()
                && x.fields.iter().all(|xf| {
                    y.fields.iter().any(|yf| {
                        yf.name == xf.name
                            && yf.optional == xf.optional
                            && same_type(&xf.ty, &yf.ty)
                    })
                })
        }
        (InferredType::Array(x), InferredType::Array(y)) => same_type(x, y),
        (InferredType::Union(xs), InferredType::Union(ys)) => {
            xs.len() == ys.len() && xs.iter().all(|x| ys.iter().any(|y| same_type(x, y)))
        }
        _ => a == b,
    }
}

/// Human-readable name of a type, matching the `.types.datom` notation.
fn type_label(ty: &InferredType) -> String {
    if let Some(name) = primitive_name(ty) {
        return name.to_string();
    }
    match ty {
        InferredType::Record(record) => record.name.clone(),
        InferredType::Array(elem) => {
            if matches!(**elem, InferredType::Union(_)) {
                format!("({})[]", type_label(elem))
            } else {
                format!("{}[]", type_label(elem))
            }
        }
        InferredType::Union(members) => members
            .iter()
            .map(type_label)
            .collect::<Vec<_>>()
            .join(" | "),
        _ => unreachable!("primitives handled above"),
    }
}

/// A field's type label, with `?` marking optional fields.
fn field_label(field: &RecordField) -> String {
    let mut label = type_label(&field.ty);
    if field.optional {
        label.push('?');
    }
    label
}

/// Append a field name to a dotted path.
fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, fields: Vec<RecordField>) -> InferredType {
        InferredType::Record(RecordType {
            name: name.to_string(),
            fields,
        })
    }

    fn field(name: &str, ty: InferredType, optional: bool) -> RecordField {
        RecordField {
            name: name.to_string(),
            ty,
            optional,
        }
    }

    fn rendered(changes: &[SchemaChange]) -> Vec<String> {
        changes.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn identical_schemas_have_no_changes() {
        let ty = record(
            "users",
            vec![
                field("id", InferredType::Integer, false),
                field("email", InferredType::String, true),
            ],
        );
        assert_eq!(diff_schemas(&ty, &ty), Vec::new());
    }

    #[test]
    fn nested_record_names_are_ignored() {
        // The stored file may carry a prefixed declaration name
        // (`orders_address`) while fresh inference names it `address`.
        let stored = record(
            "orders",
            vec![field(
                "address",
                record(
                    "orders_address",
                    vec![field("zip", InferredType::Integer, false)],
                ),
                false,
            )],
        );
        let fresh = record(
            "orders",
            vec![field(
                "address",
                record("address", vec![field("zip", InferredType::Integer, false)]),
                false,
            )],
        );
        assert_eq!(diff_schemas(&stored, &fresh), Vec::new());
    }

    #[test]
    fn reports_type_changes() {
        let stored = record("users", vec![field("id", InferredType::Integer, false)]);
        let fresh = record("users", vec![field("id", InferredType::String, false)]);

        assert_eq!(
            rendered(&diff_schemas(&stored, &fresh)),
            ["~ id: int -> string"]
        );
    }

    #[test]
    fn reports_added_and_removed_fields() {
        let stored = record(
            "users",
            vec![
                field("id", InferredType::Integer, false),
                field("legacy", InferredType::Boolean, false),
            ],
        );
        let fresh = record(
            "users",
            vec![
                field("id", InferredType::Integer, false),
                field("email", InferredType::String, true),
            ],
        );

        assert_eq!(
            rendered(&diff_schemas(&stored, &fresh)),
            ["- legacy: bool", "+ email: string?"]
        );
    }

    #[test]
    fn reports_optionality_changes() {
        let stored = record("users", vec![field("email", InferredType::String, false)]);
        let fresh = record("users", vec![field("email", InferredType::String, true)]);

        assert_eq!(
            rendered(&diff_schemas(&stored, &fresh)),
            ["~ email: string -> string?"]
        );
    }

    #[test]
    fn reports_nested_field_changes_with_dotted_paths() {
        let address = |city: InferredType| record("address", vec![field("city", city, false)]);
        let stored = record(
            "users",
            vec![field("address", address(InferredType::String), false)],
        );
        let fresh = record(
            "users",
            vec![field("address", address(InferredType::Integer), false)],
        );

        assert_eq!(
            rendered(&diff_schemas(&stored, &fresh)),
            ["~ address.city: string -> int"]
        );
    }

    #[test]
    fn reports_array_element_changes() {
        let stored = record(
            "users",
            vec![field(
                "tags",
                InferredType::Array(Box::new(InferredType::String)),
                false,
            )],
        );
        let fresh = record(
            "users",
            vec![field(
                "tags",
                InferredType::Array(Box::new(InferredType::Integer)),
                false,
            )],
        );

        assert_eq!(
            rendered(&diff_schemas(&stored, &fresh)),
            ["~ tags[]: string -> int"]
        );
    }
}
