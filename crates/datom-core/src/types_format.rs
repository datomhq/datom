//! The `.types.datom` text format for inferred schemas.
//!
//! Renders an [`InferredType`] record as a human-readable schema file and
//! parses it back. The grammar is documented in `docs/types-format.md`; a
//! file looks like:
//!
//! ```text
//! table users {
//!   id: int
//!   email: string
//!   address: address?
//! }
//!
//! record address {
//!   city: string
//! }
//! ```
//!
//! Nested record types are hoisted into named `record` declarations and
//! referenced by name. Names are kept verbatim (no case conversion) so that
//! parsing a rendered file reproduces the original schema exactly.
//!
//! A file may declare several tables — one per endpoint of a data source —
//! which share the hoisted `record` declarations. A record keeps its own
//! name while it is the only one claiming it; when several records claim a
//! name, each is qualified with the name of whatever encloses it
//! (`home.address`, `users.home.address`). Record names never shadow table
//! names: a field can only ever resolve to a `record`, so a shadowed
//! `table` would be unreachable and the file unreadable.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::datasource::{DATASOURCES_DIR, validate_datasource_name};
use crate::schema::{InferredType, RecordField, RecordType};
use crate::{CoreError, Result};

/// File-name suffix for schema files, alongside `<name>.datom.toml`.
pub const TYPES_FILE_SUFFIX: &str = ".types.datom";

/// Path of the types file for data source `name` inside `project_root`.
pub fn types_path(project_root: impl AsRef<Path>, name: &str) -> PathBuf {
    project_root
        .as_ref()
        .join(DATASOURCES_DIR)
        .join(format!("{name}{TYPES_FILE_SUFFIX}"))
}

/// Render `tables` (one record per table) and write them to the types file
/// for `datasource_name`, replacing any previous contents. Returns the path
/// written.
pub fn save_tables(
    project_root: impl AsRef<Path>,
    datasource_name: &str,
    tables: &[InferredType],
) -> Result<PathBuf> {
    validate_datasource_name(datasource_name)?;
    let contents = render_tables(tables)?;

    let dir = project_root.as_ref().join(DATASOURCES_DIR);
    fs::create_dir_all(&dir)?;

    let path = types_path(project_root, datasource_name);
    fs::write(&path, contents)?;
    Ok(path)
}

/// Read and parse the types file for `datasource_name` into its tables.
pub fn load_tables(
    project_root: impl AsRef<Path>,
    datasource_name: &str,
) -> Result<Vec<InferredType>> {
    validate_datasource_name(datasource_name)?;
    let contents = fs::read_to_string(types_path(project_root, datasource_name))?;
    parse_tables(&contents)
}

// --- Rendering ---------------------------------------------------------

/// Separator placed between a qualifier and the name it disambiguates.
const QUALIFIER_SEPARATOR: char = '.';

/// Render several table schemas as one `.types.datom` document.
///
/// Every element must be an [`InferredType::Record`] with a distinct name;
/// each becomes one `table` declaration, in input order. Nested records are
/// hoisted into `record` declarations shared across all tables. A record
/// keeps its own name while it is the sole claimant; when several records
/// claim a name, each is qualified with the name of whatever encloses it —
/// the enclosing record's already-qualified name, or the table when the
/// record hangs directly off one (`home.address`, `users.home.address`).
/// Qualifying every claimant keeps the output independent of table order.
/// A numeric suffix is the last resort, for a record with no enclosing
/// scope left to borrow from.
///
/// # Errors
///
/// Returns [`CoreError::TypesRender`] when `tables` is empty, an element is
/// not a record, two tables share a name, or a name cannot be represented
/// in the grammar.
pub fn render_tables(tables: &[InferredType]) -> Result<String> {
    if tables.is_empty() {
        return Err(CoreError::TypesRender(
            "at least one table is required".to_string(),
        ));
    }

    let mut table_records: Vec<&RecordType> = Vec::with_capacity(tables.len());
    for ty in tables {
        let InferredType::Record(table) = ty else {
            return Err(CoreError::TypesRender(
                "the top-level type must be a record".to_string(),
            ));
        };
        ensure_name(&table.name, "table name")?;
        if table_records.iter().any(|t| t.name == table.name) {
            return Err(CoreError::TypesRender(format!(
                "duplicate table name `{}`",
                table.name
            )));
        }
        table_records.push(table);
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for table in &table_records {
        collect_fields(table, None, &table.name, &mut candidates)?;
    }
    assign_decl_names(&mut candidates, &table_records);

    let mut decls: Vec<String> = table_records
        .iter()
        .map(|table| render_decl("table", &table.name, table, &candidates))
        .collect();
    for candidate in &candidates {
        decls.push(render_decl(
            "record",
            &candidate.decl_name,
            candidate.record,
            &candidates,
        ));
    }
    Ok(decls.join("\n"))
}

/// A nested record reached while walking the tables, before it is named.
struct Candidate<'a> {
    /// The record itself.
    record: &'a RecordType,
    /// Index of the enclosing record in the candidate list; `None` when the
    /// record hangs directly off a table.
    parent: Option<usize>,
    /// Name of the table the record was first reached from.
    table: String,
    /// Declaration name, filled in by [`assign_decl_names`].
    decl_name: String,
}

/// Collect every record reachable from `record`'s fields, on behalf of the
/// table named `table` and enclosed by the record at index `parent`.
fn collect_fields<'a>(
    record: &'a RecordType,
    parent: Option<usize>,
    table: &str,
    candidates: &mut Vec<Candidate<'a>>,
) -> Result<()> {
    for field in &record.fields {
        ensure_name(&field.name, "field name")?;
        collect_type(&field.ty, parent, table, candidates)?;
    }
    Ok(())
}

/// Collect every record reachable from `ty`.
fn collect_type<'a>(
    ty: &'a InferredType,
    parent: Option<usize>,
    table: &str,
    candidates: &mut Vec<Candidate<'a>>,
) -> Result<()> {
    match ty {
        InferredType::Record(record) => {
            // Structurally identical records — same name and same fields —
            // share one declaration, wherever they were reached from.
            if candidates.iter().any(|c| c.record == record) {
                return Ok(());
            }
            ensure_name(&record.name, "record name")?;
            if primitive_from(&record.name).is_some() {
                return Err(CoreError::TypesRender(format!(
                    "record name `{}` collides with a primitive type name",
                    record.name
                )));
            }

            let index = candidates.len();
            candidates.push(Candidate {
                record,
                parent,
                table: table.to_string(),
                decl_name: String::new(),
            });
            collect_fields(record, Some(index), table, candidates)
        }
        InferredType::Array(elem) => collect_type(elem, parent, table, candidates),
        InferredType::Union(members) => members
            .iter()
            .try_for_each(|m| collect_type(m, parent, table, candidates)),
        _ => Ok(()),
    }
}

/// Give every collected record a unique declaration name.
///
/// A record keeps its own name when it is the only record claiming it and
/// nothing else has taken it. Otherwise it is qualified with the name of
/// whatever encloses it.
fn assign_decl_names(candidates: &mut [Candidate], tables: &[&RecordType]) {
    let mut claims: HashMap<String, usize> = HashMap::new();
    for candidate in candidates.iter() {
        *claims.entry(candidate.record.name.clone()).or_insert(0) += 1;
    }

    // Table names are reserved: a `record` sharing a table's name would
    // make the file unreadable, since a field naming it can only ever
    // resolve to the record.
    let mut taken: HashSet<String> = tables.iter().map(|table| table.name.clone()).collect();

    for index in 0..candidates.len() {
        let bare = candidates[index].record.name.clone();
        let enclosing = match candidates[index].parent {
            Some(parent) => candidates[parent].decl_name.clone(),
            None => candidates[index].table.clone(),
        };
        let qualified = format!("{enclosing}{QUALIFIER_SEPARATOR}{bare}");

        let decl_name = if claims[&bare] == 1 && !taken.contains(&bare) {
            bare
        } else if !taken.contains(&qualified) {
            qualified
        } else {
            let mut counter = 1;
            loop {
                counter += 1;
                let suffixed = format!("{qualified}{counter}");
                if !taken.contains(&suffixed) {
                    break suffixed;
                }
            }
        };

        taken.insert(decl_name.clone());
        candidates[index].decl_name = decl_name;
    }
}

/// The declaration name assigned to `record`, if it was collected.
fn decl_name_for<'a>(candidates: &'a [Candidate], record: &RecordType) -> Option<&'a str> {
    candidates
        .iter()
        .find(|candidate| candidate.record == record)
        .map(|candidate| candidate.decl_name.as_str())
}

/// Render one `table` / `record` declaration.
fn render_decl(keyword: &str, name: &str, record: &RecordType, registry: &[Candidate]) -> String {
    let mut out = format!("{keyword} {name} {{\n");
    for field in &record.fields {
        let optional = if field.optional { "?" } else { "" };
        out.push_str(&format!(
            "  {}: {}{optional}\n",
            field.name,
            type_expr(&field.ty, registry)
        ));
    }
    out.push_str("}\n");
    out
}

/// Render a type expression, referencing hoisted records by declaration name.
fn type_expr(ty: &InferredType, registry: &[Candidate]) -> String {
    if let Some(name) = primitive_name(ty) {
        return name.to_string();
    }
    match ty {
        InferredType::Record(record) => decl_name_for(registry, record)
            .expect("every reachable record is collected before rendering")
            .to_string(),
        InferredType::Array(elem) => {
            if matches!(**elem, InferredType::Union(_)) {
                format!("({})[]", type_expr(elem, registry))
            } else {
                format!("{}[]", type_expr(elem, registry))
            }
        }
        InferredType::Union(members) => members
            .iter()
            .map(|m| type_expr(m, registry))
            .collect::<Vec<_>>()
            .join(" | "),
        _ => unreachable!("primitives handled above"),
    }
}

/// Text name of a primitive type, if `ty` is one.
pub(crate) fn primitive_name(ty: &InferredType) -> Option<&'static str> {
    Some(match ty {
        InferredType::String => "string",
        InferredType::Integer => "int",
        InferredType::Float => "float",
        InferredType::Boolean => "bool",
        InferredType::Null => "null",
        InferredType::Date => "date",
        InferredType::DateTime => "datetime",
        InferredType::Unknown => "unknown",
        _ => return None,
    })
}

/// Primitive type for a text name, if it is one.
fn primitive_from(name: &str) -> Option<InferredType> {
    Some(match name {
        "string" => InferredType::String,
        "int" => InferredType::Integer,
        "float" => InferredType::Float,
        "bool" => InferredType::Boolean,
        "null" => InferredType::Null,
        "date" => InferredType::Date,
        "datetime" => InferredType::DateTime,
        "unknown" => InferredType::Unknown,
        _ => return None,
    })
}

/// Whether `s` fits the grammar's name rule: `[A-Za-z_][A-Za-z0-9_.-]*`.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Error unless `s` is a representable name.
fn ensure_name(s: &str, what: &str) -> Result<()> {
    if is_name(s) {
        Ok(())
    } else {
        Err(CoreError::TypesRender(format!(
            "{what} `{s}` cannot be represented (names must match [A-Za-z_][A-Za-z0-9_.-]*)"
        )))
    }
}

// --- Parsing -----------------------------------------------------------

/// An unresolved type expression.
enum TypeExpr {
    Name(String),
    Array(Box<TypeExpr>),
    Union(Vec<TypeExpr>),
}

struct RawField {
    name: String,
    expr: TypeExpr,
    optional: bool,
    line: usize,
}

struct RawDecl {
    is_table: bool,
    name: String,
    fields: Vec<RawField>,
    line: usize,
}

fn parse_err(line: usize, message: impl Into<String>) -> CoreError {
    CoreError::TypesParse {
        line,
        message: message.into(),
    }
}

/// Parse a `.types.datom` file into its table schemas, in declaration order.
///
/// # Errors
///
/// Returns [`CoreError::TypesParse`] for malformed input or files without at
/// least one `table` declaration, malformed input, unknown type
/// references, duplicate declarations, or recursive record references.
pub fn parse_tables(input: &str) -> Result<Vec<InferredType>> {
    let decls = parse_decls(input)?;

    let tables: Vec<&RawDecl> = decls.iter().filter(|d| d.is_table).collect();
    if tables.is_empty() {
        return Err(parse_err(1, "expected a `table` declaration"));
    }

    let records = record_map(&decls)?;
    tables
        .iter()
        .map(|table| resolve_record(table, &records, &mut Vec::new()))
        .collect()
}

/// Index the non-table declarations by name, rejecting duplicates and any
/// record that shadows a table.
fn record_map(decls: &[RawDecl]) -> Result<HashMap<&str, &RawDecl>> {
    let mut records: HashMap<&str, &RawDecl> = HashMap::new();
    for decl in decls.iter().filter(|d| !d.is_table) {
        // A field naming a shadowed table can only ever resolve to the
        // record, so the table becomes unreachable — and the reader cannot
        // tell the two apart when it checks for recursion.
        if decls.iter().any(|d| d.is_table && d.name == decl.name) {
            return Err(parse_err(
                decl.line,
                format!(
                    "record `{}` has the same name as a table; rename one of them",
                    decl.name
                ),
            ));
        }
        if records.insert(decl.name.as_str(), decl).is_some() {
            return Err(parse_err(
                decl.line,
                format!("duplicate record declaration `{}`", decl.name),
            ));
        }
    }
    Ok(records)
}

/// First pass: split the input into raw declarations.
fn parse_decls(input: &str) -> Result<Vec<RawDecl>> {
    let mut decls: Vec<RawDecl> = Vec::new();
    let mut open: Option<RawDecl> = None;

    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line
            .split_once('#')
            .map_or(raw_line, |(before, _)| before)
            .trim();
        if line.is_empty() {
            continue;
        }

        if line == "}" {
            match open.take() {
                Some(decl) => decls.push(decl),
                None => return Err(parse_err(line_no, "unmatched `}`")),
            }
            continue;
        }

        let keyword = ["table ", "record "].iter().find(|k| line.starts_with(**k));
        if let Some(keyword) = keyword {
            if open.is_some() {
                return Err(parse_err(
                    line_no,
                    "declarations cannot be nested; missing `}`?",
                ));
            }
            let rest = line[keyword.len()..].trim();
            let Some(name) = rest.strip_suffix('{').map(str::trim) else {
                return Err(parse_err(line_no, format!("expected `{}NAME {{`", keyword)));
            };
            if !is_name(name) {
                return Err(parse_err(line_no, format!("invalid name `{name}`")));
            }
            open = Some(RawDecl {
                is_table: *keyword == "table ",
                name: name.to_string(),
                fields: Vec::new(),
                line: line_no,
            });
            continue;
        }

        let Some(decl) = open.as_mut() else {
            return Err(parse_err(
                line_no,
                "expected `table NAME {` or `record NAME {`",
            ));
        };
        let Some((field_name, type_part)) = line.split_once(':') else {
            return Err(parse_err(line_no, "expected `field: type`"));
        };
        let field_name = field_name.trim();
        if !is_name(field_name) {
            return Err(parse_err(
                line_no,
                format!("invalid field name `{field_name}`"),
            ));
        }
        if decl.fields.iter().any(|f| f.name == field_name) {
            return Err(parse_err(
                line_no,
                format!("duplicate field `{field_name}`"),
            ));
        }

        let mut type_part = type_part.trim();
        let optional = type_part.ends_with('?');
        if optional {
            type_part = type_part[..type_part.len() - 1].trim_end();
        }
        decl.fields.push(RawField {
            name: field_name.to_string(),
            expr: parse_type_expr(type_part, line_no)?,
            optional,
            line: line_no,
        });
    }

    if let Some(decl) = open {
        return Err(parse_err(
            decl.line,
            format!("declaration `{}` is missing its closing `}}`", decl.name),
        ));
    }
    Ok(decls)
}

/// Parse a full type expression, requiring it to consume all of `src`.
fn parse_type_expr(src: &str, line: usize) -> Result<TypeExpr> {
    let mut cursor = Cursor { src, pos: 0, line };
    let expr = cursor.parse_union()?;
    cursor.skip_ws();
    if cursor.pos < cursor.src.len() {
        return Err(parse_err(
            line,
            format!("unexpected `{}` after type", &cursor.src[cursor.pos..]),
        ));
    }
    Ok(expr)
}

/// Character-level cursor for type expressions.
struct Cursor<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
}

impl Cursor<'_> {
    fn skip_ws(&mut self) {
        while self.src[self.pos..].starts_with([' ', '\t']) {
            self.pos += 1;
        }
    }

    fn parse_union(&mut self) -> Result<TypeExpr> {
        let mut members = vec![self.parse_member()?];
        loop {
            self.skip_ws();
            if self.src[self.pos..].starts_with('|') {
                self.pos += 1;
                members.push(self.parse_member()?);
            } else {
                break;
            }
        }
        Ok(if members.len() == 1 {
            members.pop().expect("length checked")
        } else {
            TypeExpr::Union(members)
        })
    }

    fn parse_member(&mut self) -> Result<TypeExpr> {
        self.skip_ws();
        let mut expr = if self.src[self.pos..].starts_with('(') {
            self.pos += 1;
            let inner = self.parse_union()?;
            self.skip_ws();
            if !self.src[self.pos..].starts_with(')') {
                return Err(parse_err(self.line, "missing `)`"));
            }
            self.pos += 1;
            inner
        } else {
            let rest = &self.src[self.pos..];
            let len = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
                .count();
            if len == 0 {
                return Err(parse_err(self.line, "expected a type"));
            }
            self.pos += len;
            TypeExpr::Name(rest[..len].to_string())
        };

        loop {
            self.skip_ws();
            if self.src[self.pos..].starts_with("[]") {
                self.pos += 2;
                expr = TypeExpr::Array(Box::new(expr));
            } else {
                break;
            }
        }
        Ok(expr)
    }
}

/// Second pass: resolve a declaration into an [`InferredType::Record`],
/// inlining referenced record declarations.
fn resolve_record(
    decl: &RawDecl,
    records: &HashMap<&str, &RawDecl>,
    stack: &mut Vec<String>,
) -> Result<InferredType> {
    stack.push(decl.name.clone());
    let mut fields = Vec::with_capacity(decl.fields.len());
    for field in &decl.fields {
        fields.push(RecordField {
            name: field.name.clone(),
            ty: resolve_expr(&field.expr, field.line, records, stack)?,
            optional: field.optional,
        });
    }
    stack.pop();

    Ok(InferredType::Record(RecordType {
        name: decl.name.clone(),
        fields,
    }))
}

fn resolve_expr(
    expr: &TypeExpr,
    line: usize,
    records: &HashMap<&str, &RawDecl>,
    stack: &mut Vec<String>,
) -> Result<InferredType> {
    match expr {
        TypeExpr::Name(name) => {
            if let Some(primitive) = primitive_from(name) {
                return Ok(primitive);
            }
            let Some(decl) = records.get(name.as_str()) else {
                return Err(parse_err(line, format!("unknown type `{name}`")));
            };
            if stack.contains(name) {
                return Err(parse_err(
                    line,
                    format!("recursive reference to record `{name}`"),
                ));
            }
            resolve_record(decl, records, stack)
        }
        TypeExpr::Array(elem) => Ok(InferredType::Array(Box::new(resolve_expr(
            elem, line, records, stack,
        )?))),
        TypeExpr::Union(members) => Ok(InferredType::Union(
            members
                .iter()
                .map(|m| resolve_expr(m, line, records, stack))
                .collect::<Result<Vec<_>>>()?,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{infer_response, infer_value};
    use serde_json::json;
    use tempfile::tempdir;

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

    /// Render a document holding the single table `ty`.
    fn render_one(ty: &InferredType) -> Result<String> {
        render_tables(std::slice::from_ref(ty))
    }

    /// The single table of a one-table document.
    fn parse_one(input: &str) -> InferredType {
        let mut tables = parse_tables(input).expect("parses");
        assert_eq!(tables.len(), 1, "expected exactly one table");
        tables.remove(0)
    }

    fn assert_round_trips(ty: &InferredType) {
        let text = render_one(ty).unwrap();
        let parsed = parse_one(&text);
        assert_eq!(&parsed, ty, "not identical after round-trip:\n{text}");
    }

    #[test]
    fn renders_example_layout() {
        let ty = record(
            "users",
            vec![
                field("id", InferredType::Integer, false),
                field("email", InferredType::String, false),
                field("created_at", InferredType::DateTime, false),
                field(
                    "address",
                    record(
                        "address",
                        vec![
                            field("city", InferredType::String, false),
                            field("zip", InferredType::String, false),
                        ],
                    ),
                    true,
                ),
            ],
        );

        let expected = "\
table users {
  id: int
  email: string
  created_at: datetime
  address: address?
}

record address {
  city: string
  zip: string
}
";
        assert_eq!(render_one(&ty).unwrap(), expected);
        assert_round_trips(&ty);
    }

    #[test]
    fn round_trips_flat_record_with_all_primitives() {
        assert_round_trips(&record(
            "t",
            vec![
                field("a", InferredType::String, false),
                field("b", InferredType::Integer, true),
                field("c", InferredType::Float, false),
                field("d", InferredType::Boolean, false),
                field("e", InferredType::Null, true),
                field("f", InferredType::Date, false),
                field("g", InferredType::DateTime, false),
                field("h", InferredType::Unknown, false),
            ],
        ));
    }

    #[test]
    fn round_trips_nested_records() {
        let geo = record("geo", vec![field("lat", InferredType::Float, false)]);
        let address = record(
            "address",
            vec![
                field("city", InferredType::String, false),
                field("geo", geo, true),
            ],
        );
        assert_round_trips(&record("users", vec![field("address", address, false)]));
    }

    #[test]
    fn round_trips_arrays() {
        let item = record("items", vec![field("sku", InferredType::String, false)]);
        assert_round_trips(&record(
            "t",
            vec![
                field(
                    "tags",
                    InferredType::Array(Box::new(InferredType::String)),
                    false,
                ),
                field(
                    "matrix",
                    InferredType::Array(Box::new(InferredType::Array(Box::new(
                        InferredType::Integer,
                    )))),
                    false,
                ),
                field("items", InferredType::Array(Box::new(item)), true),
                field(
                    "empty",
                    InferredType::Array(Box::new(InferredType::Unknown)),
                    false,
                ),
                field(
                    "mixed",
                    InferredType::Array(Box::new(InferredType::Union(vec![
                        InferredType::Integer,
                        InferredType::String,
                    ]))),
                    false,
                ),
            ],
        ));
    }

    #[test]
    fn round_trips_unions() {
        let variant = record("variant", vec![field("x", InferredType::Integer, false)]);
        assert_round_trips(&record(
            "t",
            vec![
                field(
                    "v",
                    InferredType::Union(vec![InferredType::Integer, InferredType::String]),
                    true,
                ),
                field(
                    "w",
                    InferredType::Union(vec![InferredType::Date, InferredType::DateTime, variant]),
                    false,
                ),
                field(
                    "arrays",
                    InferredType::Union(vec![
                        InferredType::Array(Box::new(InferredType::Integer)),
                        InferredType::Array(Box::new(InferredType::String)),
                    ]),
                    false,
                ),
            ],
        ));
    }

    #[test]
    fn round_trips_inferred_response() {
        let response = json!({
            "data": [
                {"id": 1, "name": "Ada", "address": {"city": "London"}, "tags": ["a"]},
                {"id": 2, "name": null, "tags": []},
            ]
        });
        let schema = infer_response("resp", &response);
        assert_round_trips(&schema.ty);
    }

    #[test]
    fn structurally_identical_records_share_one_declaration() {
        let point = |n: &str| {
            record(
                n,
                vec![
                    field("x", InferredType::Float, false),
                    field("y", InferredType::Float, false),
                ],
            )
        };
        let ty = record(
            "t",
            vec![
                field("start", point("point"), false),
                field("end", point("point"), false),
            ],
        );

        let text = render_one(&ty).unwrap();
        assert_eq!(text.matches("record point {").count(), 1, "{text}");
        assert_round_trips(&ty);
    }

    #[test]
    fn disambiguates_conflicting_record_names() {
        // `address` appears at two paths within one response with different
        // shapes. Inferred from real JSON, so the record names are the field
        // names the renderer qualifies with.
        let ty = infer_response(
            "t",
            &json!([{
                "home": {"address": {"city": "London"}},
                "work": {"address": {"zip": 90210}},
            }]),
        )
        .ty;

        let text = render_one(&ty).unwrap();
        // Both claimants are qualified by their enclosing record; neither
        // keeps the bare `address`, so the result cannot depend on order.
        assert!(!text.contains("record address {"), "{text}");
        assert!(text.contains("record home.address {"), "{text}");
        assert!(text.contains("record work.address {"), "{text}");
        assert!(text.contains("address: home.address\n"), "{text}");
        assert!(text.contains("address: work.address\n"), "{text}");
        // `home` and `work` are unique, so they stay unqualified.
        assert!(text.contains("record home {"), "{text}");
        assert!(text.contains("record work {"), "{text}");

        // Structure survives; only the colliding records' names are rewritten.
        let InferredType::Record(table) = parse_one(&text) else {
            panic!("expected record");
        };
        let InferredType::Record(work) = &table.fields[1].ty else {
            panic!("expected record field");
        };
        assert_eq!(work.name, "work");
        let InferredType::Record(work_address) = &work.fields[0].ty else {
            panic!("expected record field");
        };
        assert_eq!(work_address.name, "work.address");
        assert_eq!(work_address.fields[0].name, "zip");
    }

    #[test]
    fn renders_multiple_tables_sharing_and_prefixing_records() {
        let geo = || record("geo", vec![field("lat", InferredType::Float, false)]);
        let users = record(
            "users",
            vec![
                field(
                    "address",
                    record("address", vec![field("city", InferredType::String, false)]),
                    false,
                ),
                field("geo", geo(), false),
            ],
        );
        let orders = record(
            "orders",
            vec![
                // Same name as users' `address` but a different structure.
                field(
                    "address",
                    record("address", vec![field("zip", InferredType::Integer, false)]),
                    false,
                ),
                // Structurally identical to users' `geo`: shared declaration.
                field("geo", geo(), false),
            ],
        );

        let text = render_tables(&[users, orders]).unwrap();

        assert!(text.contains("table users {"), "{text}");
        assert!(text.contains("table orders {"), "{text}");
        // `geo` is uncontested, so it keeps its bare name and stays shared.
        assert_eq!(text.matches("record geo {").count(), 1, "{text}");
        // `address` is contested, so both claimants are qualified.
        assert!(!text.contains("record address {"), "{text}");
        assert!(text.contains("record users.address {"), "{text}");
        assert!(text.contains("record orders.address {"), "{text}");
        assert!(text.contains("address: orders.address\n"), "{text}");

        // The document parses back into both tables; the colliding record
        // keeps its structure under the prefixed name.
        let parsed = parse_tables(&text).unwrap();
        assert_eq!(parsed.len(), 2);
        let InferredType::Record(orders) = &parsed[1] else {
            panic!("expected record");
        };
        let InferredType::Record(address) = &orders.fields[0].ty else {
            panic!("expected record field");
        };
        assert_eq!(address.name, "orders.address");
        assert_eq!(address.fields[0].name, "zip");
    }

    #[test]
    fn render_tables_rejects_empty_input() {
        let err = render_tables(&[]).unwrap_err();
        assert!(matches!(err, CoreError::TypesRender(_)), "{err}");
    }

    #[test]
    fn nested_record_never_shadows_its_own_table() {
        // A `users` endpoint whose records contain a `users` object: the
        // hoisted record must not be declared as `record users`, or the
        // file it lands in cannot be read back.
        let ty = record(
            "users",
            vec![
                field("id", InferredType::Integer, false),
                field(
                    "users",
                    record("users", vec![field("x", InferredType::Integer, false)]),
                    false,
                ),
            ],
        );

        let text = render_tables(std::slice::from_ref(&ty)).unwrap();

        assert!(text.contains("table users {"), "{text}");
        assert!(!text.contains("record users {"), "{text}");
        assert!(text.contains("record users.users {"), "{text}");

        // Only the colliding record's name is rewritten; the structure
        // reads back intact (so `render → parse` no longer errors out).
        let parsed = parse_tables(&text).unwrap();
        let InferredType::Record(table) = &parsed[0] else {
            panic!("expected record");
        };
        assert_eq!(table.name, "users");
        let InferredType::Record(nested) = &table.fields[1].ty else {
            panic!("expected record field");
        };
        assert_eq!(nested.name, "users.users");
        assert_eq!(nested.fields[0].name, "x");
    }

    #[test]
    fn nested_record_never_shadows_another_table() {
        // `orders` nests a record named after the *other* endpoint.
        let users = record("users", vec![field("id", InferredType::Integer, false)]);
        let orders = record(
            "orders",
            vec![field(
                "users",
                record("users", vec![field("x", InferredType::Integer, false)]),
                false,
            )],
        );

        let text = render_tables(&[users, orders]).unwrap();

        assert!(!text.contains("record users {"), "{text}");
        assert!(text.contains("record orders.users {"), "{text}");
        // Both tables survive and the document reads back.
        assert_eq!(parse_tables(&text).unwrap().len(), 2, "{text}");
    }

    #[test]
    fn numeric_suffix_when_a_record_runs_out_of_scopes_to_borrow() {
        // Two different records named `a` hanging off the *same* table:
        // both qualify to `t.a`, and neither has anything above the table
        // left to borrow, so one has to take a suffix.
        let ty = record(
            "t",
            vec![
                field(
                    "f1",
                    record("a", vec![field("p", InferredType::Integer, false)]),
                    false,
                ),
                field(
                    "f2",
                    record("a", vec![field("q", InferredType::Integer, false)]),
                    false,
                ),
            ],
        );

        let text = render_one(&ty).unwrap();

        assert!(text.contains("record t.a {"), "{text}");
        assert!(text.contains("record t.a2 {"), "{text}");
        // Still readable, which is the whole point of the fallback.
        assert_eq!(parse_tables(&text).unwrap().len(), 1, "{text}");
    }

    #[test]
    fn qualification_cascades_when_parent_and_child_are_both_contested() {
        // users → [{"home": {"address": {"city": …}}}]
        // staff → [{"home": {"address": {"zip":  …}}}]
        let nest = |inner: RecordField| {
            record(
                "home",
                vec![field("address", record("address", vec![inner]), false)],
            )
        };
        let users = record(
            "users",
            vec![field(
                "home",
                nest(field("city", InferredType::String, false)),
                false,
            )],
        );
        let staff = record(
            "staff",
            vec![field(
                "home",
                nest(field("zip", InferredType::Integer, false)),
                false,
            )],
        );

        let text = render_tables(&[users, staff]).unwrap();

        // `home` is contested, so both are qualified by their table; the
        // children inherit those resolved names rather than the raw `home`.
        assert!(text.contains("record users.home {"), "{text}");
        assert!(text.contains("record staff.home {"), "{text}");
        assert!(text.contains("record users.home.address {"), "{text}");
        assert!(text.contains("record staff.home.address {"), "{text}");
        assert_eq!(parse_tables(&text).unwrap().len(), 2, "{text}");
    }

    #[test]
    fn qualification_stops_where_the_ambiguity_stops() {
        // Parents are unique, so only the children are qualified — and by
        // their parent, not by the table: `profile.address`, never
        // `users.profile.address`.
        let users = record(
            "users",
            vec![field(
                "profile",
                record(
                    "profile",
                    vec![field(
                        "address",
                        record("address", vec![field("city", InferredType::String, false)]),
                        false,
                    )],
                ),
                false,
            )],
        );
        let staff = record(
            "staff",
            vec![field(
                "info",
                record(
                    "info",
                    vec![field(
                        "address",
                        record("address", vec![field("zip", InferredType::Integer, false)]),
                        false,
                    )],
                ),
                false,
            )],
        );

        let text = render_tables(&[users, staff]).unwrap();

        assert!(text.contains("record profile {"), "{text}");
        assert!(text.contains("record info {"), "{text}");
        assert!(text.contains("record profile.address {"), "{text}");
        assert!(text.contains("record info.address {"), "{text}");
        assert!(!text.contains("users.profile"), "{text}");
        assert_eq!(parse_tables(&text).unwrap().len(), 2, "{text}");
    }

    #[test]
    fn dotted_field_names_are_representable() {
        // Real APIs ship keys with dots in them; they used to be rejected
        // outright by the name rule.
        let ty = record(
            "users",
            vec![field(
                "users.home",
                record(
                    "users.home",
                    vec![field("zip", InferredType::Integer, false)],
                ),
                false,
            )],
        );

        let text = render_one(&ty).unwrap();

        assert!(text.contains("users.home: users.home\n"), "{text}");
        assert_round_trips(&ty);
    }

    #[test]
    fn generated_qualifier_yields_to_a_literal_dotted_key_across_tables() {
        // `home` is contested, so users' becomes `users.home` — the same
        // name a literal dotted key in `staff` already carries. The literal
        // one has a scope left to borrow, so it takes `staff.users.home`.
        let users = record(
            "users",
            vec![field(
                "home",
                record("home", vec![field("city", InferredType::String, false)]),
                false,
            )],
        );
        let staff = record(
            "staff",
            vec![
                field(
                    "home",
                    record("home", vec![field("country", InferredType::String, false)]),
                    false,
                ),
                field(
                    "users.home",
                    record(
                        "users.home",
                        vec![field("zip", InferredType::Integer, false)],
                    ),
                    false,
                ),
            ],
        );

        let text = render_tables(&[users, staff]).unwrap();

        assert!(text.contains("record users.home {"), "{text}");
        assert!(text.contains("record staff.home {"), "{text}");
        assert!(text.contains("record staff.users.home {"), "{text}");
        assert!(text.contains("users.home: staff.users.home\n"), "{text}");
        // The file is still readable — no duplicate declaration.
        assert_eq!(parse_tables(&text).unwrap().len(), 2, "{text}");
    }

    #[test]
    fn render_tables_rejects_duplicate_table_names() {
        let table = record("users", vec![field("id", InferredType::Integer, false)]);
        let err = render_tables(&[table.clone(), table]).unwrap_err();

        assert!(matches!(err, CoreError::TypesRender(_)), "{err}");
        assert!(err.to_string().contains("duplicate table name"), "{err}");
    }

    #[test]
    fn parse_rejects_a_record_shadowing_a_table() {
        // Hand-written (or previously generated) files get a message that
        // names the real problem, not a bogus recursion complaint.
        let input = "
            table users {
              users: users
            }

            record users {
              x: int
            }
        ";
        let err = parse_tables(input).unwrap_err();
        assert!(err.to_string().contains("same name as a table"), "{err}");
        assert!(!err.to_string().contains("recursive"), "{err}");
    }

    #[test]
    fn parse_tables_requires_a_table() {
        let err = parse_tables("record r {\n  x: int\n}\n").unwrap_err();
        assert!(err.to_string().contains("table"), "{err}");
    }

    #[test]
    fn parses_handwritten_file() {
        let input = "
            # A hand-written schema, record declared before the table.
            record address {
              city: string   # trailing comment
              zip: string
            }

            table users {
              id: int
              flags: (int | string)[]
              v: int | string?
              address: address?
            }
        ";
        let InferredType::Record(table) = parse_one(input) else {
            panic!("expected record");
        };
        assert_eq!(table.name, "users");

        let v = &table.fields[2];
        assert!(v.optional, "trailing ? applies to the whole field");
        assert_eq!(
            v.ty,
            InferredType::Union(vec![InferredType::Integer, InferredType::String])
        );

        let flags = &table.fields[1];
        assert_eq!(
            flags.ty,
            InferredType::Array(Box::new(InferredType::Union(vec![
                InferredType::Integer,
                InferredType::String
            ])))
        );

        let address = &table.fields[3];
        assert!(address.optional);
        assert!(matches!(&address.ty, InferredType::Record(r) if r.name == "address"));
    }

    #[test]
    fn parse_errors_are_located_and_described() {
        let unknown = parse_tables("table t {\n  x: widget\n}\n").unwrap_err();
        assert!(
            matches!(&unknown, CoreError::TypesParse { line: 2, message } if message.contains("widget")),
            "{unknown}"
        );

        let no_table = parse_tables("record r {\n  x: int\n}\n").unwrap_err();
        assert!(no_table.to_string().contains("table"), "{no_table}");

        let cycle =
            parse_tables("table t {\n  a: node\n}\nrecord node {\n  next: node\n}\n").unwrap_err();
        assert!(cycle.to_string().contains("recursive"), "{cycle}");

        let unclosed = parse_tables("table t {\n  x: int\n").unwrap_err();
        assert!(unclosed.to_string().contains("closing"), "{unclosed}");

        let garbage = parse_tables("what is this\n").unwrap_err();
        assert!(
            matches!(garbage, CoreError::TypesParse { line: 1, .. }),
            "{garbage}"
        );
    }

    #[test]
    fn render_rejects_unrepresentable_schemas() {
        let not_a_record = render_one(&InferredType::Integer).unwrap_err();
        assert!(
            matches!(not_a_record, CoreError::TypesRender(_)),
            "{not_a_record}"
        );

        let bad_field = record("t", vec![field("weird key", InferredType::String, false)]);
        assert!(render_one(&bad_field).is_err());

        let reserved = record(
            "t",
            vec![field(
                "s",
                record("string", vec![field("x", InferredType::Integer, false)]),
                false,
            )],
        );
        assert!(render_one(&reserved).is_err());
    }

    #[test]
    fn saves_and_loads_types_file() {
        let tmp = tempdir().unwrap();
        let ty = infer_value("todos", &json!({"id": 1, "title": "x", "done": false}));

        let path = save_tables(tmp.path(), "todos", std::slice::from_ref(&ty)).unwrap();
        assert_eq!(path, types_path(tmp.path(), "todos"));
        assert!(
            path.file_name().unwrap().to_str().unwrap() == "todos.types.datom",
            "file is named <name>.types.datom"
        );
        assert_eq!(load_tables(tmp.path(), "todos").unwrap(), vec![ty]);
    }
}
