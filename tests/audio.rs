//! Audio tests (behind the `audio` feature): the none recorder's contract,
//! gold-style `PlaySpec` config objects, tone synthesis + the stdlib WAV
//! encoder decoded by rodio's REAL Decoder (no device involved), and the
//! rodio backend skipped gracefully when the machine has no output device.

#![cfg(feature = "audio")]

use hydrargyros::game::audio::{
    create_audio_backend, AudioError, AudioSource, AudioWorld, PlaySpec, WaveShape,
    TONE_SAMPLE_RATE,
};
use hydrargyros::game::Engine;
use hydrargyros::{o, Var};

/// The none backend records plays faithfully: distinct ids, the source and
/// spec round-trip, unknown-id stops are clean no-ops, master volume is
/// recorded (clamped to gold's [0, 1] `setVolume` contract).
#[test]
fn none_backend_records_plays_faithfully() {
    let mut world = AudioWorld::none();
    assert_eq!(world.backend_name(), "none");

    let tone = AudioSource::Tone {
        frequency: 440.0,
        duration: 0.25,
        shape: WaveShape::Sine,
    };
    let file = AudioSource::File("sfx/does-not-exist.wav".to_string());

    let id1 = world.play(&tone, &PlaySpec::default()).unwrap();
    let id2 = world
        .play(
            &file,
            &PlaySpec::from_var(&Var::from(o! { "volume": 0.5, "loop": true })),
        )
        .unwrap();
    assert_ne!(id1, id2, "plays get distinct ids");

    let plays = world.none_plays().unwrap();
    assert_eq!(plays.len(), 2);
    assert_eq!(plays[0].id, id1);
    assert_eq!(plays[0].source, tone);
    assert_eq!(plays[0].volume, 1.0);
    assert!(!plays[0].looping);
    assert_eq!(plays[1].id, id2);
    assert_eq!(plays[1].source, file);
    assert_eq!(plays[1].volume, 0.5);
    assert!(plays[1].looping);

    // explicit spec, same faithfulness
    let id3 = world.play(&tone, &PlaySpec::new(0.25, true)).unwrap();
    let plays = world.none_plays().unwrap();
    assert_eq!(plays[2].id, id3);
    assert_eq!(plays[2].volume, 0.25);
    assert!(plays[2].looping);

    // stop of unknown ids: a clean no-op (see AudioBackend::stop doc)
    world.stop(9999);
    world.stop(0);
    assert_eq!(world.none_plays().unwrap().len(), 3, "records are a log");

    // master volume recorded, clamped into [0, 1]
    world.set_master_volume(0.7);
    assert_eq!(world.none_master_volume(), Some(0.7));
    world.set_master_volume(5.0);
    assert_eq!(world.none_master_volume(), Some(1.0));
    world.set_master_volume(-1.0);
    assert_eq!(world.none_master_volume(), Some(0.0));
}

/// gold-style config objects: missing keys keep the defaults (gold's
/// prototype-defaults voice), unknown keys are ignored, values clamp.
#[test]
fn play_specs_read_gold_style_vars() {
    // defaults survive an empty object and Null
    assert_eq!(PlaySpec::from_var(&Var::from(o! {})), PlaySpec::default());
    assert_eq!(PlaySpec::from_var(&Var::Null), PlaySpec::default());

    // the documented keys
    let spec = PlaySpec::from_var(&Var::from(o! { "volume": 0.5, "loop": true }));
    assert_eq!(spec.volume, 0.5);
    assert!(spec.looping);

    // unknown keys ignored — a stray "pan" must not fail a play
    let spec = PlaySpec::from_var(&Var::from(o! { "volume": 0.5, "pan": 0.25 }));
    assert_eq!(spec.volume, 0.5);
    assert!(!spec.looping);

    // clamping
    let spec = PlaySpec::from_var(&Var::from(o! { "volume": 42.0 }));
    assert_eq!(spec.volume, 1.0);
    let spec = PlaySpec::from_var(&Var::from(o! { "volume": -3.0 }));
    assert_eq!(spec.volume, 0.0);
}

/// The registry resolves "none"; unknown names fail with a Source error
/// naming the backend; the free create function exists for trait-object
/// users (physics's create_physics_backend parity).
#[test]
fn registry_resolves_and_reports_unknown_names() {
    let world = AudioWorld::create("none", &Var::Null).unwrap();
    assert_eq!(world.backend_name(), "none");
    // the registry hands the records handle over — the seam survives
    // creation through `create`, not just `none()`
    assert!(world.none_plays().is_some());

    let Err(err) = AudioWorld::create("definitely-not-a-backend", &Var::Null) else {
        panic!("an unknown backend name must not resolve");
    };
    match err {
        AudioError::Source(msg) => {
            assert!(msg.contains("unknown audio backend"), "{msg}");
        }
        e => panic!("expected a Source error, got {e:?}"),
    }

    let backend = create_audio_backend("none", &Var::Null).unwrap();
    assert_eq!(backend.name(), "none");
}

/// The engine carries the audio world like its physics world.
#[test]
fn engine_carries_the_audio_world() {
    let mut engine = Engine::new("acme", "audio-test");
    assert!(engine.audio_mut().is_none());

    engine.set_audio(AudioWorld::none());
    let audio = engine.audio_mut().unwrap();
    audio
        .play(
            &AudioSource::Tone {
                frequency: 220.0,
                duration: 0.5,
                shape: WaveShape::Square,
            },
            &PlaySpec::default(),
        )
        .unwrap();
    assert_eq!(
        engine.audio_mut().unwrap().none_plays().unwrap().len(),
        1,
        "plays through the engine-held world land in the records"
    );
}

/// Tone synthesis: deterministic (same tone → byte-identical samples),
/// exact shape values, and file sources synthesize nothing.
#[test]
fn tone_shapes_are_deterministic_and_exact() {
    let peak = (0.8f32 * i16::MAX as f32) as i16; // 26213

    let sine = AudioSource::Tone {
        frequency: 1.0,
        duration: 1.0,
        shape: WaveShape::Sine,
    };
    let samples = sine.tone_samples().unwrap();
    assert_eq!(sine.tone_samples().unwrap(), samples, "deterministic");
    assert_eq!(samples.len(), TONE_SAMPLE_RATE as usize);
    assert_eq!(samples[0], 0, "sin(0) is silence");
    // quarter period: sin(π/2) → peak (i16 truncation of 0.8·32767)
    assert!(samples[TONE_SAMPLE_RATE as usize / 4] > 26000);
    assert!(samples.iter().all(|s| s.abs() <= peak), "within the peak");

    // square: full-amplitude flips, first half-period up, second down
    let square = AudioSource::Tone {
        frequency: 1.0,
        duration: 1.0,
        shape: WaveShape::Square,
    };
    let samples = square.tone_samples().unwrap();
    assert_eq!(samples[100], peak);
    assert_eq!(samples[30000], -peak);
    assert!(samples.iter().all(|&s| s == peak || s == -peak));

    // noise: fixed-seed xorshift — deterministic and varied
    let noise = AudioSource::Tone {
        frequency: 0.0, // irrelevant to noise
        duration: 0.1,
        shape: WaveShape::Noise,
    };
    let samples = noise.tone_samples().unwrap();
    assert_eq!(noise.tone_samples().unwrap(), samples, "deterministic");
    assert!(samples.iter().any(|&s| s != samples[0]), "actually varies");
    assert!(samples.iter().all(|s| s.abs() <= peak));

    // file sources synthesize nothing (their samples live on disk)
    let file = AudioSource::File("music/track.ogg".to_string());
    assert!(file.tone_samples().is_none());
    assert!(file.tone_wav().is_none());
}

/// The stdlib WAV encoder produces bytes rodio's REAL Decoder eats — no
/// device involved, a full decode proof of the tone path (44.1 kHz mono i16
/// container, correct length and shape).
#[test]
fn tone_wav_decodes_with_rodio() {
    let tone = AudioSource::Tone {
        frequency: 440.0,
        duration: 1.0,
        shape: WaveShape::Sine,
    };
    let wav = tone.tone_wav().unwrap();
    // 44-byte canonical header + 1s of mono i16
    assert_eq!(wav.len(), 44 + TONE_SAMPLE_RATE as usize * 2);
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");

    let decoded = rodio::Decoder::new_wav(std::io::Cursor::new(wav)).unwrap();
    let samples: Vec<f32> = decoded.collect();
    assert_eq!(samples.len(), TONE_SAMPLE_RATE as usize, "1s mono");
    assert!(samples[0].abs() < 1e-6, "the sine starts at silence");
    assert!(samples.iter().all(|s| s.abs() <= 1.0), "i16 → f32 range");

    // the square's ±peak flips survive the decode
    let square = AudioSource::Tone {
        frequency: 10.0,
        duration: 1.0,
        shape: WaveShape::Square,
    };
    let decoded =
        rodio::Decoder::new_wav(std::io::Cursor::new(square.tone_wav().unwrap())).unwrap();
    let samples: Vec<f32> = decoded.collect();
    assert_eq!(samples.len(), TONE_SAMPLE_RATE as usize);
    let high = samples.iter().filter(|s| **s > 0.5).count();
    let low = samples.iter().filter(|s| **s < -0.5).count();
    assert!(
        high > 0 && low > 0,
        "flips survive: {high} high / {low} low"
    );
}

/// The rodio backend is device-dependent: on a machine WITH an output
/// device it plays a quiet tone and stops it; without one, `create("rodio")`
/// fails with a clear Device error (headless-safe) and this test says why
/// instead of failing.
#[test]
fn rodio_backend_plays_or_skips_without_a_device() {
    match AudioWorld::create("rodio", &Var::Null) {
        Ok(mut world) => {
            assert_eq!(world.backend_name(), "rodio");
            let id = world
                .play(
                    &AudioSource::Tone {
                        frequency: 440.0,
                        duration: 0.1,
                        shape: WaveShape::Sine,
                    },
                    &PlaySpec::new(0.01, false),
                )
                .expect("a tone decodes and appends");
            world.stop(id);
            world.stop(987654); // unknown id: clean no-op
            world.set_master_volume(0.5);
        }
        Err(AudioError::Device(msg)) => {
            println!("no audio output device on this machine — rodio backend skipped: {msg}");
        }
        Err(e) => panic!("rodio create should fail with a Device error, got {e:?}"),
    }
}
