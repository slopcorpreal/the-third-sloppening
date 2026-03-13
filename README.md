# the-third-sloppening

Native Rust prototype for a high-performance Windows-focused text editor.

## Current baseline

- Native desktop window path with `winit` + `wgpu` (no webview)
- `mimalloc` global allocator
- `memmap2`-backed immutable source buffer
- Piece-table style mutable text model (`original` + append-only `add` buffer)
- Parallel line index builder using `rayon` + SIMD-optimized `memchr`
- SIMD UTF-8 validation via `simdutf8`
- `cosmic-text` viewport text layout baseline

## Run

```bash
cargo run
cargo run -- path/to/large-file.txt
```
