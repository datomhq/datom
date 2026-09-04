use std::{cell::RefCell, fmt::Display, range::Range};

/// The severity level for the diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Warning,
    Error,
}

impl Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let repr = match self {
            Self::Warning => "warning",
            Self::Error => "error",
        };

        write!(f, "{}", repr)
    }
}

/// A single diagnostic message pointing to an issue in the source code.
#[derive(Debug, Clone)]
pub(crate) struct Diagnostic {
    severity: Severity,
    message: String,
    range: Range<usize>,
}

/// A diagnostics sink for accumulating diagnostics during compilation.
#[derive(Debug, Default, Clone)]
pub(crate) struct Diagnostics {
    diagnostics: RefCell<Vec<Diagnostic>>,
}

impl Diagnostics {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn warn(&self, message: impl Into<String>, start: usize, end: usize) {
        self.track(Severity::Warning, message.into(), Range { start, end });
    }

    pub(crate) fn error(&self, message: impl Into<String>, start: usize, end: usize) {
        self.track(Severity::Error, message.into(), Range { start, end });
    }

    /// Returns true if no diagnostics have been emitted.
    pub(crate) fn is_ok(&self) -> bool {
        self.diagnostics.borrow().is_empty()
    }

    /// Render the list of diagnostics as user-visible messages pointing to specific line:column coordinates
    /// within the given source string.
    pub(crate) fn render(&self, source: &str) -> String {
        let mut out = String::new();

        for diagnostic in self.diagnostics.borrow().iter() {
            let Location { line, col } = line_and_col(source, diagnostic.range);

            out.push_str(
                format!(
                    "[{line}:{col}] {}: {}\n",
                    diagnostic.severity, diagnostic.message
                )
                .as_str(),
            );
        }

        out
    }

    fn track(&self, severity: Severity, message: String, range: Range<usize>) {
        self.diagnostics.borrow_mut().push(Diagnostic {
            severity,
            message,
            range,
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Location {
    line: usize,
    col: usize,
}

/// Returns the 1-based line and column of the start of the specified range in source.
fn line_and_col(source: &str, range: Range<usize>) -> Location {
    let mut line = 1;
    let mut col = 1;

    for (i, char) in source.chars().enumerate() {
        if i == range.start {
            break;
        }

        if char == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }

    Location { line, col }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_character_sits_at_one_one() {
        let diagnostics = Diagnostics::new();
        diagnostics.error("bad start", 0, 1);
        assert_eq!(diagnostics.render("#"), "[1:1] error: bad start\n");
    }

    #[test]
    fn a_later_line_counts_columns_from_its_own_start() {
        let source = "type A(id: number)\ntype B(#: bool)";
        let diagnostics = Diagnostics::new();
        diagnostics.error("bad field", 26, 27);

        assert_eq!(diagnostics.render(source), "[2:8] error: bad field\n");
    }
}
