//! The audio tour (the `audio` feature): the backend registry — rodio when
//! a device answers, the headless `none` recorder otherwise — a synthesized
//! `Tone` as real WAV bytes, per-play ids, `stop(id)`, master volume.
//! Everything here introspects, no speakers required.
//!
//! Run:
//!   cargo run --example game_audio --features audio

use hyg::game::audio::{AudioSource, AudioWorld, PlaySpec, WaveShape};
use hyg::Var;

fn main() {
    // the registry: real device first, the recorder fallback keeps every
    // contract testable on a headless machine
    let mut world = AudioWorld::create("rodio", &Var::Null)
        .or_else(|e| {
            println!("no device ({e}) — the none recorder takes over");
            AudioWorld::create("none", &Var::Null)
        })
        .expect("both backends are registered");
    println!("audio backend: {}", world.backend_name());

    // a tone is PCM synthesized in-memory — 44.1 kHz mono i16
    let tone = AudioSource::Tone {
        frequency: 440.0,
        duration: 0.25,
        shape: WaveShape::Sine,
    };
    let wav = tone.tone_wav().expect("tones synthesize");
    println!("Tone(440 Hz) -> {} WAV bytes, decodable by rodio's real Decoder", wav.len());

    // play returns a per-play id: stop(id) names ONE sound
    let spec = PlaySpec::new(0.8, false); // volume [0,1], not looping
    let id = world.play(&tone, &spec).expect("play");
    let second = world.play(&tone, &PlaySpec::new(0.4, true)).expect("play");
    world.set_master_volume(0.5);
    println!("plays -> id {id} (vol 0.8), id {second} (vol 0.4, looping); master 0.5");

    // the none recorder is the introspection seam: plays are inspectable
    if let Some(plays) = world.none_plays() {
        for p in &plays {
            println!(
                "recorded: id {} volume {} looping {}",
                p.id, p.volume, p.looping,
            );
        }
    }

    world.stop(id);
    if let Some(plays) = world.none_plays() {
        println!("after stop({id}): recorder still holds {} record(s)", plays.len());
    }
}
