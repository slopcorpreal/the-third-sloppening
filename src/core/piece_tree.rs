use std::ops::Range;

use super::mmap_buffer::MmapBuffer;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BufferKind {
    Original,
    Add,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Piece {
    buffer: BufferKind,
    start: usize,
    len: usize,
}

pub struct PieceTree {
    original: MmapBuffer,
    add: Vec<u8>,
    pieces: Vec<Piece>,
    len: usize,
}

impl PieceTree {
    pub fn from_original(original: MmapBuffer) -> Self {
        let len = original.len();
        let pieces = if len == 0 {
            Vec::new()
        } else {
            vec![Piece {
                buffer: BufferKind::Original,
                start: 0,
                len,
            }]
        };

        Self {
            original,
            add: Vec::new(),
            pieces,
            len,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn insert(&mut self, offset: usize, bytes: &[u8]) {
        assert!(offset <= self.len, "offset out of bounds");
        if bytes.is_empty() {
            return;
        }

        let add_start = self.add.len();
        self.add.extend_from_slice(bytes);
        let inserted = Piece {
            buffer: BufferKind::Add,
            start: add_start,
            len: bytes.len(),
        };

        let mut output = Vec::with_capacity(self.pieces.len() + 2);
        let mut cursor = 0;
        let mut inserted_done = false;

        for piece in &self.pieces {
            let next = cursor + piece.len;

            if !inserted_done && offset <= next {
                let split = offset.saturating_sub(cursor);
                if split > 0 {
                    output.push(Piece {
                        len: split,
                        ..*piece
                    });
                }

                output.push(inserted);
                inserted_done = true;

                if split < piece.len {
                    output.push(Piece {
                        start: piece.start + split,
                        len: piece.len - split,
                        ..*piece
                    });
                }
            } else {
                output.push(*piece);
            }

            cursor = next;
        }

        if !inserted_done {
            output.push(inserted);
        }

        self.pieces = output;
        self.len += bytes.len();
    }

    pub fn delete(&mut self, range: Range<usize>) {
        assert!(range.start <= range.end, "invalid range");
        assert!(range.end <= self.len, "range out of bounds");
        if range.is_empty() {
            return;
        }

        let mut output = Vec::with_capacity(self.pieces.len());
        let mut cursor = 0;

        for piece in &self.pieces {
            let next = cursor + piece.len;

            if next <= range.start || cursor >= range.end {
                output.push(*piece);
            } else {
                if range.start > cursor {
                    output.push(Piece {
                        len: range.start - cursor,
                        ..*piece
                    });
                }

                if range.end < next {
                    output.push(Piece {
                        start: piece.start + (range.end - cursor),
                        len: next - range.end,
                        ..*piece
                    });
                }
            }

            cursor = next;
        }

        self.pieces = output;
        self.len -= range.end - range.start;
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len);
        for piece in &self.pieces {
            out.extend_from_slice(self.resolve_piece(*piece));
        }
        out
    }

    pub fn visible_text(&self, start: usize, end: usize) -> Vec<u8> {
        assert!(start <= end, "invalid visible range");
        assert!(end <= self.len, "visible range out of bounds");

        let mut out = Vec::with_capacity(end - start);
        let mut cursor = 0;

        for piece in &self.pieces {
            let next = cursor + piece.len;
            if next <= start || cursor >= end {
                cursor = next;
                continue;
            }

            let local_start = start.saturating_sub(cursor);
            let local_end = (end.min(next)) - cursor;
            let source = self.resolve_piece(*piece);
            out.extend_from_slice(&source[local_start..local_end]);

            cursor = next;
        }

        out
    }

    fn resolve_piece(&self, piece: Piece) -> &[u8] {
        match piece.buffer {
            BufferKind::Original => &self.original.as_slice()[piece.start..piece.start + piece.len],
            BufferKind::Add => &self.add[piece.start..piece.start + piece.len],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_delete_works() {
        let mut tree = PieceTree::from_original(MmapBuffer::from_bytes(b"abc\ndef".to_vec()));
        tree.insert(3, b"XYZ");
        assert_eq!(tree.to_bytes(), b"abcXYZ\ndef");

        tree.delete(2..6);
        assert_eq!(tree.to_bytes(), b"ab\ndef");
    }

    #[test]
    fn visible_text_reads_window_only() {
        let mut tree =
            PieceTree::from_original(MmapBuffer::from_bytes(b"line1\nline2\nline3".to_vec()));
        tree.insert(6, b"NEW\n");

        assert_eq!(tree.visible_text(6, 10), b"NEW\n");
    }
}
