//! Singer: a built-in instrument (Instrument slot > Singer) that sings its own track's notes
//! and lyrics, with DiffSinger, CHIMERA or Novox The Robot, through the track's channel strip.
//!
//! A singer needs whole phrases and DiffSinger is slower than real time, so nothing is sung
//! as it is played: the track's MIDI regions are cut into phrases as Vocal Top Line cuts them
//! (`vocal.rs`), each phrase is sung in the background whenever the notes, lyrics, tempo or
//! settings change (after a short pause), into the same per-phrase cache, and the finished
//! sounds go to the engine's [`prowave_engine::SingerPlayer`] in the slot, which plays each at
//! its place in the song as the transport passes it. A phrase still being sung is silent. Keys
//! played live on the track are not sung.
//!
//! The settings are Vocal Top Line's ([`VocalSettings`]), kept in the slot's state as JSON.
//! Unlike Vocal Top Line there is no second audio track: the singing is the instrument's sound,
//! so the track's Audio FX, inserts, sends, fader, pan and automation all apply. Bounce Vocal…
//! renders the track through its strip offline (no sound card) into a WAV file.
//!
//! Logic Pro has no equivalent; the nearest are Synthesizer V and ACE Studio in a DAW.
use crate::hw;
use crate::i18n::{tr, trf};
use crate::model::Project;
use crate::plugins::PluginSlot;
use crate::spectrogram_view::hw_vocal;
use crate::vocal::{self, Phrase, Place, Singer, VocalRole, VocalSettings};
use prowave_engine::{SingerClip, SingerRemote, SingerScore};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The slot id of the Singer.
pub const SINGER_ID: &str = "prowave.singer";
/// Frames an offline bounce processes at once.
const BOUNCE_BLOCK: usize = 512;
/// What a bounce keeps after the last phrase (effect tails).
const BOUNCE_TAIL: f64 = 2.0;

pub fn is_singer(slot: &PluginSlot) -> bool {
    slot.id == SINGER_ID
}

/// An Instrument slot with a Singer: DiffSinger when a voice is installed, else CHIMERA (which
/// needs nothing installed).
pub fn new_slot() -> PluginSlot {
    let mut slot = PluginSlot::new(SINGER_ID, "Singer");
    let mut set = VocalSettings::new(0);
    if vocal::voices().is_empty() {
        set.singer = Singer::Chimera;
    }
    slot.state = Some(encode(&set));
    slot
}

/// The settings as the slot keeps them.
pub fn encode(set: &VocalSettings) -> Vec<u8> {
    serde_json::to_vec(&vocal::to_doc(&VocalRole::Score(set.clone()))).unwrap_or_default()
}

/// The slot's settings (the defaults when it has none it can read).
pub fn settings(slot: &PluginSlot) -> VocalSettings {
    slot.state
        .as_deref()
        .and_then(|b| serde_json::from_slice::<prowave_project::VocalDoc>(b).ok())
        .and_then(|d| match vocal::from_doc(&d) {
            Some(VocalRole::Score(s)) => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| VocalSettings::new(0))
}

/// FNV-1a over 64-bit words.
fn hash(words: impl IntoIterator<Item = u64>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for w in words {
        for b in w.to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Names a set of phrases where they are: their sounds and their places, at `rate`.
fn phrases_key<'a>(phrases: impl IntoIterator<Item = &'a Phrase>, rate: u32) -> u64 {
    hash(
        phrases
            .into_iter()
            .flat_map(|ph| [ph.key, ph.origin.to_bits()])
            .chain([u64::from(rate)]),
    )
}

/// Where DiffSinger is tried, in order.
fn order(_: Place) -> Vec<Place> {
    vec![Place::ThisMac]
}

/// What a background job made.
struct Made {
    /// The phrases' sounds at `rate` (none when no engine was running to say the rate).
    sounds: Vec<(u64, Arc<[f32]>)>,
    rate: Option<u32>,
    /// Phrases sung (the rest came from the cache), how long that took, and the seconds of
    /// singing they hold.
    sung: usize,
    secs: f64,
    sung_secs: f64,
    place: Option<Place>,
    singer: Singer,
}

/// Sings the phrases of `todo` that are not in the cache, then reads every one of them back at
/// `rate` (the engine's).
fn make(
    set: &VocalSettings,
    todo: &[Phrase],
    rate: Option<u32>,
    order: &[Place],
    progress: &Arc<Mutex<String>>,
) -> Result<Made, String> {
    let started = Instant::now();
    let mut sing: Vec<&Phrase> = Vec::new();
    for ph in todo {
        if !vocal::cached(ph.key).is_file() && !sing.iter().any(|x| x.key == ph.key) {
            sing.push(ph);
        }
    }
    let sung_secs = sing
        .iter()
        .map(|ph| ph.notes.last().map_or(0.0, |n| n.start + n.dur))
        .sum();
    let mut place = None;
    if !sing.is_empty() && set.singer.in_process() {
        vocal::sing_in_process(set, &sing, progress)?;
        place = Some(Place::ThisMac);
    } else if !sing.is_empty() {
        let mut errors = Vec::new();
        for at in order {
            match vocal::sing(set, &sing, *at, progress) {
                Ok(()) => {
                    place = Some(*at);
                    break;
                }
                Err(e) => errors.push(format!("{}: {e}", at.label())),
            }
        }
        if place.is_none() {
            return Err(errors.join("; "));
        }
    }
    let secs = started.elapsed().as_secs_f64();
    let mut sounds = Vec::new();
    if let Some(rate) = rate {
        for ph in todo {
            if sounds.iter().any(|(k, _)| *k == ph.key) {
                continue;
            }
            let (x, from) = vocal::read_wav(&vocal::cached(ph.key))?;
            let y = crate::chimera_singer::resample(&x, from, rate);
            sounds.push((ph.key, Arc::from(y)));
        }
    }
    vocal::trim_cache();
    Ok(Made {
        sounds,
        rate,
        sung: sing.len(),
        secs,
        sung_secs,
        place,
        singer: set.singer,
    })
}

/// Phrase sounds at an engine rate, by (phrase key, rate).
type Sounds = HashMap<(u64, u32), Arc<[f32]>>;

/// The phrases of `phrases` whose sounds at `rate` are not in `sounds` (with no rate: those
/// not in the cache), each once.
fn missing(phrases: &[Phrase], sounds: &Sounds, rate: Option<u32>) -> Vec<Phrase> {
    let mut out: Vec<Phrase> = Vec::new();
    for ph in phrases {
        let have = match rate {
            Some(r) => sounds.contains_key(&(ph.key, r)),
            None => vocal::cached(ph.key).is_file(),
        };
        if !have && !out.iter().any(|x| x.key == ph.key) {
            out.push(ph.clone());
        }
    }
    out
}

/// The phrases that have sounds at `rate`, as the engine plays them.
fn clips(phrases: &[Phrase], sounds: &Sounds, rate: u32) -> Vec<SingerClip> {
    phrases
        .iter()
        .filter_map(|ph| {
            Some(SingerClip {
                origin: ph.origin,
                sound: sounds.get(&(ph.key, rate))?.clone(),
            })
        })
        .collect()
}

struct Job {
    progress: Arc<Mutex<String>>,
    result: Receiver<Result<Made, String>>,
    /// The phrases it was started for.
    want: u64,
}

/// One Singer's singing.
#[derive(Default)]
struct Run {
    /// The phrases wanted and since when (sung once they have stayed for `vocal::SETTLE`).
    seen: Option<(u64, Instant)>,
    /// Phrases whose singing failed: not tried again until something changes.
    failed: Option<u64>,
    job: Option<Job>,
    /// Sing now, whatever the pause.
    force: bool,
    /// How the last job went, for the window.
    last: String,
    /// Phrases in the track, and how many of them the engine has.
    total: usize,
    ready: usize,
}

/// A running Singer's engine end.
struct Link {
    remote: SingerRemote,
    /// The key of the score last sent.
    sent: Option<u64>,
}

/// The Singers' renders and links, shared by the window and the engine sync (UI thread).
#[derive(Default)]
struct Runtime {
    links: HashMap<u64, Link>,
    runs: HashMap<u64, Run>,
    sounds: Sounds,
    checked: Option<Instant>,
    /// What failed, to show in the status line once.
    news: Vec<String>,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

fn with_runtime<R>(f: impl FnOnce(&mut Runtime) -> R) -> R {
    let mut guard = match RUNTIME.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(guard.get_or_insert_with(Runtime::default))
}

/// A new engine instance for slot `uid` (its track was built again): it is sent its phrases
/// afresh.
pub fn attach(uid: u64, remote: SingerRemote) {
    with_runtime(|rt| {
        rt.links.insert(uid, Link { remote, sent: None });
    });
}

impl Runtime {
    /// Takes the finished jobs.
    fn poll(&mut self) {
        for run in self.runs.values_mut() {
            let Some(job) = &run.job else { continue };
            let r = match job.result.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => Err("the render stopped".into()),
            };
            let want = job.want;
            run.job = None;
            match r {
                Ok(made) => {
                    if let Some(rate) = made.rate {
                        for (k, s) in made.sounds {
                            self.sounds.insert((k, rate), s);
                        }
                    }
                    run.last = match made.place {
                        Some(_) if made.singer.in_process() => {
                            vocal::sung_message(made.singer, made.sung, made.secs, made.sung_secs)
                        }
                        Some(place) => trf(
                            "{} phrases sung on {} in {} s",
                            &[&made.sung, &place.label(), &format!("{:.1}", made.secs)],
                        ),
                        None => tr("Up to date (from the cache)").to_string(),
                    };
                }
                Err(e) => {
                    run.failed = Some(want);
                    run.last = trf("Singing failed: {}", &[&e]);
                    self.news.push(run.last.clone());
                }
            }
        }
    }

    /// One Singer: sends the engine what is ready, and starts singing what is not.
    fn tend(&mut self, uid: u64, set: &VocalSettings, phrases: &[Phrase], now: Instant) {
        let rate = self
            .links
            .get(&uid)
            .filter(|l| !l.remote.is_abandoned())
            .map(|l| l.remote.rate());
        if let (Some(rate), Some(link)) = (rate, self.links.get_mut(&uid)) {
            link.remote.collect();
            let ready: Vec<&Phrase> = phrases
                .iter()
                .filter(|ph| self.sounds.contains_key(&(ph.key, rate)))
                .collect();
            let key = phrases_key(ready.iter().copied(), rate);
            if link.sent != Some(key) {
                let score = SingerScore::new(
                    key,
                    // The map the phrases' seconds were read from: the project's (in `tick`).
                    TEMPO.with(|m| m.borrow().clone()),
                    clips(phrases, &self.sounds, rate),
                    rate,
                );
                if link.remote.send(Box::new(score)).is_ok() {
                    link.sent = Some(key);
                }
            }
        }
        let todo = missing(phrases, &self.sounds, rate);
        let run = self.runs.entry(uid).or_default();
        run.total = phrases.len();
        run.ready = rate.map_or(0, |r| {
            phrases
                .iter()
                .filter(|ph| self.sounds.contains_key(&(ph.key, r)))
                .count()
        });
        if todo.is_empty() || run.job.is_some() {
            return;
        }
        let want = phrases_key(phrases.iter(), rate.unwrap_or(0));
        if run.failed == Some(want) && !run.force {
            return;
        }
        // Only reading back what is in the cache needs no pause.
        let cached = todo.iter().all(|ph| vocal::cached(ph.key).is_file());
        let settled = match run.seen {
            Some((k, since)) if k == want => now.duration_since(since) >= vocal::SETTLE,
            _ => {
                run.seen = Some((want, now));
                false
            }
        };
        if !(settled || cached || run.force) {
            return;
        }
        run.force = false;
        run.failed = None;
        let progress = Arc::new(Mutex::new(tr("starting").to_string()));
        let (tx, rx) = channel();
        let (job_progress, set, places) = (progress.clone(), set.clone(), order(set.place));
        let spawned = std::thread::Builder::new()
            .name("prowave-singer".into())
            .spawn(move || {
                let _ = tx.send(make(&set, &todo, rate, &places, &job_progress));
            });
        match spawned {
            Ok(_) => {
                run.job = Some(Job {
                    progress,
                    result: rx,
                    want,
                })
            }
            Err(e) => run.last = e.to_string(),
        }
    }
}

thread_local! {
    /// The project's tempo map, for the scores `tend` makes.
    static TEMPO: std::cell::RefCell<prowave_engine::TempoMap> =
        std::cell::RefCell::new(prowave_engine::TempoMap::constant(120.0, (4, 4)));
}

/// The Singers of `p`: (track, slot uid, settings).
fn singers(p: &Project) -> Vec<(usize, u64, VocalSettings)> {
    p.tracks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let slot = t.au_instrument.as_ref().filter(|s| is_singer(s))?;
            Some((i, slot.uid, settings(slot)))
        })
        .collect()
}

/// Once a frame: takes finished singing, and at most every 200 ms looks for changes, sends
/// the engines what is ready and starts singing what is not. Returns whether it is busy.
fn tick(p: &Project) -> bool {
    with_runtime(|rt| {
        rt.poll();
        let now = Instant::now();
        let forced = rt.runs.values().any(|r| r.force);
        let due = rt
            .checked
            .is_none_or(|c| now.duration_since(c) >= Duration::from_millis(200));
        if due || forced {
            rt.checked = Some(now);
            let list = singers(p);
            if !list.is_empty() {
                TEMPO.with(|m| *m.borrow_mut() = p.tempo_map());
            }
            let mut wanted: HashSet<u64> = HashSet::new();
            for (ti, uid, set) in &list {
                let phrases = vocal::track_phrases(p, &p.tracks[*ti], set);
                wanted.extend(phrases.iter().map(|ph| ph.key));
                rt.tend(*uid, set, &phrases, now);
            }
            // Sounds no track sings any more, and Singers that are gone.
            rt.sounds.retain(|(k, _), _| wanted.contains(k));
            let live: HashSet<u64> = list.iter().map(|(_, uid, _)| *uid).collect();
            rt.links
                .retain(|uid, l| live.contains(uid) && !l.remote.is_abandoned());
            rt.runs
                .retain(|uid, r| live.contains(uid) || r.job.is_some());
        }
        rt.runs
            .values()
            .any(|r| r.job.is_some() || r.force || (r.ready < r.total && r.seen.is_some()))
    })
}

/// For a bounce of the project (an engine with no sound card): what the Singer on track `ti`
/// sings, at `rate`, from the phrases that have been sung (the running Singers' sounds and
/// the cache on disk). Nothing is sung here: singing takes minutes. Returns the score, how
/// many phrases the track has and how many of them are in the score.
pub(crate) fn offline_score(
    p: &Project,
    ti: usize,
    rate: u32,
) -> (Option<Box<SingerScore>>, usize, usize) {
    let Some(slot) = p
        .tracks
        .get(ti)
        .and_then(|t| t.au_instrument.as_ref())
        .filter(|s| is_singer(s))
    else {
        return (None, 0, 0);
    };
    let set = settings(slot);
    let phrases = vocal::track_phrases(p, &p.tracks[ti], &set);
    let mut sounds: Sounds = with_runtime(|rt| {
        rt.sounds
            .iter()
            .filter(|((_, r), _)| *r == rate)
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    });
    let cached: Vec<Phrase> = missing(&phrases, &sounds, Some(rate))
        .into_iter()
        .filter(|ph| vocal::cached(ph.key).is_file())
        .collect();
    if !cached.is_empty() {
        let progress = Arc::new(Mutex::new(String::new()));
        // Every phrase asked for is in the cache, so this only reads them back.
        match make(&set, &cached, Some(rate), &[], &progress) {
            Ok(made) => {
                for (k, s) in made.sounds {
                    sounds.insert((k, rate), s);
                }
            }
            Err(e) => tracing::warn!("bounce: the Singer's phrases could not be read: {e}"),
        }
    }
    let clips = clips(&phrases, &sounds, rate);
    let have = clips.len();
    if have == 0 {
        return (None, phrases.len(), 0);
    }
    let key = phrases_key(phrases.iter(), rate);
    let score = SingerScore::new(key, p.tempo_map(), clips, rate);
    (Some(Box::new(score)), phrases.len(), have)
}

/// The Singer window and a bounce asked for.
#[derive(Default)]
pub struct SingerUi {
    /// The slot whose window is open.
    pub open: Option<u64>,
    /// The installed DiffSinger voices, read again every few seconds while the window is open.
    voices: Option<(Instant, Vec<vocal::Voice>)>,
    /// Bounce Vocal… was chosen: the slot and the file.
    pub bounce: Option<(u64, std::path::PathBuf)>,
}

/// Once a frame: the singing, the window, and the status line.
pub fn frame(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    if tick(p) {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    if let Some(msg) = with_runtime(|rt| rt.news.pop()) {
        s.status = Some(msg);
    }
    if let Some(step) = with_runtime(|rt| {
        rt.runs.values().find_map(|r| {
            let j = r.job.as_ref()?;
            Some(j.progress.lock().map(|x| x.clone()).unwrap_or_default())
        })
    }) {
        s.status = Some(trf("Singing: {}", &[&step]));
    }
    show_window(ctx, p, s);
}

/// The Singer window: who sings and how, how far the singing has got, Render Now and Bounce.
fn show_window(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    let Some(uid) = s.singer.open else { return };
    let Some(ti) = p.tracks.iter().position(|t| {
        t.au_instrument
            .as_ref()
            .is_some_and(|x| x.uid == uid && is_singer(x))
    }) else {
        s.singer.open = None;
        return;
    };
    let ui_state = &mut s.singer;
    if ui_state
        .voices
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(5))
    {
        ui_state.voices = Some((Instant::now(), vocal::voices()));
    }
    let voices = ui_state
        .voices
        .as_ref()
        .map(|v| v.1.clone())
        .unwrap_or_default();
    let name = p.tracks[ti].name.clone();
    let Some(slot) = p.tracks[ti].au_instrument.as_mut() else {
        return;
    };
    let mut set = settings(slot);
    let mut changed = false;
    let mut bounce = false;
    let (open, _) = hw_vocal::window(
        ctx,
        egui::Id::unique("prowave_singer_window").with(uid),
        tr("Singer"),
        &name,
        448.0,
        None,
        |ui| {
            let voice = voices.iter().find(|v| v.id == set.voice);
            let chimera = set.singer.is_chimera();
            let (line, ready, busy) = with_runtime(|rt| {
                let run = rt.runs.entry(uid).or_default();
                let ready = (run.total > 0).then_some((run.ready, run.total));
                // The step being sung, or the last news; the phrases ready go on the ladder.
                let line = match &run.job {
                    Some(job) => {
                        let step = job.progress.lock().map(|x| x.clone()).unwrap_or_default();
                        trf("Singing: {}", &[&step])
                    }
                    None => run.last.clone(),
                };
                (line, ready, run.job.is_some())
            });
            let voice_name = vocal::voice_name(&set, voice);
            vocal::status_lcd(
                ui,
                &vocal::Status {
                    singer: set.singer,
                    voice: &voice_name,
                    line,
                    note: String::new(),
                    busy,
                    ready,
                    speeds: None,
                    shares: chimera.then(|| set.chimera_voice().shares()),
                },
            );
            hw_vocal::gap(ui, 6.0);
            vocal::settings_panel(ui, &mut set, &voices, &mut changed);
            hw_vocal::gap(ui, 12.0);
            with_runtime(|rt| {
                let run = rt.runs.entry(uid).or_default();
                let all = run.total > 0 && run.ready == run.total;
                // Render Now is the primary key: orange while phrases wait to be sung.
                let stale = run.job.is_none() && run.ready < run.total;
                let render_cap = if stale {
                    hw::Cap::Orange
                } else {
                    hw::Cap::Bone
                };
                let clicked = hw_vocal::key_row(
                    ui,
                    egui::Id::unique("singer_keys").with(uid),
                    &[
                        (tr("Render Now"), render_cap, Some(run.job.is_some()), true),
                        (tr("Bounce Vocal…"), hw::Cap::Bone, None, all),
                    ],
                );
                if clicked[0] {
                    run.force = true;
                    run.failed = None;
                }
                if clicked[1] && all {
                    bounce = true;
                }
            });
            if let Some(text) = vocal::credits(&set, voice) {
                vocal::licence(ui, &text);
            }
        },
    );
    if changed {
        slot.state = Some(encode(&set));
        p.song_rev += 1;
    }
    if !open {
        s.singer.open = None;
    }
    if bounce {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{name} Vocal.wav"))
            .add_filter("WAV", &["wav"])
            .save_file()
        {
            s.singer.bounce = Some((uid, path));
        }
    }
}

/// The sound of track `ti` (a Singer's) through its channel strip, rendered offline from the
/// song's start: `clips` are its phrases at `rate`, `effects` its Audio FX; `before_free` runs
/// once they are done with and before they are freed. Returns the left and right channels,
/// delay compensated, to `BOUNCE_TAIL` after the last phrase.
pub(crate) fn render_track(
    p: &Project,
    ti: usize,
    clips: Vec<SingerClip>,
    rate: u32,
    effects: Vec<Box<dyn prowave_engine::BlockProcessor>>,
    before_free: impl FnOnce(),
) -> (Vec<f32>, Vec<f32>) {
    use prowave_engine::{Command, EngineCore, PluginTrack, Shared, SingerPlayer};
    // The other tracks keep their strips (sends reach their buses) but play nothing.
    let mut q = p.clone();
    for (j, t) in q.tracks.iter_mut().enumerate() {
        if j != ti {
            t.regions.clear();
        }
    }
    let end = clips
        .iter()
        .map(|c| c.origin + c.sound.len() as f64 / f64::from(rate))
        .fold(0.0, f64::max)
        + BOUNCE_TAIL;
    let has_fx = !effects.is_empty();
    let sr = rate as f32;
    let delay = prowave_engine::plugin_host_delay(true, has_fx, true, 0, BOUNCE_BLOCK as u32)
        + effects
            .iter()
            .map(|e| (e.latency_seconds() * f64::from(rate)).round() as usize)
            .sum::<usize>()
        + q.tracks[ti]
            .inserts
            .iter()
            .filter(|i| !i.bypass)
            .map(|i| i.kind.latency_samples(sr))
            .sum::<usize>();
    let (player, mut remote) = SingerPlayer::new(rate, BOUNCE_BLOCK);
    let score = SingerScore::new(1, q.tempo_map(), clips, rate);
    if remote.send(Box::new(score)).is_err() {
        return (Vec::new(), Vec::new());
    }
    let mut cmds = crate::engine_sync::offline_setup(&q, rate);
    cmds.push(Command::SetPlugins {
        track: ti as u16,
        plugins: Some(Box::new(PluginTrack::new(
            Some(Box::new(player)),
            effects,
            BOUNCE_BLOCK,
        ))),
    });
    cmds.push(Command::SetPosition { beat: 0.0 });
    cmds.push(Command::Play);
    let (mut tx, rx) = rtrb::RingBuffer::<Command>::new(cmds.len() + 8);
    let (gtx, mut grx) = rtrb::RingBuffer::new(1024);
    let mut core = EngineCore::new(rate, rx, gtx, Arc::new(Shared::new()));
    for c in cmds {
        if tx.push(c).is_err() {
            tracing::warn!("singer bounce: engine command queue full");
        }
    }
    let frames = (end * f64::from(rate)).ceil() as usize + delay;
    let mut buf = vec![0f32; BOUNCE_BLOCK * 2];
    let (mut l, mut r) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut done = 0;
    while done < frames {
        core.process(&mut buf);
        for f in buf.as_chunks::<2>().0 {
            l.push(f[0]);
            r.push(f[1]);
        }
        done += BOUNCE_BLOCK;
        while grx.pop().is_ok() {}
        remote.collect();
    }
    before_free();
    drop(core);
    let keep = |mut v: Vec<f32>| {
        v.drain(..delay.min(v.len()));
        v.truncate(frames - delay);
        v
    };
    (keep(l), keep(r))
}

/// Bounce Vocal…: Singer `uid`'s track through its strip, from the song's start, into `path`
/// (stereo, 32-bit float, at the engine's rate). Its Audio FX are opened again here (their
/// settings as `host` last saved them). Returns the seconds written.
pub fn bounce(
    p: &mut Project,
    host: &mut crate::plugins::PluginHost,
    uid: u64,
    path: &Path,
) -> Result<f64, String> {
    let ti = p
        .tracks
        .iter()
        .position(|t| t.au_instrument.as_ref().is_some_and(|s| s.uid == uid))
        .ok_or_else(|| tr("nothing has been sung yet").to_string())?;
    let rate = with_runtime(|rt| rt.links.get(&uid).map(|l| l.remote.rate()))
        .ok_or_else(|| tr("nothing has been sung yet").to_string())?;
    let set = p.tracks[ti]
        .au_instrument
        .as_ref()
        .map(settings)
        .unwrap_or_else(|| VocalSettings::new(0));
    let phrases = vocal::track_phrases(p, &p.tracks[ti], &set);
    let clips = with_runtime(|rt| clips(&phrases, &rt.sounds, rate));
    if phrases.is_empty() || clips.len() < phrases.len() {
        return Err(tr("nothing has been sung yet").into());
    }
    host.save_states(p);
    let mut failed = Vec::new();
    let mut opener = crate::plugins::PluginHost::default();
    let effects = opener.open_effects_offline(p, ti, rate, BOUNCE_BLOCK, &mut failed);
    if let Some(e) = failed.into_iter().next() {
        return Err(e);
    }
    // The plug-ins' parameter watches go before the plug-ins.
    let (l, r) = render_track(p, ti, clips, rate, effects, move || drop(opener));
    let stereo: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
    prowave_engine::write_wav(path, &stereo, rate).map_err(|e| e.to_string())?;
    Ok(l.len() as f64 / f64::from(rate))
}

/// Main loop: runs the bounce the window asked for.
pub fn run_bounce(p: &mut Project, host: &mut crate::plugins::PluginHost, s: &mut crate::UiState) {
    let Some((uid, path)) = s.singer.bounce.take() else {
        return;
    };
    s.status = Some(match bounce(p, host, uid, &path) {
        Ok(secs) => trf(
            "Bounced {} s of vocal to {}",
            &[&format!("{secs:.1}"), &path.display()],
        ),
        Err(e) => trf("Bounce failed: {}", &[&e]),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::Lyric;
    use crate::model::{Insert, Note, Region, RegionParams, Track, TrackKind};
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// A cache of its own for these tests (the installed singer and voices linked in), so
    /// what they sing is really sung.
    fn test_home() -> &'static PathBuf {
        static HOME: OnceLock<PathBuf> = OnceLock::new();
        HOME.get_or_init(|| {
            let real = vocal::vocal_home();
            let dir = std::env::temp_dir().join(format!(
                "prowave-singer-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir_all(&dir).unwrap();
            for part in ["engine", "Library", "voices.json"] {
                let from = real.join(part);
                if from.exists() {
                    std::os::unix::fs::symlink(&from, dir.join(part)).unwrap();
                }
            }
            std::env::set_var("PROWAVE_VOCAL_HOME", &dir);
            dir
        })
    }

    fn note(start: f32, len: f32, pitch: u8, lyric: &str) -> Note {
        Note {
            start,
            len,
            pitch,
            vel: 100,
            mute: false,
            lyric: Lyric::new(lyric),
        }
    }

    /// A project at 100 BPM with a Singer track (with `singer`) holding `notes` in a region at
    /// bar 2, and a Compressor insert on it.
    fn project(notes: Vec<Note>, singer: Singer) -> Project {
        let mut p = Project::empty();
        p.tempo = 100.0;
        let mut t = Track::new("Singer", TrackKind::Instrument);
        let mut slot = new_slot();
        let mut set = VocalSettings::new(0);
        set.singer = singer;
        set.vibrato_depth = 0.0;
        slot.state = Some(encode(&set));
        t.au_instrument = Some(slot);
        t.inserts
            .push(Insert::new(prowave_engine::FxKind::Compressor));
        t.regions.push(Region {
            missing: None,
            name: "Singer".into(),
            start_bar: 2.0,
            len_bars: 8.0,
            notes,
            controls: Vec::new(),
            folder: None,
            audio: None,
            params: RegionParams::default(),
            generated: None,
        });
        p.tracks.push(t);
        p
    }

    /// Separate notes on `words` (a rest after each, so each is its own phrase).
    fn separate_notes(words: [&str; 6]) -> Vec<Note> {
        let pitches = [62u8, 65, 69, 67, 64, 60];
        words
            .iter()
            .zip(pitches)
            .enumerate()
            .map(|(k, (w, p))| note(k as f32 * 2.0, 1.0, p, w))
            .collect()
    }

    /// Words whose vowel each singer puts on the beat, so the voice's onset is the note's:
    /// CHIMERA's vowels (it puts a "t" or "p" on the beat and its vowel 30 to 50 ms after),
    /// and for DiffSinger words with an unvoiced consonant, which it sings before the beat (its
    /// bare vowels start breathy, reaching the pitch 15 to 75 ms in).
    const CHIMERA_WORDS: [&str; 6] = ["ah", "oh", "I", "ah", "oh", "I"];
    const DIFFSINGER_WORDS: [&str; 6] = ["two", "see", "pay", "two", "see", "pay"];

    fn ti(p: &Project) -> usize {
        p.tracks.len() - 1
    }

    fn sing_all(p: &Project, rate: u32) -> (Vec<Phrase>, Sounds, usize) {
        let t = &p.tracks[ti(p)];
        let set = settings(t.au_instrument.as_ref().unwrap());
        let phrases = vocal::track_phrases(p, t, &set);
        let mut sounds = HashMap::new();
        let todo = missing(&phrases, &sounds, Some(rate));
        let progress = Arc::new(Mutex::new(String::new()));
        let made = make(&set, &todo, Some(rate), &order(set.place), &progress).unwrap();
        for (k, s) in made.sounds {
            sounds.insert((k, rate), s);
        }
        (phrases, sounds, made.sung)
    }

    /// Samples (at 16 kHz) the pitch is measured over.
    const W: usize = 480;

    /// The pitch (MIDI, with its fraction) of `x` (at 16 kHz) in the 30 ms from sample `at`,
    /// by normalised autocorrelation, looking within a fifth of `target` either way (so a
    /// wrong note up to a fifth off is seen, and the octaves the method confuses are not), if
    /// it is clearly voiced there.
    fn pitch_at(x: &[f32], at: usize, target: f32) -> Option<f32> {
        let lag_of = |m: f32| 16_000.0 / (440.0 * 2f32.powf((m - 69.0) / 12.0));
        let (lo, hi) = (
            lag_of(target + 7.0).floor() as usize,
            lag_of(target - 7.0).ceil() as usize,
        );
        if at + W + hi + 1 >= x.len() {
            return None;
        }
        let w = &x[at..at + W];
        let e0: f32 = w.iter().map(|v| v * v).sum();
        if e0 < 1e-6 {
            return None;
        }
        let corr = |lag: usize| {
            let v = &x[at + lag..at + lag + W];
            let num: f32 = w.iter().zip(v).map(|(a, b)| a * b).sum();
            let e1: f32 = v.iter().map(|b| b * b).sum();
            num / (e0 * e1).sqrt().max(1e-12)
        };
        let lag = (lo..=hi).max_by(|a, b| corr(*a).total_cmp(&corr(*b)))?;
        if corr(lag) < 0.8 || lag == lo || lag == hi {
            return None;
        }
        let (a, b, c) = (corr(lag - 1), corr(lag), corr(lag + 1));
        let d = a - 2.0 * b + c;
        let shift = if d.abs() > 1e-9 {
            0.5 * (a - c) / d
        } else {
            0.0
        };
        let hz = 16_000.0 / (lag as f32 + shift);
        Some(69.0 + 12.0 * (hz / 440.0).log2())
    }

    /// Each note's voiced onset (seconds from its start) and pitch error (cents) in `mono`
    /// (at `rate`): the onset is the centre of the first 30 ms window, in 5 ms steps from
    /// 250 ms before the note, from which the note's pitch (within a semitone) holds for 30 ms;
    /// the pitch is the median over the note's middle.
    fn measure(mono: &[f32], rate: u32, notes: &[(f64, f64, u8)]) -> Vec<(f64, f64)> {
        let x = crate::chimera_singer::resample(mono, rate, 16_000);
        let step = 80;
        let at = |secs: f64| (secs.max(0.0) * 16_000.0).round() as usize;
        notes
            .iter()
            .map(|&(s, e, pitch)| {
                let target = f32::from(pitch);
                let near =
                    |i: usize| pitch_at(&x, i, target).is_some_and(|m| (m - target).abs() < 1.0);
                let onset = (at(s - 0.25)..at(e))
                    .step_by(step)
                    .find(|&i| (0..6).all(|k| near(i + k * step)))
                    .map_or(f64::NAN, |i| (i + W / 2) as f64 / 16_000.0 - s);
                let d = e - s;
                let mut m: Vec<f32> = (at(s + 0.3 * d)..at(e - 0.2 * d))
                    .step_by(step)
                    .filter_map(|i| pitch_at(&x, i, target))
                    .collect();
                m.sort_by(f32::total_cmp);
                let cents = m
                    .get(m.len() / 2)
                    .map_or(f64::NAN, |v| f64::from(v - target) * 100.0);
                (onset, cents)
            })
            .collect()
    }

    /// The notes of track `ti` in song seconds.
    fn note_secs(p: &Project) -> Vec<(f64, f64, u8)> {
        let map = p.tempo_map();
        let t = &p.tracks[ti(p)];
        let r = &t.regions[0];
        let origin = p.region_start_beats(r);
        r.notes
            .iter()
            .map(|n| {
                (
                    map.seconds_at(origin + f64::from(n.start)),
                    map.seconds_at(origin + f64::from(n.start + n.len)),
                    n.pitch,
                )
            })
            .collect()
    }

    /// The lag (samples, within +-`reach`) at which `b` best matches `a`, and how well
    /// (normalised correlation).
    fn best_lag(a: &[f32], b: &[f32], reach: i64) -> (i64, f64) {
        let mut best = (0, f64::MIN);
        for lag in -reach..=reach {
            let (mut ab, mut aa, mut bb) = (0f64, 0f64, 0f64);
            for (i, x) in a.iter().enumerate() {
                let j = i as i64 + lag;
                if j < 0 || j as usize >= b.len() {
                    continue;
                }
                let y = b[j as usize];
                ab += f64::from(*x) * f64::from(y);
                aa += f64::from(*x) * f64::from(*x);
                bb += f64::from(y) * f64::from(y);
            }
            let c = ab / (aa * bb).sqrt().max(1e-12);
            if c > best.1 {
                best = (lag, c);
            }
        }
        best
    }

    /// The bounce of `singer` singing [`separate_notes`] on `words`: the engine places every
    /// phrase to the sample; through a Compressor insert every note sounds within 30 ms of its
    /// start and 50 cents of its pitch; moving one note sings again only its phrase.
    fn bounce_sings_the_notes_in_time_and_in_tune(singer: Singer, words: [&str; 6]) {
        test_home();
        let rate = 48_000;
        let mut p = project(separate_notes(words), singer);
        let i = ti(&p);
        let started = Instant::now();
        let (phrases, sounds, sung) = sing_all(&p, rate);
        let took = started.elapsed().as_secs_f64();
        let singing: f64 = phrases
            .iter()
            .map(|ph| ph.notes.last().map_or(0.0, |n| n.start + n.dur))
            .sum();
        assert_eq!(phrases.len(), 6, "a phrase a note");
        assert_eq!(sung, 6);
        // Placement: the bounce without effects against the phrases laid out by hand.
        let mut dry = p.clone();
        dry.tracks[i].inserts.clear();
        let (l, _) = render_track(
            &dry,
            i,
            clips(&phrases, &sounds, rate),
            rate,
            Vec::new(),
            || (),
        );
        let mut laid = vec![0f32; l.len()];
        for ph in &phrases {
            let s = &sounds[&(ph.key, rate)];
            let at = (ph.origin * f64::from(rate)).round() as usize;
            for (k, x) in s.iter().enumerate() {
                if let Some(o) = laid.get_mut(at + k) {
                    *o += x;
                }
            }
        }
        let (lag, corr) = best_lag(&laid, &l, 64);
        // Through the strip: the Compressor insert.
        let (l, r) = render_track(
            &p,
            i,
            clips(&phrases, &sounds, rate),
            rate,
            Vec::new(),
            || (),
        );
        assert_eq!(l.len(), r.len());
        assert!(l.iter().all(|x| x.is_finite()));
        let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
        let got = measure(&mono, rate, &note_secs(&p));
        eprintln!(
            "{singer:?}: {sung} phrases ({singing:.1} s of singing) sung in {took:.1} s; placement lag {lag} samples (correlation {corr:.4}); onset ms and cents: {:?}",
            got.iter()
                .map(|(o, c)| (format!("{:.0}", o * 1000.0), format!("{c:.1}")))
                .collect::<Vec<_>>()
        );
        assert!(
            lag.abs() <= 1 && corr > 0.99,
            "lag {lag}, correlation {corr}"
        );
        for (k, (onset, cents)) in got.iter().enumerate() {
            assert!(
                onset.abs() <= 0.03,
                "note {k}: onset {:.1} ms",
                onset * 1000.0
            );
            assert!(cents.abs() <= 50.0, "note {k}: {cents:.0} cents");
        }
        // Moving the fourth note up a tone sings only its phrase again.
        p.tracks[i].regions[0].notes[3].pitch += 2;
        let t = &p.tracks[i];
        let set = settings(t.au_instrument.as_ref().unwrap());
        let moved = vocal::track_phrases(&p, t, &set);
        let todo = missing(&moved, &sounds, Some(rate));
        assert_eq!(todo.len(), 1, "only the changed phrase");
        let progress = Arc::new(Mutex::new(String::new()));
        let made = make(&set, &todo, Some(rate), &order(set.place), &progress).unwrap();
        assert_eq!(made.sung, 1);
    }

    #[test]
    fn chimera_bounce_sings_the_notes_in_time_and_in_tune() {
        bounce_sings_the_notes_in_time_and_in_tune(Singer::Chimera, CHIMERA_WORDS);
    }

    /// Novox The Robot through the whole pipeline: sung into the cache at 44.1 kHz, read back at
    /// the engine's 48 kHz, placed by the engine's player and bounced through the strip.
    #[test]
    fn novox_bounce_sings_the_notes_in_time_and_in_tune() {
        bounce_sings_the_notes_in_time_and_in_tune(Singer::Novox, CHIMERA_WORDS);
    }

    /// With the installed DiffSinger voice (ignored: CI has none).
    #[test]
    #[ignore]
    fn diffsinger_bounce_sings_the_notes_in_time_and_in_tune() {
        bounce_sings_the_notes_in_time_and_in_tune(Singer::DiffSinger, DIFFSINGER_WORDS);
    }

    /// The bounce opens the track's Audio FX again (here a native compressor) and plays the
    /// singing through them.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_bounce_plays_through_the_tracks_audio_fx() {
        test_home();
        let rate = 48_000;
        let mut p = project(separate_notes(CHIMERA_WORDS), Singer::Chimera);
        let i = ti(&p);
        let info = &prowave_engine::native_effects()[1];
        p.tracks[i]
            .au_effects
            .push(crate::native_fx_ui::new_slot(info));
        let (phrases, sounds, _) = sing_all(&p, rate);
        let mut failed = Vec::new();
        let mut opener = crate::plugins::PluginHost::default();
        let fx = opener.open_effects_offline(&p, i, rate, BOUNCE_BLOCK, &mut failed);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(fx.len(), 1);
        let (l, r) = render_track(&p, i, clips(&phrases, &sounds, rate), rate, fx, move || {
            drop(opener)
        });
        let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
        assert!(
            peak > 0.01 && l.iter().all(|x| x.is_finite()),
            "peak {peak}"
        );
        let end = note_secs(&p).last().unwrap().1;
        assert!(l.len() as f64 / f64::from(rate) > end, "{} frames", l.len());
    }

    #[test]
    fn settings_live_in_the_slot_and_survive_saving() {
        let mut slot = new_slot();
        let mut set = settings(&slot);
        set.singer = Singer::Chimera;
        set.body_female = 70.0;
        set.ring = 20.0;
        slot.state = Some(encode(&set));
        assert_eq!(settings(&slot), set);
        let mut p = project(vec![note(0.0, 1.0, 60, "la")], Singer::Chimera);
        let i = ti(&p);
        p.tracks[i].au_instrument = Some(slot.clone());
        let (doc, _) = crate::project_io::to_doc(&p);
        let json = serde_json::to_string(&doc).unwrap();
        let doc: prowave_project::ProjectDoc = serde_json::from_str(&json).unwrap();
        let (q, _) = crate::project_io::from_doc(&doc, Path::new("."), &egui::Context::default());
        let back = q.tracks[i].au_instrument.as_ref().unwrap();
        assert!(is_singer(back));
        assert_eq!(settings(back), set);
        // Novox's voice and knobs too.
        set.singer = Singer::Novox;
        set.novox = crate::novox_singer::Voice::preset("female").unwrap();
        set.novox.grit = 35.0;
        set.novox.voices = 3.0;
        set.novox.scoop = -80.0;
        set.novox.portamento = 150.0;
        slot.state = Some(encode(&set));
        p.tracks[i].au_instrument = Some(slot.clone());
        let (doc, _) = crate::project_io::to_doc(&p);
        let json = serde_json::to_string(&doc).unwrap();
        let doc: prowave_project::ProjectDoc = serde_json::from_str(&json).unwrap();
        let (q, _) = crate::project_io::from_doc(&doc, Path::new("."), &egui::Context::default());
        assert_eq!(settings(q.tracks[i].au_instrument.as_ref().unwrap()), set);
        // Unreadable state: the defaults.
        slot.state = Some(b"?".to_vec());
        assert_eq!(settings(&slot), VocalSettings::new(0));
    }

    #[test]
    fn the_engine_gets_what_is_ready_and_silence_for_the_rest() {
        let rate = 48_000;
        let p = project(separate_notes(CHIMERA_WORDS), Singer::Chimera);
        let t = &p.tracks[ti(&p)];
        let set = settings(t.au_instrument.as_ref().unwrap());
        let phrases = vocal::track_phrases(&p, t, &set);
        let mut sounds = HashMap::new();
        sounds.insert((phrases[1].key, rate), Arc::from(vec![0.5f32; 10]));
        let c = clips(&phrases, &sounds, rate);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].origin, phrases[1].origin);
        assert_eq!(missing(&phrases, &sounds, Some(rate)).len(), 5);
        assert_ne!(
            phrases_key(phrases.iter(), rate),
            phrases_key(phrases.iter(), 44_100)
        );
    }

    /// The demos: a few bars of melody with lyrics and a Compressor insert, bounced through the strip into PROWAVE_SINGER_DEMO_DIR (never over an existing file).
    #[test]
    #[ignore]
    fn demos() {
        let Some(dir) = std::env::var_os("PROWAVE_SINGER_DEMO_DIR").map(PathBuf::from) else {
            return;
        };
        let line = [
            ("I", 64, 0.5),
            ("can", 67, 0.5),
            ("hear", 69, 1.0),
            ("the", 67, 0.5),
            ("city", 64, 0.5),
            ("+", 62, 1.0),
            ("sing", 60, 1.5),
            ("-", 62, 0.5),
            ("all", 64, 1.0),
            ("night", 67, 1.0),
            ("long", 64, 2.0),
            ("we", 62, 0.5),
            ("are", 64, 0.5),
            ("the", 67, 0.5),
            ("ones", 69, 1.5),
            ("who", 67, 0.5),
            ("stay", 64, 1.0),
            ("awake", 62, 0.5),
            ("+", 60, 2.5),
        ];
        let mut beat = 0.0f32;
        let mut notes = Vec::new();
        for (w, pitch, len) in line {
            notes.push(note(beat, len, pitch, w));
            beat += len;
        }
        test_home();
        let rate = 48_000;
        for (singer, file) in [
            (Singer::DiffSinger, "singer-instrument-diffsinger-v1.wav"),
            (Singer::Chimera, "singer-instrument-chimera-v2.wav"),
            (Singer::Chimera09fb776, "singer-instrument-chimera-v1.wav"),
            (Singer::Novox, "singer-instrument-novox-v1.wav"),
        ] {
            let path = dir.join(file);
            assert!(!path.exists(), "{} exists", path.display());
            let p = project(notes.clone(), singer);
            let i = ti(&p);
            let started = Instant::now();
            let (phrases, sounds, sung) = sing_all(&p, rate);
            let took = started.elapsed().as_secs_f64();
            let (l, r) = render_track(
                &p,
                i,
                clips(&phrases, &sounds, rate),
                rate,
                Vec::new(),
                || (),
            );
            let stereo: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
            prowave_engine::write_wav(&path, &stereo, rate).unwrap();
            let secs: f64 = phrases
                .iter()
                .map(|ph| ph.notes.last().map_or(0.0, |n| n.start + n.dur))
                .sum();
            eprintln!(
                "{}: {sung} phrases, {secs:.1} s of singing sung in {took:.1} s ({:.2} s per s); {:.1} s bounced, peak {:.2}",
                path.display(),
                took / secs,
                l.len() as f64 / f64::from(rate),
                l.iter().fold(0f32, |m, x| m.max(x.abs()))
            );
        }
    }

    /// Pictures of the Singer window, partly sung, with CHIMERA and with Novox:
    /// `MANUAL_PNG_DIR=… cargo test -p prowave-app singer_window_picture -- --ignored`.
    #[test]
    #[ignore = "pictures of the window"]
    fn singer_window_picture() {
        singer_window_picture_of(Singer::Chimera, "singer.png");
        singer_window_picture_of(Singer::Novox, "singer-novox.png");
    }

    fn singer_window_picture_of(singer: Singer, file: &str) {
        let mut p = project(
            separate_notes(["row", "row", "row", "your", "boat", "gent"]),
            singer,
        );
        let ti = ti(&p);
        p.tracks[ti].name = "Lead Vocal".into();
        let uid = p.tracks[ti].au_instrument.as_ref().unwrap().uid;
        let mut s = crate::UiState::new();
        s.singer.open = Some(uid);
        with_runtime(|rt| {
            let run = rt.runs.entry(uid).or_default();
            run.total = 3;
            run.ready = 2;
            run.last = vocal::sung_message(singer, 2, 0.8, 2.4);
        });
        crate::ui_test::render_png(
            500.0,
            660.0,
            2.0,
            crate::theme::BONE3(),
            |ui| {
                crate::theme::apply(ui.ctx());
                show_window(ui.ctx(), &mut p, &mut s)
            },
            &hw_vocal::picture_path(file),
        )
        .unwrap();
    }
}
