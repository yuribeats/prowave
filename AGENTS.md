# AGENTS.md: how to work in this repository

For AI coding agents and people. Every statement here was checked against the code on
2026-11-09 (the bounce entry again on 2026-11-18). If you find one that is no longer true, fix this file in the same change.

## What this is

Pro Wave is a digital audio workstation written in Rust. The interface is drawn with egui
(eframe); audio goes out through cpal, and on macOS through the project's own Core Audio
stream. The workflow follows Apple's Logic Pro. `ROADMAP.md` is the long-term plan; much of it
is not built. `README.md` has a status count made from the code (`cargo xtask status`).

## The crates

| Path | What it is | Main files |
| :--- | :--- | :--- |
| `crates/prowave-engine` | The real-time engine and everything that makes or changes sound. | `core.rs` (`EngineCore::process`, the `Command` enum), `output.rs` (cpal stream, the command sender), `duplex.rs` (macOS input and output in one callback), `wasapi.rs` (Windows output), `disk.rs` and `mapped.rs` (audio files streamed from disk), `capture.rs` (recording), `fx.rs`, `convolver.rs` and `native_fx/` (effects), `routing.rs` (sends, buses, automation), `synth.rs`, `soundfont.rs`, `sample_rack.rs` (instruments), `live_groove.rs` (Groove Live: keys played live held back to a late groove step), `arp.rs` (the Arpeggiator's note generator: it makes the notes of a track's regions ahead of time for the app's `midi_fx.rs`, and plays the keys played live inside `core.rs`), `plugin_track.rs`, `au.rs`, `vst3.rs`, `plugin_handle.rs` (plug-in hosting, macOS), `prerender.rs` (playback threads), `tempo_map.rs`, `triple.rs` (lock-free state for the interface), `flex.rs`, `analysis.rs`, `note_render.rs` (offline audio work), `reference_match/` (Match to Reference: the Matchering port; its parity test runs the Python original from `tools/matchering/`) |
| `crates/prowave-app` | The desktop program (binary `prowave`): every window, menu and editor. | `main.rs`, `model.rs` (the project in memory), `engine_sync.rs` (turns the project into engine commands), `project_io.rs` (open and save), `menus.rs`, `file_commands.rs`, `autosave.rs` and `recovery.rs` (recovery copies, the session folder), `crash_log.rs`, `tracks_area.rs`, `mixer.rs`, `piano_roll.rs`, `transport.rs`, `plugins.rs` and `vst3_scan.rs` (plug-in lists, scan, crash list, the hidden list: `plugin-hidden` and `plugin-hide-apple` in the settings folder), `plugin_ratings.rs` (the ratings and types in `plugin-ratings.json` that the Instrument and Audio FX menus group, sort and colour by), `channel_rack.rs` and the Kit Browser on its Kits key (`kit_browser.rs` the window, `kit_library.rs` the index of the library folders, made on background threads and cached with a listing of every kit's files, and the kit list (`KitList`, the `kit_list` setting) that narrows the browser to a ranked TSV of kits, `kit_files.rs` one kit's files and its SFZ, Hydrogen or DrumGizmo definition, `kit_classify.rs` what a sample is and which channel it goes on; `kit_copy.rs` the copies on this computer of kits loaded from a network share; `rack_groove.rs` each drum class's swing and micro-timing offsets, the cells of split steps, the swing and offset amounts, and the moving of the rack's notes to them; `rack_follow.rs` Follow Rack Groove: a MIDI track's notes moved, when the song is made and in the MIDI export, onto the hits of a rack's drum class as the rack plays them (played times, no averaging), and Auto's choice of class; Groove Q, the quantize toward those hits, and the tables of Groove Live; its audio version, Groove Flex, is in `flex_edit.rs`), `theme.rs` and `hw.rs` (the look), `i18n.rs` and `locales/` (translations) |
| `crates/prowave-project` | The `.prowave` file format: a zip with `project.json` and `media/`. | `lib.rs` |
| `xtask` | The checks, run as `cargo xtask <task>`. | `main.rs`, `bench.rs`, `status.rs` |
| `crates/prowave-plug` | The built-in instruments and effects as VST3, CLAP and Audio Unit plug-ins, one bundle each (nih-plug). A workspace of its own, not a member of this one: see its `README.md`. | `core.rs` (the audio thread), `params.rs`, `editor/`, `tools/build-plugins.sh` |

The Singer's and Vocal Top Line's voice engines are vendored, not workspace members, and not
linted or tested as ours: `crates/chimera` (CHIMERA, Rust, MIT, upstream eeb0c19),
`crates/chimera-09fb776` (the same engine at its first vendored commit, kept for its sound) and
`crates/novox` (Novox The Robot, C++ built with `cc` behind a small C interface, MIT). Each has a
NOTICE naming its upstream commit and what was changed. Pro Wave's code around them is
`chimera_singer.rs` and `novox_singer.rs` in the app, which are linted and tested.
The native instruments (`crates/prowave-engine/src/native_inst/`) are one table, as the native
effects are (`native_fx/`): an instrument is a `NativeInst` (notes in, audio out) listed in
`native_inst/mod.rs`, which also holds what every instrument needs around its sound (the MIDI
queue `Hosted`, `voices.rs`, `envelope.rs`, and `rate.rs` for an engine built for one sample
rate). The app's window for all of them is `native_inst_ui.rs`. Their engines that are
third-party code are vendored the same way as the voice engines, each with a NOTICE:
`crates/circuit-dsp` and `crates/mi-plaits-dsp` (Rust), `crates/mi-dsp` and `crates/chip-dsp`
(C++ built with `cc` behind a C interface; not in the wasm build). Instruments ported to Rust
from GPL programs name their source in the file's header.

The control interface: the running app takes requests on a Unix socket (`control.sock` in the
settings folder) so that another program on the Mac, such as an AI coding agent, can drive it.
The app's side is `crates/prowave-app/src/control_api/` (`registry.rs` is the one list of
methods; `mod.rs` says how a request travels), the client and MCP server is the crate
`crates/prowavectl`, and `docs/CONTROL-API.md` is the reference (its method table is made from
the registry; a test fails when it is stale).

`crates/prowave-plug` is left out of this workspace (`exclude` in `Cargo.toml`), so
`cargo xtask check`, `--workspace` and `cargo deny` do not cover it; check it with
`cargo clippy --manifest-path crates/prowave-plug/Cargo.toml --all-targets -- -D warnings` and
`cargo test --manifest-path crates/prowave-plug/Cargo.toml`, and build the bundles with
`bash crates/prowave-plug/tools/build-plugins.sh OUT` (its README says what that needs).

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
  opened automatically again (`plugins.rs`, "crash quarantine"). A project with unsaved
  changes has a recovery copy written every two minutes, offered at the next start
  (`autosave.rs`, `recovery.rs`); a panic is written to `crash.log` in the settings folder
  (`crash_log.rs`); and a block a plug-in returns with a NaN or an infinity in it is made
  silent (`plugin_track.rs`).
- **No CLAP hosting.**
- **No second renderer.** The bounce (`bounce.rs`: File > Bounce > Project or Section, File >
  Export > All Tracks as Audio Files, `export.bounce`) plays the project through
  `EngineCore::process` itself, on an engine that has no sound card (`Engine::offline`), set
  up by `EngineSync` and `PluginHost` as the running one is. Do not add code that renders a
  project another way: it would leave things out. Bounce > Regions in Place, Tracks in Place,
  Join of audio from different files and Convert Regions to New Audio Files use it too
  (`bounce_in_place.rs`). Track as Audio File and MP3 or FLAC files are not built.
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

- **Tokens:** `crates/prowave-app/src/theme.rs`. Never hard-code a colour in a panel; use the tokens (chassis `BONE`/`BONE2`/`BONE3`, seams `SEAM`, print `INK`/`INK2`/`PRINT`, the one signal colour `ORANGE`, print on it `ON_ACCENT`, LCD glass `LCD`/`LCD_INK`, black caps `CAP`, meter wells `METER_WELL`, text on dark `ON_DARK`). The tokens are functions (`theme::INK()`) that answer for the look in use: Graphite Amber (the default, dark) or Bone (View > Theme, `theme::current()`). The names are the Bone look's materials: in Graphite Amber `BONE` is charcoal, `INK` light and `ORANGE` amber. Text tokens always read on surface tokens; dark materials use `ON_DARK`, the signal colour `ON_ACCENT`. `theme::text_on(c)` picks text for any fill; a painter's own shades go through `theme::pick(bone, graphite)`, highlight lines through `theme::emboss(alpha)`. The meters' segments are `theme::METER_LO`/`METER_MID`/`METER_HOT` (olive, amber, orange) in both looks. Pictures of one look: `theme::set_for_thread`.
- **Type:** bundled OFL fonts, `theme::font(Face, size)`: `Silk` (Barlow Condensed caps, spaced) for labels and captions, `SilkX` for headings, `Bold` (JetBrains Mono) for key legends, menus and names, `Body` for text, `Lcd` for LCD text. Seven-segment digits for time, tempo and level readouts: `hw::seg`.
- **Controls:** custom-painted with `crates/prowave-app/src/hw.rs`: key caps (`hw::key`, bone/black/orange, LED dot), toggles, track pads, knobs (black, orange pointer, tick ring), faders with tick scales, segmented meters, LCD panels and readouts, insert/routing slots, dark value fields (dropdowns), faceplates, seams, screws, bracketed section labels. `hw-kit.png` (`cargo test -p prowave-app manual_pictures_hardware_kit -- --ignored`) shows them all.
- **Regions:** `theme::region_look` + `hw::paint_region`: black MIDI bodies with bone notes, warm-grey audio bodies with dark waveforms, a header bar in the track colour; selected = orange header and outline. New tracks take `theme::TRACK_PALETTE` tones.
- **Rules:** orange only for signal (selection, record, lit LEDs, playhead); labels in caps; keep hit targets at least 20 pt; no invented captions or hint text.
