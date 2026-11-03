//! Draws layers on the GPU and reads the finished picture back for encoding.

use std::{
    borrow::Cow,
    fmt,
    sync::{Arc, Mutex, OnceLock},
};

use bareclip_core::picture::{ColorFormat, Yuv420};

use crate::{color, scene::Layer};

/// Wide enough that adding and removing see-through layers doesn't build up rounding errors.
const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// What the window's renderer takes for a picture it didn't make itself.
const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const LAYER_UNIFORM_SIZE: u64 = 112;
const BLACK: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    NoGpu(String),
    TooLarge { width: u32, height: u32, limit: u32 },
    Gpu(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoGpu(reason) => write!(f, "No usable graphics chip was found. {reason}"),
            Self::TooLarge {
                width,
                height,
                limit,
            } => write!(
                f,
                "{width} x {height} is larger than this graphics chip can draw, which is {limit} pixels across."
            ),
            Self::Gpu(reason) => write!(f, "The graphics chip reported a problem. {reason}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// A picture's planes on the GPU, kept to be filled again by the next frame of its size.
struct LayerSlot {
    size: (u32, u32),
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// A plane of the finished picture, on its way back from the GPU.
struct OutputPlane {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    row_bytes: u32,
}

impl OutputPlane {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        pixel_bytes: u32,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output plane"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let row_bytes = (size.0 * pixel_bytes).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output readback"),
            size: u64::from(row_bytes) * u64::from(size.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = texture.create_view(&Default::default());
        Self {
            texture,
            view,
            readback,
            row_bytes,
        }
    }
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    gpu_name: String,
    size: (u32, u32),
    layer_pipeline: wgpu::RenderPipeline,
    layer_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    slots: Vec<LayerSlot>,
    canvas_view: wgpu::TextureView,
    brightness_pipeline: wgpu::RenderPipeline,
    colour_pipeline: wgpu::RenderPipeline,
    display_pipeline: wgpu::RenderPipeline,
    /// What the canvas starts as, wherever no layer covers it.
    background: wgpu::Color,
    output_bind_group: wgpu::BindGroup,
    /// The finished canvas as the window shows it. Unused by exports.
    display: OutputPlane,
    brightness: OutputPlane,
    colour: OutputPlane,
    finished_y: Vec<u8>,
    finished_u: Vec<u8>,
    finished_v: Vec<u8>,
    problem: Arc<Mutex<Option<String>>>,
}

/// The graphics chip, opened for this run of the app.
#[derive(Clone)]
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    name: String,
    /// The last thing the GPU complained about, waiting to be reported.
    problem: Arc<Mutex<Option<String>>>,
}

fn open_gpu() -> Result<Gpu, RenderError> {
    // Only DirectX 12. Asking for every graphics system on the machine loads drivers for all of
    // them, including old OpenGL ones that have deadlocked here while loading.
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::DX12;
    let instance = wgpu::Instance::new(descriptor);
    // The fast graphics chip by name, then whatever Windows offers in software, so a machine
    // without a usable chip exports slowly rather than not at all.
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .or_else(|_| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: true,
            ..Default::default()
        }))
    })
    .map_err(|error| RenderError::NoGpu(error.to_string()))?;
    let name = adapter.get_info().name;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("compositor"),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .map_err(|error| RenderError::NoGpu(error.to_string()))?;

    let problem = Arc::new(Mutex::new(None));
    let noticed = problem.clone();
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        if let Ok(mut problem) = noticed.lock() {
            *problem = Some(error.to_string());
        }
    }));
    Ok(Gpu {
        device,
        queue,
        name,
        problem,
    })
}

/// The GPU, opened the first time it is needed and then kept. Loading and unloading graphics
/// drivers is slow, and not something they cope well with happening alongside other work.
fn gpu() -> Result<Gpu, RenderError> {
    static GPU: OnceLock<Result<Gpu, RenderError>> = OnceLock::new();
    GPU.get_or_init(open_gpu).clone()
}

fn binding(
    index: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: index,
        visibility,
        ty,
        count: None,
    }
}

const TEXTURE_BINDING: wgpu::BindingType = wgpu::BindingType::Texture {
    sample_type: wgpu::TextureSampleType::Float { filterable: true },
    view_dimension: wgpu::TextureViewDimension::D2,
    multisampled: false,
};
const SAMPLER_BINDING: wgpu::BindingType =
    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);

/// A pipeline drawing one four-cornered strip to a target of this format.
fn strip_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    fragment: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Copies a plane into memory without the padding the GPU insists on.
fn copy_rows(mapped: &[u8], row_bytes: usize, used_bytes: usize, into: &mut Vec<u8>) {
    into.clear();
    for row in mapped.chunks_exact(row_bytes) {
        into.extend_from_slice(&row[..used_bytes]);
    }
}

impl Compositor {
    /// Opens the GPU, ready to draw pictures of one size.
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        Self::on_gpu(gpu()?, width, height)
    }

    /// Draws on a GPU that is already open, such as the window's. That way what is drawn can be
    /// shown without ever coming back through memory. Whoever opened it hears about its problems.
    pub fn on_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let gpu = Gpu {
            device,
            queue,
            name: "the window's GPU".to_owned(),
            problem: Arc::new(Mutex::new(None)),
        };
        Self::on_gpu(gpu, width, height)
    }

    fn on_gpu(gpu: Gpu, width: u32, height: u32) -> Result<Self, RenderError> {
        let Gpu {
            device,
            queue,
            name: gpu_name,
            problem,
        } = gpu;
        let limit = device.limits().max_texture_dimension_2d;
        if width > limit || height > limit {
            return Err(RenderError::TooLarge {
                width,
                height,
                limit,
            });
        }

        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &[
                binding(
                    0,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                binding(1, fragment, TEXTURE_BINDING),
                binding(2, fragment, TEXTURE_BINDING),
                binding(3, fragment, TEXTURE_BINDING),
                binding(4, fragment, SAMPLER_BINDING),
            ],
        });
        let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("output"),
            entries: &[
                binding(0, fragment, TEXTURE_BINDING),
                binding(1, fragment, SAMPLER_BINDING),
            ],
        });

        let layer_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("layer.wgsl"))),
        });
        let output_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("output"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("output.wgsl"))),
        });
        let layer_pipeline = strip_pipeline(
            &device,
            &layer_layout,
            &layer_shader,
            "fragment_main",
            CANVAS_FORMAT,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let brightness_pipeline = strip_pipeline(
            &device,
            &output_layout,
            &output_shader,
            "brightness_main",
            wgpu::TextureFormat::R8Unorm,
            None,
        );
        let colour_pipeline = strip_pipeline(
            &device,
            &output_layout,
            &output_shader,
            "colour_main",
            wgpu::TextureFormat::Rg8Unorm,
            None,
        );

        let display_pipeline = strip_pipeline(
            &device,
            &output_layout,
            &output_shader,
            "display_main",
            DISPLAY_FORMAT,
            None,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("smooth"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let canvas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let canvas_view = canvas.create_view(&Default::default());
        let output_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("output"),
            layout: &output_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&canvas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let display = OutputPlane::new(&device, DISPLAY_FORMAT, (width, height), 4);
        let brightness =
            OutputPlane::new(&device, wgpu::TextureFormat::R8Unorm, (width, height), 1);
        let colour = OutputPlane::new(
            &device,
            wgpu::TextureFormat::Rg8Unorm,
            (width.div_ceil(2), height.div_ceil(2)),
            2,
        );

        Ok(Self {
            device,
            queue,
            gpu_name,
            size: (width, height),
            layer_pipeline,
            layer_layout,
            sampler,
            slots: Vec::new(),
            canvas_view,
            brightness_pipeline,
            colour_pipeline,
            display_pipeline,
            background: BLACK,
            output_bind_group,
            display,
            brightness,
            colour,
            finished_y: Vec::new(),
            finished_u: Vec::new(),
            finished_v: Vec::new(),
            problem,
        })
    }

    /// Which graphics chip is doing the drawing.
    pub fn gpu_name(&self) -> &str {
        &self.gpu_name
    }

    fn new_slot(&self, size: (u32, u32)) -> LayerSlot {
        let chroma = (size.0.div_ceil(2), size.1.div_ceil(2));
        let plane = |plane_size: (u32, u32)| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("picture plane"),
                size: wgpu::Extent3d {
                    width: plane_size.0,
                    height: plane_size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let (y, u, v) = (plane(size), plane(chroma), plane(chroma));
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer"),
            size: LAYER_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let views = [&y, &u, &v].map(|texture| texture.create_view(&Default::default()));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer"),
            layout: &self.layer_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        LayerSlot {
            size,
            y,
            u,
            v,
            uniform,
            bind_group,
        }
    }

    fn upload(&mut self, index: usize, layer: &Layer) {
        let picture = &layer.picture;
        let size = (picture.width, picture.height);
        if index == self.slots.len() {
            let slot = self.new_slot(size);
            self.slots.push(slot);
        } else if self.slots[index].size != size {
            self.slots[index] = self.new_slot(size);
        }

        let slot = &self.slots[index];
        let chroma = picture.chroma_size();
        for (texture, data, stride, plane_size) in [
            (&slot.y, picture.y, picture.y_stride, size),
            (&slot.u, picture.u, picture.uv_stride, chroma),
            (&slot.v, picture.v, picture.uv_stride, chroma),
        ] {
            self.queue.write_texture(
                texture.as_image_copy(),
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride as u32),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: plane_size.0,
                    height: plane_size.1,
                    depth_or_array_layers: 1,
                },
            );
        }

        let (width, height) = (self.size.0 as f32, self.size.1 as f32);
        let rect = layer.rect;
        let rows = color::to_rgb(picture.color);
        let radians = layer.look.degrees.to_radians();
        // Cropping reads only part of the picture, and a flip reads it from the other side.
        let [left, top, right, bottom] = layer.look.source;
        let (from_x, to_x) = if layer.look.flip_x {
            (right, left)
        } else {
            (left, right)
        };
        let (from_y, to_y) = if layer.look.flip_y {
            (bottom, top)
        } else {
            (top, bottom)
        };
        // Edges only need softening where they don't line up with the pixels.
        let soft_edges = if layer.look.degrees % 90.0 == 0.0 {
            0.0
        } else {
            1.0
        };
        let uniform: [f32; 28] = [
            rect.x + rect.width / 2.0,
            rect.y + rect.height / 2.0,
            rect.width / 2.0,
            rect.height / 2.0,
            radians.cos(),
            radians.sin(),
            width,
            height,
            from_x,
            from_y,
            to_x,
            to_y,
            rows[0][0],
            rows[0][1],
            rows[0][2],
            rows[0][3],
            rows[1][0],
            rows[1][1],
            rows[1][2],
            rows[1][3],
            rows[2][0],
            rows[2][1],
            rows[2][2],
            rows[2][3],
            layer.opacity,
            soft_edges,
            0.0,
            0.0,
        ];
        self.queue
            .write_buffer(&slot.uniform, 0, bytemuck::cast_slice(&uniform));
    }

    fn check_for_problem(&self) -> Result<(), RenderError> {
        match self.problem.lock().ok().and_then(|mut found| found.take()) {
            Some(problem) => Err(RenderError::Gpu(problem)),
            None => Ok(()),
        }
    }

    /// What shows wherever no layer covers the canvas.
    pub fn set_background(&mut self, colour: [u8; 3]) {
        let part = |value: u8| f64::from(value) / 255.0;
        self.background = wgpu::Color {
            r: part(colour[0]),
            g: part(colour[1]),
            b: part(colour[2]),
            a: 1.0,
        };
    }

    /// Draws the layers on the canvas, without bringing the result back. This is all a preview
    /// needs, which is why the speed test measures it on its own.
    pub fn draw(&mut self, layers: &[Layer]) -> Result<(), RenderError> {
        for (index, layer) in layers.iter().enumerate() {
            self.upload(index, layer);
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("canvas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.canvas_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.background),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.layer_pipeline);
            for slot in &self.slots[..layers.len()] {
                pass.set_bind_group(0, &slot.bind_group, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.check_for_problem()
    }

    /// Draws the layers and makes them ready to show in the window, through `display_view`.
    pub fn draw_for_display(&mut self, layers: &[Layer]) -> Result<(), RenderError> {
        self.draw(layers)?;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("display"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.display.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.display_pipeline);
            pass.set_bind_group(0, &self.output_bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.check_for_problem()
    }

    /// What `draw_for_display` drew, for the window's renderer to show.
    pub fn display_view(&self) -> &wgpu::TextureView {
        &self.display.view
    }

    /// The size of the canvas.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Brings back what `draw_for_display` drew as RGBA, so tests can see what the window shows.
    pub fn read_display(&mut self) -> Result<Vec<u8>, RenderError> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.display.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.display.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.display.row_bytes),
                    rows_per_image: None,
                },
            },
            self.display.texture.size(),
        );
        self.queue.submit([encoder.finish()]);
        let mapped = Arc::new(Mutex::new(None));
        let noted = mapped.clone();
        self.display
            .readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                if let (Err(error), Ok(mut found)) = (result, noted.lock()) {
                    *found = Some(error.to_string());
                }
            });
        self.wait()?;
        if let Some(problem) = mapped.lock().ok().and_then(|mut found| found.take()) {
            return Err(RenderError::Gpu(problem));
        }
        let mut pixels = Vec::new();
        {
            let data = self
                .display
                .readback
                .get_mapped_range(..)
                .map_err(|error| RenderError::Gpu(error.to_string()))?;
            copy_rows(
                &data,
                self.display.row_bytes as usize,
                self.size.0 as usize * 4,
                &mut pixels,
            );
        }
        self.display.readback.unmap();
        Ok(pixels)
    }

    /// Waits until everything asked of the GPU so far is done.
    pub fn wait(&self) -> Result<(), RenderError> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| RenderError::Gpu(error.to_string()))?;
        self.check_for_problem()
    }

    /// Brings the drawn canvas back as a picture ready to encode.
    pub fn read(&mut self) -> Result<Yuv420<'_>, RenderError> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for (pipeline, plane) in [
            (&self.brightness_pipeline, &self.brightness),
            (&self.colour_pipeline, &self.colour),
        ] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("output"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &plane.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.output_bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        for plane in [&self.brightness, &self.colour] {
            encoder.copy_texture_to_buffer(
                plane.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &plane.readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(plane.row_bytes),
                        rows_per_image: None,
                    },
                },
                plane.texture.size(),
            );
        }
        self.queue.submit([encoder.finish()]);

        let mapped = Arc::new(Mutex::new(Vec::new()));
        for plane in [&self.brightness, &self.colour] {
            let mapped = mapped.clone();
            plane
                .readback
                .map_async(wgpu::MapMode::Read, .., move |result| {
                    if let (Err(error), Ok(mut found)) = (result, mapped.lock()) {
                        found.push(error.to_string());
                    }
                });
        }
        self.wait()?;
        if let Some(problem) = mapped.lock().ok().and_then(|mut found| found.pop()) {
            return Err(RenderError::Gpu(problem));
        }

        let (width, height) = self.size;
        {
            let data = self
                .brightness
                .readback
                .get_mapped_range(..)
                .map_err(|error| RenderError::Gpu(error.to_string()))?;
            copy_rows(
                &data,
                self.brightness.row_bytes as usize,
                width as usize,
                &mut self.finished_y,
            );
        }
        self.brightness.readback.unmap();
        let chroma_width = width.div_ceil(2) as usize;
        {
            let data = self
                .colour
                .readback
                .get_mapped_range(..)
                .map_err(|error| RenderError::Gpu(error.to_string()))?;
            self.finished_u.clear();
            self.finished_v.clear();
            for row in data.chunks_exact(self.colour.row_bytes as usize) {
                for [u, v] in row[..chroma_width * 2].as_chunks::<2>().0 {
                    self.finished_u.push(*u);
                    self.finished_v.push(*v);
                }
            }
        }
        self.colour.readback.unmap();

        Ok(Yuv420 {
            width,
            height,
            y: &self.finished_y,
            y_stride: width as usize,
            u: &self.finished_u,
            v: &self.finished_v,
            uv_stride: chroma_width,
            color: ColorFormat::OUTPUT,
        })
    }

    /// Draws the layers and brings the result back.
    pub fn render(&mut self, layers: &[Layer]) -> Result<Yuv420<'_>, RenderError> {
        self.draw(layers)?;
        self.read()
    }
}

impl fmt::Debug for Compositor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Compositor")
            .field("gpu", &self.gpu_name)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{PictureLook, Rect, fit_inside};
    use bareclip_core::picture::ColorMatrix;

    /// Stored values for a flat colour, with the planes the size a picture needs.
    struct Flat {
        width: u32,
        height: u32,
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
        color: ColorFormat,
    }

    impl Flat {
        fn new(width: u32, height: u32, values: (u8, u8, u8)) -> Self {
            let chroma = (width.div_ceil(2) * height.div_ceil(2)) as usize;
            Self {
                width,
                height,
                y: vec![values.0; (width * height) as usize],
                u: vec![values.1; chroma],
                v: vec![values.2; chroma],
                color: ColorFormat::OUTPUT,
            }
        }

        fn view(&self) -> Yuv420<'_> {
            Yuv420 {
                width: self.width,
                height: self.height,
                y: &self.y,
                y_stride: self.width as usize,
                u: &self.u,
                v: &self.v,
                uv_stride: self.width.div_ceil(2) as usize,
                color: self.color,
            }
        }

        fn filling(&self, canvas: (u32, u32)) -> Layer<'_> {
            Layer {
                picture: self.view(),
                rect: fit_inside((self.width, self.height), canvas),
                opacity: 1.0,
                look: PictureLook::PLAIN,
            }
        }
    }

    /// Red on the left half and blue on the right, or the other way up if told to.
    fn half_and_half(width: u32, height: u32) -> Flat {
        let mut picture = Flat::new(width, height, RED_HD);
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
        for y in 0..height {
            for x in width / 2..width {
                picture.y[(y * width + x) as usize] = BLUE_HD.0;
            }
        }
        for y in 0..chroma_height {
            for x in chroma_width / 2..chroma_width {
                let at = (y * chroma_width + x) as usize;
                picture.u[at] = BLUE_HD.1;
                picture.v[at] = BLUE_HD.2;
            }
        }
        picture
    }

    #[test]
    fn the_background_shows_only_where_no_layer_covers_the_canvas() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        compositor.set_background([255, 0, 0]);
        let blue = Flat::new(16, 18, BLUE_HD);
        let layer = Layer {
            picture: blue.view(),
            rect: Rect {
                x: 32.0,
                y: 0.0,
                width: 32.0,
                height: 36.0,
            },
            opacity: 1.0,
            look: PictureLook::PLAIN,
        };

        let drawn = compositor.render(&[layer]).expect("it draws");

        assert_near(at(&drawn, 8, 18), RED_HD);
        assert_near(at(&drawn, 56, 18), BLUE_HD);
    }

    #[test]
    fn with_nothing_to_draw_the_whole_canvas_is_the_background() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        compositor.set_background([0, 0, 255]);

        let drawn = compositor.render(&[]).expect("it draws");

        assert_near(at(&drawn, 32, 18), BLUE_HD);
    }

    #[test]
    fn the_window_shows_the_background_as_the_colour_it_was_given() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        compositor.set_background([255, 148, 34]);

        compositor.draw_for_display(&[]).expect("it draws");
        let pixels = compositor.read_display().expect("it reads back");

        assert_eq!(&pixels[..4], &[255, 148, 34, 255]);
    }

    #[test]
    fn a_crop_shows_only_the_part_kept_stretched_over_the_place() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let picture = half_and_half(64, 36);
        let mut layer = picture.filling((64, 36));
        // Only the right half, which is all blue, across the whole place.
        layer.look.source = [0.5, 0.0, 1.0, 1.0];

        let drawn = compositor.render(&[layer]).expect("it draws");

        assert_near(at(&drawn, 8, 18), BLUE_HD);
        assert_near(at(&drawn, 56, 18), BLUE_HD);
    }

    #[test]
    fn a_crop_and_a_flip_together_read_the_kept_part_backwards() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let picture = half_and_half(64, 36);
        let mut layer = picture.filling((64, 36));
        // The middle half, so red then blue, but flipped so blue comes first.
        layer.look.source = [0.25, 0.0, 0.75, 1.0];
        layer.look.flip_x = true;

        let drawn = compositor.render(&[layer]).expect("it draws");

        assert_near(at(&drawn, 8, 18), BLUE_HD);
        assert_near(at(&drawn, 56, 18), RED_HD);
    }

    #[test]
    fn a_picture_flipped_left_to_right_has_its_sides_swapped() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let picture = half_and_half(64, 36);
        let mut layer = picture.filling((64, 36));
        layer.look.flip_x = true;

        let drawn = compositor.render(&[layer]).expect("it draws");

        assert_near(at(&drawn, 8, 18), BLUE_HD);
        assert_near(at(&drawn, 56, 18), RED_HD);
    }

    #[test]
    fn a_picture_flipped_top_to_bottom_keeps_its_sides() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let picture = half_and_half(64, 36);
        let mut layer = picture.filling((64, 36));
        layer.look.flip_y = true;

        let drawn = compositor.render(&[layer]).expect("it draws");

        assert_near(at(&drawn, 8, 18), RED_HD);
        assert_near(at(&drawn, 56, 18), BLUE_HD);
    }

    #[test]
    fn a_picture_turned_a_quarter_clockwise_has_its_left_at_the_top() {
        let Some(mut compositor) = compositor(64, 64) else {
            return;
        };
        let picture = half_and_half(64, 64);
        let mut layer = picture.filling((64, 64));
        layer.look.degrees = 90.0;

        let drawn = compositor.render(&[layer]).expect("it draws");

        assert_near(at(&drawn, 32, 8), RED_HD);
        assert_near(at(&drawn, 32, 56), BLUE_HD);
    }

    #[test]
    fn a_picture_turned_a_half_turn_has_its_sides_swapped() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let picture = half_and_half(64, 36);
        let mut layer = picture.filling((64, 36));
        layer.look.degrees = 180.0;

        let drawn = compositor.render(&[layer]).expect("it draws");

        assert_near(at(&drawn, 8, 18), BLUE_HD);
        assert_near(at(&drawn, 56, 18), RED_HD);
    }

    #[test]
    fn a_picture_turned_part_way_keeps_its_shape_on_a_wide_canvas() {
        // Turning in the canvas's pixels rather than its shares is what keeps corners square.
        let Some(mut compositor) = compositor(128, 64) else {
            return;
        };
        let picture = Flat::new(32, 32, RED_HD);
        let mut layer = picture.filling((128, 64));
        layer.rect = Rect {
            x: 48.0,
            y: 16.0,
            width: 32.0,
            height: 32.0,
        };
        layer.look.degrees = 45.0;

        let drawn = compositor.render(&[layer]).expect("it draws");

        // A square on its point reaches further straight out than diagonally.
        assert_near(at(&drawn, 64, 32), RED_HD);
        assert_near(at(&drawn, 80, 32), RED_HD);
        assert_near(at(&drawn, 64, 16), RED_HD);
        assert_near(at(&drawn, 82, 14), (16, 128, 128));
    }

    #[test]
    fn a_turned_pictures_edges_are_softened() {
        let Some(mut compositor) = compositor(128, 128) else {
            return;
        };
        let picture = Flat::new(32, 32, (235, 128, 128));
        let mut layer = picture.filling((128, 128));
        layer.rect = Rect {
            x: 32.0,
            y: 32.0,
            width: 64.0,
            height: 64.0,
        };
        layer.look.degrees = 30.0;

        let drawn = compositor.render(&[layer]).expect("it draws");

        let in_between = drawn
            .y
            .chunks(drawn.y_stride)
            .flatten()
            .filter(|value| **value > 40 && **value < 210)
            .count();
        assert!(in_between > 30, "only {in_between} pixels are in between");
    }

    #[test]
    fn the_window_gets_the_same_layers_as_ordinary_colours() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let red = Flat::new(64, 36, RED_HD);
        let blue = Flat::new(16, 18, BLUE_HD);
        let layers = [
            red.filling((64, 36)),
            Layer {
                picture: blue.view(),
                rect: Rect {
                    x: 32.0,
                    y: 0.0,
                    width: 32.0,
                    height: 36.0,
                },
                opacity: 1.0,
                look: PictureLook::PLAIN,
            },
        ];

        compositor.draw_for_display(&layers).expect("it draws");
        let pixels = compositor.read_display().expect("it reads back");

        let at = |x: usize, y: usize| {
            let start = (y * 64 + x) * 4;
            (
                pixels[start],
                pixels[start + 1],
                pixels[start + 2],
                pixels[start + 3],
            )
        };
        let (r, g, b, a) = at(8, 18);
        assert!(r > 240 && g < 16 && b < 16 && a == 255, "{:?}", at(8, 18));
        let (r, g, b, _) = at(56, 18);
        assert!(r < 16 && g < 16 && b > 240, "{:?}", at(56, 18));
    }

    const RED_HD: (u8, u8, u8) = (63, 102, 240);
    const GREEN_HD: (u8, u8, u8) = (173, 42, 26);
    const BLUE_HD: (u8, u8, u8) = (32, 240, 118);

    /// A compositor, or nothing on a machine with no GPU at all, such as a build server.
    fn compositor(width: u32, height: u32) -> Option<Compositor> {
        match Compositor::new(width, height) {
            Ok(compositor) => Some(compositor),
            Err(RenderError::NoGpu(reason)) => {
                eprintln!("skipped, because there is no GPU here: {reason}");
                None
            }
            Err(error) => panic!("{error}"),
        }
    }

    fn at(picture: &Yuv420, x: usize, y: usize) -> (i32, i32, i32) {
        let (chroma_x, chroma_y) = (x / 2, y / 2);
        (
            i32::from(picture.y[y * picture.y_stride + x]),
            i32::from(picture.u[chroma_y * picture.uv_stride + chroma_x]),
            i32::from(picture.v[chroma_y * picture.uv_stride + chroma_x]),
        )
    }

    #[track_caller]
    fn assert_near(found: (i32, i32, i32), wanted: (u8, u8, u8)) {
        let wanted = (
            i32::from(wanted.0),
            i32::from(wanted.1),
            i32::from(wanted.2),
        );
        let off = (found.0 - wanted.0)
            .abs()
            .max((found.1 - wanted.1).abs())
            .max((found.2 - wanted.2).abs());
        assert!(off <= 2, "{found:?} instead of {wanted:?}");
    }

    #[test]
    fn a_picture_comes_through_the_gpu_with_its_colours_unchanged() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        for colour in [RED_HD, GREEN_HD, BLUE_HD, (16, 128, 128), (235, 128, 128)] {
            let flat = Flat::new(64, 36, colour);
            let result = compositor
                .render(&[flat.filling((64, 36))])
                .expect("it draws");
            assert_near(at(&result, 20, 10), colour);
        }
    }

    #[test]
    fn nothing_on_the_canvas_is_black() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };

        let result = compositor.render(&[]).expect("it draws");

        assert_near(at(&result, 32, 18), (16, 128, 128));
    }

    #[test]
    fn a_smaller_picture_is_scaled_up_to_fill_a_canvas_of_its_shape() {
        let Some(mut compositor) = compositor(128, 72) else {
            return;
        };
        let flat = Flat::new(64, 36, RED_HD);

        let result = compositor
            .render(&[flat.filling((128, 72))])
            .expect("it draws");

        for (x, y) in [(2, 2), (64, 36), (125, 69)] {
            assert_near(at(&result, x, y), RED_HD);
        }
    }

    #[test]
    fn a_picture_of_another_shape_gets_black_bars_and_keeps_all_of_itself() {
        let Some(mut compositor) = compositor(128, 72) else {
            return;
        };
        // A square on a wide canvas: 72 wide in the middle, so bars of 28 at each side.
        let flat = Flat::new(36, 36, BLUE_HD);

        let result = compositor
            .render(&[flat.filling((128, 72))])
            .expect("it draws");

        assert_near(at(&result, 10, 36), (16, 128, 128));
        assert_near(at(&result, 118, 36), (16, 128, 128));
        assert_near(at(&result, 64, 36), BLUE_HD);
        assert_near(at(&result, 64, 2), BLUE_HD);
    }

    #[test]
    fn the_top_of_a_picture_stays_at_the_top() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let mut flat = Flat::new(64, 36, (16, 128, 128));
        flat.y[..64 * 18].fill(235);

        let result = compositor
            .render(&[flat.filling((64, 36))])
            .expect("it draws");

        assert!(at(&result, 32, 4).0 > 200, "the bright half is on top");
        assert!(at(&result, 32, 32).0 < 40);
    }

    #[test]
    fn the_left_of_a_picture_stays_on_the_left() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let mut flat = Flat::new(64, 36, (16, 128, 128));
        for row in flat.y.as_chunks_mut::<64>().0 {
            row[..32].fill(235);
        }

        let result = compositor
            .render(&[flat.filling((64, 36))])
            .expect("it draws");

        assert!(at(&result, 4, 18).0 > 200);
        assert!(at(&result, 60, 18).0 < 40);
    }

    #[test]
    fn later_layers_cover_earlier_ones_only_where_they_are() {
        let Some(mut compositor) = compositor(128, 72) else {
            return;
        };
        let bottom = Flat::new(128, 72, RED_HD);
        let top = Flat::new(32, 18, BLUE_HD);
        let small = Layer {
            picture: top.view(),
            rect: Rect {
                x: 64.0,
                y: 36.0,
                width: 32.0,
                height: 18.0,
            },
            opacity: 1.0,
            look: PictureLook::PLAIN,
        };

        let result = compositor
            .render(&[bottom.filling((128, 72)), small])
            .expect("it draws");

        assert_near(at(&result, 10, 10), RED_HD);
        assert_near(at(&result, 80, 44), BLUE_HD);
    }

    #[test]
    fn three_layers_at_once_all_show() {
        let Some(mut compositor) = compositor(128, 72) else {
            return;
        };
        let bottom = Flat::new(128, 72, RED_HD);
        let middle = Flat::new(32, 18, GREEN_HD);
        let top = Flat::new(32, 18, BLUE_HD);
        fn placed(flat: &Flat, x: f32) -> Layer<'_> {
            Layer {
                picture: flat.view(),
                rect: Rect {
                    x,
                    y: 0.0,
                    width: 32.0,
                    height: 18.0,
                },
                opacity: 1.0,
                look: PictureLook::PLAIN,
            }
        }
        let layers = [
            bottom.filling((128, 72)),
            placed(&middle, 0.0),
            placed(&top, 64.0),
        ];

        let result = compositor.render(&layers).expect("it draws");

        assert_near(at(&result, 100, 50), RED_HD);
        assert_near(at(&result, 16, 8), GREEN_HD);
        assert_near(at(&result, 80, 8), BLUE_HD);
    }

    #[test]
    fn a_see_through_layer_lets_what_is_under_it_show() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let white = Flat::new(64, 36, (235, 128, 128));
        let black = Flat::new(64, 36, (16, 128, 128));
        let half = Layer {
            opacity: 0.5,
            ..black.filling((64, 36))
        };

        let result = compositor
            .render(&[white.filling((64, 36)), half])
            .expect("it draws");

        let brightness = at(&result, 32, 18).0;
        assert!((brightness - 125).abs() <= 3, "{brightness}");
    }

    #[test]
    fn old_video_colours_are_converted_to_the_hd_standard() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        // The same red, stored the way old video stores it.
        let mut flat = Flat::new(64, 36, (81, 90, 240));
        flat.color.matrix = ColorMatrix::Bt601;

        let result = compositor
            .render(&[flat.filling((64, 36))])
            .expect("it draws");

        assert_near(at(&result, 32, 18), RED_HD);
    }

    #[test]
    fn full_range_values_are_converted_to_the_usual_range() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let mut flat = Flat::new(64, 36, (255, 128, 128));
        flat.color.full_range = true;

        let result = compositor
            .render(&[flat.filling((64, 36))])
            .expect("it draws");

        assert_near(at(&result, 32, 18), (235, 128, 128));
    }

    #[test]
    fn rows_with_padding_are_read_correctly() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        // Decoders commonly leave spare bytes at the end of each row.
        let stride = 96;
        let y = vec![173; stride * 36];
        let chroma_stride = 48;
        let (u, v) = (vec![42; chroma_stride * 18], vec![26; chroma_stride * 18]);
        let picture = Yuv420 {
            width: 64,
            height: 36,
            y: &y,
            y_stride: stride,
            u: &u,
            v: &v,
            uv_stride: chroma_stride,
            color: ColorFormat::OUTPUT,
        };
        let layer = Layer {
            picture,
            rect: fit_inside((64, 36), (64, 36)),
            opacity: 1.0,
            look: PictureLook::PLAIN,
        };

        let result = compositor.render(&[layer]).expect("it draws");

        assert_near(at(&result, 32, 18), GREEN_HD);
    }

    #[test]
    fn an_odd_sized_canvas_and_picture_still_work() {
        let Some(mut compositor) = compositor(65, 37) else {
            return;
        };
        let flat = Flat::new(33, 19, RED_HD);

        let result = compositor
            .render(&[flat.filling((65, 37))])
            .expect("it draws");

        assert_eq!(result.u.len(), 33 * 19);
        assert_near(at(&result, 32, 18), RED_HD);
    }

    #[test]
    fn pictures_of_a_new_size_replace_the_old_ones() {
        let Some(mut compositor) = compositor(64, 36) else {
            return;
        };
        let small = Flat::new(32, 18, RED_HD);
        let large = Flat::new(64, 36, BLUE_HD);

        let first = compositor
            .render(&[small.filling((64, 36))])
            .map(|picture| at(&picture, 32, 18))
            .expect("it draws");
        let second = compositor
            .render(&[large.filling((64, 36))])
            .map(|picture| at(&picture, 32, 18))
            .expect("it draws");

        assert_near(first, RED_HD);
        assert_near(second, BLUE_HD);
    }

    #[test]
    fn a_canvas_too_large_for_the_gpu_is_refused_with_a_reason() {
        match Compositor::new(1_000_000, 16) {
            Err(RenderError::TooLarge { limit, .. }) => assert!(limit < 1_000_000),
            Err(RenderError::NoGpu(_)) => {}
            other => panic!("{other:?}"),
        }
    }
}
