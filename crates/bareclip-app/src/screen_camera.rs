//! Draws the screen with the real renderer into pictures, with no window, for tests to save and
//! for people to look at.

use std::{path::PathBuf, sync::Arc, thread, time::Duration};

use eframe::{
    egui::{self, pos2, vec2},
    egui_wgpu, wgpu,
};

thread_local! {
    /// Texture changes saved for `Camera`, which draws frames some time later.
    pub static KEPT_TEXTURES: std::cell::RefCell<Option<Vec<egui::TexturesDelta>>> =
        const { std::cell::RefCell::new(None) };
}

/// Draws the screen with the real renderer into a picture, with no window involved.
pub struct Camera {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    folder: PathBuf,
}

pub const CAMERA_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
thread_local! {
    /// The window's size in tests, which a test can change for its own thread.
    pub static SCREEN: std::cell::Cell<[u32; 2]> = const { std::cell::Cell::new([1280, 800]) };
}

pub fn screen() -> egui::Rect {
    let [width, height] = SCREEN.get();
    egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(width as f32, height as f32))
}

pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

impl Camera {
    pub fn new(folder: PathBuf) -> Self {
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let instance = wgpu::Instance::default();
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .expect("a GPU is available");
        let (device, queue) =
            block_on(adapter.request_device(&Default::default())).expect("the GPU can be opened");
        let renderer = Arc::new(egui::mutex::RwLock::new(egui_wgpu::Renderer::new(
            &device,
            CAMERA_FORMAT,
            Default::default(),
        )));
        KEPT_TEXTURES.with_borrow_mut(|kept| *kept = Some(Vec::new()));
        Self {
            device,
            queue,
            renderer,
            folder,
        }
    }

    /// Draws a frame that has already run, wherever it came from, and saves it.
    pub fn save(&mut self, ctx: &egui::Context, output: egui::FullOutput, name: &str) {
        let mut textures = KEPT_TEXTURES
            .with_borrow_mut(|kept| std::mem::take(kept.as_mut().expect("the camera is on")));
        for delta in &textures {
            for (id, images) in &delta.set {
                for image in images {
                    self.renderer
                        .write()
                        .update_texture(&self.device, &self.queue, *id, image);
                }
            }
        }
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: SCREEN.get(),
            pixels_per_point: output.pixels_per_point,
        };
        let size = wgpu::Extent3d {
            width: SCREEN.get()[0],
            height: SCREEN.get()[1],
            depth_or_array_layers: 1,
        };
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CAMERA_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let row_bytes = (SCREEN.get()[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row_bytes * SCREEN.get()[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self.device.create_command_encoder(&Default::default());
        let extra = self.renderer.write().update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.read().render(&mut pass, &jobs, &screen);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: None,
                },
            },
            size,
        );
        self.queue
            .submit(extra.into_iter().chain([encoder.finish()]));
        readback.map_async(wgpu::MapMode::Read, .., |result| {
            result.expect("the picture can be read back");
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the GPU finishes");

        let data = readback
            .get_mapped_range(..)
            .expect("the picture was mapped");
        let mut picture = format!("P6 {} {} 255\n", SCREEN.get()[0], SCREEN.get()[1]).into_bytes();
        for row in data.chunks(row_bytes as usize) {
            for pixel in row[..SCREEN.get()[0] as usize * 4].chunks(4) {
                picture.extend_from_slice(&pixel[..3]);
            }
        }
        std::fs::write(self.folder.join(format!("{name}.ppm")), picture)
            .expect("the picture can be saved");
        drop(data);
        for delta in &mut textures {
            for id in &delta.free {
                self.renderer.write().free_texture(id);
            }
            delta.clear();
        }
    }
}

/// Runs one frame of anything that draws into the root `Ui`, at the test screen's size, keeping
/// its texture changes for the camera.
pub fn run(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    show: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(screen()),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, show);
    let mut textures = std::mem::take(&mut output.textures_delta);
    KEPT_TEXTURES.with_borrow_mut(|kept| match kept {
        Some(kept) => kept.push(textures),
        None => textures.clear(),
    });
    output
}

/// What screen readers are told is on screen in a frame: each control's name, where it is, and
/// whether it is greyed out. The frame must have run with accesskit on.
pub fn named_nodes(output: &egui::FullOutput) -> Vec<(String, egui::Rect, bool)> {
    let update = output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("the tree is reported");
    update
        .nodes
        .iter()
        .filter_map(|(_, node)| {
            let bounds = node.bounds()?;
            let rect = egui::Rect::from_min_max(
                pos2(bounds.x0 as f32, bounds.y0 as f32),
                pos2(bounds.x1 as f32, bounds.y1 as f32),
            );
            Some((node.label()?.to_owned(), rect, node.is_disabled()))
        })
        .collect()
}
