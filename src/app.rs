use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use glyphon::{
    Attrs, Buffer, Color, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::SurfaceError;
use winit::{
    dpi::PhysicalSize,
    event::{ElementState, Event, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey},
    window::WindowBuilder,
};

use crate::core::{mmap_buffer::MmapBuffer, utf8::validate_utf8};

const FONT_SIZE: f32 = 16.0;
const LINE_HEIGHT: f32 = 22.0;
const VIEWPORT_PADDING: f32 = 24.0;
const TEXT_INSET: f32 = VIEWPORT_PADDING / 2.0;
// Approximate column hit-testing for proportional fonts until glyph-position mapping is added.
const CHAR_WIDTH_RATIO: f32 = 0.6;
const PIXELS_PER_SCROLL_LINE: f64 = 24.0;

pub fn run() -> Result<()> {
    let file_path = env::args().nth(1).map(PathBuf::from);
    let initial_text = if let Some(path) = file_path.as_deref() {
        let buffer = MmapBuffer::open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        validate_utf8(buffer.as_slice())
            .unwrap_or("[file unavailable: invalid UTF-8]")
            .to_owned()
    } else {
        "the-third-sloppening\n\nType to edit. Use Backspace/Delete, Arrow Up/Down, Page Up/Down, or mouse wheel to scroll."
            .to_owned()
    };

    let event_loop = EventLoop::new()?;
    let window = WindowBuilder::new()
        .with_title("the-third-sloppening")
        .with_inner_size(PhysicalSize::new(1280, 720))
        .build(&event_loop)
        .context("failed to create window")?;

    let mut renderer = pollster::block_on(GpuRenderer::new(&window, &initial_text, file_path))?;

    event_loop.run(|event, target| {
        target.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(new_size) => renderer.resize(new_size),
                WindowEvent::Ime(Ime::Commit(text)) => {
                    renderer.insert_text(&text);
                    window.request_redraw();
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    renderer.set_modifiers(modifiers.state());
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    if !renderer.handle_save_shortcut(&event.logical_key) {
                        match &event.logical_key {
                            Key::Named(NamedKey::Backspace) => renderer.backspace(),
                            Key::Named(NamedKey::Delete) => renderer.delete_forward(),
                            Key::Named(NamedKey::Enter) => renderer.insert_text("\n"),
                            Key::Named(NamedKey::Tab) => renderer.insert_text("\t"),
                            Key::Named(NamedKey::Home) => renderer.move_to_line_start(),
                            Key::Named(NamedKey::End) => renderer.move_to_line_end(),
                            Key::Named(NamedKey::ArrowLeft) => renderer.move_left(),
                            Key::Named(NamedKey::ArrowRight) => renderer.move_right(),
                            Key::Named(NamedKey::ArrowUp) => renderer.scroll_lines(-1),
                            Key::Named(NamedKey::ArrowDown) => renderer.scroll_lines(1),
                            Key::Named(NamedKey::PageUp) => renderer.scroll_lines(-20),
                            Key::Named(NamedKey::PageDown) => renderer.scroll_lines(20),
                            _ => {}
                        }
                    }
                    window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    renderer.update_pointer(position.x as f32, position.y as f32);
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    renderer.click_primary();
                    window.request_redraw();
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
                        MouseScrollDelta::PixelDelta(pos) => {
                            (pos.y / PIXELS_PER_SCROLL_LINE).round() as i32
                        }
                    };
                    renderer.scroll_lines(-lines);
                    window.request_redraw();
                }
                WindowEvent::RedrawRequested => {
                    if renderer.render().is_err() {
                        target.exit();
                    }
                }
                _ => {}
            },
            Event::AboutToWait => window.request_redraw(),
            _ => {}
        }
    })?;

    Ok(())
}

#[derive(Debug, Clone)]
struct EditorState {
    text: String,
    cursor: usize,
    scroll_line: usize,
    total_lines: usize,
}

impl EditorState {
    fn new(text: String) -> Self {
        let cursor = text.len();
        let total_lines = count_lines(&text);
        Self {
            text,
            cursor,
            scroll_line: 0,
            total_lines,
        }
    }

    fn insert_text(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
        self.total_lines = count_lines(&self.text);
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.prev_boundary(self.cursor);
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
        self.total_lines = count_lines(&self.text);
    }

    fn delete_forward(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let next = self.next_boundary(self.cursor);
        self.text.drain(self.cursor..next);
        self.total_lines = count_lines(&self.text);
    }

    fn move_left(&mut self) {
        self.cursor = self.prev_boundary(self.cursor);
    }

    fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor = self.next_boundary(self.cursor);
        }
    }

    fn move_to_line_start(&mut self) {
        let line_start = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |idx| idx + 1);
        self.cursor = line_start;
    }

    fn move_to_line_end(&mut self) {
        let line_end = self.text[self.cursor..]
            .find('\n')
            .map(|offset| self.cursor + offset)
            .unwrap_or(self.text.len());
        self.cursor = line_end;
    }

    fn scroll_lines(&mut self, delta: i32) {
        let max_scroll = self.total_lines.saturating_sub(1);
        let next = (self.scroll_line as i64 + delta as i64).clamp(0, max_scroll as i64);
        self.scroll_line = next as usize;
    }

    fn visible_text(&self, viewport_lines: usize) -> String {
        self.text
            .split('\n')
            .skip(self.scroll_line)
            .take(viewport_lines.max(1))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn set_cursor_from_view_position(
        &mut self,
        x: f32,
        y: f32,
        line_height: f32,
        top: f32,
        left: f32,
    ) {
        let lines: Vec<&str> = self.text.split('\n').collect();
        let view_line = ((y - top).max(0.0) / line_height).floor() as usize;
        let line_idx = (self.scroll_line + view_line).min(lines.len() - 1);

        let approx_col = ((x - left).max(0.0) / (FONT_SIZE * CHAR_WIDTH_RATIO)).floor() as usize;
        let line = lines[line_idx];
        let col_byte = line
            .char_indices()
            .nth(approx_col)
            .map(|(idx, _)| idx)
            .unwrap_or(line.len());

        let line_start = lines
            .iter()
            .take(line_idx)
            .fold(0usize, |acc, part| acc + part.len() + 1);
        self.cursor = line_start + col_byte;
    }

    fn prev_boundary(&self, from: usize) -> usize {
        if from == 0 {
            return 0;
        }
        self.text[..from]
            .char_indices()
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    fn next_boundary(&self, from: usize) -> usize {
        self.text[from..]
            .chars()
            .next()
            .map(|ch| from + ch.len_utf8())
            .unwrap_or(from)
    }
}

fn count_lines(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn save_text_to_path(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("failed to save {}", path.display()))
}

struct GpuRenderer<'w> {
    surface: wgpu::Surface<'w>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    editor: EditorState,
    font_system: FontSystem,
    text_buffer: Buffer,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    pointer: (f32, f32),
    file_path: Option<PathBuf>,
    modifiers: ModifiersState,
}

impl<'w> GpuRenderer<'w> {
    async fn new(
        window: &'w winit::window::Window,
        initial_text: &str,
        file_path: Option<PathBuf>,
    ) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window)?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("failed to create adapter")?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    // Prioritize low-latency interaction over memory minimization for responsive editing.
                    memory_hints: wgpu::MemoryHints::Performance,
                    label: Some("the-third-sloppening-device"),
                },
                None,
            )
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&device, &config);

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = glyphon::Cache::new(&device);
        let mut viewport = Viewport::new(&device, &cache);
        viewport.update(
            &queue,
            Resolution {
                width: config.width,
                height: config.height,
            },
        );

        let mut atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        let mut text_buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        text_buffer.set_size(
            &mut font_system,
            Some(config.width as f32 - VIEWPORT_PADDING),
            Some(config.height as f32 - VIEWPORT_PADDING),
        );

        let mut renderer = Self {
            surface,
            device,
            queue,
            config,
            size,
            editor: EditorState::new(initial_text.to_owned()),
            font_system,
            text_buffer,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            pointer: (TEXT_INSET, TEXT_INSET),
            file_path,
            modifiers: ModifiersState::empty(),
        };
        renderer.refresh_text();
        renderer.prepare_text()?;

        Ok(renderer)
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }

        self.size = new_size;
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        self.text_buffer.set_size(
            &mut self.font_system,
            Some(self.config.width as f32 - VIEWPORT_PADDING),
            Some(self.config.height as f32 - VIEWPORT_PADDING),
        );
        self.prepare_text_logged();
    }

    fn insert_text(&mut self, value: &str) {
        self.editor.insert_text(value);
        self.refresh_text();
        self.prepare_text_logged();
    }

    fn backspace(&mut self) {
        self.editor.backspace();
        self.refresh_text();
        self.prepare_text_logged();
    }

    fn delete_forward(&mut self) {
        self.editor.delete_forward();
        self.refresh_text();
        self.prepare_text_logged();
    }

    fn move_left(&mut self) {
        self.editor.move_left();
    }

    fn move_right(&mut self) {
        self.editor.move_right();
    }

    fn move_to_line_start(&mut self) {
        self.editor.move_to_line_start();
    }

    fn move_to_line_end(&mut self) {
        self.editor.move_to_line_end();
    }

    fn scroll_lines(&mut self, lines: i32) {
        self.editor.scroll_lines(lines);
        self.refresh_text();
        self.prepare_text_logged();
    }

    fn update_pointer(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
    }

    fn click_primary(&mut self) {
        self.editor.set_cursor_from_view_position(
            self.pointer.0,
            self.pointer.1,
            LINE_HEIGHT,
            TEXT_INSET,
            TEXT_INSET,
        );
    }

    fn set_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
    }

    fn handle_save_shortcut(&mut self, logical_key: &Key) -> bool {
        let control_or_super = self.modifiers.control_key() || self.modifiers.super_key();
        if !control_or_super {
            return false;
        }
        match logical_key {
            Key::Character(ch) if ch.eq_ignore_ascii_case("s") => {
                if let Some(path) = self.file_path.as_deref() {
                    if let Err(error) = save_text_to_path(path, &self.editor.text) {
                        eprintln!("{error:#}");
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn refresh_text(&mut self) {
        let viewport_lines =
            ((self.config.height as f32 - VIEWPORT_PADDING) / LINE_HEIGHT).max(1.0) as usize;
        let mut visible = self.editor.visible_text(viewport_lines);
        if visible.is_empty() {
            // Prevent glyphon prep/render issues on empty input by keeping one drawable codepoint.
            visible.push(' ');
        }
        self.text_buffer.set_text(
            &mut self.font_system,
            &visible,
            Attrs::new(),
            Shaping::Advanced,
        );
    }

    fn prepare_text_logged(&mut self) {
        if let Err(error) = self.prepare_text() {
            eprintln!("text preparation failed: {error:#}");
        }
    }

    fn prepare_text(&mut self) -> Result<()> {
        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                [TextArea {
                    buffer: &self.text_buffer,
                    left: TEXT_INSET,
                    top: TEXT_INSET,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: TEXT_INSET as i32,
                        top: TEXT_INSET as i32,
                        right: self.config.width as i32 - TEXT_INSET as i32,
                        bottom: self.config.height as i32 - TEXT_INSET as i32,
                    },
                    default_color: Color::rgb(230, 236, 244),
                    custom_glyphs: &[],
                }],
                &mut self.swash_cache,
            )
            .context("failed to prepare text rendering")?;

        Ok(())
    }

    fn render(&mut self) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(SurfaceError::Outdated | SurfaceError::Lost) => {
                self.resize(self.size);
                return Ok(());
            }
            Err(SurfaceError::OutOfMemory) => {
                anyhow::bail!("out of memory from wgpu surface")
            }
            Err(SurfaceError::Timeout) => {
                return Ok(());
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("the-third-sloppening-render"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("editor-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.12,
                            g: 0.14,
                            b: 0.18,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });

            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .context("failed to render text")?;
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{EditorState, LINE_HEIGHT, save_text_to_path};

    #[test]
    fn editor_state_edits_and_scrolls() {
        let mut state = EditorState::new("a\nb\nc".to_owned());
        state.insert_text("\nd");
        state.backspace();
        state.move_left();
        state.delete_forward();
        state.scroll_lines(2);

        assert!(state.text.contains("a"));
        assert_eq!(state.scroll_line, 2);
        assert_eq!(state.visible_text(1), "c");
    }

    #[test]
    fn editor_state_handles_utf8_boundaries() {
        let mut state = EditorState::new("h🌍".to_owned());
        state.move_left();
        state.backspace();
        assert_eq!(state.text, "🌍");
        state.delete_forward();
        assert_eq!(state.text, "");
    }

    #[test]
    fn editor_state_preserves_trailing_empty_line_for_viewport() {
        let mut state = EditorState::new("a\n".to_owned());
        state.scroll_lines(1);
        assert_eq!(state.total_lines, 2);
        assert_eq!(state.visible_text(1), "");
        state.insert_text("b");
        assert_eq!(state.text, "a\nb");
    }

    #[test]
    fn editor_state_moves_cursor_from_mouse_position() {
        let mut state = EditorState::new("abc\ndef".to_owned());
        state.set_cursor_from_view_position(24.0, 35.0, LINE_HEIGHT, 12.0, 12.0);
        state.insert_text("X");
        assert_eq!(state.text, "abc\ndXef");
    }

    #[test]
    fn save_text_to_path_writes_expected_contents() {
        let path = std::env::temp_dir().join(format!(
            "the-third-sloppening-save-{}-{}.txt",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after unix epoch")
                .as_nanos()
        ));
        save_text_to_path(&path, "hello\nworld").expect("save should succeed");
        let saved = std::fs::read_to_string(&path).expect("saved file should be readable");
        assert_eq!(saved, "hello\nworld");
        let _ = std::fs::remove_file(path);
    }
}
