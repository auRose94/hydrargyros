//! The wgpu render backend: an offscreen renderer — no window, no surface,
//! no display needed. Frames render to a texture, get read back into RAM,
//! and can be saved as PNGs (the screenshot debug aid). Tests assert
//! actual pixels; a window-presenting backend is a later rung.
//!
//! Two pipelines share each frame: the quad scene (2D shapes, drawn in
//! list order, no depth) and the mesh scene (triangle models — depth
//! buffer, back-face culling, one fixed key light; meshes draw after
//! quads, so a mesh covers a quad it overlaps). Meshes arrive as
//! world-space transforms and build their model matrices here.

use std::collections::HashMap;

#[cfg(feature = "winit")]
use std::sync::Arc;

use crate::game::camera::Camera;
use crate::game::light::SceneLight;
use crate::game::mesh::Mesh;
use crate::game::model::Model;
use crate::game::texture::Texture;
use crate::game::transform::Transform;

/// The quad scene: 2D solid shapes in clip space (silhouette logic below).
const WGSL_QUAD: &str = r#"
struct Quad {
    pos: vec2f,
    size: vec2f,
    color: vec4f,
    shape: u32,
    _pad: u32,
};

struct Scene {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    quads: array<Quad, 256>,
};

@group(0) @binding(0) var<uniform> scene: Scene;

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) color: vec3f,
    @location(1) @interpolate(flat) shape: u32,
    @location(2) local: vec2f,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let quad = i / 6u;              // which quad
    let corner = i % 6u;            // which of the 6 triangle corners
    var offsets = array(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(-1.0, 1.0),
        vec2f(-1.0, 1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
    );
    var out: VsOut;
    out.pos = vec4f(scene.quads[quad].pos + offsets[corner] * scene.quads[quad].size, 0.0, 1.0);
    out.color = scene.quads[quad].color.rgb;
    out.shape = scene.quads[quad].shape;
    out.local = offsets[corner];
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    // circles: clip to the inscribed disc
    if (in.shape == 1u && length(in.local) > 1.0) {
        discard;
    }
    return vec4f(in.color, 1.0);
}
"#;

/// The mesh scene: one draw per instance, its `Globals` selected by a
/// dynamic uniform offset and the scene's lights by a second uniform
/// (group 0), its texture by a per-draw bind group (group 1 — untextured
/// draws bind a 1×1 white fallback, so one pipeline serves both). Normals
/// rotate with the model. Lighting follows the KHR punctual-light
/// contract: the uniform row layout (`dir+range / color+intensity /
/// pos+innerCos / outerCos+type`), KHR range attenuation and smoothstep
/// spot cones, shaded per-fragment over a fixed 0.45 ambient (IBL is its
/// own rung). A scene with no lights renders ambient-only.
const WGSL_MESH: &str = r#"
struct Globals {
    camera: mat4x4<f32>,   // world → clip (x, y = the 2D view; z = depth window)
    model: mat4x4<f32>,    // the entity's world transform
    color: vec4<f32>,      // tint over the vertex colors
};

// the u_Lights row layout (KHR punctual packing):
// row0 dir.xyz + range, row1 color.rgb + intensity,
// row2 pos.xyz + innerConeCos, row3 outerConeCos + type.
struct Light {
    dir_range: vec4<f32>,
    color_intensity: vec4<f32>,
    pos_inner: vec4<f32>,
    outer_type: vec4<f32>,
};

struct Lighting {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    lights: array<Light, 8>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<uniform> lighting: Lighting;
@group(1) @binding(0) var base_tex: texture_2d<f32>;
@group(1) @binding(1) var base_samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world_normal: vec3<f32>,
    @location(3) world_pos: vec3<f32>,
};

@vertex
fn vs(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
) -> VsOut {
    let world = globals.model * vec4<f32>(pos, 1.0);
    var out: VsOut;
    out.clip = globals.camera * world;
    out.color = color.rgb * globals.color.rgb;
    out.uv = uv;
    out.world_normal = normalize((globals.model * vec4<f32>(normal, 0.0)).xyz);
    out.world_pos = world.xyz;
    return out;
}

// KHR_lights_punctual range attenuation (punctual.sh): range ≤ 0 is
// unlimited, else (1 − (d/r)⁴)⁺ / d². Distance clamps to a hair above
// zero so a surface exactly at the light can't poison NaNs (a dead
// light must not divide by zero either).
fn range_attenuation(range: f32, dist: f32) -> f32 {
    if (range <= 0.0) {
        return 1.0;
    }
    let d = max(dist, 1e-4);
    return max(min(1.0 - pow(d / range, 4.0), 1.0), 0.0) / (d * d);
}

fn light_contribution(n: vec3<f32>, world_pos: vec3<f32>, L: Light) -> vec3<f32> {
    let kind = u32(L.outer_type.y);
    var radiance = vec3<f32>(0.0, 0.0, 0.0);
    if (kind == 0u) {
        // directional: dir is where the light shines; no range falloff
        let l = normalize(-L.dir_range.xyz);
        let nl = max(dot(n, l), 0.0);
        radiance = L.color_intensity.rgb * (L.color_intensity.a * nl);
    } else {
        let to_light = L.pos_inner.xyz - world_pos;
        let dist = length(to_light);
        let l = to_light / max(dist, 1e-4);
        let nl = max(dot(n, l), 0.0);
        if (nl > 0.0) {
            var att = range_attenuation(L.dir_range.w, dist);
            if (kind == 2u) {
                // spot: smoothstep between the outer/inner cone cosines;
                // dir is where the light shines (KHR spot-attenuation shape)
                let actual_cos = dot(normalize(L.dir_range.xyz), normalize(-to_light));
                var spot = 0.0;
                if (actual_cos > L.outer_type.x) {
                    if (actual_cos < L.pos_inner.w) {
                        spot = smoothstep(L.outer_type.x, L.pos_inner.w, actual_cos);
                    } else {
                        spot = 1.0;
                    }
                }
                att = att * spot;
            }
            radiance = L.color_intensity.rgb * (L.color_intensity.a * att * nl);
        }
    }
    return radiance;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_tex, base_samp, in.uv).rgb;
    // fixed ambient floor (IBL/envMap is a later rung)
    var light = vec3<f32>(0.45, 0.45, 0.45);
    for (var i = 0u; i < lighting.count; i = i + 1u) {
        light = light + light_contribution(in.world_normal, in.world_pos, lighting.lights[i]);
    }
    return vec4f(in.color * tex * light, 1.0);
}
"#;

/// The per-frame uniform: up to 256 quads. CPU layout must match the WGSL:
/// `count` + 3 pad words, then 48-byte quads (color is 16-aligned, so the
/// quad's stride rounds 40 up to 48).
const MAX_QUADS: usize = 256;
const QUAD_STRIDE: usize = 48;
const SCENE_BYTES: usize = 16 + MAX_QUADS * QUAD_STRIDE;

fn scene_bytes(quads: &[crate::game::render::Quad]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SCENE_BYTES);
    out.extend_from_slice(&(quads.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for quad in quads.iter().take(MAX_QUADS) {
        for f in quad
            .pos
            .iter()
            .chain(quad.size.iter())
            .chain(quad.color.iter())
        {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&quad.shape.as_u32().to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.resize(out.len() + 8, 0); // pad the stride to 48
    }
    out.resize(SCENE_BYTES, 0);
    out
}

/// One model part queued for this frame (a `Model` with N parts queues N).
struct MeshDraw {
    model: Transform,
    color: [f32; 4],
    mesh: std::sync::Arc<Mesh>,
    texture: Option<std::sync::Arc<Texture>>,
}

/// Mesh draws per frame (each is one draw call — fine at v0 counts).
const MAX_MESH_DRAWS: usize = 64;
/// Dynamic uniform offsets align to the default 256.
const MESH_STRIDE: usize = 256;
const MESH_GLOBALS_BYTES: usize = MAX_MESH_DRAWS * MESH_STRIDE;
/// camera + model + color, in f32 words: 16 + 16 + 4 = 144 bytes.
const MESH_GLOBALS_SIZE: usize = 144;

/// World → clip for the mesh scene — the camera's own matrix
/// ([`Camera::view_projection`]): the v0 ortho window (x, y the view, z
/// the depth window into WebGPU's `[0, 1]` — `Less` is "closer wins") or
/// the perspective look-at. Column-major, WGSL's order.
fn camera_matrix(camera: &Camera) -> [[f32; 4]; 4] {
    camera.view_projection()
}

/// The entity transform as a model matrix: `T · R · S` — scale, then
/// rotate, then translate (the order `Transform::compose` applies).
fn model_matrix(t: &Transform) -> [[f32; 4]; 4] {
    let q = t.rotation;
    let (xx, yy, zz) = (q.x * q.x, q.y * q.y, q.z * q.z);
    let (xy, xz, yz) = (q.x * q.y, q.x * q.z, q.y * q.z);
    let (wx, wy, wz) = (q.w * q.x, q.w * q.y, q.w * q.z);
    // rotation matrix, row-major
    let r = [
        [1.0 - 2.0 * (yy + zz), 2.0 * (xy - wz), 2.0 * (xz + wy)],
        [2.0 * (xy + wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz - wx)],
        [2.0 * (xz - wy), 2.0 * (yz + wx), 1.0 - 2.0 * (xx + yy)],
    ];
    let s = [t.scale.x, t.scale.y, t.scale.z];
    let mut m = [[0.0f32; 4]; 4];
    for c in 0..3 {
        for row in 0..3 {
            // column c of the rotation, scaled by scale[c]
            m[c][row] = r[row][c] * s[c];
        }
    }
    m[3][0] = t.position.x;
    m[3][1] = t.position.y;
    m[3][2] = t.position.z;
    m[3][3] = 1.0;
    m
}

/// Serialize one Globals block per draw, at dynamic-offset strides.
fn mesh_globals_bytes(camera: &Camera, draws: &[MeshDraw]) -> Vec<u8> {
    let cam = camera_matrix(camera);
    let mut out = vec![0u8; MESH_GLOBALS_BYTES];
    for (i, draw) in draws.iter().take(MAX_MESH_DRAWS).enumerate() {
        let model = model_matrix(&draw.model);
        let mut words: Vec<f32> = Vec::with_capacity(36);
        for col in cam.iter().chain(model.iter()) {
            words.extend_from_slice(col);
        }
        words.extend_from_slice(&draw.color);
        let offset = i * MESH_STRIDE;
        for (j, word) in words.iter().enumerate() {
            let at = offset + j * 4;
            out[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
    out
}

/// Lights per scene — a whole uniform bank of eight slots.
const MAX_LIGHTS: usize = 8;
/// One light = four uniform rows: 64 bytes.
const LIGHT_BYTES: usize = 64;
const LIGHTING_BYTES: usize = 16 + MAX_LIGHTS * LIGHT_BYTES;

/// Pack the scene's lights in the uniform row layout (see WGSL_MESH):
/// header count, then `dir+range / color+intensity / pos+innerCos /
/// outerCos+type` per light. Lights past MAX_LIGHTS drop (documented
/// capacity).
fn lighting_bytes(lights: &[SceneLight]) -> Vec<u8> {
    let mut out = vec![0u8; LIGHTING_BYTES];
    let count = lights.len().min(MAX_LIGHTS) as u32;
    out[0..4].copy_from_slice(&count.to_le_bytes());
    for (i, light) in lights.iter().take(MAX_LIGHTS).enumerate() {
        let mut rows = [0.0f32; 16];
        rows[0] = light.direction.x;
        rows[1] = light.direction.y;
        rows[2] = light.direction.z;
        rows[3] = light.range;
        rows[4] = light.color[0];
        rows[5] = light.color[1];
        rows[6] = light.color[2];
        rows[7] = light.intensity;
        rows[8] = light.position.x;
        rows[9] = light.position.y;
        rows[10] = light.position.z;
        rows[11] = light.cone[0];
        rows[12] = light.cone[1];
        rows[13] = light.kind.as_u32() as f32;
        let at = 16 + i * LIGHT_BYTES;
        for (j, word) in rows.iter().enumerate() {
            out[at + j * 4..at + j * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
    out
}

/// A mesh's GPU-side vertex + index buffers, cached by [`Mesh::id`] —
/// meshes are immutable, so one upload serves every frame.
struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

fn upload_mesh(device: &wgpu::Device, mesh: &Mesh) -> GpuMesh {
    let mut vbytes = Vec::with_capacity(mesh.vertices().len() * 48);
    for v in mesh.vertices() {
        for f in v
            .pos
            .iter()
            .chain(v.normal.iter())
            .chain(v.uv.iter())
            .chain(v.color.iter())
        {
            vbytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hyg-mesh-vbo"),
        size: vbytes.len() as u64,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: true,
    });
    {
        let mut view = vertex_buffer
            .slice(..)
            .get_mapped_range_mut()
            .expect("map a fresh vertex buffer");
        view.copy_from_slice(&vbytes);
    }
    vertex_buffer.unmap();

    let mut ibytes = Vec::with_capacity(mesh.indices().len() * 4);
    for &i in mesh.indices() {
        ibytes.extend_from_slice(&i.to_le_bytes());
    }
    let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hyg-mesh-ibo"),
        size: ibytes.len() as u64,
        usage: wgpu::BufferUsages::INDEX,
        mapped_at_creation: true,
    });
    {
        let mut view = index_buffer
            .slice(..)
            .get_mapped_range_mut()
            .expect("map a fresh index buffer");
        view.copy_from_slice(&ibytes);
    }
    index_buffer.unmap();

    GpuMesh {
        vertex_buffer,
        index_buffer,
        index_count: mesh.indices().len() as u32,
    }
}

/// pos + normal + uv + color, 48-byte stride (matches `MeshVertex`).
const MESH_VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 48,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &[
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 24,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 32,
            shader_location: 3,
        },
    ],
};

/// Everything the two pipelines need: created once per target format (the
/// quad scene's uniform at binding 0, the mesh scene's dynamic-offset
/// globals at group 0 and its texture/sampler at group 1, with a 1×1
/// white fallback bind group for untextured draws).
struct ScenePipelines {
    quad_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    mesh_globals: wgpu::Buffer,
    lighting: wgpu::Buffer,
    mesh_bind_group: wgpu::BindGroup,
    /// Layout for per-draw texture bind groups (created lazily on upload).
    tex_layout: wgpu::BindGroupLayout,
    /// Bind group of a 1×1 white texture — bound for untextured draws so
    /// one pipeline serves both.
    white_bind_group: wgpu::BindGroup,
}

fn create_pipelines(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
) -> ScenePipelines {
    let quad_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hyg-quad"),
        source: wgpu::ShaderSource::Wgsl(WGSL_QUAD.into()),
    });
    let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hyg-mesh"),
        source: wgpu::ShaderSource::Wgsl(WGSL_MESH.into()),
    });

    // quad scene: one uniform of the whole quad list
    let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(SCENE_BYTES as u64),
            },
            count: None,
        }],
    });
    // mesh scene: one dynamic-offset uniform per draw + the scene's
    // lights (group 0), the draw's texture + sampler (group 1)
    let mesh_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(MESH_GLOBALS_SIZE as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(LIGHTING_BYTES as u64),
                },
                count: None,
            },
        ],
    });
    let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let quad_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&scene_layout)],
        immediate_size: 0,
    });
    let mesh_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&mesh_layout), Some(&tex_layout)],
        immediate_size: 0,
    });

    // the pass carries a depth attachment (for meshes); the quad pipeline
    // ignores it — always-pass, never write — so list order still wins
    let depth_ignore = wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth24Plus,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hyg-quad"),
        layout: Some(&quad_pipe_layout),
        vertex: wgpu::VertexState {
            module: &quad_shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &quad_shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(depth_ignore),
        multisample: wgpu::MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    });

    let depth = wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth24Plus,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hyg-mesh"),
        layout: Some(&mesh_pipe_layout),
        vertex: wgpu::VertexState {
            module: &mesh_shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(MESH_VERTEX_LAYOUT)],
        },
        fragment: Some(wgpu::FragmentState {
            module: &mesh_shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        // glTF winding: CCW front faces, culled backs
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(depth),
        multisample: wgpu::MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    });

    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hyg-scene"),
        size: SCENE_BYTES as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &scene_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniforms.as_entire_binding(),
        }],
    });
    let mesh_globals = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hyg-mesh-globals"),
        size: MESH_GLOBALS_BYTES as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let lighting = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hyg-lighting"),
        size: LIGHTING_BYTES as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mesh_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &mesh_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                // one Globals-sized window: dynamic offsets slide it per draw
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &mesh_globals,
                    offset: 0,
                    size: wgpu::BufferSize::new(MESH_GLOBALS_SIZE as u64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: lighting.as_entire_binding(),
            },
        ],
    });

    let white = Texture::white();
    let white_gpu = upload_texture(device, queue, &tex_layout, &white);
    let white_bind_group = white_gpu.bind_group;

    ScenePipelines {
        quad_pipeline,
        mesh_pipeline,
        uniforms,
        bind_group,
        mesh_globals,
        lighting,
        mesh_bind_group,
        tex_layout,
        white_bind_group,
    }
}

/// One texture's GPU side: image + view + sampler + a ready bind group,
/// cached by [`Texture::id`] — textures are immutable, one upload serves
/// every frame.
struct GpuTexture {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex_layout: &wgpu::BindGroupLayout,
    texture: &Texture,
) -> GpuTexture {
    let gpu = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hyg-texture"),
        size: wgpu::Extent3d {
            width: texture.width().max(1),
            height: texture.height().max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // sRGB: sampling linearizes, matching the linear pipeline colors
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &gpu,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        texture.rgba(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture.width().max(1) * 4),
            rows_per_image: Some(texture.height().max(1)),
        },
        wgpu::Extent3d {
            width: texture.width().max(1),
            height: texture.height().max(1),
            depth_or_array_layers: 1,
        },
    );
    let view = gpu.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("hyg-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        // v0: no mipmaps — clamp instead of mipping
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hyg-texture-bind"),
        layout: tex_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    GpuTexture {
        texture: gpu,
        bind_group,
    }
}

fn depth_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hyg-depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth24Plus,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// An offscreen wgpu renderer: clears to the set color and draws the queued
/// quads and meshes. `screenshot()` returns the last frame as RGBA8 rows.
pub struct WgpuRenderer {
    // kept alive for the winit presentation path (surface creation)
    #[allow(dead_code)]
    instance: wgpu::Instance,
    #[allow(dead_code)]
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: ScenePipelines,
    scene: Vec<crate::game::render::Quad>,
    lights: Vec<SceneLight>,
    mesh_draws: Vec<MeshDraw>,
    mesh_buffers: HashMap<u64, GpuMesh>,
    tex_buffers: HashMap<u64, GpuTexture>,
    target: wgpu::Texture,
    // kept alive only to back `depth_view` (destroying it would invalidate)
    #[allow(dead_code)]
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    size: (u32, u32),
    clear_color: [f64; 4],
    camera: Option<Camera>,
    frame_sink: Option<crate::game::render::FrameSink>,
    last_frame: Vec<u8>,
    /// When a window is attached, frames present to its surface instead of
    /// the offscreen target (no readback — the compositor is the consumer).
    #[cfg(feature = "winit")]
    window: Option<(Arc<winit::window::Window>, wgpu::Surface<'static>)>,
    #[cfg(feature = "winit")]
    window_config: Option<wgpu::SurfaceConfiguration>,
}

impl WgpuRenderer {
    /// Create the renderer bound to a winit window: the adapter is chosen
    /// against the window's surface, so frames present to the window.
    /// Still `async` (adapter/device requests); call under `pollster`.
    #[cfg(feature = "winit")]
    pub async fn new_for_window(window: Arc<winit::window::Window>) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("window surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("no wgpu adapter for the window surface: {e}"))?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let (width, height) = (
            window.inner_size().width.max(1),
            window.inner_size().height.max(1),
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| format!("wgpu device: {e}"))?;

        let pipelines = create_pipelines(&device, &queue, format);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Srgb,
        };
        surface.configure(&device, &config);
        let target = Self::offscreen_target(&device, 64, 64);
        let readback = Self::offscreen_readback(&device, 64, 64);
        let (depth, depth_view) = depth_target(&device, width, height);

        Ok(WgpuRenderer {
            instance,
            adapter,
            device,
            queue,
            pipelines,
            target,
            depth,
            depth_view,
            readback,
            size: (64, 64),
            clear_color: [0.1, 0.2, 0.3, 1.0],
            camera: None,
            scene: Vec::new(),
            lights: Vec::new(),
            mesh_draws: Vec::new(),
            mesh_buffers: HashMap::new(),
            tex_buffers: HashMap::new(),
            frame_sink: None,
            last_frame: Vec::new(),
            window: Some((window, surface)),
            window_config: Some(config),
        })
    }

    /// Create the offscreen renderer: device + pipeline + target. Tries the
    /// platform's real backends (Vulkan/Metal/DX12/GL).
    ///
    /// # Errors
    /// No adapter, device creation failure, or pipeline creation failure —
    /// e.g. a machine with no GPU stack at all.
    pub async fn new_offscreen(width: u32, height: u32) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .map_err(|e| format!("no wgpu adapter: {e}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| format!("wgpu device: {e}"))?;

        let pipelines = create_pipelines(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let target = Self::offscreen_target(&device, width, height);
        let readback = Self::offscreen_readback(&device, width, height);
        let (depth, depth_view) = depth_target(&device, width, height);

        Ok(WgpuRenderer {
            instance,
            adapter,
            device,
            queue,
            pipelines,
            target,
            depth,
            depth_view,
            readback,
            size: (width, height),
            clear_color: [0.1, 0.2, 0.3, 1.0],
            camera: None,
            scene: Vec::new(),
            lights: Vec::new(),
            mesh_draws: Vec::new(),
            mesh_buffers: HashMap::new(),
            tex_buffers: HashMap::new(),
            frame_sink: None,
            last_frame: Vec::new(),
            #[cfg(feature = "winit")]
            window: None,
            #[cfg(feature = "winit")]
            window_config: None,
        })
    }

    pub fn set_clear_color(&mut self, color: [f64; 4]) {
        self.clear_color = color;
    }

    /// The scene: solid quads in clip space (center, half-extents, color),
    /// drawn back-to-front in list order — the v0 "mesh", enough to see
    /// entities move (max 64).
    pub fn set_scene(&mut self, quads: &[crate::game::render::Quad]) {
        self.scene = quads.to_vec();
    }

    /// Convenience: a single-quad scene.
    pub fn set_quad(&mut self, pos: [f32; 2], size: [f32; 2], color: [f32; 4]) {
        self.scene = vec![crate::game::render::Quad::rect(pos, size, color)];
    }

    fn offscreen_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hyg-offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn offscreen_readback(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Buffer {
        let bytes_per_row = (width * 4).div_ceil(256) * 256;
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hyg-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    }

    /// Publish every rendered frame into this sink (a streaming window
    /// backend reads from it).
    pub fn attach_sink(&mut self, sink: crate::game::render::FrameSink) {
        self.frame_sink = Some(sink);
    }

    /// Present frames to a winit window's surface instead of the offscreen
    /// target. The adapter must be the same one the surface's display
    /// belongs to — build the renderer with
    /// [`new_for_window`](Self::new_for_window) to guarantee that.
    #[cfg(feature = "winit")]
    pub fn attach_window(&mut self, window: Arc<winit::window::Window>) -> Result<(), String> {
        let surface = self
            .instance
            .create_surface(window.clone())
            .map_err(|e| format!("window surface: {e}"))?;
        let caps = surface.get_capabilities(&self.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window.inner_size().width.max(1),
            height: window.inner_size().height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Srgb,
        };
        surface.configure(&self.device, &config);
        let (depth, depth_view) = depth_target(&self.device, config.width, config.height);
        self.depth = depth;
        self.depth_view = depth_view;
        self.window_config = Some(config);
        self.window = Some((window, surface));
        Ok(())
    }

    /// The last rendered frame: RGBA8 rows, de-padded, top to bottom.
    /// Quads first (list order, no depth), then mesh draws (depth-tested,
    /// one draw call per instance at its dynamic-offset globals).
    fn draw_pass(&mut self, view: &wgpu::TextureView, depth_view: &wgpu::TextureView) {
        let camera = self.camera.unwrap_or_default();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.queue
            .write_buffer(&self.pipelines.uniforms, 0, &scene_bytes(&self.scene));
        self.queue
            .write_buffer(&self.pipelines.lighting, 0, &lighting_bytes(&self.lights));
        if !self.mesh_draws.is_empty() {
            self.queue.write_buffer(
                &self.pipelines.mesh_globals,
                0,
                &mesh_globals_bytes(&camera, &self.mesh_draws),
            );
        }
        let clear = wgpu::Color {
            r: self.clear_color[0],
            g: self.clear_color[1],
            b: self.clear_color[2],
            a: self.clear_color[3],
        };
        // meshes and textures already uploaded stay cached; new ones
        // upload once here, before the pass borrows the pipeline state
        for draw in &self.mesh_draws {
            if !self.mesh_buffers.contains_key(&draw.mesh.id()) {
                let gpu = upload_mesh(&self.device, &draw.mesh);
                self.mesh_buffers.insert(draw.mesh.id(), gpu);
            }
            if let Some(texture) = &draw.texture {
                if !self.tex_buffers.contains_key(&texture.id()) {
                    let gpu = upload_texture(
                        &self.device,
                        &self.queue,
                        &self.pipelines.tex_layout,
                        texture,
                    );
                    self.tex_buffers.insert(texture.id(), gpu);
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hyg-frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None, // Depth24Plus has no stencil aspect
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipelines.quad_pipeline);
            pass.set_bind_group(0, &self.pipelines.bind_group, &[]);
            let quads = self.scene.len().max(1) as u32;
            pass.draw(0..6 * quads, 0..1);

            if !self.mesh_draws.is_empty() {
                pass.set_pipeline(&self.pipelines.mesh_pipeline);
                for (i, draw) in self.mesh_draws.iter().take(MAX_MESH_DRAWS).enumerate() {
                    let gpu = &self.mesh_buffers[&draw.mesh.id()];
                    let offset = (i * MESH_STRIDE) as u32;
                    pass.set_bind_group(0, &self.pipelines.mesh_bind_group, &[offset]);
                    // the draw's texture, or the white fallback (one
                    // pipeline serves textured and untextured draws)
                    let tex_bind = match &draw.texture {
                        Some(texture) => &self.tex_buffers[&texture.id()].bind_group,
                        None => &self.pipelines.white_bind_group,
                    };
                    pass.set_bind_group(1, tex_bind, &[]);
                    pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
                    pass.set_index_buffer(gpu.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..gpu.index_count, 0, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    pub fn screenshot(&self) -> &[u8] {
        &self.last_frame
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

impl super::render::RenderBackend for WgpuRenderer {
    fn name(&self) -> &str {
        "wgpu-offscreen"
    }

    fn pre_frame(&mut self) {}

    fn begin_scene(&mut self, camera: &Camera, lights: &[SceneLight]) {
        self.camera = Some(*camera);
        self.scene.clear();
        self.lights = lights.to_vec();
        self.mesh_draws.clear();
    }

    fn push_quad(&mut self, mut quad: crate::game::render::Quad) {
        if self.scene.len() >= MAX_QUADS {
            return;
        }
        if let Some(camera) = &self.camera {
            // position AND size transform: half-extents divide by the view's
            // half-extents, or every quad renders at the wrong scale. Quads
            // stay an orthographic image of the world (the quad_window —
            // under a perspective camera, the window derived at the target
            // plane); the mesh scene gets the real projection.
            let window = camera.quad_window();
            quad.pos = camera.world_to_clip(crate::var::Vec3::new(quad.pos[0], quad.pos[1], 0.0));
            quad.size = [quad.size[0] / window.x, quad.size[1] / window.y];
        }
        self.scene.push(quad);
    }

    fn push_model(&mut self, world: &Transform, model: &Model, color: [f32; 4]) {
        // one draw per part (a part = one texture group of the asset)
        for part in model.parts() {
            if self.mesh_draws.len() >= MAX_MESH_DRAWS {
                return;
            }
            self.mesh_draws.push(MeshDraw {
                model: *world,
                color,
                mesh: part.mesh.clone(),
                texture: part.texture.clone(),
            });
        }
    }

    fn render_frame(&mut self) {
        // window mode: present straight to the surface (no readback)
        #[cfg(feature = "winit")]
        if let Some((window, surface)) = &mut self.window {
            let config = self.window_config.as_mut().unwrap();
            let new_size = window.inner_size();
            if new_size.width.max(1) != config.width || new_size.height.max(1) != config.height {
                config.width = new_size.width.max(1);
                config.height = new_size.height.max(1);
                surface.configure(&self.device, config);
                let (depth, depth_view) = depth_target(&self.device, config.width, config.height);
                self.depth = depth;
                self.depth_view = depth_view;
            }
            let frame = match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(frame)
                | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                _ => return, // skip the frame: timeout/occluded/outdated/lost
            };
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let depth_view = self.depth_view.clone();
            self.draw_pass(&view, &depth_view);
            self.queue.present(frame);
            return;
        }

        let (width, height) = self.size;
        let view = self
            .target
            .create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self.depth_view.clone();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.draw_pass(&view, &depth_view);
        let bytes_per_row = (width * 4).div_ceil(256) * 256;
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        // read the frame back
        let total = (bytes_per_row * height) as u64;
        self.readback
            .map_async(wgpu::MapMode::Read, ..total, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(data) = self.readback.get_mapped_range(..total) else {
            return;
        };
        let mut frame = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * bytes_per_row) as usize;
            frame.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        drop(data);
        self.readback.unmap();
        if let Some(sink) = &self.frame_sink {
            sink.publish(width, height, frame.clone());
        }
        self.last_frame = frame;
    }
}
