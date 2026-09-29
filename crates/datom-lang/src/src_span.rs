use std::range::Range;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub(crate) struct SrcSpan {
    location: Range<usize>,
}

impl SrcSpan {
    pub fn new(start: usize, end: usize) -> Self {
        (start..end).into()
    }

    pub fn lexeme<'src>(&self, source: &'src str) -> &'src str {
        &source[self.location]
    }
}

impl From<Range<usize>> for SrcSpan {
    fn from(value: Range<usize>) -> Self {
        Self { location: value }
    }
}

impl From<std::ops::Range<usize>> for SrcSpan {
    fn from(value: std::ops::Range<usize>) -> Self {
        Self {
            location: Range::from(value),
        }
    }
}
