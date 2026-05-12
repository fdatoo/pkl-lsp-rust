//! Byte-offset spans over source text.

use std::fmt;
use std::ops::Range;

/// A half-open byte range `[start, end)` into a source file.
///
/// Offsets are byte offsets into the original UTF-8 source. Conversion to
/// line/column positions is performed by callers (the LSP layer) that hold the
/// rope buffer for the document.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const EMPTY: Span = Span { start: 0, end: 0 };

    #[inline]
    pub const fn new(start: u32, end: u32) -> Self {
        debug_assert!(start <= end, "Span::new: start must be <= end");
        Span { start, end }
    }

    #[inline]
    pub fn from_range(range: Range<usize>) -> Self {
        Span {
            start: range.start as u32,
            end: range.end as u32,
        }
    }

    #[inline]
    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    #[inline]
    pub fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    /// True if `offset` lies within the span or at either endpoint (useful for
    /// cursor-position queries where the caret sits between two characters).
    #[inline]
    pub fn touches(&self, offset: u32) -> bool {
        self.start <= offset && offset <= self.end
    }

    #[inline]
    pub fn join(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    #[inline]
    pub fn as_range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}
