//! A headless game: the pump + event dispatch, no window or GPU — components
//! update in priority order, events cross as `Var`, and the entity config
//! is a `FromVar`-derived struct (the typed hot-path seam).
//!
//! Run:
//!   cargo run --example headless_game

use hyg::game::component::{Component, Priority};
use hyg::game::{Engine, Entity, HeadlessWindow};
use hyg::{o, FromVar, Var};

// ---- the typed config seam: Var in the prototype, structs in the loop ----

#[derive(FromVar, Debug)]
struct PlayerConfig {
    name: String,
    speed: f64,
    #[from_var(default)]
    start_x: f64,
}

struct Player {
    config: PlayerConfig,
    x: f64,
    frames: u64,
}

impl Player {
    fn from_config(config: &Var) -> Result<Self, hyg::FromVarError> {
        Ok(Player { config: PlayerConfig::from_var(config)?, x: 0.0, frames: 0 })
    }
}

impl Component for Player {
    fn priority(&self) -> Priority { Priority::Generic }
    fn initialize(&mut self) {
        println!("  player {:?} spawns at x={}", self.config.name, self.config.start_x);
    }
    fn update(&mut self, dt: f64) {
        self.x += self.config.speed * dt;
        self.frames += 1;
        println!("  {:?} at x={:.2} (dt={dt})", self.config.name, self.x);
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

// a draw-priority component: runs after all updates, like renderables will
struct Hud {
    player_name: String,
}

impl Component for Hud {
    fn priority(&self) -> Priority { Priority::Draw }
    fn draw(&mut self) {
        println!("  [hud] drawing frame for {:?}", self.player_name);
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn main() {
    // entity definitions arrive as Var — the script/config seam
    let player_config = Var::from(o!{
        "name": "rose",
        "speed": 2.5,
        "start_x": 10.0,
    });

    let player = match Player::from_config(&player_config) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("bad player config: {e}");
            std::process::exit(1);
        }
    };

    let mut engine = Engine::new("acme", "headless-demo");
    engine.frame_time(0.0); // headless: no vsync cap

    let mut entity = Entity::new("player");
    entity.add_component(Box::new(player));
    entity.add_component(Box::new(Hud { player_name: "rose".to_string() }));

    let mut child = Entity::new("companion");
    child.add_component(Box::new(Player {
        config: PlayerConfig::from_var(&Var::from(o!{ "name": "cube", "speed": 1.0 })).unwrap(),
        x: 0.0,
        frames: 0,
    }));
    entity.add_child(child);

    engine.add_entity(entity);

    // scripted window: two frames of play, then the automatic quit
    println!("pumping 2 headless frames:");
    engine.start_with(Box::new(HeadlessWindow::with_events([
        Var::from(o!{ "type": "key", "key": "d" }),
    ])));

    // settings ride the same seam
    engine
        .settings_root(std::env::temp_dir())
        .save_settings(&Var::from(o!{ "last_player": "rose", "frames_played": 2 }))
        .unwrap();
    let loaded = engine.load_settings().unwrap();
    println!(
        "settings: last_player={:?} frames={}",
        loaded.get("last_player").as_str(),
        loaded.get("frames_played").as_i64().unwrap_or(0)
    );
}
