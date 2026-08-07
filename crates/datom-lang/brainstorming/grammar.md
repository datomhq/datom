```
program |- (statement)*

statement |- typeStatement

typeStatement |- "type" typeConstructor
              | "type" ident "{" typeConstructor ("," typeConstructor)* (",")? "}"

typeConstructor |- ident "(" typeFields ")"

typeFields |- typeField ("," typeField)* (",")?

typeField |- ident ":" primitive

ident |- [a-zA-Z]([a-zA-Z0-9_])*

primitive |- "string" | "u32" | "i32" | "bool" | "datetime" | "f32" | "f64"
```

legend:
- `|-` denotes a production rule
- `()` used for grouping items in a multi-valued production
- `?` 0-1 of the preceding group
- `*` 0 or more of the preceding group
- `+` 1 or more of the preceding group
- `""` denotes literal characters
- `|` one of the productions
