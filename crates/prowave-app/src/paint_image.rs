//! Paint Image to Audio (File > Import, or the Tracks area's right-click menu): a picture
//! becomes a sound whose spectrogram shows it, MetaSynth's Image Synth run backwards. The
//! picture's columns are time, its rows frequency (bottom = low) and its brightness loudness;
//! the sound is worked out on a worker thread (`prowave_engine::paint`, fast Griffin–Lim),
//! saved as a WAV file next to the recent projects and added as a new audio track at the
//! playhead, and the Spectrogram window opens on it so the picture can be seen.
use crate::i18n::{tr, trf};
use crate::model::Project;
use prowave_engine::{FreqScale, PaintSettings, Picture, MAX_PAINT_SECONDS};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

/// The longest side a picture is read at: larger ones are scaled down first (the sound has
/// fewer frames and frequency bins than that anyway).
const MAX_SIDE: u32 = 2048;
/// The side of the preview in the dialog, in points.
const PREVIEW: f32 = 220.0;
/// The picture file types offered.
pub const EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

/// Paint Image to Audio: the dialog and the paintings running.
#[derive(Default)]
pub struct PaintState {
    /// The menu entry was chosen: ask for a picture next frame.
    pub pick: bool,
    dialog: Option<Dialog>,
    jobs: Vec<Job>,
}

/// The options dialog for one picture.
struct Dialog {
    name: String,
    picture: Arc<Picture>,
    /// The picture's own size, before any scaling down.
    size: (u32, u32),
    preview: Option<egui::TextureHandle>,
    settings: PaintSettings,
}

struct Job {
    name: String,
    progress: Arc<AtomicU32>,
    result: Receiver<Result<PathBuf, String>>,
}

impl PaintState {
    /// What the running paintings are doing, for the status line.
    pub fn progress(&self) -> Option<String> {
        let job = self.jobs.first()?;
        let done = job.progress.load(Ordering::Relaxed) / 10;
        Some(trf("Painting {}: {}%", &[&job.name, &done]))
    }

    pub fn running(&self) -> bool {
        !self.jobs.is_empty()
    }
}

/// The picture at `path` as brightness (transparent pixels count as black), at most
/// [`MAX_SIDE`] on a side, and its own size.
pub fn load_picture(path: &Path) -> Result<(Picture, (u32, u32)), String> {
    let img = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let size = (img.width(), img.height());
    let img = if size.0 > MAX_SIDE || size.1 > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let la = img.to_luma_alpha32f();
    let (width, height) = (la.width() as usize, la.height() as usize);
    let luma = la.pixels().map(|p| p.0[0] * p.0[1]).collect();
    Ok((
        Picture {
            width,
            height,
            luma,
        },
        size,
    ))
}

/// The length a picture is painted at unless changed: 8 seconds for a square one, longer
/// for a wide one and shorter for a tall one (so it keeps its shape on a spectrogram about
/// twice as wide as it is tall), from 2 to 30 seconds.
pub fn default_seconds(width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 8.0;
    }
    (8.0 * width as f32 / height as f32).clamp(2.0, 30.0)
}

/// Paints `picture` and saves it as a stereo WAV file at `path` (the same sound in both
/// channels, so it cannot cancel when summed to mono).
pub fn render(
    picture: &Picture,
    settings: &PaintSettings,
    path: &Path,
    progress: &AtomicU32,
) -> Result<(), String> {
    let mono = prowave_engine::paint(picture, settings, progress);
    let stereo: Vec<f32> = mono.iter().flat_map(|v| [*v, *v]).collect();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    prowave_engine::write_wav(path, &stereo, settings.clamped().sample_rate)
        .map_err(|e| e.to_string())
}

/// Where the painting of picture `name` is saved.
fn out_path(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    crate::recent_files::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("painted")
        .join(format!("{safe}-{stamp}.wav"))
}

/// Asks for a picture and opens the options dialog for it.
fn pick(s: &mut crate::UiState) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter(tr("Image"), &EXTENSIONS)
        .set_title(tr("Paint Image to Audio"))
        .pick_file()
    else {
        return;
    };
    match load_picture(&path) {
        Ok((picture, size)) => {
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("Image")
                .to_string();
            let settings = PaintSettings {
                seconds: default_seconds(picture.width, picture.height),
                ..PaintSettings::default()
            };
            s.paint.dialog = Some(Dialog {
                name,
                picture: Arc::new(picture),
                size,
                preview: None,
                settings,
            });
        }
        Err(e) => s.status = Some(trf("cannot read the image {}: {}", &[&path.display(), &e])),
    }
}

/// Starts painting the dialog's picture on a worker thread.
fn start(s: &mut crate::UiState, d: &Dialog) {
    let progress = Arc::new(AtomicU32::new(0));
    let (tx, result) = channel();
    let (picture, settings, path) = (d.picture.clone(), d.settings, out_path(&d.name));
    let job_progress = progress.clone();
    let spawned = std::thread::Builder::new()
        .name("prowave-paint".into())
        .spawn(move || {
            let done = render(&picture, &settings, &path, &job_progress).map(|()| path);
            let _ = tx.send(done);
        });
    match spawned {
        Ok(_) => s.paint.jobs.push(Job {
            name: d.name.clone(),
            progress,
            result,
        }),
        Err(e) => s.status = Some(trf("Paint Image to Audio failed: {}", &[&e])),
    }
}

/// A grey picture of `pic` for the dialog, at most `side` pixels on a side.
fn preview_image(pic: &Picture, side: usize) -> egui::ColorImage {
    let scale = (side as f32 / pic.width.max(pic.height).max(1) as f32).min(1.0);
    let w = ((pic.width as f32 * scale).round() as usize).max(1);
    let h = ((pic.height as f32 * scale).round() as usize).max(1);
    let mut grey = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let sx = (x * pic.width / w).min(pic.width.saturating_sub(1));
            let sy = (y * pic.height / h).min(pic.height.saturating_sub(1));
            let l = pic.luma.get(sy * pic.width + sx).copied().unwrap_or(0.0);
            grey.push((l.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    egui::ColorImage::from_gray([w, h], &grey)
}

/// The options: length, frequency range and scale, brightness curve, invert, level, rounds.
fn options(ui: &mut egui::Ui, st: &mut PaintSettings) {
    egui::Grid::new("paint-options")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(tr("Length"));
            ui.add(
                egui::DragValue::new(&mut st.seconds)
                    .range(0.25..=MAX_PAINT_SECONDS)
                    .speed(0.1)
                    .suffix(" s"),
            );
            ui.end_row();
            let nyquist = st.sample_rate as f32 / 2.0;
            ui.label(tr("Lowest frequency"));
            ui.add(
                egui::DragValue::new(&mut st.low_hz)
                    .range(1.0..=nyquist * 0.5)
                    .speed(1.0)
                    .suffix(" Hz"),
            );
            ui.end_row();
            ui.label(tr("Highest frequency"));
            ui.add(
                egui::DragValue::new(&mut st.high_hz)
                    .range(100.0..=nyquist * 0.995)
                    .speed(10.0)
                    .suffix(" Hz"),
            );
            ui.end_row();
            ui.label(tr("Frequency scale"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut st.scale, FreqScale::Log, tr("Logarithmic"));
                ui.radio_value(&mut st.scale, FreqScale::Linear, tr("Linear"));
            });
            ui.end_row();
            ui.label(tr("Brightness curve (gamma)"));
            ui.add(egui::Slider::new(&mut st.gamma, 0.2..=3.0));
            ui.end_row();
            ui.label(tr("Dynamic range"));
            ui.add(egui::Slider::new(&mut st.range_db, 20.0..=100.0).suffix(" dB"));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut st.invert, tr("Invert (dark is loud)"));
            ui.end_row();
            ui.label(tr("Output level"));
            ui.add(egui::Slider::new(&mut st.level_db, -24.0..=0.0).suffix(" dBFS"));
            ui.end_row();
            ui.label(tr("Phase rounds (cleaner picture, slower)"));
            ui.add(egui::Slider::new(&mut st.iterations, 0..=100));
            ui.end_row();
        });
    if st.low_hz >= st.high_hz * 0.9 {
        st.low_hz = st.high_hz * 0.5;
    }
}

/// Once per frame: asks for a picture when the entry was chosen, draws the dialog, and adds
/// finished paintings as tracks.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    if std::mem::take(&mut s.paint.pick) {
        pick(s);
    }
    if let Some(mut d) = s.paint.dialog.take() {
        let (mut keep, mut cancel, mut go) = (true, false, false);
        let preview = d
            .preview
            .get_or_insert_with(|| {
                ctx.load_texture(
                    "paint-preview",
                    preview_image(&d.picture, PREVIEW as usize * 2),
                    egui::TextureOptions::LINEAR,
                )
            })
            .clone();
        egui::Window::new(tr("Paint Image to Audio"))
            .collapsible(false)
            .resizable(false)
            .open(&mut keep)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let size = preview.size_vec2();
                    let fit = (PREVIEW / size.x.max(size.y).max(1.0)).min(1.0);
                    ui.add(egui::Image::new(&preview).fit_to_exact_size(size * fit));
                    ui.vertical(|ui| {
                        ui.strong(&d.name);
                        ui.label(trf("{} × {} pixels", &[&d.size.0, &d.size.1]));
                        ui.add_space(6.0);
                        options(ui, &mut d.settings);
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    cancel = ui.button(tr("Cancel")).clicked();
                    if ui.button(tr("Paint")).clicked() {
                        go = true;
                    }
                });
            });
        if go {
            start(s, &d);
        } else if keep && !cancel {
            s.paint.dialog = Some(d);
        }
    }
    poll(ctx, p, s);
}

/// Finished paintings become audio tracks at the playhead.
fn poll(ctx: &egui::Context, p: &mut Project, s: &mut crate::UiState) {
    if s.paint.jobs.is_empty() {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let mut i = 0;
    while i < s.paint.jobs.len() {
        let done = match s.paint.jobs[i].result.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => {
                i += 1;
                continue;
            }
            Err(TryRecvError::Disconnected) => Err("the painting stopped".into()),
        };
        let job = s.paint.jobs.remove(i);
        let added = done
            .and_then(|path| crate::import::import_audio(p, &path, ctx).map_err(|e| e.to_string()));
        match added {
            Ok(()) => {
                let t = p.tracks.len() - 1;
                if let Some(track) = p.tracks.get_mut(t) {
                    track.name = job.name.clone();
                    if let Some(r) = track.regions.first_mut() {
                        r.name = job.name.clone();
                    }
                }
                s.status = Some(trf("painted {} into a new audio track", &[&job.name]));
                crate::spectrogram_view::open(p, s, t, 0);
            }
            Err(e) => s.status = Some(trf("Paint Image to Audio failed: {}", &[&e])),
        }
    }
    if let Some(progress) = s.paint.progress() {
        s.status = Some(progress);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_png_is_read_as_brightness_and_painted_to_a_stereo_wav_with_its_tone() {
        let dir = tempfile::tempdir().unwrap();
        // 40 × 100: black, one white row a quarter of the way up, a half-transparent pixel.
        let mut img = image::RgbaImage::new(40, 100);
        for x in 0..40 {
            img.put_pixel(x, 75, image::Rgba([255, 255, 255, 255]));
        }
        img.put_pixel(0, 0, image::Rgba([255, 255, 255, 0]));
        let png = dir.path().join("line.png");
        img.save(&png).unwrap();
        let (pic, size) = load_picture(&png).unwrap();
        assert_eq!(size, (40, 100));
        assert_eq!((pic.width, pic.height), (40, 100));
        assert!((pic.luma[75 * 40 + 3] - 1.0).abs() < 1e-3);
        assert_eq!(pic.luma[0], 0.0, "transparent counts as black");

        let settings = PaintSettings {
            seconds: 1.0,
            iterations: 4,
            ..PaintSettings::default()
        };
        let wav = dir.path().join("out").join("line.wav");
        let progress = AtomicU32::new(0);
        render(&pic, &settings, &wav, &progress).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), 1000);
        let file = prowave_engine::AudioFile::open(&wav).unwrap();
        assert_eq!(
            (file.channels, file.sample_rate, file.frames),
            (2, 48_000, 48_000)
        );
        let stereo = file.read_stereo().unwrap();
        let mono: Vec<f32> = stereo.chunks(2).map(|f| f[0]).collect();
        assert!(
            stereo.chunks(2).all(|f| f[0] == f[1]),
            "same in both channels"
        );
        let sp = prowave_engine::spectrogram(&mono, 48_000, 4096, 1024);
        let m = sp.frames / 2;
        let k = (0..sp.bins)
            .max_by(|a, b| sp.at(m, *a).total_cmp(&sp.at(m, *b)))
            .unwrap();
        let want = settings.hz_at(1.0 - 75.5 / 100.0);
        let got = sp.bin_hz(k as f32);
        assert!((got / want - 1.0).abs() < 0.05, "{got} Hz vs {want} Hz");
    }

    #[test]
    fn the_default_length_follows_the_shape_of_the_picture() {
        assert_eq!(default_seconds(500, 500), 8.0);
        assert_eq!(default_seconds(1000, 500), 16.0);
        assert_eq!(default_seconds(100, 1000), 2.0);
        assert_eq!(default_seconds(10_000, 100), 30.0);
        assert_eq!(default_seconds(0, 0), 8.0);
    }

    #[test]
    fn the_preview_fits_inside_its_square() {
        let pic = Picture {
            width: 1000,
            height: 250,
            luma: vec![0.5; 250_000],
        };
        let img = preview_image(&pic, 200);
        assert_eq!(img.size, [200, 50]);
        assert!(img.pixels.iter().all(|c| c.r() == 128));
    }
}
