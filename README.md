# the-third-sloppening

Native Rust prototype for a high-performance Windows-focused text editor.

## Current baseline

- Native desktop editor window path with `winit` + `wgpu` (no webview)
- `mimalloc` global allocator
- `memmap2`-backed immutable source buffer
- Piece-table style mutable text model (`original` + append-only `add` buffer)
- Parallel line index builder using `rayon` + SIMD-optimized `memchr`
- SIMD UTF-8 validation via `simdutf8`
- `glyphon`/`cosmic-text` viewport text rendering with keyboard typing, deletion, and scrolling

## Run

```bash
cargo run
cargo run -- path/to/large-file.txt

# controls
# - Type to edit
# - Backspace/Delete, Enter, Tab, Home, End
# - Arrow Up/Down, PgUp/PgDn, or mouse wheel to scroll the viewport
# - Left click to place the insertion point
# - Ctrl+S (or Cmd+S) saves when opened with a file path
```
