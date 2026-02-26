/// Pre-computed index for converting byte offsets to line/column positions.
pub struct LineIndex {
    /// Byte offset of the start of each line (0-indexed).
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(source: &[u8]) -> Self {
        let mut line_starts = vec![0];
        for (i, &byte) in source.iter().enumerate() {
            if byte == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts }
    }

    /// Returns 0-based (line, column) for the given byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let col = offset - self.line_starts[line];
        (line, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line() {
        let idx = LineIndex::new(b"hello");
        assert_eq!(idx.line_col(0), (0, 0));
        assert_eq!(idx.line_col(4), (0, 4));
    }

    #[test]
    fn multiple_lines() {
        let idx = LineIndex::new(b"foo\nbar\nbaz");
        assert_eq!(idx.line_col(0), (0, 0)); // 'f'
        assert_eq!(idx.line_col(3), (0, 3)); // '\n'
        assert_eq!(idx.line_col(4), (1, 0)); // 'b' in "bar"
        assert_eq!(idx.line_col(7), (1, 3)); // '\n'
        assert_eq!(idx.line_col(8), (2, 0)); // 'b' in "baz"
        assert_eq!(idx.line_col(10), (2, 2)); // 'z'
    }

    #[test]
    fn empty_lines() {
        let idx = LineIndex::new(b"a\n\nb");
        assert_eq!(idx.line_col(0), (0, 0)); // 'a'
        assert_eq!(idx.line_col(2), (1, 0)); // empty line's '\n'
        assert_eq!(idx.line_col(3), (2, 0)); // 'b'
    }

    #[test]
    fn multibyte_utf8() {
        // "あ" is 3 bytes (0xE3 0x81 0x82)
        let src = "あ\nい".as_bytes();
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_col(0), (0, 0)); // start of "あ"
        assert_eq!(idx.line_col(3), (0, 3)); // '\n'
        assert_eq!(idx.line_col(4), (1, 0)); // start of "い"
    }
}
