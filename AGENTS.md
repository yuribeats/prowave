# AGENTS.md: how to work in this repository

For AI coding agents and people. Every statement here was checked against the code on
2026-11-09. If you find one that is no longer true, fix this file in the same change.

## What this is

Pro Wave is a digital audio workstation written in Rust. The interface is drawn with egui
(eframe); audio goes out through cpal, and on macOS through the project's own Core Audio
stream. The workflow follows Apple's Logic Pro. `ROADMAP.md` is the long-term plan; much of it
is not built. `README.md` has a status count made from the code (`cargo xtask status`).

## The crates

| Path | What it is | Main files |
| :--- | :--- | :--- |
| `crates/prowave-engine` | The real-time engine and everything that makes or changes sound. | `core.rs` (`EngineCore::process`, the `Command` enum), `output.rs` (cpal stream, the command sender), `duplex.rs` (macOS input and output in one callback), `wasapi.rs` (Windows output), `disk.rs` and `mapped.rs` (audio files streamed from disk), `capture.rs` (recording), `fx.rs`, `convolver.rs` and `native_fx/` (effects), `routing.rs` (sends, buses, automation), `synth.rs`, `soundfont.rs`, `sample_rack.rs` (instruments), `plugin_track.rs`, `au.rs`, `vst3.rs`, `plugin_handle.rs` (plug-in hosting, macOS), `prerender.rs` (playback threads), `tempo_map.rs`, `triple.rs` (lock-free state for the interface), `flex.rs`, `analysis.rs`, `note_render.rs` (offline audio work) |
| `crates/prowave-app` | The desktop program (binary `prowave`): every window, menu and editor. | `main.rs`, `model.rs` (the project in memory), `engine_sync.rs` (turns the project into engine commands), `project_io.rs` (open and save), `menus.rs`, `file_commands.rs`, `tracks_area.rs`, `mixer.rs`, `piano_roll.rs`, `transport.rs`, `plugins.rs` and `vst3_scan.rs` (plug-in lists, scan, crash list), `theme.rs` and `hw.rs` (the look), `i18n.rs` and `locales/` (translations) |
| `crates/prowave-project` | The `.prowave` file format: a zip with `project.json` and `media/`. | `lib.rs` |
| `xtask` | The checks, run as `cargo xtask <task>`. | `main.rs`, `bench.rs`, `status.rs` |


There are no other crates. Earlier versions of this file named `prowave-dsp`,
`prowave-instruments`, `prowave-midi`, `prowave-mixer`, `prowave-plugin-host`, `prowave-ui`,
`prowave-wasm` and `prowave-logicx-import`, and the traits `DspNode`, `Instrument` and
`EventBus`. None of them exist. `LOGICX-FORMAT-SPEC.md` is a document only: no code reads or
writes Logic projects.

## What does not exist

Do not assume any of these:

- **No plug-in sandbox.** Audio Units and VST3 plug-ins run inside the program (macOS only;
  there is no plug-in hosting on Windows or Linux). A plug-in that crashes takes Pro Wave with
  it. Two things limit the damage: each `.vst3` bundle is scanned in a helper process with a
  30-second limit (`vst3_scan.rs`), and a plug-in that crashed the program while opening is not
  opened automatically again (`plugins.rs`, "crash quarantine").
- **No CLAP hosting.**
- **No bounce, mixdown or audio export of a project.** The Bounce and Export menu entries are
  listed in `menus.rs` but are not enabled.
- **No autosave, and no "save changes?" question when the program quits.** New, Open and Close
  Project do ask.
- **No fuzzing.**
- **No browser version.** The only wasm work is a build check of the engine (see below).
- **No preset-compatibility tests** against Logic presets.

## Audio-thread rules

The audio callback (`EngineCore::process` in `core.rs` and what it calls) must not allocate or
free memory, take a lock, make a system call that can block (files, printing, logging) or
panic.

How this is enforced:

- **Allocation:** the callback runs inside `assert_no_alloc` (see `output.rs`). Debug builds
  and tests abort if it allocates or frees; see `output.rs` for what a release build does.
  Tests named `*does_not_allocate` cover single paths; `cargo xtask rt-check` runs them.
- **Panics:** `prowave-engine` has `#![deny(clippy::unwrap_used, clippy::expect_used)]`
  (`lib.rs`), so `unwrap` and `expect` fail clippy there. The other crates do not have it.
  Release builds use `panic = "abort"`.
- **Locks and system calls:** convention, not enforced. Review your own change.

File access in the engine is allowed, but only off the audio thread: `disk.rs`, `mapped.rs`
and `capture.rs` run their own threads and pass audio through lock-free rings. Plug-ins on
tracks that are not being played live run on separate real-time playback threads
(`prerender.rs`).

## How the interface talks to the engine

- **Commands:** the interface sends `Command` values (`core.rs`) through an `rtrb` queue; the
  audio thread takes them off the queue (`drain_commands`). Memory the audio thread no longer needs
  goes back through a second queue (`Garbage`) and is freed on the interface's side.
- **State back:** the audio thread publishes one `EngineState` per block through the triple
  buffer (`triple.rs`); a few values are atomics.
- **The exception:** a running plug-in's settings, presets, parameters and window are reached
  directly from the interface thread through `plugin_handle.rs` (`PluginHandle::state`,
  `set_state` and the rest), not through `Command`.

## Checks

`cargo xtask check` runs, in this order, and fails if any step fails:

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace -- --nocapture`
4. rt-check: `cargo test --workspace -- does_not_allocate --nocapture`
5. miri: `cargo +nightly miri test -p prowave-engine --lib triple` (the triple buffer's tests
   only; no other code runs under miri)
6. `cargo deny check advisories bans licenses sources` (the check fails if cargo-deny is not
   installed)
7. wasm-check: `cargo check -p prowave-engine --target wasm32-unknown-unknown` (a build check;
   no test runs on wasm)


Other tasks:

- `cargo xtask rt-check`, `cargo xtask wasm-check`: steps 4 and 7 alone.
- `cargo xtask status`: counts the enabled menu entries and the `ROADMAP.md` checkboxes, and
  rewrites the status block in `README.md` and `docs/status.svg`. Commit both.
- `cargo xtask bench`: builds the release program, starts it (it opens a window) and compares
  start-up time, memory, binary size and DSP load with `LINUX-MACOS-LIGHTWEIGHT.md`.

Tests that need something a clean machine does not have are marked `#[ignore = "reason"]` and
run with `-- --ignored`: picture tests, the 1000-stream disk test, tests that need a SoundFont
file (`PROWAVE_TEST_SF2`). A test must not return early and pass when its input is missing.

`.github/workflows/ci.yml` runs `cargo xtask check` on Linux, Windows and macOS, the bench, the
wasm check and release builds on every push to `main` and on pull requests.


## Rules for a change

- **New interface text:** every text passed to `tr("…")` or `trf("…")`, and every menu label,
  needs a line `English text = translation` in all 16 files of
  `crates/prowave-app/locales/`. The test `i18n::tests::every_language_translates_every_text`
  fails on a missing line, on a line no text uses, and on a different number of `{}`.
- **wasm:** `capture.rs` is not built for `wasm32`. Engine code that uses `crate::capture`
  must be under `#[cfg(not(target_arch = "wasm32"))]`, or wasm-check fails. The same goes for
  anything that needs threads, files or a native library.
- **`unsafe`:** write a `// SAFETY:` comment saying why it is sound. Convention, not enforced.
- **Naming:** a file is named for what it does (`tempo_map.rs`, `gain_smoothing.rs`), not
  `utils.rs`. Convention, not enforced.
- **Commits:** small, one subject each, with a message that says what changed and why. There is
  no enforced limit on the size of a change or on how many crates it touches.
- **Dependencies:** `cargo deny` checks licences and advisories (`deny.toml`). The project is
  GPL-3.0-or-later.
- **Secrets:** none in the repository. Never commit certificates, keys or `.env` files.

## UI design system: Hardware (PW-1)

The app's look is "Hardware": a piece of studio gear, not a Logic-gray clone. Every window follows it.

- **Tokens:** `crates/prowave-app/src/theme.rs`. Never hard-code a colour in a panel; use the tokens (bone chassis `BONE`/`BONE2`/`BONE3`, seams `SEAM`, print `INK`/`INK2`/`PRINT`, the one signal colour `ORANGE`, LCD glass `LCD`/`LCD_INK`, black caps `CAP`, meter wells `METER_WELL`, text on dark `ON_DARK`). Text tokens are dark and surface tokens light; dark materials use `ON_DARK`. `theme::text_on(c)` picks text for any fill.
- **Type:** bundled OFL fonts, `theme::font(Face, size)`: `Silk` (Barlow Condensed caps, spaced) for labels and captions, `SilkX` for headings, `Bold` (JetBrains Mono) for key legends, menus and names, `Body` for text, `Lcd` for LCD text. Seven-segment digits for time, tempo and level readouts: `hw::seg`.
- **Controls:** custom-painted with `crates/prowave-app/src/hw.rs`: key caps (`hw::key`, bone/black/orange, LED dot), toggles, track pads, knobs (black, orange pointer, tick ring), faders with tick scales, segmented meters, LCD panels and readouts, insert/routing slots, dark value fields (dropdowns), faceplates, seams, screws, bracketed section labels. `hw-kit.png` (`cargo test -p prowave-app manual_pictures_hardware_kit -- --ignored`) shows them all.
- **Regions:** `theme::region_look` + `hw::paint_region`: black MIDI bodies with bone notes, warm-grey audio bodies with dark waveforms, a header bar in the track colour; selected = orange header and outline. New tracks take `theme::TRACK_PALETTE` tones.
- **Rules:** orange only for signal (selection, record, lit LEDs, playhead); labels in caps; keep hit targets at least 20 pt; no invented captions or hint text.
