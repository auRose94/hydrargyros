//! Render backends: the engine calls `pre_frame` before the update pass
//! and `render_frame` after the draw pass. The wgpu backend (behind the
//! `wgpu` feature) renders offscreen — no window or surface needed — so
//! frames can be pixel-tested and screenshotted headlessly (the
//! `{"screenshot", path}` debug aid).

use crate::game::light::SceneLight;
use crate::game::model::Model;
use crate::game::transform::Transform;

/// The geometry one renderable contributes to a scene: a solid quad (the
/// 2D shape path, rendered in list order) or a model — one or more
/// triangle-mesh parts with optional textures, drawn at its entity's
/// world transform, depth-tested against other models and shaded.
#[derive(Clone)]
pub enum Geometry {
    Quad(Quad),
    Model {
        model: std::sync::Arc<Model>,
        color: [f32; 4],
    },
}

/// The silhouette of a quad: a rectangle, or a circle inscribed in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuadShape {
    Rect,
    Circle,
}

impl QuadShape {
    /// The shader's encoding (1 = circle).
    pub fn as_u32(self) -> u32 {
        match self {
            QuadShape::Rect => 0,
            QuadShape::Circle => 1,
        }
    }
}

/// One solid quad in clip space: center `pos`, half-extents `size`, `color`,
/// `shape` silhouette. (World-space quads go through
/// [`RenderBackend::begin_scene`] + [`RenderBackend::push_quad`], which
/// apply the camera.)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
    pub shape: QuadShape,
}

impl Quad {
    /// A rectangular quad.
    pub fn rect(pos: [f32; 2], size: [f32; 2], color: [f32; 4]) -> Self {
        Quad {
            pos,
            size,
            color,
            shape: QuadShape::Rect,
        }
    }

    /// A circular quad (the circle is inscribed in the quad's bounds).
    pub fn circle(pos: [f32; 2], radius: [f32; 2], color: [f32; 4]) -> Self {
        Quad {
            pos,
            size: radius,
            color,
            shape: QuadShape::Circle,
        }
    }
}

/// One rendered frame, RGBA8, top to bottom.
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Monotonic counter — consumers skip frames they've already seen.
    pub generation: u64,
    pub rgba: Vec<u8>,
}

/// A shared slot for the latest rendered frame. The renderer publishes
/// into it; anything else (a streaming window backend, a debug tool) reads
/// the newest frame without coupling to the renderer.
#[derive(Clone, Default)]
pub struct FrameSink(std::sync::Arc<std::sync::Mutex<Inner>>);

#[derive(Default)]
struct Inner {
    frame: Option<Frame>,
    generation: u64,
}

impl FrameSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a frame (bumps the generation).
    pub fn publish(&self, width: u32, height: u32, rgba: Vec<u8>) {
        let mut inner = self.0.lock().unwrap();
        inner.generation += 1;
        inner.frame = Some(Frame {
            width,
            height,
            generation: inner.generation,
            rgba,
        });
    }

    /// The newest frame, if any.
    pub fn latest(&self) -> Option<Frame> {
        self.0.lock().unwrap().frame.clone()
    }

    /// The newest frame whose generation is greater than `seen`.
    pub fn latest_since(&self, seen: u64) -> Option<Frame> {
        let inner = self.0.lock().unwrap();
        match &inner.frame {
            Some(f) if f.generation > seen => Some(f.clone()),
            _ => None,
        }
    }
}

/// A rendering backend, as a trait.
pub trait RenderBackend: Send {
    fn name(&self) -> &str;

    /// Per-frame setup, before the update pass.
    fn pre_frame(&mut self) {}

    /// Begin a camera-driven scene: clears the quad list; subsequent
    /// [`push_quad`](Self::push_quad) calls take WORLD-space positions and
    /// are mapped through this camera. `lights` is the scene's gathered
    /// light list (passed in with the draw call; a scene with no lights
    /// renders ambient-only).
    fn begin_scene(&mut self, _camera: &crate::game::camera::Camera, _lights: &[SceneLight]) {}

    /// Append a quad to the current scene. Positions are world-space after
    /// [`begin_scene`](Self::begin_scene), clip-space otherwise.
    fn push_quad(&mut self, _quad: Quad) {}

    /// Append a model at `world` — the entity's world transform (position,
    /// rotation, scale all apply); `color` tints over the parts' vertex
    /// colors and textures. Defaults to a no-op: backends that can't draw
    /// models skip them.
    fn push_model(&mut self, _world: &Transform, _model: &Model, _color: [f32; 4]) {}

    /// Draw the frame. Called after the engine's draw pass.
    fn render_frame(&mut self) {}
}
