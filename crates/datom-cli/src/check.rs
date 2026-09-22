//! Implementation of the `datom check` command.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};

/// `datom check <file>`: validate a source file's syntax and print its AST.
pub fn check(file: &Path) -> Result<()> {
    run(file, &mut io::stdout(), &mut io::stderr())
}

/// [`check`], writing the tree to `out` and diagnostics to `err`.
fn run(file: &Path, out: &mut impl Write, err: &mut impl Write) -> Result<()> {
    let source =
        fs::read_to_string(file).with_context(|| format!("could not read `{}`", file.display()))?;

    match lang::parse(&source) {
        Ok(tree) => {
            write!(out, "{tree}")?;
            Ok(())
        }
        Err(failure) => {
            writeln!(err, "{failure}")?;
            bail!("could not parse `{}`", file.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// What one run of the command returned and wrote.
    struct Output {
        result: Result<()>,
        stdout: String,
        stderr: String,
    }

    /// Write `source` to a temp file and check it.
    fn check_source(source: &str) -> Output {
        let dir = tempdir().unwrap();
        let file = dir.path().join("input.datom");
        fs::write(&file, source).expect("failed to write source file");

        check_file(&file)
    }

    fn check_file(file: &Path) -> Output {
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let result = run(file, &mut stdout, &mut stderr);

        Output {
            result,
            stdout: String::from_utf8(stdout).expect("stdout should be utf-8"),
            stderr: String::from_utf8(stderr).expect("stderr should be utf-8"),
        }
    }

    #[test]
    fn a_valid_file_prints_its_tree_and_succeeds() {
        let output = check_source("type Person(name: string, tags: list<string>)\n");

        assert!(output.result.is_ok());
        assert_eq!(
            output.stdout,
            "\
program
└─ single type `Person`
   ├─ field `name`: string
   └─ field `tags`: list<string>
"
        );
        assert_eq!(output.stderr, "");
    }

    #[test]
    fn an_empty_file_is_a_valid_empty_program() {
        let output = check_source("");

        assert!(output.result.is_ok());
        assert_eq!(output.stdout, "program\n");
    }

    #[test]
    fn a_parse_error_fails_and_names_what_it_wanted() {
        let output = check_source("type Person(name string)\n");

        assert!(output.result.is_err());
        assert!(
            output
                .stderr
                .contains("[1:18] error: Expected `:`, found `string`")
        );
    }

    #[test]
    fn a_scan_error_reports_the_offending_character_and_its_position() {
        let output = check_source("type Person(id: number)\ntype Rob#t(id: number)\n");

        assert!(output.result.is_err());

        // the `#` is the ninth character of the second line
        assert!(
            output
                .stderr
                .contains("[2:9] error: Unexpected character '#'")
        );
    }

    /// Diagnostics belong on stderr so the tree stays pipeable, and a failed
    /// parse should not emit a partial tree at all.
    #[test]
    fn a_failed_parse_writes_nothing_to_stdout() {
        let output = check_source("type Person(name string)\n");

        assert!(output.result.is_err());
        assert_eq!(output.stdout, "");
    }

    #[test]
    fn a_missing_file_fails_without_mentioning_syntax() {
        let dir = tempdir().unwrap();
        let output = check_file(&dir.path().join("nope.datom"));

        let error = output.result.expect_err("a missing file should fail");
        assert!(error.to_string().contains("could not read"));
        assert_eq!(output.stderr, "");
    }
}
