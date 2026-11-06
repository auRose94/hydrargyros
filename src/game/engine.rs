//! The engine — gold's `engine`: the pump + dispatch loop.
//!
//! gold's frame, ported: poll window events (`quit` stops the loop),
//! physics step (no physics backend yet), sort + initialize components,
//! priority-ordered `update` with the frame's delta time, the draw pass,
//! then the frame-rate cap (`frameTime` config, gold's default 16 ms).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::camera::Camera;
use super::component::Priority;
use super::entity::Entity;
use super::light::SceneLight;
use super::physics::PhysicsWorld;
use super::physics_body::PhysicsBody;
use super::render::RenderBackend;
use super::transform::Transform;
use super::window::{create_window_backend, WindowSystem};
use crate::var::Var;

/// Tree path to a component slot: `[top_entity_slot, child_idx, child_slot, …]`.
type SlotPath = Vec<usize>;

/// The game engine (gold's `engine`): entities, the pump, settings.
pub struct Engine {
    company: String,
    game_name: String,
    entities: Vec<Entity>,
    running: bool,
    /// Frame cap in milliseconds (gold's `frameTime`; 0 = uncapped, used by
    /// headless runs and tests).
    frame_time: f64,
    settings_root: PathBuf,
    physics: Option<PhysicsWorld>,
    renderer: Option<Box<dyn RenderBackend>>,
    camera: Camera,
    /// Fixed delta time for the pump (seconds). `None` = wall clock (gold's
    /// behavior); headless sims set it for deterministic frames.
    fixed_dt: Option<f64>,
}

impl Engine {
    /// An engine for `company`/`game_name` — the settings namespace
    /// (gold's `boot(company, gameName)`).
    pub fn new(company: impl Into<String>, game_name: impl Into<String>) -> Engine {
        Engine {
            company: company.into(),
            game_name: game_name.into(),
            entities: Vec::new(),
            running: false,
            frame_time: 16.0,
            settings_root: PathBuf::from("."),
            physics: None,
            renderer: None,
            camera: Camera::default(),
            fixed_dt: None,
        }
    }

    /// Attach the camera the scene renders through (a default 16:9 view is
    /// set up for you).
    pub fn set_camera(&mut self, camera: Camera) -> &mut Self {
        self.camera = camera;
        self
    }

    /// The camera the scene renders through.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// Drive the pump with a fixed delta time instead of the wall clock —
    /// the headless way to get deterministic frames (240 frames at 1/60 =
    /// 4 simulated seconds, however fast the machine runs).
    pub fn fixed_dt(&mut self, seconds: f64) -> &mut Self {
        self.fixed_dt = Some(seconds);
        self
    }

    /// Attach the physics world (gold's engine holds a `world` object).
    pub fn set_physics(&mut self, world: PhysicsWorld) -> &mut Self {
        self.physics = Some(world);
        self
    }

    /// The physics world, if one is attached.
    pub fn physics_mut(&mut self) -> Option<&mut PhysicsWorld> {
        self.physics.as_mut()
    }

    /// Attach a render backend (gold's `graphics` object).
    pub fn set_renderer(&mut self, renderer: Box<dyn RenderBackend>) -> &mut Self {
        self.renderer = Some(renderer);
        self
    }

    /// Attach a top-level entity (gold's `engine += [component, entity]`).
    pub fn add_entity(&mut self, entity: Entity) -> &mut Self {
        self.entities.push(entity);
        self
    }

    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    /// A top-level entity by name — the direct handle gold scripts get
    /// with `engine.entities[i]`. First match wins; children are reached
    /// through [`Entity::child_mut`](super::entity::Entity::child_mut).
    pub fn entity_mut(&mut self, name: &str) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.name() == name)
    }

    /// The frame cap in milliseconds; `0` disables it (headless runs).
    pub fn frame_time(&mut self, ms: f64) -> &mut Self {
        self.frame_time = ms;
        self
    }

    /// Where settings files live (default: the working directory).
    pub fn settings_root(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.settings_root = path.into();
        self
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// Set `running` false — the pump exits after the current frame (gold's
    /// quit semantics: the frame in flight still completes).
    pub fn stop(&mut self) {
        self.running = false;
    }

    /// The settings file path: `{root}/{company}/{gameName}/settings.json`.
    pub fn settings_path(&self) -> PathBuf {
        self.settings_root
            .join(&self.company)
            .join(&self.game_name)
            .join("settings.json")
    }

    /// Save settings as JSON (gold's `saveSettings`).
    ///
    /// # Errors
    /// Filesystem trouble along the settings path.
    pub fn save_settings(&self, settings: &Var) -> std::io::Result<()> {
        let path = self.settings_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(settings)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, bytes)
    }

    /// Load settings (gold's `loadSettings`); `Null` when absent.
    ///
    /// # Errors
    /// Filesystem trouble, or unparsable JSON.
    pub fn load_settings(&self) -> std::io::Result<Var> {
        match std::fs::read(self.settings_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Var::Null),
            Err(e) => Err(e),
        }
    }

    /// The pump (gold's `start`): creates the named window backend and runs
    /// frames until a `quit` event or [`stop`](Self::stop). Blocks.
    ///
    /// # Panics
    /// On an unknown window backend name (the registry's job to report
    /// earlier) — prefer creating the backend yourself with
    /// [`create_window_backend`] and calling [`start_with`](Self::start_with).
    pub fn start(&mut self, backend_name: &str, config: &Var) {
        let window = create_window_backend(backend_name, config)
            .unwrap_or_else(|e| panic!("cannot start engine: {e}"));
        self.start_with(window);
    }

    /// The pump with a window you made yourself — the headless path.
    pub fn start_with(&mut self, mut window: Box<dyn WindowSystem>) {
        self.running = true;
        let frame_interval = Duration::from_secs_f64(self.frame_time / 1000.0);
        let mut last = Instant::now();

        while self.running {
            // events: quit stops the loop after this frame (gold's semantics)
            for event in window.poll() {
                if event.get("type").as_str() == Some("quit") {
                    self.running = false;
                }
                window.handle_event(&event);
            }

            // gold's frame: gfx.preFrame before the update pass
            if let Some(renderer) = &mut self.renderer {
                renderer.pre_frame();
            }

            let dt = self
                .fixed_dt
                .unwrap_or_else(|| last.elapsed().as_secs_f64());
            self.update(dt);
            self.draw();
            self.render();

            if self.frame_time > 0.0 {
                let elapsed = last.elapsed();
                if elapsed < frame_interval {
                    std::thread::sleep(frame_interval - elapsed);
                }
            }
            last = Instant::now();
        }
    }

    /// One frame's update pass: the physics world steps and its body poses
    /// sync back into entity transforms (gold's step-and-sync), then
    /// components sort by priority (stable, every frame — priorities can
    /// change), initialize newcomers, and update with the frame's delta
    /// time. Public so tests and headless code can drive frames without a
    /// window.
    pub fn update(&mut self, dt: f64) {
        self.physics_step_and_sync(dt as f32);
        for (_, path) in self.update_order() {
            let slot = self.slot_at_mut(&path);
            if !slot.initialized {
                slot.component.initialize();
                slot.initialized = true;
            }
            slot.component.update(dt);
        }
    }

    /// One frame's draw pass: the scene opens with the camera and every
    /// light component gathered at its entity's WORLD transform (gold's
    /// draw passes the lights list), every renderable component
    /// contributes its geometry at its entity's WORLD transform (composed
    /// down the parent chain), then non-mesh draw-tier components get
    /// their `draw()`. Public so tests and headless code can drive frames
    /// without a window.
    pub fn draw(&mut self) {
        let order = self.update_order();
        let Self {
            renderer,
            entities,
            camera,
            ..
        } = self;

        // the scene's lights: light components at their entity's world
        // transform (gold gathers these per draw call; the engine gathers
        // once for the frame)
        let lights: Vec<SceneLight> = order
            .iter()
            .filter_map(|(_, path)| {
                let owner_path = &path[..path.len() - 1];
                let world = world_transform_at(entities, owner_path);
                let [first, rest @ ..] = owner_path else {
                    return None;
                };
                let entity = walk(entities, *first, rest)?;
                let slot = entity.slot_at(&[path[path.len() - 1]])?;
                let light = slot.component.as_light()?;
                Some(SceneLight::from_component(light, &world))
            })
            .collect();

        if let Some(renderer) = renderer.as_mut() {
            renderer.begin_scene(camera, &lights);
        }

        for (priority, path) in order {
            let is_draw_tier = priority == Priority::Draw.value();
            let owner_path = &path[..path.len() - 1];

            // world transform of the owning entity (composed down the chain)
            let world = world_transform_at(entities, owner_path);
            let last = *path.last().unwrap();
            let mut geometry = None;
            {
                let entity = walk_mut(entities, *owner_path.first().unwrap(), &owner_path[1..]);
                let Some(entity) = entity else { continue };
                let slot = entity.slot_at_mut(&[last]);
                if let Some(renderable) = slot.component.as_renderable() {
                    geometry = Some(renderable.geometry(&world));
                } else if is_draw_tier {
                    slot.component.draw();
                }
            }

            if let (Some(geometry), Some(renderer)) = (geometry, renderer.as_mut()) {
                match geometry {
                    super::render::Geometry::Quad(quad) => renderer.push_quad(quad),
                    super::render::Geometry::Model { model, color } => {
                        renderer.push_model(&world, &model, color)
                    }
                }
            }
        }
    }

    /// One frame's render: the backend draws the scene [`draw`](Self::draw)
    /// collected — offscreen or into a window — and publishes it (gold's
    /// `gfx.renderFrame`). Public with [`update`](Self::update) and
    /// [`draw`](Self::draw) so custom pumps can script the frame's phases.
    pub fn render(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            renderer.render_frame();
        }
    }

    /// The slot at a tree path.
    fn slot_at_mut(&mut self, path: &[usize]) -> &mut super::entity::ComponentSlot {
        let [entity_idx, rest @ ..] = path else {
            unreachable!("non-empty slot paths only")
        };
        self.entities[*entity_idx].slot_at_mut(rest)
    }

    /// All slot paths in the tree, unsorted (physics walks these).
    fn all_slot_paths(&self) -> Vec<SlotPath> {
        let mut order: Vec<(i64, SlotPath)> = Vec::new();
        let mut path = Vec::new();
        for (entity_idx, entity) in self.entities.iter().enumerate() {
            path.push(entity_idx);
            entity.collect_order(&mut path, &mut order);
            path.pop();
        }
        order.into_iter().map(|(_, p)| p).collect()
    }

    /// gold's step-and-sync: the physics world advances, then every
    /// materialized body's pose writes back into its entity's transform.
    /// Under the `none` backend (or with no world), transforms stay under
    /// script control — the sync finds nothing to write.
    fn physics_step_and_sync(&mut self, dt: f32) {
        if self.physics.is_none() {
            return;
        }
        let paths = self.all_slot_paths();
        let Self {
            physics, entities, ..
        } = self;
        let Some(physics) = physics else { return };

        // materialize bodies that joined since the last frame — gold's
        // physicsBody spawns at its ENTITY's transform
        for path in &paths {
            let entity_path = &path[..path.len() - 1];
            let [first, rest @ ..] = entity_path else {
                continue;
            };
            let Some(entity) = walk_mut(entities, *first, rest) else {
                continue;
            };
            let last = *path.last().unwrap();
            let pose = *entity.transform();
            let slot = entity.slot_at_mut(&[last]);
            let Some(body) = slot.component.as_any_mut().downcast_mut::<PhysicsBody>() else {
                continue;
            };
            if !body.materialized {
                physics.add_body(
                    &body.name,
                    &body.shape,
                    body.body_type,
                    pose.position,
                    pose.rotation,
                    body.gravity_scale,
                );
                body.materialized = true;
            }
        }

        physics.step(dt);

        // sync poses → entity transforms
        for path in &paths {
            let name = {
                let slot = entities[path[0]].slot_at_mut(&path[1..]);
                match slot.component.as_any().downcast_ref::<PhysicsBody>() {
                    Some(body) if body.materialized => body.name.clone(),
                    _ => continue,
                }
            };
            let Some((position, rotation)) = physics.body_pose(&name) else {
                continue;
            };
            let entity_path = &path[..path.len() - 1];
            let [first, rest @ ..] = entity_path else {
                continue;
            };
            let Some(entity) = walk_mut(entities, *first, rest) else {
                continue;
            };
            entity.transform_mut().position = position;
            entity.transform_mut().rotation = rotation;
        }
    }

    /// Flat slot paths over the whole tree, in priority order — gold's
    /// `sortComponents` across the flat component list.
    fn update_order(&self) -> Vec<(i64, SlotPath)> {
        let mut order: Vec<(i64, SlotPath)> = Vec::new();
        let mut path = Vec::new();
        for (entity_idx, entity) in self.entities.iter().enumerate() {
            path.push(entity_idx);
            entity.collect_order(&mut path, &mut order);
            path.pop();
        }
        order.sort_by_key(|(p, _)| *p);
        order
    }
}

/// Walk an entity tree immutably: top-level index, then child indices.
fn walk<'a>(entities: &'a [Entity], top: usize, rest: &[usize]) -> Option<&'a Entity> {
    let mut entity = entities.get(top)?;
    for idx in rest {
        entity = entity.children().get(*idx)?;
    }
    Some(entity)
}

/// Walk an entity tree: top-level index, then child indices.
fn walk_mut<'a>(entities: &'a mut [Entity], top: usize, rest: &[usize]) -> Option<&'a mut Entity> {
    let mut entity = entities.get_mut(top)?;
    for idx in rest {
        entity = entity.child_mut(*idx)?;
    }
    Some(entity)
}

/// The world transform of an entity at an owner path (composed down the
/// parent chain — gold's transform hierarchy).
fn world_transform_at(entities: &[Entity], owner_path: &[usize]) -> Transform {
    let Some((first, rest)) = owner_path.split_first() else {
        return Transform::default();
    };
    let Some(mut entity) = entities.get(*first) else {
        return Transform::default();
    };
    let mut world = *entity.transform();
    for idx in rest {
        let Some(child) = entity.children().get(*idx) else {
            break;
        };
        world = child.transform().compose(&world);
        entity = child;
    }
    world
}
