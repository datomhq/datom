//! Implementation of the `datom parse` command.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// `datom parse <file>`: validate a source file's syntax and print its AST.
pub fn parse(file: &Path) -> Result<()> {
    let source =
        fs::read_to_string(file).with_context(|| format!("could not read `{}`", file.display()))?;

    match lang::parse(&source) {
        Ok(parsed) => {
            print!("{}", parsed.tree);
            eprint!("{}", parsed.diagnostics);
            Ok(())
        }
        Err(failure) => {
            eprint!("{}", failure.diagnostics());
            Err(failure).with_context(|| format!("could not parse `{}`", file.display()))
        }
    }
}
