//! A folder of real records: every audio file in it decodes, and how long that takes.
//!
//! Not part of the normal run (it needs a folder of records). Read only: nothing in the folder
//! is changed. Run with
//! `cargo test -p prowave-engine --release --test sampler_folder -- --ignored --nocapture`,
//! and `PROWAVE_SAMPLE_FOLDER=/some/folder` for another folder than [`DEFAULT`].
use prowave_engine::sample_tools;
use std::time::Instant;

/// The folder looked at, under the home folder.
const DEFAULT: &str = "Music";

#[test]
#[ignore]
fn every_file_in_the_sample_folder_decodes() {
    let dir = std::env::var("PROWAVE_SAMPLE_FOLDER").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/{DEFAULT}")
    });
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("{dir} is not here; nothing to check");
        return;
    };
    let audio = ["mp3", "wav", "aif", "aiff", "flac", "m4a"];
    let (mut files, mut skipped) = (Vec::new(), Vec::new());
    for p in entries.flatten().map(|e| e.path()) {
        let ok = p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| audio.contains(&e.to_ascii_lowercase().as_str()));
        if ok {
            files.push(p);
        } else {
            skipped.push(p);
        }
    }
    files.sort();
    let all = Instant::now();
    let (mut failed, mut seconds, mut slowest) = (Vec::new(), 0.0, (0.0f64, String::new()));
    let mut heads = Vec::new();
    for f in &files {
        let t = Instant::now();
        let mut head_at = None;
        match sample_tools::decode_with_head(
            f,
            2.0,
            |_| head_at = Some(t.elapsed().as_secs_f64() * 1000.0),
            |_| true,
        ) {
            Ok(b) => {
                seconds += b.seconds();
                let took = t.elapsed().as_secs_f64();
                if took > slowest.0 {
                    slowest = (took, f.display().to_string());
                }
                if b.frames() == 0 {
                    failed.push(format!("{}: empty", f.display()));
                }
            }
            Err(e) => failed.push(format!("{}: {e}", f.display())),
        }
        heads.extend(head_at);
    }
    let total = all.elapsed().as_secs_f64();
    heads.sort_by(f64::total_cmp);
    eprintln!(
        "{} audio files ({} other files skipped), {:.0} s of audio decoded in {total:.1} s \
         ({:.0}x real time); slowest {:.2} s ({}); first 2 s ready in median {:.1} ms, \
         worst {:.1} ms",
        files.len(),
        skipped.len(),
        seconds,
        seconds / total.max(1e-9),
        slowest.0,
        slowest.1,
        heads.get(heads.len() / 2).copied().unwrap_or(0.0),
        heads.last().copied().unwrap_or(0.0),
    );
    let t = Instant::now();
    for f in files.iter().take(5) {
        if let Ok(b) = sample_tools::decode(f, |_| true) {
            let bpm = sample_tools::estimate_bpm(&b);
            let key = sample_tools::estimate_key(&b);
            eprintln!(
                "  {bpm:.1?} BPM, key {key:?}: {}",
                f.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }
    eprintln!(
        "5 files decoded, tempo and key found in {:.1} s",
        t.elapsed().as_secs_f64()
    );
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
