```
program |- (statement)*

statement |- typeStatement
          | exprStatement

typeStatement |- "type" typeConstructor
              | "type" ident "{" typeConstructor ("," typeConstructor)* (",")? "}"
              | "type" ident "=" typeName ("|" typeName)* ";"

typeName |- ident | primitive | collection

typeConstructor |- ident "(" typeFields ")"

typeFields |- typeField ("," typeField)* (",")?

typeField |- ident ":" typeName

ident |- [a-zA-Z]([a-zA-Z0-9_])*

primitive |- "number" | "string" | "bool" | "datetime"

collection |- "list" generic | "map" generic | "set" generic

generic |- "<" typeName ">"

exprStatement |- expr ";"

expr |- number | string | bool

number |- [0-9]([0-9._])*

string |- """ .* """

bool |- "true" | "false"
```

legend:
- `|-` denotes a production rule
- `()` used for grouping items in a multi-valued production
- `?` 0-1 of the preceding group
- `*` 0 or more of the preceding group
- `+` 1 or more of the preceding group
- `""` denotes literal characters
- `|` one of the productions
