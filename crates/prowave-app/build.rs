//! Lists the template packages (`templates/*.json`) for `genre_templates.rs`, in the order
//! `templates/order.txt` gives, with their one-shots (`templates/samples/*.wav`), and on
//! Windows embeds the Pro Wave icon and product name in
//! `prowave.exe`, so Explorer, shortcuts and taskbar pins show the logo. Other targets set
//! their icons at run time (`ViewportBuilder::with_icon`) or through their package files
//! (dist/linux, dist/macos).
use std::path::{Path, PathBuf};

fn main() {
    templates();
    windows_icon();
}

/// Writes `$OUT_DIR/templates.rs`: `const SOURCES: &[&str]` with every package built in, and
/// `const SAMPLES: &[(&str, &[u8])]` with the one-shots.
/// The ids in `order.txt` (one a line, `#` comments) come first, in that order; packages it
/// does not name follow by file name.
fn templates() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    let order_file = dir.join("order.txt");
    println!("cargo:rerun-if-changed={}", order_file.display());
    let order: Vec<String> = std::fs::read_to_string(&order_file)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    let rank = |p: &PathBuf| {
        let id = p.file_stem().map(|s| s.to_string_lossy().into_owned());
        id.and_then(|id| order.iter().position(|o| *o == id))
            .unwrap_or(order.len())
    };
    files.sort_by_key(|p| rank(p));
    let mut out = String::from("/// The packages, in the order the chooser shows them.\n");
    out.push_str("const SOURCES: &[&str] = &[\n");
    for f in &files {
        out.push_str(&format!(
            "    include_str!({:?}),\n",
            f.display().to_string()
        ));
    }
    out.push_str("];\n");
    // The one-shots the packages' drum racks play (`templates/samples/*.wav`).
    let samples_dir = dir.join("samples");
    println!("cargo:rerun-if-changed={}", samples_dir.display());
    let mut samples: Vec<PathBuf> = std::fs::read_dir(&samples_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "wav"))
                .collect()
        })
        .unwrap_or_default();
    samples.sort();
    out.push_str("/// The built-in one-shots: file name and contents.\n");
    out.push_str("const SAMPLES: &[(&str, &[u8])] = &[\n");
    for f in &samples {
        println!("cargo:rerun-if-changed={}", f.display());
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        out.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            name,
            f.display().to_string()
        ));
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap_or_default()).join("templates.rs");
    if let Err(e) = std::fs::write(&dest, out) {
        panic!("cannot write {}: {e}", dest.display());
    }
}

fn windows_icon() {
    println!("cargo:rerun-if-changed=assets/prowave.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/prowave.ico")
        .set("ProductName", "Pro Wave")
        .set("FileDescription", "Pro Wave");
    // An icon is not worth failing a build over (e.g. no Windows SDK resource compiler).
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
}
