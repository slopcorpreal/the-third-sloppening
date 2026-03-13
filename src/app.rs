use std::env;

use anyhow::{Context, Result};
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};
use wgpu::SurfaceError;
use winit::{
    dpi::PhysicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use crate::core::{line_index::LineIndex, mmap_buffer::MmapBuffer, piece_tree::PieceTree, utf8::validate_utf8};

pub fn run() -> Result<()> {
    let mut viewport = ViewportText::new();
    if let Some(path) = env::args().nth(1) {
        let buffer = MmapBuffer::open(&path).with_context(|| format!("failed to open {path}"))?;
        let tree = PieceTree::from_original(buffer);
        let index = LineIndex::build(tree.to_bytes().as_slice(), 8 * 1024 * 1024);

        let preview_end = tree.len().min(16 * 1024);
        let preview = tree.visible_text(0, preview_end);
        let text = validate_utf8(&preview).unwrap_or("");

        let summary = format!("{path} ({}) lines indexed\n{text}", index.line_count());
        viewport.set_text(&summary);
    } else {
        viewport.set_text("the-third-sloppening: mmap + piece tree + SIMD + rayon + wgpu baseline");
    }

    let event_loop = EventLoop::new()?;
    let window = WindowBuilder::new()
        .with_title("the-third-sloppening")
        .with_inner_size(PhysicalSize::new(1280, 720))
        .build(&event_loop)
        .context("failed to create window")?;

    let mut renderer = pollster::block_on(GpuRenderer::new(&window))?;

    event_loop.run(|event, target| {
        target.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(new_size) => renderer.resize(new_size),
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

struct ViewportText {
    font_system: FontSystem,
    buffer: Buffer,
}

impl ViewportText {
    fn new() -> Self {
        let mut font_system = FontSystem::new();
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(16.0, 22.0));
        buffer.set_size(&mut font_system, Some(1200.0), Some(800.0));

        Self {
            font_system,
            buffer,
        }
    }

    fn set_text(&mut self, text: &str) {
        self.buffer
            .set_text(&mut self.font_system, text, Attrs::new(), Shaping::Advanced);
    }
}

struct GpuRenderer<'w> {
    surface: wgpu::Surface<'w>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
}

impl<'w> GpuRenderer<'w> {
    async fn new(window: &'w winit::window::Window) -> Result<Self> {
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

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
        })
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }

        self.size = new_size;
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
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
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.06,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();

        Ok(())
    }
}
