# datom

The code-first business intelligence platform.

## Build

```sh
cargo build --workspace
```

## Install

Install the `datom` CLI into `~/.cargo/bin`:

```sh
cargo install --path crates/datom-cli
```

## Run

Check a source file's syntax and print its AST:

```sh
datom check crates/datom-lang/samples/tour.datom
```
