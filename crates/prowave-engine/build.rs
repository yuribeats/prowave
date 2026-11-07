//! Compiles the WORLD vocoder (world/, modified BSD) for the Pitch Editor's resynthesis. The
//! browser build has no C++ compiler, so it is left out there (see src/vocoder.rs).
fn main() {
    println!("cargo:rerun-if-changed=world/src");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        return;
    }
    let files = [
        "cheaptrick",
        "common",
        "d4c",
        "dio",
        "fft",
        "harvest",
        "matlabfunctions",
        "stonemask",
        "synthesis",
    ];
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .include("world/src")
        .opt_level(3)
        .warnings(false);
    for f in files {
        build.file(format!("world/src/{f}.cpp"));
    }
    build.compile("world");
}
