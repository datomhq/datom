//! Implementation of the `datom parse` command.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// `datom parse <file>`: validate a source file's syntax and print its AST.
pub fn parse(file: &Path) -> Result<()> {
    let source =
        fs::read_to_string(file).with_context(|| format!("could not read `{}`", file.display()))?;

    match lang::parse(&source) {
        Ok(tree) => {
            print!("{tree}");
            Ok(())
        }
        Err(failure) => {
            eprintln!("{failure}");
            bail!("could not parse `{}`", file.display())
        }
    }
}
