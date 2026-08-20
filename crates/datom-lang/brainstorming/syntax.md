# Syntax

Some quick thoughts and explorations around the syntax of the Datom language.
The language should be optimized for columnar data transformations, including table splits, merges, joins, etc.
The language should be strongly typed and pragmatically functional.
Pipelines should be cleanly represented as a sequence of pure transformations, with allowances for necessary side effects (network calls, logging).

## Type definitions

```datom
// A table of orders
type Order (
  id: number,
  date: datetime,
)

type OrderStatus {
  Active("active"),
  Cancelled("cancelled"),
}

type Product {
  Online(
    sku: string,
    url: string,
    name: string,
  ),
  InStore(
    sku: string,
    name: string,
    aisle: number,
    bay: number,
  )
}

type Id = string | number;
```

Some notes on the above:
- every `type` is a sum type with multiple variants
- `type IDENT (...)` indicates there is only one variant of the type, implicitly named the same as the type itself
  - in the example above, `Order` is a single-variant type
- `type IDENT { IDENT(), ... }` indicates there are multiple variants, each with potentially different nested fields
  - in the example above, `Product` is a multiple-variant type
- `Active("active")` would indicate this is like a string-backed enum, so parsing from the source object would convert `"status": "active"` to the `Active` variant
  - main concern: this clashes somewhat with defining the fields for the variant?
  - might be ok to disambiguate as there are no field definitions, just a flat string literal
- trailing commas on the last struct item are accepted but not required

## Pipelines

```datom
tables.empty()
  .with(data.database.load(prefix: "db_"))
  .with(data.sap.load(prefix: "sap_"))
  .with(data.some_internal_api.load())
|> |tables| {
  tables.join(
    left: tables.db_orders.id, 
    right: tables.sap_orders.order_id, 
    out: "orders", 
    left_prefix: "db_", 
    right_prefix: "sap_"
  )
}
|> |tables| {
  tables.relate(tables.orders.customer_id, tables.db_customers.id)
}
|> |tables, log| {
  tables.extend(tables.db_customers, name: "full_name", value: |row| {
    log.info(f"computing full name for row with first name '{row.first_name}' and last name '{row.last_name}'");
    f"{row.first_name} {row.last_name}"
  })
}
```

Some notes:
- pipelines are structured as a chain of `|>`, where each stage can capture the current tables (aliased to any name), plus some extra params (like `log`)
  - this is inspired by Gleam
- all the defined data sources in a project are available on the global `data` value
- functions can be defined to have argument labels (i.e. `.load(prefix: "db_")`) or to not (i.e. `tables.relate(left, right)`)
  - this is inpsired by Swift, where it can be defined like the below

```swift
func withLabel(arg: string) {}

func withoutLabel(_ arg: string) {}
```

- the final statement in a block can omit the semicolon to indicate the value of the expression should be returned
- statements like `tables.relate(...)` and `tables.extend(...)` implicitly return the tables object itself after performing the operation
- statements like `tables.join(...)` are similar but implicitly collapse the two joined tables down into one
  - so in the example above, tables `db_orders` and `sap_orders` are not available in steps after the `tables.join` call
