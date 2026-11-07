//! Genre and artist template packages (File > New from Template…, the Genre Templates and
//! Artist Sounds sections), Logic Pro equivalent: the Project Templates of the Project Chooser.
//!
//! Each package is a project document (`templates/<id>.json`, made by
//! `tools/templates/build_templates.py`) with its tracks, buses, plug-ins and their
//! settings, the master chain and notes for the Note Pad. Packages name plug-ins by id and keep
//! the settings the plug-ins themselves returned; a plug-in that is not installed where the
//! project opens keeps its slot and name and is reported as not installed when the engine
//! opens the track (see `plugins.rs`), like any project from another Mac.
//!
//! Every `templates/*.json` is built into the program (see `build.rs`), in the order
//! `templates/order.txt` gives (one id a line; packages it leaves out follow by file name),
//! and so is every one-shot in `templates/samples/`: the Channel Rack and the Sampler of a
//! package name them `$TEMPLATE_SAMPLES/<file>`, and they are written once to the settings
//! folder's `template-samples` for them to play.
use crate::model::Project;
use crate::plugins::PluginSlot;
use crate::track_icons::IconKind;
use prowave_project::ProjectDoc;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// The packages, in the order the chooser shows them (`const SOURCES: &[&str]`), and the
// one-shots (`const SAMPLES: &[(&str, &[u8])]`, file name and contents).
include!(concat!(env!("OUT_DIR"), "/templates.rs"));

/// Where `$TEMPLATE_SAMPLES` points: the folder the built-in one-shots are written to.
const SAMPLES_TOKEN: &str = "$TEMPLATE_SAMPLES";

/// The built-in one-shots' folder, written (again, when a file is missing or another size)
/// before a package's project is made. `None` when it cannot be written.
fn samples_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = if cfg!(test) {
            std::env::temp_dir().join(format!("prowave-template-samples-{}", std::process::id()))
        } else {
            crate::recent_files::config_dir()?.join("template-samples")
        };
        for (name, bytes) in SAMPLES {
            let path = dir.join(name);
            if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
                std::fs::create_dir_all(&dir).ok()?;
                std::fs::write(&path, bytes).ok()?;
            }
        }
        Some(dir)
    })
    .clone()
}

/// `token/…` paths in the settings of a Channel Rack or Sampler slot point into `dir`.
fn put_folder(slot: &mut PluginSlot, token: &str, dir: &Path) {
    let Some(text) = slot
        .state
        .as_deref()
        .and_then(|b| std::str::from_utf8(b).ok())
    else {
        return;
    };
    if !(crate::channel_rack::is_rack(slot) || crate::sampler::is_sampler(slot))
        || !text.contains(token)
    {
        return;
    }
    // The token sits inside JSON strings: put the folder in as a JSON string body.
    let folder = serde_json::to_string(&dir.to_string_lossy()).unwrap_or_default();
    let folder = folder.trim_matches('"');
    slot.state = Some(text.replace(token, folder).into_bytes());
}

/// Which chooser section a package belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Genre,
    Artist,
}

/// A package: its name, one-line description and icon, and its document.
#[derive(Debug)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub about: String,
    pub group: Group,
    pub icon: IconKind,
    source: &'static str,
}

/// Every package (read once; a package that does not parse is left out and logged).
pub fn packs() -> &'static [Pack] {
    static PACKS: OnceLock<Vec<Pack>> = OnceLock::new();
    PACKS.get_or_init(|| {
        SOURCES
            .iter()
            .filter_map(|src| match header(src) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::error!("template package: {e}");
                    None
                }
            })
            .collect()
    })
}

/// The packages of one group, with their indices into [`packs`].
pub fn of_group(group: Group) -> Vec<usize> {
    packs()
        .iter()
        .enumerate()
        .filter(|(_, p)| p.group == group)
        .map(|(i, _)| i)
        .collect()
}

fn header(src: &'static str) -> Result<Pack, String> {
    let v: serde_json::Value = serde_json::from_str(src).map_err(|e| e.to_string())?;
    let t = &v["template"];
    let text = |k: &str| t[k].as_str().unwrap_or_default().to_string();
    let id = text("id");
    if id.is_empty() {
        return Err("a package without an id".into());
    }
    Ok(Pack {
        name: text("name"),
        about: text("about"),
        group: if t["group"].as_str() == Some("artist") {
            Group::Artist
        } else {
            Group::Genre
        },
        icon: t["icon"]
            .as_str()
            .and_then(IconKind::from_name)
            .unwrap_or(IconKind::Note),
        id,
        source: src,
    })
}

impl Pack {
    /// The package's project document. Its Stereo Out chain is the built-in effects
    /// (`master_fx`) and then the plug-ins (`master_plugins`), as in any saved project.
    pub fn doc(&self) -> Result<ProjectDoc, String> {
        let mut v: serde_json::Value =
            serde_json::from_str(self.source).map_err(|e| e.to_string())?;
        serde_json::from_value(v["project"].take()).map_err(|e| e.to_string())
    }

    /// The new, untitled project this package starts.
    pub fn project(&self) -> Project {
        let Ok(doc) = self.doc() else {
            return Project::empty();
        };
        let ctx = egui::Context::default();
        let (mut p, warnings) = crate::project_io::from_doc(&doc, &std::env::temp_dir(), &ctx);
        for w in warnings {
            tracing::warn!("template {}: {w}", self.id);
        }
        // The built-in one-shots are where the drum racks and samplers look for them.
        let samples = self
            .source
            .contains(SAMPLES_TOKEN)
            .then(samples_dir)
            .flatten();
        for t in p.tracks.iter_mut() {
            if let (Some(slot), Some(dir)) = (t.au_instrument.as_mut(), samples.as_deref()) {
                put_folder(slot, SAMPLES_TOKEN, dir);
            }
        }
        p.name = "Untitled".into();
        p.selected_track = 0;
        p
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::project_io::{from_doc, to_doc};
    use prowave_project::PluginDoc;

    #[test]
    fn every_package_parses_and_has_its_parts() {
        assert_eq!(packs().len(), SOURCES.len(), "a package did not parse");
        let mut ids: Vec<&str> = packs().iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), packs().len(), "two packages share an id");
        for p in packs() {
            let doc = p.doc().unwrap();
            let master = &doc.master_plugins;
            assert!(!p.name.is_empty() && !p.about.is_empty(), "{}", p.id);
            assert!(!doc.tracks.is_empty(), "{}", p.id);
            assert!((40.0..=200.0).contains(&doc.tempo), "{}", p.id);
            // Every plug-in slot names its format, and every Audio Unit carries settings.
            let slots = doc
                .tracks
                .iter()
                .flat_map(|t| t.au_instrument.iter().chain(&t.au_effects))
                .chain(master);
            for s in slots {
                if crate::plugins::is_au_id(&s.id) && !s.id.starts_with("prowave:") {
                    assert_eq!(s.format, "au", "{}: {}", p.id, s.name);
                    assert!(!s.state.is_empty(), "{}: {} has no settings", p.id, s.name);
                    assert!(
                        crate::plugins::decode_state(&s.state).is_some(),
                        "{}: {} settings are not base64",
                        p.id,
                        s.name
                    );
                }
            }
        }
    }

    /// Every package becomes a project, and that project saves and opens to the same
    /// document (the template is a valid project file).
    #[test]
    fn every_package_round_trips_as_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        for (k, pack) in packs().iter().enumerate() {
            let p = pack.project();
            assert_eq!(p.name, "Untitled");
            let doc = pack.doc().unwrap();
            assert_eq!(p.tracks.len(), doc.tracks.len(), "{}", pack.id);
            assert!((p.tempo - doc.tempo).abs() < 1e-3, "{}", pack.id);
            for (t, d) in p.tracks.iter().zip(&doc.tracks) {
                assert_eq!(t.name, d.name, "{}", pack.id);
                assert_eq!(t.au_effects.len(), d.au_effects.len(), "{}", pack.id);
                assert_eq!(t.inserts.len(), d.fx.len(), "{}/{}", pack.id, d.name);
                assert_eq!(t.sends.len(), d.sends.len(), "{}/{}", pack.id, d.name);
                assert_eq!(t.output, d.output, "{}/{}", pack.id, d.name);
                // Audio Unit settings arrive byte for byte.
                for (s, ds) in t.au_effects.iter().zip(&d.au_effects) {
                    assert_eq!(s.id, ds.id);
                    assert_eq!(crate::plugins::encode_state(&s.state), ds.state);
                }
                if let (Some(s), Some(ds)) = (&t.au_instrument, &d.au_instrument) {
                    assert_eq!(s.id, ds.id);
                    if !s.id.starts_with("prowave:") {
                        assert_eq!(crate::plugins::encode_state(&s.state), ds.state);
                    }
                }
            }
            assert_eq!(p.master_inserts.len(), doc.master_fx.len(), "{}", pack.id);
            // The Stereo Out plug-ins load into the project with their settings.
            assert_eq!(
                p.master_au_effects.len(),
                doc.master_plugins.len(),
                "{}",
                pack.id
            );
            for (s, ds) in p.master_au_effects.iter().zip(&doc.master_plugins) {
                assert_eq!(s.id, ds.id, "{}", pack.id);
                assert_eq!(crate::plugins::encode_state(&s.state), ds.state);
            }
            assert!(!p.notes.is_empty());
            // Save and open: the same document comes back.
            let path = dir.path().join(format!("t{k}.prowave"));
            let saved = crate::project_io::save_project(&p, &path).unwrap();
            let media = dir.path().join(format!("m{k}"));
            let back = prowave_project::load(&saved, &media).unwrap();
            let (q, _) = from_doc(&back, &media, &ctx);
            let (a, b) = (to_doc(&p).0, to_doc(&q).0);
            assert_eq!(a.tracks.len(), b.tracks.len(), "{}", pack.id);
            for (x, y) in a.tracks.iter().zip(&b.tracks) {
                assert_eq!(x.au_effects, y.au_effects, "{}/{}", pack.id, x.name);
                assert_eq!(x.fx, y.fx, "{}/{}", pack.id, x.name);
                assert_eq!(x.sends, y.sends, "{}/{}", pack.id, x.name);
                assert_eq!(x.regions.len(), y.regions.len(), "{}/{}", pack.id, x.name);
                let plain = |d: &Option<PluginDoc>| d.as_ref().map(|d| d.id.clone());
                assert_eq!(plain(&x.au_instrument), plain(&y.au_instrument));
            }
            assert_eq!(a.master_fx, b.master_fx, "{}", pack.id);
            assert_eq!(a.master_plugins, b.master_plugins, "{}", pack.id);
            assert_eq!(a.notes, b.notes, "{}", pack.id);
            assert_eq!(a.tempo, b.tempo);
        }
    }

    /// The public edition's packages (`build_templates.py --public`).
    const PUBLIC_DIR: &str = "templates";

    /// The public edition: every package parses as a project document and opens as a
    /// project with all its tracks.
    #[test]
    fn public_templates_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PUBLIC_DIR);
        let mut n = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert!(v["template"]["id"].as_str().is_some(), "{}", path.display());
            let doc: ProjectDoc = serde_json::from_value(v["project"].take())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let (p, warnings) = from_doc(&doc, &std::env::temp_dir(), &Default::default());
            assert!(warnings.is_empty(), "{}: {warnings:?}", path.display());
            assert_eq!(p.tracks.len(), doc.tracks.len(), "{}", path.display());
            n += 1;
        }
        assert_eq!(n, SOURCES.len(), "one public package per template");
    }

    /// Every drum channel of every public package sounds: its one-shot
    /// (`$TEMPLATE_SAMPLES/…`, taken here from the packages' `samples` folder) loads into the
    /// engine's Channel Rack and a hit renders audio (offline, nothing is played); and the
    /// drum pattern holds a starter beat.
    #[test]
    fn public_drum_racks_sound() {
        use prowave_engine::{BlockProcessor, SampleRack, RACK_BASE_NOTE};
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PUBLIC_DIR);
        let samples = base.join("samples").to_string_lossy().into_owned();
        let mut racks = 0;
        for entry in std::fs::read_dir(&base).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let mut v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let doc: ProjectDoc = serde_json::from_value(v["project"].take()).unwrap();
            let (p, _) = from_doc(&doc, &std::env::temp_dir(), &Default::default());
            for (t, d) in p.tracks.iter().zip(&doc.tracks) {
                let Some(slot) = t
                    .au_instrument
                    .as_ref()
                    .filter(|s| crate::channel_rack::is_rack(s))
                else {
                    continue;
                };
                racks += 1;
                let mut slot = slot.clone();
                let text = String::from_utf8(slot.state.clone().unwrap()).unwrap();
                assert!(text.contains("$TEMPLATE_SAMPLES/"), "{}", path.display());
                slot.state = Some(text.replace("$TEMPLATE_SAMPLES", &samples).into_bytes());
                let specs = crate::channel_rack::specs(&slot);
                let (mut rack, errors) = SampleRack::new(&specs, 48_000);
                assert!(errors.is_empty(), "{}: {errors:?}", path.display());
                for k in 0..specs.len() {
                    rack.midi(0x90, RACK_BASE_NOTE + k as u8, 100, 0);
                    let (mut l, mut r) = (vec![0f32; 4800], vec![0f32; 4800]);
                    rack.process((&[], &[]), (&mut l, &mut r));
                    let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
                    assert!(
                        peak > 0.05 && peak.is_finite(),
                        "{} / {} channel {k}: peak {peak}",
                        path.display(),
                        d.name
                    );
                }
                let pattern = d.regions[0].pattern.as_ref().unwrap();
                let hits: usize = pattern
                    .rows
                    .iter()
                    .map(|r| r.steps.iter().filter(|v| **v > 0).count())
                    .sum();
                assert!(
                    hits >= 4,
                    "{} / {}: no starter beat",
                    path.display(),
                    d.name
                );
            }
        }
        assert!(racks >= 25, "{racks} drum racks");
    }

    /// A package's project finds the built-in one-shots: no `$TEMPLATE_SAMPLES` is left in
    /// its slots, and every file its Channel Racks name is there.
    #[test]
    fn built_in_samples_are_written_and_found() {
        for pack in packs().iter().filter(|p| p.source.contains(SAMPLES_TOKEN)) {
            let p = pack.project();
            for slot in p.tracks.iter().filter_map(|t| t.au_instrument.as_ref()) {
                let text = String::from_utf8_lossy(slot.state.as_deref().unwrap_or_default());
                assert!(!text.contains(SAMPLES_TOKEN), "{}", pack.id);
                if crate::channel_rack::is_rack(slot) {
                    for c in crate::channel_rack::decode(&slot.state) {
                        let missing = !c.path.is_empty() && !Path::new(&c.path).is_file();
                        assert!(!missing, "{}: {}", pack.id, c.path);
                    }
                }
            }
        }
    }

    /// A package whose plug-ins are not installed still opens: the slots keep their ids and
    /// names (the engine reports them as not installed and opens the rest).
    #[test]
    fn missing_plugins_keep_their_slots() {
        let Some(pack) = packs().first() else {
            return;
        };
        let mut doc = pack.doc().unwrap();
        for t in doc.tracks.iter_mut() {
            for s in t.au_effects.iter_mut() {
                s.id = "aufx:Nope:Gone".into();
            }
        }
        let (p, warnings) = from_doc(
            &doc,
            std::path::Path::new("/nonexistent"),
            &Default::default(),
        );
        assert!(warnings.is_empty());
        for (t, d) in p.tracks.iter().zip(&doc.tracks) {
            for (s, ds) in t.au_effects.iter().zip(&d.au_effects) {
                assert_eq!(s.id, "aufx:Nope:Gone");
                assert_eq!(s.name, ds.name, "the slot shows the plug-in's name");
            }
        }
    }

    /// Every built-in plug-in a package names (`prowave:…`) is one this program has.
    #[test]
    fn every_built_in_plugin_exists() {
        let instruments = ["sampler", "rack"];
        for p in packs() {
            let doc = p.doc().unwrap();
            let slots = doc
                .tracks
                .iter()
                .flat_map(|t| t.au_instrument.iter().chain(&t.au_effects))
                .chain(&doc.master_plugins);
            for s in slots {
                if let Some(native) = s.id.strip_prefix("prowave:") {
                    assert!(
                        prowave_engine::native_info(&s.id).is_some()
                            || instruments.contains(&native),
                        "{}: {} ({})",
                        p.id,
                        s.name,
                        s.id
                    );
                }
            }
        }
    }
}
