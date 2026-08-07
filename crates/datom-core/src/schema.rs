//! Schema inference for JSON API responses.
//!
//! Turns sampled [`serde_json::Value`]s into a structural schema: primitive
//! types (with ISO 8601 date / datetime detection), named record types for
//! nested objects, arrays with unified element types, and unions for
//! genuinely conflicting types. Merging across multiple sampled records
//! tracks optionality: a field missing in some records or null in others is
//! marked optional rather than unioned with null.

use serde_json::Value;

/// A type inferred from JSON samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferredType {
    /// UTF-8 text.
    String,
    /// A whole number.
    Integer,
    /// A floating-point number. Mixing integers and floats widens to `Float`.
    Float,
    /// `true` or `false`.
    Boolean,
    /// Only `null` was ever observed at this position.
    Null,
    /// An ISO 8601 calendar date, e.g. `2024-01-15`.
    Date,
    /// An ISO 8601 date and time, e.g. `2024-01-15T10:30:00Z`.
    DateTime,
    /// A nested object.
    Record(RecordType),
    /// An array with the unified type of its elements.
    Array(Box<InferredType>),
    /// Conflicting types observed at the same position.
    Union(Vec<InferredType>),
    /// Nothing observed (e.g. the element type of an always-empty array).
    Unknown,
}

/// A named record (object) type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordType {
    /// Name of the record, derived from the field or table it appeared under.
    pub name: String,
    /// The record's fields.
    pub fields: Vec<RecordField>,
}

/// One field of a [`RecordType`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordField {
    /// Field name as it appears in the JSON.
    pub name: String,
    /// Inferred type of the field's values.
    pub ty: InferredType,
    /// Whether the field was missing or null in some sampled records.
    pub optional: bool,
}

/// The schema inferred from an API response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredSchema {
    /// Name of the primary record set: the envelope field name when one was
    /// detected, otherwise the caller-provided root name.
    pub name: String,
    /// The field the records were unwrapped from, when the response was an
    /// envelope object like `{"data": [...]}`.
    pub envelope: Option<String>,
    /// How many records inference sampled.
    pub record_count: usize,
    /// Unified type of one record of the primary record set.
    pub ty: InferredType,
}

/// Infer the schema of a full API response body.
///
/// The primary record set is located as follows:
/// - a top-level array is the record set itself;
/// - an object with exactly one array-valued field (a common envelope, e.g.
///   `{"data": [...], "total": 10}`) yields that array, named after the
///   field;
/// - anything else is treated as a single record.
pub fn infer_response(root_name: &str, response: &Value) -> InferredSchema {
    if let Value::Object(map) = response {
        let mut array_fields = map.iter().filter(|(_, value)| value.is_array());
        if let (Some((key, Value::Array(items))), None) = (array_fields.next(), array_fields.next())
        {
            return InferredSchema {
                name: key.clone(),
                envelope: Some(key.clone()),
                record_count: items.len(),
                ty: infer_records(key, items),
            };
        }
    }

    if let Value::Array(items) = response {
        return InferredSchema {
            name: root_name.to_string(),
            envelope: None,
            record_count: items.len(),
            ty: infer_records(root_name, items),
        };
    }

    InferredSchema {
        name: root_name.to_string(),
        envelope: None,
        record_count: 1,
        ty: infer_value(root_name, response),
    }
}

/// Infer one unified type from several sampled records.
///
/// Record fields that are missing or null in some samples come back marked
/// [`optional`](RecordField::optional); conflicting types unify per
/// [`InferredType`]'s rules. An empty sample set yields
/// [`InferredType::Unknown`].
pub fn infer_records(name: &str, records: &[Value]) -> InferredType {
    records
        .iter()
        .map(|record| infer_value(name, record))
        .reduce(unify)
        .unwrap_or(InferredType::Unknown)
}

/// Infer the type of a single JSON value.
///
/// `name_hint` names any record types encountered: the value's own position
/// (field name or table name) becomes the record name.
pub fn infer_value(name_hint: &str, value: &Value) -> InferredType {
    match value {
        Value::Null => InferredType::Null,
        Value::Bool(_) => InferredType::Boolean,
        Value::Number(n) => {
            if n.is_f64() {
                InferredType::Float
            } else {
                InferredType::Integer
            }
        }
        Value::String(s) => {
            if is_iso_date(s) {
                InferredType::Date
            } else if is_iso_datetime(s) {
                InferredType::DateTime
            } else {
                InferredType::String
            }
        }
        Value::Array(items) => InferredType::Array(Box::new(infer_records(name_hint, items))),
        Value::Object(map) => InferredType::Record(RecordType {
            name: name_hint.to_string(),
            fields: map
                .iter()
                .map(|(key, value)| RecordField {
                    name: key.clone(),
                    ty: infer_value(key, value),
                    optional: false,
                })
                .collect(),
        }),
    }
}

/// Rename every record in `ty` the way [`infer_value`] would have, given
/// that `ty` sits at `name_hint`. Undo the qualification.
pub fn rename_as_inferred(ty: &mut InferredType, name_hint: &str) {
    match ty {
        InferredType::Record(record) => {
            record.name = name_hint.to_string();
            for field in &mut record.fields {
                let field_name = field.name.clone();
                rename_as_inferred(&mut field.ty, &field_name);
            }
        }
        // Array elements and union members are inferred under the same
        // hint as the value they sit in.
        InferredType::Array(elem) => rename_as_inferred(elem, name_hint),
        InferredType::Union(members) => {
            for member in members {
                rename_as_inferred(member, name_hint);
            }
        }
        _ => {}
    }
}

/// Unify two inferred types observed at the same position.
fn unify(a: InferredType, b: InferredType) -> InferredType {
    use InferredType::*;

    match (a, b) {
        (Unknown, t) | (t, Unknown) => t,
        (Union(members), Union(others)) => union_of(others.into_iter().fold(members, add_to_union)),
        (Union(members), t) => union_of(add_to_union(members, t)),
        (t, Union(members)) => union_of(members.into_iter().fold(vec![t], add_to_union)),
        (a, b) => unify_pair(a, b),
    }
}

/// Unify two non-union, non-unknown types.
fn unify_pair(a: InferredType, b: InferredType) -> InferredType {
    use InferredType::*;

    if a == b {
        return a;
    }
    match (a, b) {
        (Integer, Float) | (Float, Integer) => Float,
        (Date, String) | (String, Date) => String,
        (DateTime, String) | (String, DateTime) => String,
        (Record(x), Record(y)) => Record(merge_records(x, y)),
        (Array(x), Array(y)) => Array(Box::new(unify(*x, *y))),
        (a, b) => Union(vec![a, b]),
    }
}

/// Fold `t` into an existing union's members, merging with the first member
/// it is compatible with and appending otherwise.
fn add_to_union(mut members: Vec<InferredType>, t: InferredType) -> Vec<InferredType> {
    for member in &mut members {
        let candidate = unify_pair(member.clone(), t.clone());
        if !matches!(candidate, InferredType::Union(_)) {
            *member = candidate;
            return members;
        }
    }
    members.push(t);
    members
}

/// Wrap members in a union, collapsing a single-member union to that member.
fn union_of(mut members: Vec<InferredType>) -> InferredType {
    if members.len() == 1 {
        members.pop().expect("length checked")
    } else {
        InferredType::Union(members)
    }
}

/// Merge two record types field by field.
///
/// A field present in only one record, or null in one of them, becomes
/// optional.
fn merge_records(mut a: RecordType, b: RecordType) -> RecordType {
    let b_names: Vec<String> = b.fields.iter().map(|f| f.name.clone()).collect();

    for bf in b.fields {
        match a.fields.iter_mut().find(|af| af.name == bf.name) {
            Some(af) => {
                let current = std::mem::replace(&mut af.ty, InferredType::Unknown);
                let (ty, saw_null) = unify_field(current, bf.ty);
                af.ty = ty;
                af.optional = af.optional || bf.optional || saw_null;
            }
            None => a.fields.push(RecordField {
                optional: true,
                ..bf
            }),
        }
    }
    for af in &mut a.fields {
        if !b_names.contains(&af.name) {
            af.optional = true;
        }
    }
    a
}

/// Unify two field types, dissolving null into optionality: null on one side
/// makes the field optional instead of producing a union with null.
fn unify_field(a: InferredType, b: InferredType) -> (InferredType, bool) {
    use InferredType::*;

    match (a, b) {
        (Null, Null) => (Null, false),
        (Null, t) | (t, Null) => (t, true),
        (a, b) => (unify(a, b), false),
    }
}

/// Whether `s` is a plausible ISO 8601 calendar date (`YYYY-MM-DD`).
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || !s.is_ascii() || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let all_digits = |range: std::ops::Range<usize>| b[range].iter().all(u8::is_ascii_digit);
    if !(all_digits(0..4) && all_digits(5..7) && all_digits(8..10)) {
        return false;
    }
    let month: u32 = s[5..7].parse().expect("digits checked");
    let day: u32 = s[8..10].parse().expect("digits checked");
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// Whether `s` is a plausible ISO 8601 datetime: a date, `T`, a `HH:MM:SS`
/// time, then optional fractional seconds and an optional `Z` or
/// `±HH[:]MM`-style offset.
fn is_iso_datetime(s: &str) -> bool {
    if s.len() < 19 || !s.is_ascii() || !is_iso_date(&s[..10]) {
        return false;
    }
    let b = s.as_bytes();
    if b[10] != b'T' && b[10] != b't' {
        return false;
    }
    if b[13] != b':' || b[16] != b':' {
        return false;
    }
    let digit_pairs = [(11, 13), (14, 16), (17, 19)];
    if !digit_pairs
        .iter()
        .all(|&(lo, hi)| b[lo..hi].iter().all(u8::is_ascii_digit))
    {
        return false;
    }
    let hour: u32 = s[11..13].parse().expect("digits checked");
    let minute: u32 = s[14..16].parse().expect("digits checked");
    let second: u32 = s[17..19].parse().expect("digits checked");
    if hour > 23 || minute > 59 || second > 59 {
        return false;
    }

    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        rest = &frac[digits..];
    }
    matches!(rest, "" | "Z" | "z") || is_tz_offset(rest)
}

/// Whether `s` is a `±HH`, `±HHMM`, or `±HH:MM` timezone offset.
fn is_tz_offset(s: &str) -> bool {
    let Some(rest) = s.strip_prefix(['+', '-']) else {
        return false;
    };
    let (hours, minutes) = match rest.len() {
        2 => (&rest[..2], None),
        4 => (&rest[..2], Some(&rest[2..])),
        5 if rest.as_bytes()[2] == b':' => (&rest[..2], Some(&rest[3..])),
        _ => return false,
    };
    let valid_pair = |part: &str, max: u32| {
        part.bytes().all(|c| c.is_ascii_digit()) && part.parse::<u32>().is_ok_and(|n| n <= max)
    };
    valid_pair(hours, 23) && minutes.is_none_or(|m| valid_pair(m, 59))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn as_record(ty: &InferredType) -> &RecordType {
        match ty {
            InferredType::Record(record) => record,
            other => panic!("expected record, got {other:?}"),
        }
    }

    fn field<'a>(record: &'a RecordType, name: &str) -> &'a RecordField {
        record
            .fields
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no field `{name}` in `{}`", record.name))
    }

    #[test]
    fn infers_flat_object_primitives() {
        let ty = infer_value(
            "user",
            &json!({
                "id": 1,
                "name": "Ada",
                "score": 4.5,
                "active": true,
                "deleted_at": null,
                "created_at": "2024-01-15T10:30:00Z",
                "birthday": "1990-02-03",
            }),
        );

        let record = as_record(&ty);
        assert_eq!(record.name, "user");
        assert_eq!(field(record, "id").ty, InferredType::Integer);
        assert_eq!(field(record, "name").ty, InferredType::String);
        assert_eq!(field(record, "score").ty, InferredType::Float);
        assert_eq!(field(record, "active").ty, InferredType::Boolean);
        assert_eq!(field(record, "deleted_at").ty, InferredType::Null);
        assert_eq!(field(record, "created_at").ty, InferredType::DateTime);
        assert_eq!(field(record, "birthday").ty, InferredType::Date);
        assert!(record.fields.iter().all(|f| !f.optional));
    }

    #[test]
    fn detects_iso_dates_and_datetimes() {
        let cases = [
            ("2024-01-15", InferredType::Date),
            ("2024-01-15T10:30:00", InferredType::DateTime),
            ("2024-01-15T10:30:00Z", InferredType::DateTime),
            ("2024-01-15t10:30:00z", InferredType::DateTime),
            ("2024-01-15T10:30:00.123456Z", InferredType::DateTime),
            ("2024-01-15T10:30:00+02:00", InferredType::DateTime),
            ("2024-01-15T10:30:00-0530", InferredType::DateTime),
            ("2024-01-15T10:30:00+14", InferredType::DateTime),
        ];
        for (input, expected) in cases {
            assert_eq!(infer_value("v", &json!(input)), expected, "{input}");
        }
    }

    #[test]
    fn rejects_date_lookalikes() {
        let lookalikes = [
            "2024-13-01",            // month out of range
            "2024-00-10",            // month zero
            "2024-01-32",            // day out of range
            "20240115",              // no separators
            "2024-01-15T25:00:00Z",  // hour out of range
            "2024-01-15T10:30",      // missing seconds
            "2024-01-15 10:30:00",   // space instead of T
            "2024-01-15T10:30:00.Z", // empty fraction
            "2024-01-15T10:30:00X",  // bogus timezone
            "hello",
        ];
        for input in lookalikes {
            assert_eq!(
                infer_value("v", &json!(input)),
                InferredType::String,
                "{input}"
            );
        }
    }

    #[test]
    fn infers_nested_objects_as_named_records() {
        let ty = infer_value(
            "user",
            &json!({
                "name": "Ada",
                "address": {
                    "city": "London",
                    "geo": { "lat": 51.5, "lon": -0.1 },
                },
            }),
        );

        let user = as_record(&ty);
        let address = as_record(&field(user, "address").ty);
        assert_eq!(address.name, "address");
        assert_eq!(field(address, "city").ty, InferredType::String);

        let geo = as_record(&field(address, "geo").ty);
        assert_eq!(geo.name, "geo");
        assert_eq!(field(geo, "lat").ty, InferredType::Float);
    }

    #[test]
    fn infers_arrays_and_unifies_elements() {
        let array = |value: &Value| match infer_value("v", value) {
            InferredType::Array(elem) => *elem,
            other => panic!("expected array, got {other:?}"),
        };

        assert_eq!(array(&json!(["a", "b"])), InferredType::String);
        assert_eq!(array(&json!([1, 2.5])), InferredType::Float);
        assert_eq!(array(&json!([])), InferredType::Unknown);
        assert_eq!(
            array(&json!([1, "x"])),
            InferredType::Union(vec![InferredType::Integer, InferredType::String])
        );
        assert_eq!(
            array(&json!([null, 1])),
            InferredType::Union(vec![InferredType::Null, InferredType::Integer])
        );

        // Elements that are objects merge into one record with optionality.
        let elem = array(&json!([{"a": 1}, {"a": 2, "b": "x"}]));
        let record = as_record(&elem);
        assert!(!field(record, "a").optional);
        assert!(field(record, "b").optional);
    }

    #[test]
    fn merges_optionality_across_records() {
        let records = [
            json!({"id": 1, "email": "a@x.io", "plan": "pro", "tags": ["new"]}),
            json!({"id": 2, "email": null}),
            json!({"id": 3, "email": "c@x.io", "plan": null, "tags": []}),
        ];
        let ty = infer_records("users", &records);
        let record = as_record(&ty);

        // Present and non-null everywhere: required.
        let id = field(record, "id");
        assert_eq!(id.ty, InferredType::Integer);
        assert!(!id.optional);

        // Null in one record: optional, null dissolved into the flag.
        let email = field(record, "email");
        assert_eq!(email.ty, InferredType::String);
        assert!(email.optional);

        // Missing in one record and null in another: optional.
        let plan = field(record, "plan");
        assert_eq!(plan.ty, InferredType::String);
        assert!(plan.optional);

        // Empty array unifies with populated one; missing once: optional.
        let tags = field(record, "tags");
        assert_eq!(tags.ty, InferredType::Array(Box::new(InferredType::String)));
        assert!(tags.optional);
    }

    #[test]
    fn always_null_field_stays_null_type() {
        let ty = infer_records("t", &[json!({"x": null}), json!({"x": null})]);
        let x = field(as_record(&ty), "x");
        assert_eq!(x.ty, InferredType::Null);
        assert!(!x.optional);

        let ty = infer_records("t", &[json!({"x": null}), json!({})]);
        let x = field(as_record(&ty), "x");
        assert_eq!(x.ty, InferredType::Null);
        assert!(x.optional);
    }

    #[test]
    fn unifies_conflicting_types_across_records() {
        let ty = infer_records("t", &[json!({"v": 1}), json!({"v": "s"})]);
        assert_eq!(
            field(as_record(&ty), "v").ty,
            InferredType::Union(vec![InferredType::Integer, InferredType::String])
        );

        // Integers widen to float rather than forming a union.
        let ty = infer_records("t", &[json!({"v": 1}), json!({"v": 1.5})]);
        assert_eq!(field(as_record(&ty), "v").ty, InferredType::Float);

        // A widening candidate merges into an existing union member.
        let ty = infer_records(
            "t",
            &[json!({"v": 1}), json!({"v": "s"}), json!({"v": 2.5})],
        );
        assert_eq!(
            field(as_record(&ty), "v").ty,
            InferredType::Union(vec![InferredType::Float, InferredType::String])
        );

        // Dates mixed with arbitrary strings widen to string.
        let ty = infer_records("t", &[json!({"v": "2024-01-15"}), json!({"v": "hello"})]);
        assert_eq!(field(as_record(&ty), "v").ty, InferredType::String);

        // Dates and datetimes stay distinct.
        let ty = infer_records(
            "t",
            &[
                json!({"v": "2024-01-15"}),
                json!({"v": "2024-01-15T00:00:00Z"}),
            ],
        );
        assert_eq!(
            field(as_record(&ty), "v").ty,
            InferredType::Union(vec![InferredType::Date, InferredType::DateTime])
        );
    }

    #[test]
    fn merges_nested_records_across_samples() {
        let ty = infer_records(
            "t",
            &[json!({"u": {"a": 1}}), json!({"u": {"b": 2}}), json!({})],
        );
        let u = field(as_record(&ty), "u");
        assert!(u.optional, "u missing in one record");

        let u_record = as_record(&u.ty);
        assert!(field(u_record, "a").optional);
        assert!(field(u_record, "b").optional);
    }

    #[test]
    fn detects_envelope() {
        let schema = infer_response("response", &json!({"data": [{"id": 1}, {"id": 2}]}));

        assert_eq!(schema.name, "data");
        assert_eq!(schema.envelope.as_deref(), Some("data"));
        assert_eq!(schema.record_count, 2);
        let record = as_record(&schema.ty);
        assert_eq!(record.name, "data");
        assert_eq!(field(record, "id").ty, InferredType::Integer);
    }

    #[test]
    fn detects_envelope_alongside_metadata_fields() {
        let schema = infer_response(
            "response",
            &json!({"items": [{"id": 1}], "total": 10, "page": 1}),
        );

        assert_eq!(schema.envelope.as_deref(), Some("items"));
        assert_eq!(schema.name, "items");
        assert_eq!(schema.record_count, 1);
    }

    #[test]
    fn ambiguous_envelope_falls_back_to_single_record() {
        let schema = infer_response("response", &json!({"a": [1], "b": [2]}));

        assert_eq!(schema.envelope, None);
        assert_eq!(schema.name, "response");
        assert_eq!(schema.record_count, 1);
        let record = as_record(&schema.ty);
        assert_eq!(
            field(record, "a").ty,
            InferredType::Array(Box::new(InferredType::Integer))
        );
    }

    #[test]
    fn top_level_array_is_the_record_set() {
        let schema = infer_response("todos", &json!([{"id": 1}, {"id": 2}, {"id": 3}]));

        assert_eq!(schema.name, "todos");
        assert_eq!(schema.envelope, None);
        assert_eq!(schema.record_count, 3);
        assert_eq!(as_record(&schema.ty).name, "todos");
    }

    #[test]
    fn single_object_response_is_one_record() {
        let schema = infer_response("user", &json!({"id": 1, "name": "Ada"}));

        assert_eq!(schema.envelope, None);
        assert_eq!(schema.record_count, 1);
        assert_eq!(
            field(as_record(&schema.ty), "name").ty,
            InferredType::String
        );
    }

    #[test]
    fn rename_as_inferred_restores_names_the_renderer_qualified() {
        // What a `.types.datom` file gives back: declaration names that the
        // renderer disambiguated.
        let mut stored = InferredType::Record(RecordType {
            name: "users".to_string(),
            fields: vec![
                RecordField {
                    name: "address".to_string(),
                    ty: InferredType::Record(RecordType {
                        name: "users.address".to_string(),
                        fields: vec![RecordField {
                            name: "geo".to_string(),
                            ty: InferredType::Record(RecordType {
                                name: "users.address.geo".to_string(),
                                fields: vec![],
                            }),
                            optional: false,
                        }],
                    }),
                    optional: false,
                },
                RecordField {
                    name: "tags".to_string(),
                    ty: InferredType::Array(Box::new(InferredType::Record(RecordType {
                        name: "users.tags".to_string(),
                        fields: vec![],
                    }))),
                    optional: false,
                },
            ],
        });

        rename_as_inferred(&mut stored, "users");

        // Every record is renamed after the field it hangs from, exactly as
        // inference would have named it — including through the array.
        let expected = infer_value("users", &json!({"address": {"geo": {}}, "tags": [{}]}));
        assert_eq!(stored, expected);
    }

    #[test]
    fn rename_as_inferred_keeps_a_dotted_field_name() {
        // A field whose key really contains a dot keeps it: the name comes
        // from the field, not from stripping a qualifier.
        let mut stored = InferredType::Record(RecordType {
            name: "t".to_string(),
            fields: vec![RecordField {
                name: "users.home".to_string(),
                ty: InferredType::Record(RecordType {
                    name: "anything".to_string(),
                    fields: vec![],
                }),
                optional: false,
            }],
        });

        rename_as_inferred(&mut stored, "t");

        let InferredType::Record(table) = &stored else {
            panic!("expected record");
        };
        let InferredType::Record(nested) = &table.fields[0].ty else {
            panic!("expected record field");
        };
        assert_eq!(nested.name, "users.home");
    }

    #[test]
    fn empty_sample_set_is_unknown() {
        assert_eq!(infer_records("t", &[]), InferredType::Unknown);
    }
}
