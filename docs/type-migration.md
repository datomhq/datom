# Migrating the lang type system into `datom-core`

`datom-lang/src/types.rs` becomes the platform's one type model. The
inference-era types in `datom-core` — `InferredType`, `RecordType`,
`RecordField` — are retired. Step sequencing lives in
[type-migration-plan.md](type-migration-plan.md); this file records the
decisions those steps implement.

## 1. Variant mapping

| `InferredType` | `Type` | |
|---|---|---|
| `String` | `Primitive::String` | |
| `Boolean` | `Primitive::Bool` | |
| `Integer` | `Primitive::Number` | **lossy** — collapses with `Float` |
| `Float` | `Primitive::Number` | **lossy** — collapses with `Integer` |
| `Date` | `Primitive::DateTime` | **lossy** — a date-only value gains a time it never had |
| `DateTime` | `Primitive::DateTime` | |
| `Record(RecordType)` | `Type::single(name, Fields)` | see field losses below |
| `Array(inner)` | `Type::collection(Collection::List, inner)` | |
| `Union(members)` | `Type::inline_variadic(name, members)` | needs a name — §3 |
| `Null` | none | §2 |
| `Unknown` | none | §2 |

Two losses live on records rather than on a variant:

| | |
|---|---|
| `RecordField.optional` | **dropped.** `Fields` is `HashMap<String, Type>`; there is nowhere to put it. |
| Field declaration order | **dropped.** `Display` sorts field names alphabetically, so output stays deterministic — but it no longer matches the order the JSON presented. |

Three knock-on effects worth expecting:

- `schema.rs:203 unify_pair` has arms for `(Integer, Float)` and for
  `(Date, String)` / `(DateTime, String)`. Once `Date` and `DateTime` are one
  type and `Integer` and `Float` are one type, those arms are either dead or
  merge into identity.
- `schema.rs:273 unify_field` dissolves null into optionality: a field null in
  some samples becomes optional rather than a union with null. With
  optionality gone, that machinery has no output to write to.
- `schema_diff.rs:139 same_type` deliberately **ignores** nested record names
  because the old renderer rewrote them for uniqueness. `Type`'s derived
  `PartialEq` compares `name`. `schema_diff` therefore cannot switch to `==`;
  it needs its own name-blind structural comparison.

## 2. `Null` and `Unknown`

`Null` reaches a field only when every sample had null there
(`schema.rs:273`); `Unknown` is the element type of an always-empty array and
the result of inferring zero records (`schema.rs:119`).

**Decision: drop the field from the record, and name the dropped fields in the
introspect report.** The data has told us nothing about these positions. A
schema that omits them is honest about that; the report is where the user
learns the field exists but was never populated, which is exactly the prompt
to sample more records.

The alternative was substituting a primitive — `string` being the usual
choice. It lost because it converts "unknown" into a specific, checkable
claim. `datom datasource test` would then diff that invented `string` against
whatever the field turns out to hold and report drift that is really just the
placeholder being wrong. An always-empty array is the sharpest case: `list<T>`
has no honest `T`, and picking one guarantees a false contract failure the
first time the array is non-empty.

## 3. Naming anonymous unions

`Type::inline_variadic(name, variants)` requires a name; every `Type` has one.
`InferredType::Union` is positional and has none.

**Decision: name a union after the field that holds it** — the rule records
already follow, where `infer_value(key, value)` at `schema.rs:157` passes the
field name down as the record's name. A union under `id` becomes
`type id = string | number;` and the field reads `id: id`.

Two cases need more than that rule:

- **Top of a table.** A union at the root has no enclosing field, but it does
  have the table name from the endpoint, which `introspect.rs:192` already
  passes as `infer_response`'s `root_name`. Use it. In practice this is rare:
  `introspect.rs:199` already rejects any endpoint whose response is not a
  record, so a top-level union fails introspection before it needs a name.
- **Collisions.** Field names are unique within a record but not across a
  file, so two unions under different records can both claim `id`. The old
  renderer solved this by qualifying with the enclosing scope
  (`home.address`); `.` is not legal in a datom identifier, so the qualifier
  needs a new separator — `_` reads best (`users_id`). Name uniquing is a
  whole-file concern and belongs in the renderer, not in inference.

## 4. Dependency direction

`Primitive` and `Collection` are not only semantic. `scanner.rs:166-172` maps
keywords onto them, and `parser.rs:69-76` builds `TYPE_NAME_KINDS` out of
them. They cannot stay behind in lang and lang cannot stop using them, so
**`datom-lang` gains a dependency on `datom-core`.**

That forbids `datom-core -> datom-lang`. Core reads `.types.datom` back in two
places today:

- `introspect.rs:151 recorded_tables` — the previous schema, to diff against
- `connectivity.rs:421 load_stored_tables` — the recorded contract, for `test`

Once the file format is datom source, both need the lang parser, which core
may not call. Core declares the boundary instead:

```rust
// datom-core
pub trait TypeReader {
    fn read_types(&self, source: &str) -> std::result::Result<Vec<Type>, String>;
}
```

`datom-lang` implements it — legal, since lang depends on core — and
`datom-cli`, which depends on both, injects it through
`introspect_datasource` (`introspect.rs:83`) and `test_datasource`
(`connectivity.rs:198`). Writing needs no boundary: rendering is `Display`,
which travels with `Type` into core.

## 5. Consumers to change

| File | Lines | What |
|---|---|---|
| `datom-core/src/schema.rs` | 14-71 | `InferredType`, `RecordType`, `RecordField`, `InferredSchema` definitions — deleted |
| | 82, 119, 131 | `infer_response`, `infer_records`, `infer_value` — return `Type` |
| | 168 | `rename_as_inferred` — nested naming, revisit against `Type`'s naming rules |
| | 190-281 | `unify`, `unify_pair`, `add_to_union`, `union_of`, `merge_records`, `unify_field` — retarget; optionality paths die |
| `datom-core/src/schema_diff.rs` | 55-192 | whole module retargets; `same_type:139` needs name-blind equality; `field_label:183` loses `?` |
| | 11, 161 | imports `types_format::primitive_name`, which the format switch deletes |
| `datom-core/src/types_format.rs` | 1-1348 | grammar deleted; `TYPES_FILE_SUFFIX` and `types_path:43` survive; `save_tables:53` renders via lang; `load_tables:70` goes behind `TypeReader` |
| `datom-core/src/introspect.rs` | 15-16, 83, 99, 138, 151-167, 192-201 | imports, `introspect_datasource` signature, `recorded_tables`, `introspect_endpoint` |
| `datom-core/src/connectivity.rs` | 21-23, 198, 354, 392-398, 419-437 | imports, `test_datasource` signature, `StoredTypes`, `load_stored_tables` |
| | 490-526, 608 | test fixtures build schemas and write a types file |
| `datom-core/src/lib.rs` | 35-36, 38, 40 | re-export lists |
| | 194, 198 | `CoreError::TypesRender` and `TypesParse` — no source once the grammar goes |
| `datom-lang/Cargo.toml` | — | add the `datom-core` dependency |
| `datom-lang/src/{scanner,parser,error}.rs` | 6, 7, 94 | import `Primitive`/`Collection` from core |
| `datom-cli/src/datasource.rs` | 63, 162 | pass the `TypeReader` into introspect and test |
| `datom-cli/tests/introspect.rs` | 137, 222 | `parse_tables` on written output |
| `datom-cli/tests/list.rs` | 78, 120 | `infer_value` + `save_tables` fixtures |

`datom-cli/src/datasource.rs:100` and `:264` call `types_path` only to test
for existence, and are unaffected.

Test counts moving with the work: `schema.rs` 17, `types_format.rs` 24,
`schema_diff.rs` 7, `introspect.rs` 8, `connectivity.rs` 12, and 28 CLI
integration tests.
