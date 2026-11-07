# AGENTS.md — Multi-Agent Development Protocol for Pro Wave
This file tells AI coding agents (Claude Code, Codex, Cursor, Jolly) how to work in this repo. If you are an AI agent, read this first.

## 1. Project Intent
We are building Pro Wave — a complete functional clone of Apple Logic Pro 11 in pure Rust + WASM. Every Logic Pro feature must be cloned: Drummer, Flex Time/Pitch, Sampler, Quick Sampler, Drum Machine Designer, Alchemy-lite, ES2, Vintage B3/Clav/E-Piano, Sculpture, Retro Synth, Ultrabeat, Studio Strings/Horns, Channel EQ, Compressor (6 models), Space Designer, ChromaVerb, Delay Designer, Pedalboard, Amp Designer, Mastering Assistant, Stem Splitter, Live Loops, Smart Tempo, Score Editor, etc. See roadmap.md Section 1 for exhaustive inventory.

This is NOT a wrapper around JUCE or Electron. Every agent must respect audio-thread constraints.

## 2. Agent Roles — Each owns a crate, no overlap

| Agent Name | Crate Ownership | Responsibilities | Forbidden | Logic Pro Parity Task |
| :--- | :--- | :--- | :--- | :--- |
| **engine-agent** | `prowave-engine` | Lock-free queues, transport, tempo map, audio graph, PDC, disk streaming | No `std::fs`, no alloc in `process()` | Transport, Cycle, Count-in, Varispeed, Low Latency Mode |
| **dsp-agent** | `prowave-dsp` | All FX: Channel EQ, Linear EQ, Vintage EQs, Compressor models, Space Designer, ChromaVerb, Delay Designer, Pedalboard, Amp Designer, Modulation, Distortion, Imaging, Metering | No UI code, no file I/O | Must match Logic param ranges exactly, A/B null test -60dB |
| **instruments-agent** | `prowave-instruments` | Sampler, Quick Sampler, DMD, DKD, ES2, Vintage B3/Clav/E-Piano/Mellotron, Sculpture, Retro Synth, Ultrabeat, Alchemy-lite, Studio Strings/Horns | No UI code | Must import SFZ/EXS, must have Mod Matrix |
| **midi-agent** | `prowave-midi` | Piano Roll logic, Score data, Step Seq, Event List, MIDI Transform, MIDI FX chain (Arp, Chord Trigger, Scripter via Rhai), MPE, MIDI 2.0, Articulation ID | No direct audio buffer access | Quantize, Humanize, Flam, Brush, Time Handles, MIDI Draw |
| **mixer-agent** | `prowave-mixer` | Summing, busses, Aux, VCA, Groups, Sends on Fader, Automation (Read/Touch/Latch/Write/Trim, Bezier curves) | No UI code | Must implement PDC, 64-bit summing, automation sample-accurate |
| **plugin-agent** | `prowave-plugin-host` | CLAP primary, VST3, AU, out-of-process sandbox (seccomp/seatbelt/AppContainer), scanner, quarantine | No unsafe without SAFETY comment + miri test | Must survive plugin crash without DAW crash |
| **ui-agent** | `prowave-ui` | egui widgets: Tracks area, Mixer, Piano Roll, Score Editor, Step Seq Grid, Audio Track Editor, Smart Controls, Library/Patches, Loop Browser, Live Loops Grid, Video Track, Key Commands window, Channel Strip | No audio processing, only Commands via ringbuf | Must look like Logic dark gray, 1:1 controls |
| **wasm-agent** | `prowave-wasm` | wasm-bindgen, AudioWorkletGlobalScope (128 samples), SharedArrayBuffer + Atomics, OPFS, Web Remote (Logic Remote clone via browser) | No cpal, no native-only deps | Must play 32 tracks in Chrome |
| **qa-agent** | all | RTL benchmarks, clippy -D warnings, miri, fuzzing (wav, project), drop-out detection, preset compatibility tests vs Logic presets corpus | No feature work | Must verify no alloc on audio thread |

### NEW CRATE: prowave-logicx-import — Logic Pro Official File Support (see LOGICX_FORMAT_SPEC.md)
- **logicx-agent** (new) owns `prowave-logicx-import`: parse .logicx bundle, ProjectData linear scan (AU triple, GAME, JSON, channel-strip, region), seed synthesis for export, Linux (no AU) vs macOS (AU) handling, lightweight (<50MB binary, <150MB RAM)

### How to Spawn (Claude Code / Jolly)
```
@engine-agent Implement Transport { sample_rate, tempo_map: TempoMap { events: Vec<TempoEvent> }, position } with sample-accurate clock
@dsp-agent Implement Channel EQ with 8 bands, Q 0.1-10, Freq 20-20000, Gain -30/+30, with FFT analyzer using rustfft
@instruments-agent Implement Sampler with Zones { key range, vel range, root note }, Groups, Round Robin, Mod Matrix
```

## 3. Repo Conventions

### Mandatory Checks Before PR (Claude must run)
```bash
cargo xtask check # runs:
# - cargo fmt --check
# - cargo clippy -- -D warnings
# - cargo test --workspace -- --nocapture
# - cargo miri test -p prowave-engine -p prowave-dsp -p prowave-instruments (for unsafe blocks)
# - cargo deny check advisories bans licenses sources
# - cargo test -p prowave-dsp --test preset_compat -- --nocapture (compare vs Logic presets)
# - wasm-agent: wasm-pack test --node
```

### Coding Standards for Agents

1.  **Audio Thread Rules (engine, dsp, instruments):**
    ```rust
    // GOOD - no alloc
    fn process(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        for (i, o) in inputs[0].iter().zip(outputs[0].iter_mut()) { *o = *i * self.gain; }
    }
    // BAD - will be rejected by CI
    fn process(&mut self) {
        let v = Vec::new(); // ALLOC!
        println!("debug"); // SYSCALL!
        self.mutex.lock();
    }
    ```
    Use `#[cfg(debug_assertions)]` + `assert_no_alloc` crate.

2.  **Command Pattern (All agents):** UI never mutates engine directly.
    ```rust
    enum Command {
        SetVolume { track_id: TrackId, db: f32 },
        SetChannelEqBand { track_id: TrackId, band: u8, freq: f32, gain: f32, q: f32 },
        AddRegion { track_id: TrackId, region: Region },
        SetFlexMode { region_id: RegionId, mode: FlexMode },
    }
    // Send via rtrb::Producer<Command>
    ```

3.  **No Panics in Engine:** `#![forbid(clippy::unwrap_used, clippy::expect_used)]`. Use `Option`/`Result` + `tracing::error!`

4.  **Logic Pro Parity:** Any param must match Logic's range exactly. Example: Compressor Threshold -50.0 to 0.0 dB, Ratio 1.0 to 20.0, Attack 0.1ms to 200ms. Check Logic Pro manual.

5.  **WASM Parity:** Any new DSP feature in `prowave-dsp` must have a `wasm32` CI test. `cargo xtask wasm-check`.

### File Naming
- DSP nodes: `channel_eq.rs` -> struct `ChannelEq` implements `DspNode { fn process(), fn latency(), fn reset() }`
- Instruments: `sampler.rs` -> struct `Sampler` implements `Instrument`
- UI widgets: `mixer_fader.rs` -> fn `mixer_fader(ui: &mut Ui, state: &mut FaderState)`
- No `utils.rs` - name it what it does: `latency_compensation.rs`, `tempo_map.rs`, `flex_pitch_detector.rs`

## 4. Task Coordination

**Single Source of Truth:** `roadmap.md` Phase. Agents pick tasks only from current phase. Do not skip phases.

**Communication:**
- Use `// AGENT: @dsp-agent needs smoothing param from @engine-agent` in code comments
- No direct cross-crate mutable state. Use events: `EventBus<MeteringEvent>`

**PR Template for Agents:**
```markdown
Agent: dsp-agent
Crate: prowave-dsp
Phase: 4
Logic Parity: Channel EQ with 8 bands, matches Logic param ranges
Changes: Implements Channel EQ with FFT analyzer
Audio Thread Safe: yes (checked with cargo xtask rt-check)
WASM Build: yes (cargo check --target wasm32-unknown-unknown)
Test: cargo test -p prowave-dsp channel_eq -- --nocapture, preset_compat
```

## 5. Secrets & API Keys
No secrets. If you need signing cert for macOS, ask human. Never commit `.p12`, `.env`.


## 6. LLM-Specific Instructions for Claude

- **Claude Code:** You have plan mode. Always read `roadmap.md` Section 1 before implementing. Always run `cargo xtask check` after editing 2 files. Keep diffs <300 lines. One crate per PR.
- **For Cursor / Copilot:** Use `@codebase` to find `AudioGraph`. Do not invent JUCE classes. Use `fundsp`, `symphonia`, `nih-plug` crates.
- **For Codex:** Keep diffs <300 lines. One crate per PR. Use `prowave-` prefix always.
- **For Jolly:** This is a Rust workspace, not Node. Use `container.python_execution` only for scripts, not engine.

## 7. Definition of Done for an Agent Task
1. Code compiles for `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `wasm32-unknown-unknown`
2. No new clippy warnings
3. Unit test + 1 integration test (e.g., process 1 second of silence, check no NaN/Inf) + preset compat test if applicable
4. Documentation comment on public API explaining Logic Pro equivalent
5. Update `roadmap.md` checkbox if phase task completed

## 8. Emergency Stop
If plugin sandbox crashes >3 times on scan, @plugin-agent must quarantine plugin and log to `quarantine.json` - never infinite loop scan. If audio thread xrun >10 times per minute, @qa-agent must open issue.

---
> TL;DR for agents: You are building Pro Wave, a complete Logic Pro clone in Rust + WASM. Audio thread is real-time, no alloc. UI is egui + wgpu. Everything is Rust. If you allocate on audio thread, you break music. Check roadmap.md Section 1 for exact Logic features to clone.
