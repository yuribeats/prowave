# Pro Wave — Linux & macOS Lightweight Implementation Spec
**Goal: Super lightweight, pure Rust, works on Linux and macOS, 10-20x lighter than official 【entity-Logic¦canonical_name=Logic】 Pro**

## 1. Why is ProWave Lightweight

【entity-Logic¦canonical_name=Logic】 Pro: 1.2GB app + 70GB library, Objective-C++ with 30 years of legacy, CoreData, NSKeyedArchiver, loads everything on startup, 1.5GB RAM idle, 3-4s cold start, macOS only.

ProWave:
- **Binary:** <50MB single binary (Rust release + LTO + strip + mimalloc)
- **RAM idle:** <150MB with 0 tracks (vs 1.5GB 【entity-Logic¦canonical_name=Logic】)
- **Cold start:** <400ms (vs 3-4s) — lazy loading, no ObjC runtime, rkyv zero-copy
- **Library:** <100MB essentials (IRs, wavetables), not 70GB. Optional: read from `/Library/Application Support/Logic/` if user has 【entity-Logic¦canonical_name=Logic】 installed
- **Engine:** Same `prowave-engine` core for native and WASM, no duplication

## 2. Linux — How to Make it Work

### Audio I/O
- `cpal` crate with backends: ALSA (default), JACK (pro), PipeWire (via ALSA/JACK compat)
- Do not use CoreAudio, do not use AU. Only CLAP (primary, open, Rust-friendly) and VST3 via `vst3-sys`
- Latency: ALSA 128 samples @ 48kHz = ~8ms RTL, JACK <8ms
- Permissions: no root, Flatpak with `--socket=pulseaudio --filesystem=xdg-Music:rw`

### Linux Dependencies
- `libasound2`, `libjack`, `vulkan` (via wgpu)
- No need for `libcoreaudio`, `AudioToolbox`
- Build: `cargo build --release --target x86_64-unknown-linux-gnu`, `cargo build --target aarch64-unknown-linux-gnu` for ARM Linux

### UI
- `winit` + `wgpu` with Vulkan (Linux) — not Metal
- `egui` — immediate mode, works the same on Linux and macOS
- No Finder bundle magic: `.logicx` is just a directory on Linux, `std::fs::read_dir`

### Linux Benchmarks
- `cargo test --workspace -- --nocapture` on Ubuntu 22.04 + Fedora
- `cargo xtask check` runs clippy, fmt, miri (for unsafe blocks)

## 3. macOS — How to Make it Work

### Audio I/O
- `cpal` with CoreAudio backend
- AU support via `coreaudio-sys` + `rust-au` (or `coreaudio-rs`). Only on macOS, with feature flag `#[cfg(target_os = "macos")]`
- Aggregate devices (like Logic)
- Metal via wgpu (wgpu automatically selects Metal on macOS)

### .logicx Bundle on macOS
- `.logicx` is a package (directory with extension that Finder shows as a file)
- Detect via `is_dir && contains Alternatives/ProjectData`
- No need for `xattr`, but to save a .logicx that Finder shows as a package, creating a dir with the `.logicx` extension is enough. Optional: set `com.apple.FinderInfo` package bit via `xattr`

### Signature & Notorization
- Hardened Runtime: entitlements `com.apple.security.device.audio-input`, `com.apple.security.files.user-selected.read-write`
- AU hosting requires `com.apple.security.cs.allow-unsigned-executable-memory` ONLY in the sandboxed process, NOT in main
- Notarization: `cargo xtask bundle --release` + `codesign` + `notarytool`

### Universal Binary
- `cargo build --target x86_64-apple-darwin --target aarch64-apple-darwin`
- `lipo -create -output ProWaveStudio target/x86_64.../prowave target/aarch64.../prowave`

## 4. Optimizations (Pure Rust)

1. **Allocator:** `mimalloc` (or `rpmalloc`) — 10-20% faster than system malloc, less fragmentation
2. **LTO + Strip:** `Cargo.toml`: `lto = true`, `strip = true`, `codegen-units = 1`
3. **Lazy Loading:** Don't load all plugins on startup. Only scan ProjectData (linear scan), instantiate channel strip when track is selected
4. **Disk Streaming:** Audio files via `memmap2` + prefetch thread, don't load everything into RAM
5. **Zero-copy:** `rkyv` for `project.json`, not `serde_json` which allocates
6. **No GC:** Rust has no GC, no pauses
7. **SIMD:** `wide` + `std::simd` for DSP, 4x faster than scalar

### Cargo.toml (workspace root)
```toml
[profile.release]
lto = true
codegen-units = 1
strip = true
panic = "abort"
opt-level = 3

[dependencies]
mimalloc = { version = "0.1", default-features = false }

### Main.rs
```rust
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    // <400ms startup: only init logger + parse args + create window
    // Defer loading project until after window shown
}
```

## 5. Crate Features for Linux/macOS

```toml
# crates/prowave-engine/Cargo.toml
[features]
default = []
macos-au = ["coreaudio-sys", "rust-au"] # only on macos
linux-jack = ["jack"] # only on linux

# Conditional compilation
#[cfg(target_os = "macos")]
mod au_host;
#[cfg(target_os = "linux")]
mod alsa_host;
```

## 6. Lightness Tests (for QA)

`cargo xtask bench` builds the release app, runs `prowave --bench <file>` and checks:

- release binary under 50 MB
- start to first frame under 400 ms
- memory with an empty project, idle, under 150 MB
- 24 software-instrument tracks at 48 kHz rendered by the engine in under 15 % of one CPU core,
  and memory under 300 MB

CI runs it on macOS and Windows on every push (`.github/workflows/ci.yml`, job `bench`). On
Windows the first run (2026-10-20, 4K display) gave 12.6 MB, 314 ms, 125 MB, 3.6 % and 125 MB.
What made the difference was the graphics API: OpenGL alone instead of starting Vulkan,
DirectX 12 and OpenGL together (`crates/prowave-app/src/graphics.rs`).

`prowave --layout-shots <folder>` saves pictures of the window at 1920x1080, 1470x920 (13-inch
MacBook Air), 1280x800 and 1024x700, to check that nothing overlaps on small screens.
- Compare with Logic Pro: same 24-track project in Logic = 1.2GB RAM, ProWave = 300MB

---

**For Claude:** Start with `prowave-engine` cross-platform (cpal + wgpu), then `prowave-logicx-import` with separate `macos` and `linux` features. Use `#[cfg]` for platform-specific code.
