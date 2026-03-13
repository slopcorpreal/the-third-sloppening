use memchr::memchr_iter;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn build(bytes: &[u8], chunk_size: usize) -> Self {
        let chunk_size = chunk_size.max(1);

        let mut line_starts = vec![0usize];
        let mut chunks: Vec<Vec<usize>> = bytes
            .par_chunks(chunk_size)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let base = chunk_idx * chunk_size;
                memchr_iter(b'\n', chunk)
                    .map(|idx| base + idx + 1)
                    .collect::<Vec<_>>()
            })
            .collect();

        for entries in chunks.iter_mut() {
            line_starts.append(entries);
        }

        line_starts.sort_unstable();
        line_starts.dedup();

        Self { line_starts }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line).copied()
    }

    pub fn line_of_offset(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|start| *start <= offset) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_is_correct_across_chunks() {
        let text = b"a\nb\nc\nd\n";
        let index = LineIndex::build(text, 2);

        assert_eq!(index.line_count(), 5);
        assert_eq!(index.line_start(3), Some(6));
        assert_eq!(index.line_of_offset(0), 0);
        assert_eq!(index.line_of_offset(3), 1);
    }
}
