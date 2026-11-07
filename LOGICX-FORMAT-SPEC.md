# Pro Wave — Official Logic Pro .logicx Support Spec
**Target: Claude must implement this 100% in pure Rust | File: `crates/prowave-logicx-import`**
> This spec allows ProWave to OPEN and SAVE official Apple Logic Pro .logicx projects on Linux and macOS, while being 10-20x lighter than Logic Pro.

## 1. Why This Matters & Lightweight Goal

Logic Pro official is ~1.2GB app + 70GB library, Objective-C++ legacy, 3 sec cold start, 1.5GB RAM idle, macOS only.
ProWave goal:
- **Binary size:** <50MB single binary (vs 1.2GB)
- **RAM idle:** <150MB (vs 1.5GB)
- **Cold start:** <400ms (vs 3-4 sec) — achieved by no NSKeyedArchiver, no CoreData, lazy loading
- **Linux support:** No CoreAudio, no AU, only ALSA/JACK/PipeWire + CLAP/VST3, Vulkan via wgpu
- **macOS support:** CoreAudio + AU + Metal via wgpu, universal binary arm64/x86_64, Hardened Runtime, notarized
- **Pure Rust:** `#![forbid(unsafe_code)]` except FFI shims, no Objective-C runtime for parsing

## 2. What is a .logicx File?

A `.logicx` is NOT a file. It's a **macOS bundle** — a directory that Finder shows as one file.

```
MySong.logicx/  (actually a directory)
├── Alternatives/
│   ├── 000/                          # Alternative 0 = default arrangement
│   │   ├── ProjectData               # <<< MAIN BINARY BLOB — 1-50 MB, undocumented
│   │   ├── MetaData.plist            # BPM, key, time signature, sample rate (binary or XML plist)
│   │   ├── DisplayState.plist        # Window positions, NOT needed
│   │   ├── DisplayStateArchive       # Binary UI state, NOT needed
│   │   ├── WindowImage.jpg           # Screenshot, NOT needed
│   │   └── Undo Data.nosync/         # Undo history, ignore
│   ├── 001/, 002/...                 # Other alternatives (versions of arrangement)
│   └── ...
├── Resources/
│   └── ProjectInformation.plist      # Which alternative is active, Logic version
└── Media/
    └── Audio Files/                  # WAV/AIFF/CAF recorded audio
```

Package vs Folder: Logic can save as Package (single .logicx) or Folder (plain folder with .logicx sub-bundle). ProWave must support both.

### 2.1 Plists — Easy Part

Two plists are standard Apple plists (XML or binary). Parse with `plist` crate in Rust (pure Rust, no Foundation).

- `MetaData.plist`: Contains `tempo`, `keySignature`, `timeSignature`, `sampleRate`, `channelCount`
- `ProjectInformation.plist`: Contains `activeAlternative` (e.g., 0), `logicVersion` (e.g., 11.0)

**Rust:**
```rust
use plist::Value;
let metadata = Value::from_file("Alternatives/000/MetaData.plist")?;
let tempo = metadata.as_dictionary().unwrap()["tempo"].as_real().unwrap();
```

### 2.2 ProjectData — Hard Part — Undocumented Binary Blob

This is where all musical data lives. No documentation from Apple. Based on reverse engineering from `lpx-explorer`, `logicx-analyzer`, `LogicProFormatWriter`.

**Key facts:**
- **Magic:** First 4 bytes `23 47 C0 AB` (file signature) — see `logicx-analyzer`[^1]
- **Endianness:** Mixed, primarily Big-Endian for lengths, Little-Endian for FourCC codes
- **Structure:** Chunk-based like IFF/RIFF, NO central header table. Must linear scan entire file. No seek table.
- **Size:** 1-50 MB, read-only. Never write directly without re-synthesizing via seed method.

#### Chunk Markers — Reversed FourCC

Apple stores FourCC reversed in file. Example: Track marker stored as `karT` = "Trak" backwards.

Known markers (from `logicx-analyzer`):

| Stored (LE in file) | Reversed = Display | Purpose | Frequency in typical project |
|---|---|---|---|
| `karT` | Trak | Track definition | ~320 |
| `qeSM` | MSeq | MIDI sequences | ~169 |
| `qSvE` | EvSq | Event sequences (automation) | ~169 |
| `gRuA` | AuRg | Audio regions | ~38 |
| `tSxT` | TxSt | Text/notation styles | ~32 |
| `LFUA` / `lFuA` | AUFL/AuFl | Audio file references | ~23 |
| `PMOC` | COMP | Comping/take data | ~23 |
| `tSnI` | InSt | Instrument definition | ~1 |

**Rust scanning pattern (must be linear, no assumptions):**
```rust
const KNOWN_MARKERS: &[&[u8;4]] = &[b"karT", b"gRuA", b"qeSM", b"qSvE"];
for i in 0..data.len()-4 {
    let marker = &data[i..i+4];
    if KNOWN_MARKERS.contains(&marker.try_into().unwrap()) {
        // found chunk at offset i
    }
}
```
Guard against false positives: check printable ASCII around marker, length caps, neighboring-byte invariants.

#### 3 Distinct Storage Shapes Inside ProjectData

**Shape 1: AU Component Descriptors — 3x FourCC Triple (for 3rd party plugins)**

Anchor: AU type 4CC stored little-endian, one of `umua` (aumu), `xfua` (aufx), `fmua` (aumf), `imua` (aumi)

Layout relative to type anchor T:
```
T-4: Manufacturer 4CC (4 bytes LE)
T:   Type 4CC (4 bytes LE)  // e.g., umua = instrument
T+4: Subtype 4CC (4 bytes LE)
```
- All three must be printable ASCII 0x20-0x7e or discard (guard against noise)
- Reverse before display: `umua` -> `aumu`
- Fingerprint: `"{type}/{subtype}/{manufacturer}"` e.g., `aumu/Synt/appl`
- Trailing spaces significant: Kilohearts manufacturer `"kHs "` has space
- Types: `aumu` = MusicDevice (instrument), `aufx` = effect, `aumf` = MIDI effect, `aumi` = MIDI processor

**Shape 2: Apple Stock Plug-ins — GAME Anchor (Compressor, ChromaVerb, Alchemy...)**

Apple stock plugins NOT stored as AU triple. Anchored on literal ASCII `GAME`.

Layout relative to G of GAME:
```
-14: flag1 (0x02 = FX insert -> aufx, 0x00 = instrument -> aumu, else reject)
-13: flag2 (0x02 = FX/instrument, 0x01 = Klopfgeist metronome, else reject)
-12..-1: 12-byte ASCII name, NUL-padded, must be NUL-padded not garbage
0..3: GAME marker
```
- Name examples: "Compressor", "Channel EQ", "SpaceDesigner"
- Fingerprint not recoverable from storage, so:
  - Lookup in static table `STOCK_FINGERPRINTS`: `Compressor -> aufx/Comp/appl`
  - Fallback: synthesize subtype from name lowercased, strip non-alphanumeric, pad to 4 chars
- Display name = recovered name

**Shape 3: Drummer / Bass Player / Keyboard Player — Embedded JSON**

Logic's Session Players store full JSON state inside ProjectData.

Anchor: literal string `"selectedPersistentCharacterTypeIdentifier":"`

Value read until next `"` within 96 bytes, must start with `Type_`.

Example:
```json
{
  "Preset": {"Name": "Sweet Memories", "CharacterIdentifier": "Acoustic Piano - Strummed"},
  "RegionType": "Type_AcousticPianoV2",
  "GeneratorMemento": {"Seeds": [...], "MementoParameters": {...}}
}
```
- One slot emits MANY snapshots (per-region). Cluster by offset proximity: `CLUSTER_THRESHOLD = 32768` bytes. Two hits >32KB apart = new cluster. Latest Type in cluster = active.
- Known mapping: `Type_AcousticDrummerV2` -> `Drummer`, `Type_ElectricBassV2` -> `Bass Player`

**Shape 4: Channel-strip / Track Records**

Anchor: 16-byte name field opening with `0x20` (space) preceded by NUL `0x00`, followed by 8-byte descriptor.

Layout:
```
N-1: 0x00
N: 0x20 + ascii-name (printable 0x21-0x7e) NUL-padded to 16 bytes, whitespace-only rejected
N+16: descriptor 8 bytes
  descriptor[0] = head: 0x89=Master, 0x49=Output, 0xE9=Bus, 0xAB with b1=0xF5=Aux else Audio, 0x29 with b2=0xF3/0xF7=Instrument else Input
  descriptor[2] & 0x04 !=0 or descriptor[4]!=0 => is_active (plugin loaded or routing)
```

**Shape 5: Region Records — 0x61 0xFF Anchor (Audio Region Names)**

User track names like "Acoustic GTR" live in audio region records, not channel-strip.

Layout:
```
[4 bytes opaque id]
0x61 0xFF + 24x 0x00
LEN: u16 LE (1..=200 max, else garbage)
name: LEN bytes printable ASCII 0x20-0x7f
```
- Regions clustered: strip suffixes `: Take 14`, `: Comp A.1`, `_001`, `#06`, `.N` iteratively. Consecutive records sharing base name = one user track.

## 3. How to Write .logicx (Export) — Seed Synthesis Method

You CANNOT hand-craft ProjectData from scratch reliably. Logic is strict, will show "Project corrupted" if bytes wrong.

Proven method from `LogicProFormatWriter` (MIT, pure Python, Logic-validated):

- Bundle ~62KB donor data in `logicx/data/` = two base sessions: `audio_base.seed`, `mixed_base.seed` + `infra.json.gz` (constant records)
- Donor data is GENERATED ARTIFACT from real Logic control sessions. Never hand-edited. Recipe in DONORS.md
- To synthesize new session with M instrument + N audio tracks:
  1. Load base seeds
  2. Parse TimeMap from MIDI (tempo/meter/markers)
  3. For each track, clone track record + channel strip + AU descriptor, assign new UUIDs
  4. For each region, synthesize AuRg (audio) or MSeq (MIDI) records with correct tick positions (PPQ 960)
  5. Patch offsets, lengths, fix 24-byte root header `u32 @ +0x10 = payload length`
  6. Write bundle structure

**Rust port for ProWave:**
- Crate `prowave-logicx-import` will have `logicx/data/` embedded via `include_bytes!`
- Function `synthesize_av_bundle(tracks: Vec<TrackDef>) -> Vec<u8>` where `TrackDef { name, kind, regions: Vec<RegionDef> }`
- `RegionDef` = `Audio { file: Path, start_tick, length_ticks, gain }` or `Midi { notes: Vec<MidiNote>, start_tick }`
- Audio files: must be 44.1/48kHz WAV (Logic expects). If mp3/aiff, transcode via `symphonia` + `wav` writer, or ALAC in CAF via `alac` crate for lossless 3-4x smaller (Logic plays ALAC/CAF natively)
- For reading, we parse. For writing, we synthesize via seeds.

**Key Rust crates:**
- `plist` for MetaData
- `byteorder` for BE/LE reads
- `memchr` for fast scanning
- `serde_json` for Drummer JSON
- `uuid` for track UUIDs

## 4. ProWave Implementation Plan for Claude (Linux + macOS Lightweight)

### Crate Structure
```
crates/prowave-logicx-import/
  src/
    lib.rs              # public API: open_logicx(path) -> ProWaveProject, save_logicx(project, path)
    bundle.rs           # parse .logicx bundle structure, handle Package vs Folder, Alternatives
    plist.rs            # parse MetaData.plist, ProjectInformation.plist
    project_data/
      mod.rs            # linear scan dispatcher
      scanner.rs        # fast memchr scan for markers
      au.rs             # Shape 1: AU triple
      stock.rs          # Shape 2: GAME anchor
      drummer.rs        # Shape 3: JSON
      tracks.rs         # Shape 4: channel-strip records
      regions.rs        # Shape 5: audio region records + clustering
      time_map.rs       # tempo/meter/markers extraction
    synth/
      mod.rs            # synthesis from seeds (export)
      seeds.rs          # include_bytes! for audio_base.seed, mixed_base.seed, infra.json
      audio.rs          # audio region synthesis
      midi.rs           # MIDI note synthesis
  data/
    audio_base.seed     # embedded donor (62KB)
    mixed_base.seed
    infra.json.gz
  tests/
    fixtures/           # real .logicx from Logic Pro 11 (small, sine tones)
```

### API for ProWave Core

```rust
// Open official Logic Pro project on Linux/macOS
pub fn open_logicx(path: &Path) -> Result<ProWaveProject, LogicxError> {
    // 1. Detect if path is bundle (dir with Alternatives) or folder project
    // 2. Read Resources/ProjectInformation.plist -> active alternative index
    // 3. Read Alternatives/XXX/MetaData.plist -> tempo, key, sample rate
    // 4. Read Alternatives/XXX/ProjectData via linear scan:
    //    - scan_au_descriptors() -> Vec<AuPlugin>
    //    - scan_stock_plugins() -> Vec<StockPlugin>
    //    - scan_drummer() -> Vec<DrummerPlayer>
    //    - scan_tracks() -> Vec<TrackRecord>
    //    - scan_regions() -> Vec<RegionRecord> -> cluster to tracks
    // 5. Resolve: assign AUs to tracks by nearest preceding track offset (like lpx-explorer)
    // 6. Convert to ProWaveProject { tracks: Vec<Track>, tempo_map, audio_files: Vec<Path> }
}

pub struct ProWaveProject {
    pub tempo_map: TempoMap, // tempo events, time sig, markers
    pub tracks: Vec<Track>,
    pub audio_files: Vec<AudioFileRef>, // paths inside Media/Audio Files/
}

pub enum Track {
    Audio { name: String, regions: Vec<AudioRegion>, channel_strip: ChannelStrip },
    Instrument { name: String, instrument: Instrument, midi_regions: Vec<MidiRegion>, fx: Vec<Fx> },
    Drummer { name: String, drummer_type: String, regions: Vec<DrummerRegion> },
    Aux { name: String }, Bus, Master
}

// Save to .logicx that Logic Pro opens clean (no repair prompt)
pub fn save_logicx(project: &ProWaveProject, out_path: &Path) -> Result<(), LogicxError> {
    // Use seed synthesis method, not hand-crafted
}
```

### Linux vs macOS Specifics

**Linux:**
- No AU hosting. When opening .logicx with AU plugins (e.g., Alchemy), map to ProWave native equivalents: Alchemy -> ProWave Alchemy-lite, Channel EQ -> prowave Channel EQ, Compressor -> prowave Compressor. Show UI: "Alchemy not available on Linux, replaced with ProWave Alchemy-lite (presets preserved where possible)"
- Audio I/O: ALSA/JACK/PipeWire via cpal. No CoreAudio. No need for `com.apple.*` entitlements.
- File system: .logicx bundle is just a directory on Linux (no Finder magic). Handle both as dir. Case-sensitive FS: preserve case.
- Performance: Use `mimalloc` allocator, `mmap` for ProjectData, lazy load audio files (streaming, not load all). Target <150MB RAM idle.

**macOS:**
- AU hosting: Can host AU via `coreaudio-sys`. When opening .logicx, if AU present on system (e.g., FabFilter), use it. If not, fallback to ProWave native.
- CoreAudio: Use `cpal` with CoreAudio backend, support aggregate devices.
- Bundle: .logicx is package, use `std::fs::is_dir` + check for `Alternatives` subdir to detect.
- Notarization: No need for parsing, but for saving .logicx, ensure bundle has correct Finder attributes (package bit). Use `xattr`? Not needed, just dir.

### Lightweight Optimizations (Why 20x lighter than Logic)

1. **No 70GB Library:** ProWave ships with <100MB of essential IRs and wavetables, not 70GB. User can optionally download Logic library if they have Logic installed (read from `/Library/Application Support/Logic/`), but not required.
2. **No Objective-C / Swift runtime:** Logic uses NSKeyedArchiver, CoreData. ProWave uses pure Rust `rkyv` zero-copy, no ObjC.
3. **Lazy loading:** Logic loads all plugins on open. ProWave: scan ProjectData linearly, only instantiate channel strips when track selected, plugins sandboxed and lazy.
4. **Single binary:** Logic = 1.2GB app + helpers. ProWave = <50MB single binary via `cargo build --release` + `strip` + `mimalloc` + LTO.
5. **WASM core:** Same engine for native and web, no duplicate code.
6. **No legacy:** Logic has 30 years of legacy (Notator, etc.). ProWave starts clean, only modern formats (CLAP primary, VST3, AU on macOS).

### Tests (Must pass for Claude)

- `cargo test --test logicx_import -- fixtures/logic_11_empty.logicx` -> should parse 0 tracks, tempo 120, 4/4
- `cargo test --test logicx_import -- fixtures/logic_11_drummer.logicx` -> should parse 1 Drummer track, type Acoustic, 8 bars
- `cargo test --test logicx_import -- fixtures/logic_11_audio_midi.logicx` -> 1 audio + 1 instrument, audio region name, MIDI notes count
- Round-trip: `open_logicx -> save_logicx -> open_logicx` should preserve track count, names, tempo, regions (byte-identical ProjectData not required, but musical data identical)
- Fuzz: `cargo fuzz run project_data_parser -- -max_total_time=300` with corpus of ProjectData files

## 5. Security (for .logicx)

- ProjectData is untrusted (downloaded from internet). Limits:
  - MAX_PROJECTDATA_SIZE = 200 MB
  - MAX_TRACKS = 2000 (Logic max 1000, we allow 2000)
  - MAX_REGIONS_PER_TRACK = 10000
  - String lengths: region name max 200 chars (from lpx-explorer), track name 16 chars, but allow up to 100 for ProWave
  - No path traversal: audio file refs inside Media/Audio Files/ only, reject `..` or absolute paths
  - No exec: never `eval` Drummer JSON params, parse as JSON only

## 6. Future Options

- **Write full Alchemy preset parser:** Alchemy stores presets as binary blobs inside ProjectData, not yet decoded. For v2, reverse engineer Alchemy preset chunk (likely `Alch` marker)
- **Flex Time/Pitch import:** Flex markers stored in ProjectData, not yet decoded. Need to find `FlxT` marker?
- **Automation import:** Event sequences `qSvE` contains automation, need to decode curve type (linear, bezier)
- **Score import:** Text styles `tSxT` contains notation, need MusicXML conversion
- **Export to Logic with full instruments:** Currently seed method supports audio + basic instrument tracks. Extend to support Alchemy, Sampler zones, Drum Machine Designer pads

---

**For Claude:** Start with `crates/prowave-logicx-import/src/bundle.rs` + `plist.rs` + `project_data/scanner.rs`. Get empty project parsing working first, then add AU, then stock, then drummer, then regions. Use `lpx-explorer` Rust crate as reference (MIT), but rewrite in pure Rust with no Tauri dependency.

[^1]: logicx-analyzer/CLAUDE.md at 655fa43dd74be15a83e85e59bb8f886e0f25ed6f · geoffmyers/logicx-analyzer — https://github.com/geoffmyers/logicx-analyzer/blob/HEAD/CLAUDE.md
