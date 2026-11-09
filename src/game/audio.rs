//! Audio — gold's `audioSystem` split: an [`AudioBackend`] trait with
//! register/create registries (gold's `registerAudioSystem`/`createAudioSystem`
//! factory map under a mutex), and the real device behind the `audio`
//! feature: **rodio** (cpal underneath) — the wheel gold loads SDL for (its
//! SDL2/SDL3 `audioSystem` adapters ship as plugins).
//!
//! gold's `audioSystem` (include/game/audioSystem.hpp): backends registered
//! by name, `loadSound(path)` → handle, `play(handle, loop)`, `stop()`
//! (everything), `setVolume` master [0, 1], `name()`. The port mirrors the
//! split with deliberate divergences:
//!
//! - `play` takes the [`AudioSource`] directly and returns a **per-play
//!   id** — gold's load-then-play two-step collapses into one call, and
//!   [`AudioBackend::stop`] names ONE sound where gold's `stop()` stops
//!   everything.
//! - gold's registry has **no** no-op fallback (`createAudioSystem` returns
//!   nullptr on a miss); the port ships the headless-first `none` recorder
//!   backend — the physics pattern — so the facade's contracts test without
//!   a device (its play records are introspectable through
//!   [`AudioWorld::none_plays`]).
//! - [`AudioSource::Tone`] synthesizes PCM in-memory (44.1 kHz mono i16,
//!   stdlib WAV encoder below) — sounds testable with zero assets.
//!
//! The [`AudioWorld`] facade is what components and tests talk to — the
//! engine carries one like its physics world (`set_audio`/`audio_mut`).
//!
//! ```
//! use hyg::game::audio::{AudioSource, AudioWorld, PlaySpec, WaveShape};
//! use hyg::{o, Var};
//!
//! // headless: the none backend records plays instead of sounding
//! let mut audio = AudioWorld::none();
//! let id = audio
//!     .play(
//!         &AudioSource::Tone { frequency: 440.0, duration: 0.25, shape: WaveShape::Sine },
//!         &PlaySpec::from_var(&Var::from(o!{ "volume": 0.5, "loop": true })),
//!     )
//!     .unwrap();
//! let plays = audio.none_plays().unwrap();
//! assert_eq!(plays.len(), 1);
//! assert_eq!(plays[0].id, id);
//! assert_eq!(plays[0].volume, 0.5);
//! assert!(plays[0].looping);
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::var::Var;

/// Why an audio call failed — path-style messages, the way gold's
/// `loadSound` reports 0-and-move-on is replaced by reporting WHY.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioError {
    /// A source could not be loaded or decoded: the message carries the
    /// file path (or the tone that failed to synthesize). Registry misses
    /// report here too — the requested backend name is the bad "path".
    Source(String),
    /// The audio device could not be opened (headless machines, no output
    /// device) or failed mid-session.
    Device(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::Source(msg) => write!(f, "audio source error: {msg}"),
            AudioError::Device(msg) => write!(f, "audio device error: {msg}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// What to play: a file path (the backend decodes it — rodio sniffs WAV/
/// OGG/MP3/FLAC; the none backend records the string) or a generated tone
/// (PCM synthesized in-memory at [`TONE_SAMPLE_RATE`], mono i16, wrapped in
/// a stdlib WAV container by [`AudioSource::tone_wav`]).
#[derive(Debug, Clone, PartialEq)]
pub enum AudioSource {
    /// Path to an audio file.
    File(String),
    /// A synthesized tone: `frequency` Hz for `duration` seconds.
    Tone {
        frequency: f32,
        duration: f32,
        shape: WaveShape,
    },
}

/// The tone's waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveShape {
    /// A pure sine.
    Sine,
    /// Sign-of-sine — alternating full-amplitude half-periods.
    Square,
    /// Fixed-seed xorshift white noise — the same tone generates
    /// byte-identical samples (deterministic headless tests).
    Noise,
}

/// Tones synthesize at 44.1 kHz, mono, i16 — the WAV container below.
pub const TONE_SAMPLE_RATE: u32 = 44_100;
/// Peak amplitude as a fraction of full scale (headroom against clipping).
const TONE_AMPLITUDE: f32 = 0.8;

impl AudioSource {
    /// The tone's PCM samples (44.1 kHz mono i16) — stdlib math only.
    /// `None` for file sources (their samples live on disk; backends read
    /// the path themselves).
    #[must_use]
    pub fn tone_samples(&self) -> Option<Vec<i16>> {
        match self {
            AudioSource::File(_) => None,
            AudioSource::Tone {
                frequency,
                duration,
                shape,
            } => Some(tone_samples(*frequency, *duration, *shape)),
        }
    }

    /// The tone wrapped in a WAV container — the bytes the rodio backend
    /// decodes for a tone play. `None` for file sources.
    #[must_use]
    pub fn tone_wav(&self) -> Option<Vec<u8>> {
        self.tone_samples().map(|samples| wav_bytes(&samples))
    }
}

/// Synthesize a tone: `frequency` Hz for `duration` seconds (negative
/// durations produce silence — samples count clamps at zero).
fn tone_samples(frequency: f32, duration: f32, shape: WaveShape) -> Vec<i16> {
    let count = (duration.max(0.0) * TONE_SAMPLE_RATE as f32).round() as usize;
    let peak = TONE_AMPLITUDE * i16::MAX as f32;
    match shape {
        WaveShape::Sine => (0..count)
            .map(|i| {
                let t = i as f32 / TONE_SAMPLE_RATE as f32;
                (peak * (2.0 * std::f32::consts::PI * frequency * t).sin()) as i16
            })
            .collect(),
        WaveShape::Square => {
            // sign of the sine, full amplitude — crisp for byte-level tests
            (0..count)
                .map(|i| {
                    let t = i as f32 / TONE_SAMPLE_RATE as f32;
                    if (2.0 * std::f32::consts::PI * frequency * t).sin() >= 0.0 {
                        peak as i16
                    } else {
                        (-peak) as i16
                    }
                })
                .collect()
        }
        WaveShape::Noise => {
            // xorshift64*, fixed seed — deterministic "random"
            let mut state: u64 = 0x2545_F491_4F6C_DD1D;
            (0..count)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let unit = (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
                    (peak * unit as f32) as i16
                })
                .collect()
        }
    }
}

/// The WAV container: a canonical 44-byte PCM header + little-endian i16
/// data. ~20 lines of stdlib — no wav crate (rodio's `wav_output` wheel
/// writes device output to files, not in-memory bytes; the header shape is
/// the same one gold's SDL adapters decode).
fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    // fmt chunk: PCM, 1 channel, i16
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&TONE_SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(TONE_SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// How a source plays — gold's `play(handle, loop)` plus a per-play volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaySpec {
    /// Per-play gain, 0..=1 (1 = the source at full volume). Values built
    /// through [`PlaySpec::new`]/[`PlaySpec::from_var`] clamp into the
    /// range; a struct-literal spec passes through and backends guard
    /// negatives (a negative gain would invert the phase).
    pub volume: f32,
    /// Loop the source until [`AudioBackend::stop`].
    pub looping: bool,
}

impl Default for PlaySpec {
    /// gold's `play(handle, loop)` defaults: full volume, no loop.
    fn default() -> Self {
        PlaySpec {
            volume: 1.0,
            looping: false,
        }
    }
}

impl PlaySpec {
    /// A spec with explicit values (volume clamped into 0..=1).
    pub fn new(volume: f32, looping: bool) -> PlaySpec {
        PlaySpec {
            volume: volume.clamp(0.0, 1.0),
            looping,
        }
    }

    /// gold-style config object — `o!{ "volume": 0.5, "loop": true }`.
    /// Missing keys keep the defaults (gold's prototype-defaults voice);
    /// unknown keys are IGNORED, on purpose: the spec reads only the knobs
    /// it owns, and gold's prototype objects carry unrelated fields all the
    /// time — a stray `"pan"` should not fail a play. The script seam can
    /// pass these objects through later.
    #[must_use]
    pub fn from_var(var: &Var) -> PlaySpec {
        let mut spec = PlaySpec::default();
        if let Some(volume) = var.get("volume").as_f64() {
            spec.volume = (volume as f32).clamp(0.0, 1.0);
        }
        if let Some(looping) = var.get("loop").as_bool() {
            spec.looping = looping;
        }
        spec
    }
}

/// One audio device behind the facade — gold's `audioSystem` (backends
/// registered by name, created through the registry below). One trait
/// object per world — gold's `audioSystem*` flattened into the natural
/// Rust shape.
pub trait AudioBackend: Send {
    /// Backend name, e.g. "none" / "rodio" (gold's `name()`).
    fn name(&self) -> &str;

    /// Play a source now; returns the backend's play id (gold's
    /// loadSound→play two-step, collapsed — see the module doc).
    ///
    /// # Errors
    /// [`AudioError::Source`] when the source can't be loaded or decoded,
    /// [`AudioError::Device`] when the device fails mid-session.
    fn play(&mut self, source: &AudioSource, spec: &PlaySpec) -> Result<u64, AudioError>;

    /// Stop one play by id. Unknown ids are a CLEAN NO-OP — stopping an
    /// already-finished sound is routine, not a protocol violation (gold's
    /// `stop()` never errors; it just stops everything).
    fn stop(&mut self, id: u64);

    /// Master volume in [0, 1] (gold's `setVolume` contract; values clamp
    /// into the range). Scales every live play — per-play volume × master.
    fn set_master_volume(&mut self, volume: f32);
}

/// What a backend factory hands the facade: the backend, plus — for the
/// built-in `none` recorder — the shared records handle the facade's
/// introspection seam ([`AudioWorld::none_plays`]) snapshots through.
/// Keeping it OUT of the trait keeps the trait clean: no `as_any` hack, and
/// custom factories just hand back `none_state: None`.
pub struct CreatedBackend {
    /// The backend itself.
    pub backend: Box<dyn AudioBackend>,
    /// The none recorder's shared records, when the backend is one.
    pub none_state: Option<Arc<Mutex<NoneState>>>,
}

/// Factory shape for the backend registry: config `Var` in, backend out
/// (gold's `audioSystem* (*)()` factory pointers, as Rust closures).
pub type AudioFactory = Arc<dyn Fn(&Var) -> Result<CreatedBackend, AudioError> + Send + Sync>;
type BackendMap = HashMap<String, AudioFactory>;

fn backends() -> &'static RwLock<BackendMap> {
    static BACKENDS: OnceLock<RwLock<BackendMap>> = OnceLock::new();
    BACKENDS.get_or_init(|| {
        let mut map: BackendMap = HashMap::new();
        map.insert(
            "none".to_string(),
            Arc::new(|_config| {
                let backend = NoneAudioBackend::new();
                let none_state = Some(Arc::clone(&backend.state));
                Ok(CreatedBackend {
                    backend: Box::new(backend),
                    none_state,
                })
            }),
        );
        map.insert(
            "rodio".to_string(),
            Arc::new(|_config| {
                Ok(CreatedBackend {
                    backend: Box::new(RodioBackend::new()?),
                    none_state: None,
                })
            }),
        );
        RwLock::new(map)
    })
}

/// Register an audio backend under a name (gold's plugin registration).
pub fn register_audio_backend(name: &str, factory: AudioFactory) {
    backends()
        .write()
        .unwrap()
        .insert(name.to_string(), factory);
}

/// Create an audio backend by name.
///
/// # Errors
/// Unknown backend name, or the backend's factory failure (e.g. the rodio
/// backend on a machine without an output device).
pub fn create_audio_backend(name: &str, config: &Var) -> Result<Box<dyn AudioBackend>, AudioError> {
    Ok(create_backend(name, config)?.backend)
}

fn create_backend(name: &str, config: &Var) -> Result<CreatedBackend, AudioError> {
    let factory = backends()
        .read()
        .unwrap()
        .get(name)
        .cloned()
        .ok_or_else(|| AudioError::Source(format!("unknown audio backend {name:?}")))?;
    factory(config)
}

/// One recorded play of the none backend — the introspection seam's
/// payload (a snapshot record: what was played, at what spec).
#[derive(Debug, Clone, PartialEq)]
pub struct NonePlay {
    /// The id [`AudioWorld::play`] returned for this record.
    pub id: u64,
    pub source: AudioSource,
    /// The spec's volume, recorded faithfully (as passed).
    pub volume: f32,
    pub looping: bool,
}

/// The none backend's shared records — the state the facade's
/// introspection seam snapshots. `NoneAudioBackend` owns one
/// `Arc<Mutex<NoneState>>` and hands a clone to the facade at construction
/// (the "none" registry entry), so records reach tests without any
/// downcast hack on the trait.
#[derive(Default)]
pub struct NoneState {
    /// Master volume, clamped [0, 1].
    pub master_volume: f32,
    /// The play log, append-only (stops are no-ops — history, not live
    /// state).
    pub plays: Vec<NonePlay>,
}

/// The built-in recording backend — the none-physics analogue: plays are
/// recorded (ids assigned from an atomic counter), nothing sounds, and
/// headless tests assert on the records instead of listening. gold's
/// audioSystem registry ships NO fallback (`createAudioSystem` returns
/// nullptr on a miss); this one is Hydrargyros's, so the facade and its
/// contracts are testable without a device.
pub struct NoneAudioBackend {
    next_id: AtomicU64,
    state: Arc<Mutex<NoneState>>,
}

impl NoneAudioBackend {
    /// A fresh recorder; ids start at 1 (0 stays "no play", gold's
    /// loadSound-failure sentinel).
    #[must_use]
    pub fn new() -> NoneAudioBackend {
        NoneAudioBackend {
            next_id: AtomicU64::new(1),
            state: Arc::new(Mutex::new(NoneState::default())),
        }
    }
}

impl Default for NoneAudioBackend {
    /// A fresh recorder (ids start at 1 — a derived Default would start at
    /// 0, the no-play sentinel).
    fn default() -> Self {
        NoneAudioBackend::new()
    }
}

impl AudioBackend for NoneAudioBackend {
    fn name(&self) -> &str {
        "none"
    }

    fn play(&mut self, source: &AudioSource, spec: &PlaySpec) -> Result<u64, AudioError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        crate::lock(&self.state).plays.push(NonePlay {
            id,
            source: source.clone(),
            volume: spec.volume,
            looping: spec.looping,
        });
        Ok(id)
    }

    /// The records are a play LOG (history, not live state) — stopping
    /// anything is a no-op; the none backend has no voices to silence.
    fn stop(&mut self, _id: u64) {}

    fn set_master_volume(&mut self, volume: f32) {
        crate::lock(&self.state).master_volume = volume.clamp(0.0, 1.0);
    }
}

/// The audio facade (the `AudioWorld` to physics's `PhysicsWorld`): play/
/// stop/master-volume over whichever backend it was created with.
pub struct AudioWorld {
    backend: Box<dyn AudioBackend>,
    /// Shared with the none backend when this world IS one — the
    /// introspection seam. The facade knows its none construction (the
    /// "none" registry entry hands the handle over), so records reach
    /// tests without any downcast hack on the trait.
    none_state: Option<Arc<Mutex<NoneState>>>,
}

impl AudioWorld {
    /// A world over the named backend ("none" always; "rodio" under the
    /// `audio` feature — which this module IS).
    ///
    /// # Errors
    /// Unknown backend name, or the backend's factory failure — on a
    /// machine without an output device, `create("rodio", …)` fails with a
    /// clear [`AudioError::Device`] (headless-safe; build a `none` world
    /// for headless runs).
    pub fn create(backend: &str, config: &Var) -> Result<AudioWorld, AudioError> {
        let created = create_backend(backend, config)?;
        Ok(AudioWorld {
            backend: created.backend,
            none_state: created.none_state,
        })
    }

    /// A world over the built-in recording backend.
    pub fn none() -> AudioWorld {
        let backend = NoneAudioBackend::new();
        let none_state = Some(Arc::clone(&backend.state));
        AudioWorld {
            backend: Box::new(backend),
            none_state,
        }
    }

    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    /// Play a source; returns the play id for [`AudioWorld::stop`].
    ///
    /// # Errors
    /// Whatever the backend reports — see [`AudioBackend::play`].
    pub fn play(&mut self, source: &AudioSource, spec: &PlaySpec) -> Result<u64, AudioError> {
        self.backend.play(source, spec)
    }

    /// Stop one play by id (unknown ids are a clean no-op).
    pub fn stop(&mut self, id: u64) {
        self.backend.stop(id);
    }

    /// Master volume in [0, 1] (gold's `setVolume`).
    pub fn set_master_volume(&mut self, volume: f32) {
        self.backend.set_master_volume(volume);
    }

    /// A snapshot of the none backend's play records — `None` unless this
    /// world is a none world ([`AudioWorld::none`] or `create("none", …)`).
    pub fn none_plays(&self) -> Option<Vec<NonePlay>> {
        let state = self.none_state.as_ref()?;
        Some(crate::lock(state).plays.clone())
    }

    /// The none backend's master volume (clamped [0, 1]) — `None` unless
    /// this world is a none world.
    pub fn none_master_volume(&self) -> Option<f32> {
        let state = self.none_state.as_ref()?;
        Some(crate::lock(state).master_volume)
    }
}

/// The real device backend: rodio 0.23 (cpal underneath) — the wheel gold
/// loads SDL for (its SDL2/SDL3 `audioSystem` adapters). One
/// `MixerDeviceSink` owns the device stream; each play gets its own
/// `Player` connected to the sink's mixer, tracked per id; master volume
/// multiplies every play's volume (per-player volume × master). Dropping
/// the backend drops the sink — playback ends with it (a rodio contract).
pub struct RodioBackend {
    sink: rodio::MixerDeviceSink,
    plays: HashMap<u64, (rodio::Player, f32)>,
    master_volume: f32,
    next_id: AtomicU64,
}

impl RodioBackend {
    /// Open the default output device.
    ///
    /// # Errors
    /// [`AudioError::Device`] when no output device can be opened —
    /// headless machines get a clear error, not a panic.
    pub fn new() -> Result<RodioBackend, AudioError> {
        // rodio 0.23: DeviceSinkBuilder::open_default_sink() →
        // MixerDeviceSink (the old OutputStream::try_default is gone); the
        // sink's mixer() is what players connect to
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|e| AudioError::Device(format!("cannot open audio output device: {e}")))?;
        sink.log_on_drop(false); // dropping at process end is routine, not worth stderr
        Ok(RodioBackend {
            sink,
            plays: HashMap::new(),
            master_volume: 1.0,
            next_id: AtomicU64::new(1),
        })
    }

    /// The decoded source for a play, boxed for [`rodio::Player::append`]:
    /// file paths decode from the file (rodio sniffs the format); tones
    /// decode from their in-memory WAV bytes. Looping wraps the decode in
    /// rodio's `repeat_infinite` (rodio 0.23 has no per-Player loop flag;
    /// the source restarts from a buffered copy).
    fn decoded(
        source: &AudioSource,
        looping: bool,
    ) -> Result<Box<dyn rodio::Source + Send>, AudioError> {
        match source {
            AudioSource::File(path) => {
                let file = std::fs::File::open(path).map_err(|e| {
                    AudioError::Source(format!("cannot open audio file {path:?}: {e}"))
                })?;
                let decoded = rodio::Decoder::new(std::io::BufReader::new(file)).map_err(|e| {
                    AudioError::Source(format!("cannot decode audio file {path:?}: {e}"))
                })?;
                Ok(Self::maybe_loop(decoded, looping))
            }
            AudioSource::Tone { .. } => {
                let wav = source.tone_wav().expect("tone sources always synthesize");
                let decoded = rodio::Decoder::new_wav(std::io::Cursor::new(wav))
                    .map_err(|e| AudioError::Source(format!("cannot decode tone: {e}")))?;
                Ok(Self::maybe_loop(decoded, looping))
            }
        }
    }

    /// Box a decode, wrapping it in rodio's infinite repeat when `looping`
    /// (`repeat_infinite` is Sized-bound, so it applies before the box).
    fn maybe_loop<S>(source: S, looping: bool) -> Box<dyn rodio::Source + Send>
    where
        S: rodio::Source + Send + 'static,
    {
        if looping {
            Box::new(source.repeat_infinite())
        } else {
            Box::new(source)
        }
    }
}

impl AudioBackend for RodioBackend {
    fn name(&self) -> &str {
        "rodio"
    }

    fn play(&mut self, source: &AudioSource, spec: &PlaySpec) -> Result<u64, AudioError> {
        let decoded = Self::decoded(source, spec.looping)?;
        let player = rodio::Player::connect_new(self.sink.mixer());
        // volume BEFORE append — the queue starts feeding the mixer the
        // moment the samples land, and the control covers the whole player
        let volume = spec.volume.max(0.0) * self.master_volume;
        player.set_volume(volume);
        player.append(decoded);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.plays.insert(id, (player, spec.volume.max(0.0)));
        Ok(id)
    }

    fn stop(&mut self, id: u64) {
        // each play owns a Player — removing it from the map drops it,
        // which stops its sounds; the explicit stop() says so
        if let Some((player, _)) = self.plays.remove(&id) {
            player.stop();
        }
    }

    fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 1.0);
        for (player, base) in self.plays.values() {
            player.set_volume(*base * self.master_volume);
        }
    }
}

// Compile-time proof the world crosses threads (the engine holds it; the
// value-graph assertion in lib.rs covers the Var side) — same shape as the
// physics world's Send backend trait.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<AudioWorld>();
    assert_send::<NoneAudioBackend>();
};
