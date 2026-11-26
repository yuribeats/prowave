# Changelog

What changed in each Pro Wave release. The release workflow copies the section of the version
being released into the GitHub Release notes, so keep one `## <version>` heading per release.

## Unreleased

## 0.4.0

### 2026-11-26: Plug-in menus without a ratings file are grouped by maker

Checked by tests (no app was opened).

- With no `plugin-ratings.json` (or one that rates nothing), the Instrument and Audio FX
  menus have one submenu per maker again (alphabetical, Pro Wave's own built-ins under
  "Pro Wave", a plug-in without a maker under "Other") instead of putting everything in
  "Other". With a ratings file the menus are by type and rating as before.


### 2026-11-25: The Channel Rack's title row is gone

Checked by headless tests and offscreen pictures (no app was opened).

- The Channel Rack no longer draws its bracketed "CHANNEL RACK" title over the pane. Its
  Split to Tracks, Pattern, Groove and Kits keys now sit on the right of the amounts row
  (Groove, Offset, Velocity, VEL), so the rack opens 38 points shorter and shows that much more
  of its rows. When the pane is too narrow for one row, the keys go over the amounts; the
  pattern's and kit's names shorten first. While a kit is copied from a share, its slot shows
  only where there is room left of the keys.

### 2026-11-25: An Offset for each drum in the Channel Rack

Checked by headless tests and offscreen pictures (no app was opened).

- Each Channel Rack row has an **OFS** readout after its swing: how much of its drum class's
  micro-timing offsets it plays, 0 to 100 % (drag; double-click for 100). Every channel of the
  class follows, as with the swing. The Offset knob over the rows still scales every class, so a
  step plays its offset times both. A loaded groove (with its per-class offsets) starts every
  class at 100 %. Saved with the project (`offset_amount` in a
  class's feel, left out at 100 %); `rack.offset` sets it, `rack.get` gives it per channel and
  per feel (`offset`, percent).

### 2026-11-25: The bottom pane's handle and head no longer hidden

Checked by headless tests and offscreen pictures (no app was opened).

- A pane whose contents were taller than the pane (a Channel Rack with eight channels, or any
  pane dragged short) pushed the Tracks area down over its top: the grab handle and the
  pane's head (the Channel Rack's Kits, Groove, Pattern and Split to Tracks keys) were covered
  and took no clicks, so the pane could not be dragged taller. The pane now keeps its contents
  inside it, and the Channel Rack keeps its head and amounts in place and scrolls only its
  rows.
- The grab handle is easier to see: a light bar across the pane with a black grip in the
  middle; it brightens under the pointer.
- The Channel Rack opens as tall as its head and every channel's row (the Tracks area keeping
  at least its menu row, ruler and one track).
- The Piano Roll dragged to its shortest no longer stops the program (its scroll bars had a
  negative size).

### 2026-11-25: Emulation names in the plug-in menus, Master Chain, a draggable bottom pane

Checked by headless tests and offscreen pictures (no app was opened).

- Instrument and Audio FX menus: a plug-in that emulates a piece of hardware or a classic
  instrument is listed by the emulated thing's proper name ("Roland TB-303"), its own name
  ("JC303") in small print where the maker was; the tooltip names the plug-in and its maker,
  then the rating and why. A search field at the top matches either name or the maker. Read
  from `emulates` (and `name`, `maker`) in `plugin-ratings.json`; `plugin.list` gives
  `emulates` and searches it.
- Audio FX menu: a **Master Chain** folder first, with the mastering tools (`master_chain` in
  `plugin-ratings.json`); they stay in their type folders too. `plugin.list` gives
  `master_chain`.
- The pane under the Tracks area (Mixer, Piano Roll, Channel Rack, Pitch Editor) has a grab
  handle on its top edge: drag it to make the pane shorter or taller, double-click it for the
  pane's own height. The height is kept per pane with the project (not counted as an unsaved
  change). A Mixer dragged shorter than its strips shows them compact, the wheel bringing back
  the rows hidden at the top.
- Musical Typing's close key works: Caps Lock opens and closes the window when it changes, so
  the window no longer reopens the next moment while Caps Lock stays on.
- The Bounce progress panel's close key cancels the bounce (it did nothing).

### 2026-11-25: Channel Rack Solo key and sample stepping

- Channels: a **Solo** key beside Mute (`rack.mix` `solo`), and on the sample's right-click
  Hear, ‹ › through the kit's samples of the channel's class, Samples and All Other Samples.


### 2026-11-25: Graphite Amber look, with walnut end cheeks

Checked by tests and offscreen pictures (no app was opened).

- New default look, Graphite Amber: a charcoal body (#1c1c1e), raised keys #323237, light
  print, amber (#ffb000) as the signal colour with dark print on it, amber digits on dark LCD
  glass, graphite MIDI regions with amber notes and slate audio regions with light waveforms.
  The level meters, in both looks: olive segments (#7f8c6a), amber near the top (#c98a2e,
  above -6 dB) and signal orange at the top (#ff5a14, over 0 dB), unlit ones dark; the
  Mixer's segments are 3 points tall with 2-point gaps.
- View > Theme switches between Graphite Amber and Bone (the earlier look) at once; View >
  Wood Cheeks draws walnut end cheeks down both sides of the main window, its grain made by
  code once at start, hidden below 1000 points of window width. Both are kept in the settings
  files `theme` and `wood-cheeks`.
- The plug-in menus' rating chips run from #323237 (1) to amber (5) in Graphite Amber, their
  numbers dark or light by contrast.
- For code: the colour tokens in `theme.rs` are functions of the look in use (`theme::INK()`),
  and painters mix their own shades with `theme::pick(bone, graphite)`.

### 2026-11-25: Instrument and Audio FX menus by type and rating; removed plug-ins gone

Checked by tests (no app was opened).

- The Instrument and Audio FX menus have one submenu per type (Compressor, Delay, EQ, Reverb,
  Synth, ...; alphabetical, "Other" last) instead of one per maker. Pro Wave's own
  instruments and effects sit in the same submenus as the plug-ins; Pro Wave's separately
  installed plug-in builds of a built-in are left out (the built-in is offered).
- In each submenu the best rated come first, the unrated last. Each entry shows a chip with
  its rating (1 to 5 in halves), coloured in nine steps from the theme's raised neutral (1)
  to its accent orange (5), its maker in small print, and why it has that rating as its
  tooltip. Ratings and types come from the settings file `plugin-ratings.json` (written by the
  owner's plug-in catalog tool), read again when it changes. `plugin.list` gives `rating`
  and `type` too.
- Audio Units whose `.component` bundle was deleted are no longer offered, even while macOS's
  registry still lists them (they failed to open with -3000). A unit is kept when it is
  Apple's, an app extension, or declared by a bundle in the Components folders; when a bundle
  there has no readable list, nothing is left out.
- VST3s whose bundle was deleted are no longer offered from the scan cache while the next
  scan runs.
### 2026-11-25: The Mixer opens with its strips whole

Checked by tests and an offscreen picture of the main window (no app was opened).

- The Mixer pane under the tracks opens as tall as its channel strips, worked out from the
  strip's rows (Setting, Input, every Audio FX and Send row the strips have, Output, Group,
  Pan, the readouts, the fader, the keys and the name plate) instead of about half the room.
  The strips no longer start cut off in the middle; the Tracks area gets the rest.
- A window too short for the strips at the main size keeps them whole: the Tracks area shrinks
  down to its menu row, ruler and one track row, and the strips take their compact size.
- The Mixer has a pane of its own, so showing it no longer changes the height a Piano Roll,
  Channel Rack or Pitch Editor pane was dragged to.

### 2026-11-25: Hidden plug-ins

Checked by tests (no app was opened).

- The Instrument and Audio FX menus and `plugin.list` leave out Apple's Audio Units (the ones
  macOS comes with, maker code `appl`: AUSampler, DLSMusicDevice, AUDelay, AUMatrixReverb and
  the rest). "Show macOS built-in plug-ins" at the foot of the maker submenus shows them again;
  the choice is kept per Mac in the settings file `plugin-hide-apple` (`0` shows them).
- Any instrument or effect, Pro Wave's own included, is hidden by its `plugin.list` id on a
  line of the settings file `plugin-hidden` (`prowave:808`, `builtin:Space D`,
  `aufx:dely:appl`, `vst3:…`). The files are read again within a second; no restart.
- A project that already uses a hidden plug-in opens and plays it as before.

### 2026-11-24: Groove Flex for audio tracks

Checked by tests on synthetic clicks (no app was opened and nothing was listened to): straight
sixteenths at 120 BPM with a 60 % hat groove come out with every "e" and "a" 25 ms late and
the beats and "&" in place (within 8 ms, measured by onset detection on the made sound), the
same length as the recording; 0 % plays the recording itself.

- Region and Track Inspector ▸ **Groove Flex** (audio), 0–100 %: the hits of each audio region
  are found once and moved toward the same per-hit target as Groove Q (where the rack's own hit
  of the followed class on the nearest sixteenth plays; none there: the hit stays; Rack and
  Follow rows, Auto by hit density and the track's name), by the strength; the sound between hits is stretched with the existing Flex Time WSOLA (pitch kept,
  Rhythmic mode by default). The region keeps its length; the recording is never changed; the
  markers are made again when the groove, strength, tempo or place changes. Playback and the
  bounce use the made sound.
- Control interface: `rack.groove_flex_get` and `rack.groove_flex`.

### 2026-11-24: Groove Q for MIDI tracks

Checked by tests (no app was opened and nothing was listened to).

- Region Inspector ▸ **Groove Q** (under Quantize) and Track Inspector ▸ **Groove Q**, 0–100 %:
  a quantize toward a Channel Rack's hits. Each note is pulled toward Follow Rack's own target
  for the nearest step (the region's Quantize grid, else sixteenths) of the class Follow picks:
  where that step's one hit plays (played times, never a mean), between two hits its share of
  the way, and where the class has no hit the note stays. By the strength. A region takes its track's strength unless it has its own, so a recorded take
  is grooved at once. Non-destructive: playback, the bounce and the MIDI export take it; the
  stored notes do not move. With Follow Rack on too, the result is the same at 100 % (no
  double move).
- Track Inspector ▸ **Groove Live** (off by default): keys played live while the song plays
  wait for a hit the rack plays late, by the Groove Q strength (never earlier, at most a
  sixteenth); their releases wait as long. Takes are recorded at the moment the keys were
  pressed.
- Control interface: `rack.groove_q_get` and `rack.groove_q`; `rack.follow_get` reports
  `groove_q` and `live`.

### 2026-11-24: Follow Rack Groove for MIDI tracks

Checked by tests and end to end on a windowless app: a Channel Rack with a measured groove
(closed hat swing 53 % and its own offset on each sixteenth), hats on all sixteen steps, and a
MIDI track of straight sixteenths following the hat. Exported MIDI (480 ppq): the sixteenths moved
by −4 to +15 ticks (−4.2 to +15.6 ms at 120 bpm) and all 16 of bar 1 land on the rack's own hat
ticks; with the option off, or exported as stored, every note is back on the grid. A bounce
through Apple's DLS Music Device AU, one note per bar on each sixteenth, moved the onsets by
−196 to +740 samples at 48 kHz, each within 2.4 samples of the expected shift. Nothing was
listened to.

- Track Inspector ▸ **Follow Rack** (off by default): a MIDI track's notes play on the hits
  of one drum class of a Channel Rack, where the rack plays them: a loaded pattern's hits at
  their played times, steps added by hand where the groove places them. A note on a hit's step
  plays as far from it as that one hit, so every bar follows its own hits (no mean timing); a
  note where the class plays no hit stays where it is. **Rack** (the first by default),
  **Follow**: Auto, Hat, Snare, Kick, then the other classes; **Groove Velocity** (off by
  default): the velocity of the hit carrying a note over the mean of the class's hits.
- Auto: Kick for 808 and bass parts (by name, or median pitch below C2 with one note at a
  time), the closed hat for dense parts (2 onsets a beat, or 10 % on the "e" and "a"),
  else the snare (the clap when the rack plays claps and no snares). Thresholds measured on
  3,740 labelled producer MIDI files.
- Non-destructive: notes are moved when the song is made (playback, bounce, AU/VST3 and
  built-in instruments, external MIDI), never in the regions. A note between two hits at most a
  sixteenth apart plays its fraction of the way between where they play; note-offs move with note-ons and stop at the next note of the
  same pitch. Early notes play early (pre-roll), clamped at beat 0 and, with Cycle on, at
  the cycle's start.
- File ▸ Export ▸ **Apply Follow Rack Groove** (on): the MIDI export writes the notes as they
  play; off, as stored.
- A track bounced alone (export.bounce with a track, All Tracks as Audio Files, Bounce in
  Place) keeps the rack it follows, silent, so its notes keep the feel.
- `prowavectl`: new `rack.follow`, `rack.follow_get`; `export.midi` takes
  `apply_follow_groove`.

### 2026-11-24: Kit Browser kit list

Checked by tests and through the control interface of a windowless app on the real library;
not seen in the live window.

- A kit list (`kit_list` in the Kit Browser's `settings.json`, or `kits.list`): a TSV naming
  kits by their folder relative to the library folder, with `rank`, `type` and `reason`
  columns. Set, the Kit Browser shows only its kits (plus favourites and the kit for new
  racks), in rank order, grouped by type, and the left column lists the types. A list path
  not found as written is looked for inside the library folders, so the same setting works
  where the library is mounted at another path. Right-click the TYPE column or the Kit List key in Folders to turn it
  off. Unset, nothing changes.
- `prowavectl`: new `kits.list`; `kits.search` takes `list` and `type` and reports each kit's
  `rank` and `type`; `kits.status` reports `kit_list` and `all_kits`.

### 2026-11-24: Channel Rack patterns, velocity, split steps, amounts, volume and pan knobs

Checked by tests and through the control interface of a windowless app; nothing was played
or listened to.

- Every hit has its own velocity: a lit step shows it as an ink bar; drag a step up or down to
  change it; right-click a step for a Velocity slider. The VEL field sets the velocity of hits
  added by hand (default 100).
- A step can be split into 2, 3 or 4 hits (right-click > Divide: 32nds, triplet thirds, 64ths)
  for rolls and ratchets; each part turns on and off and has its own velocity. A split hit
  plays its fraction of the way to where the next step swings to, so it stays in its step at
  any swing.
- Amounts row: Groove, Offset and Velocity knobs, 0-100 % (default 100 %), take the swing,
  the micro-timing offsets and a loaded pattern's or groove's velocities toward straight, none and the
  hits' velocities before (a pattern's: the VEL value); turning back restores them.
- Each rack channel has a volume knob (−48 to +12 dB, the same level as the dB readout) and a
  pan knob.
- The rack shows "Copying kit" while a kit is copied here from a share.
- `prowavectl`: new `rack.mix`, `rack.amounts`, `rack.hit`, `rack.divide` and `rack.pattern`;
  `rack.get` also reports each channel's volume, pan, mute and hits (step, part, division,
  velocity, start), the amounts and the loaded pattern.

### 2026-11-24: kits from another computer are copied here when loaded

- Kit Browser: a library folder on a network share is browsed and auditioned in place; Load
  Kit, a sample put on a pad or rack channel, and Add Channel… copy the samples into
  `kit-library/local/<source>/<kit>/` first and the rack plays the copies, so projects keep
  their sounds without the share. `copy_kits` in `kit-library/settings.json`: `remote`
  (default, network shares only) or `always`. A default kit on a share is copied once and
  `default_kit` then names the copy. Load Kit and the default kit copy on another thread, so
  the window keeps working (a first build blocked it: 19.4 s for the 54-file OZ kit, 8.6 s
  for a 16-channel kit over SMB, and the app hung). A library folder on a network share is no
  longer read kit by kit in the background; its kits are read when picked.
- `prowavectl`: new `kits.copy`; `kits.status` reports `default_kit`, `copy_kits` and `local`.

### 2026-11-24: Channel Rack swing, micro-timing and grooves; new racks start with a kit

Checked by tests and through the control interface of a windowless app; nothing was played
or listened to.

- Each rack channel has a drum class (Load Kit sets it from the sample; changeable in the
  rack). Each class has a swing (16th or 8th, 35-85 %) and sixteen micro-timing offsets, one a
  sixteenth of the bar; a channel's steps are notes at the place its class's feel gives them,
  so the Piano Roll, playback, the bounce and exported MIDI keep that timing. The rack shows
  each channel's class and its class's swing (drag to change).
- The Groove key's Straight puts every step back on the grid.
- A new rack starts with the kit named by `default_kit` in `kit-library/settings.json` (none
  unless set), empty when the folder is not there. Kit Browser: right-click a kit > Use for
  New Racks.
- Kit search compares look-alike letters as plain ones (small capitals, full-width, circled,
  mathematical, accented), so "oz" finds "ᴏᴢ".
- `prowavectl`: new `rack.get`, `rack.class`, `rack.swing`, `rack.steps` and `rack.groove`;
  `kits.load` and `kits.assign` report each channel's class.

### 2026-11-24: the Kit Browser at the library's size

- A kit picked opens at once: a scan reads the library folders first (the list is there in
  milliseconds), then every kit's files on four background threads, and keeps a listing a kit
  in `kit-library/kits/`. A kit is made from its listing; the library's disk is read again only
  for kits never read, kits whose folder changed, and on Rescan (all of them). The list, the
  search and picking kits all work while the files are read; the Folders panel shows how many
  kits are read so far.
- No limit on a kit's files (it was 10,000); the lists draw only the rows in view.
- Arrow keys: Up and Down move through the kits or the samples, Right goes into the samples and
  Left back, Return loads the kit or puts the sample on the lit pad. The window has the keys
  from when it opens and while it was the last thing clicked, and the arrange area does not
  also get them.
- Audition is a key that stays lit (off at first, kept): picking a sample plays it, and picking
  a kit, by click or arrow, plays its kick, snare and hat. Lighting it plays what is picked.
  The triangle of a sample still plays it with Audition out.
- SoundFont files are listed with a kit's samples (SF2) and counted when the kit is read; a kit
  of SoundFonts alone loads onto a new instrument track as its SoundFont (its drum preset when
  it has one), as does a double-click or Return on an SF2 row.
- A source whose kits are nearly all one sample each (90 % of 20 or more, such as a site that
  gives each sample a page) is one pool: one line, its samples listed together.
- `MANIFEST.tsv` is read by its header's column names, with eight or nine columns; kits its
  `other_names_same_content` column says hold the same files are shown once, found by any of
  their names and ranked by the best rank among them.
- A library folder that is not there is marked missing in Folders.
- `prowavectl`: `kits.status` reports `listed`, `pools`, `listing` and `indexing`;
  `kits.rescan` takes `full`; `kits.get` pages with `offset` and lists classes and the
  audition's samples; `kits.load` loads a SoundFont kit; new `kits.audition` plays a sample or
  a kit, or with `capture` writes what the audition voice plays to a WAV file and compares it
  with the files.

### 2026-11-24: plug-ins 0.3.2 (factory presets, FM6 banks, Tune's MIDI input)

Checked with auval -strict (27 of 27, on both Macs), pluginval at strictness 10 without its
window tests (27 VST3 and 27 Audio Units), clap-validator built from its current source (27
CLAP), the crate's tests, and Ableton Live 12.3.5 on the MacBook Air: every VST3 and every Audio
Unit loaded in a set and rendered with Export (all 17 instruments sounded, through the ten
effects). Logic Pro was not checked inside Logic (the second Mac's screen was locked); Logic
loads only Audio Units that pass auval.

- Presets: Engine's, Tune's and Texture's presets are the host's factory presets in every
  format (CLAP preset discovery, Audio Unit factory presets, a VST3 program list), and still a
  menu on the faceplate. The crate now uses a fork of nih-plug that adds them
  (github.com/yuribeats/nih-plug, branch prowave-presets).
- FM6 opens .syx banks: SYX key, the system's file dialog, then a voice from the bank; its
  parameters are set through the host, so the host saves and undoes it.
- Pro Wave Tune takes MIDI notes: in MIDI mode the newest note held is the target, and with
  none held it tunes to the key and scale. Its Audio Unit is now a music effect (`aumf`), so a
  Logic project that used the 0.3.1 Tune Audio Unit does not find it. Key and Scale "Project"
  read "C (default)" and "Major (default)" in the plug-in.
- CLAP fixes found by clap-validator in every 0.3.1 plug-in: a damaged saved state no longer
  aborts the host, the host is told to read the parameters again after a state is loaded, and
  a latency change while the plug-in is active asks the host to restart it instead of calling
  a function only allowed during activation.

### 2026-11-24: the built-in instruments and effects as plug-ins

Checked by tests, `auval`, `prowave --vst3-check` and offline renders only; nothing was played
or listened to, and no host program opened a window.

- Each built-in instrument and effect of the public build is also a plug-in for other music
  programs: VST3, CLAP and Audio Unit, universal (Apple silicon and Intel), named "Pro Wave"
  and its name ("Pro Wave Kick", "Pro Wave Tune") so they are listed together. Seventeen
  instruments and ten effects. Each has the parameters, value texts and pages of its window in
  Pro Wave, the Hardware look, and the presets in a menu on the faceplate. Instruments take
  notes, velocity, pitch bend, the sustain pedal and the "all off" controllers; effects report
  their latency. Built by `crates/prowave-plug/tools/build-plugins.sh`; see
  `crates/prowave-plug/README.md`. The bundles are not notarized.

## 0.3.1

### 2026-11-24: more free plug-ins

Checked by the free plug-in tests and `bash -n` only; the installer was not run.

- Window > Get Free Plug-ins lists 273 plug-ins (84 synths, 189 effects), up from 190. The
  installer fetches 178 of them; the other 95 are got by hand from the maker's page. New
  installer rows: Magical 8bit Plug 2, NIBBI, Colony, GLUECOMP-3B, Kirin Hypha, Maim, Rare,
  RareSE, Virtual Sound Stage, Dimension Expander, DJMFilter and Blue Cat's Freeceiver. New
  rows by hand include the Knightsnare plug-ins, the eighteen Riverflux Relic units and the
  four Wavesfactory freebies. The script is the site's, copied unchanged; with it come the
  site's earlier changes: rows added on 11-23, Smoke Mono Free removed, installers named
  "legacy" inside a download skipped.

### 2026-11-24: three more built-in instruments

Checked by tests only; nothing was played or listened to.

- Engine: a car engine simulated (cylinders, valves, throttle, pipes), ported from REDLINE's
  engine model, with its eight engines as the Engine choice and as presets. The note sets the
  engine speed so that it fires at the note's frequency, between idle and redline; Release
  Stop lets it spin down to silence, Release Idle lets it idle.
- Drum Synth: eight pads, each a falling tone and a noise burst through a falling low-pass,
  ported from Bespoke Synth's DrumSynth. Pad 1 to 8 are notes 36 to 43 (C1 to G1), and 0
  to 7 as in the original.
- Monstro: LMMS's three-oscillator synthesizer with two envelopes, two LFOs and its
  modulation matrix (each envelope and LFO to each oscillator's volume, phase and pitch,
  oscillator 1's pulse width and oscillator 3's sub mix), sync from oscillator 1, and
  oscillator 2 mixed in or modulating oscillator 3 by AM, FM or PM. Eight voices.

## 0.3.0

### 2026-11-23: the Kit Browser

Checked by tests, offscreen pictures and the app without a window against the real library;
nothing was looked at in the running window or listened to.

- The Channel Rack has a Kits key. It opens the Kit Browser: every drum kit found in the
  library folders, searched as it is typed (every word has to be in the kit's name or its
  source), by source, the kits a library's `BEST.md` ranks, and the favourites (the star).
- Load Kit (or a double-click on a kit, or Return in the search field) puts the kit on the
  rack's channels: Kick, Snare, Clap, Closed Hat, Open Hat, Rim, Low, Mid and High Tom, Perc,
  Shaker, Crash, Ride, 808, FX, and one more; a channel the kit has nothing for takes a
  stand-in, and the rest of the kit fills what is still empty. Levels, pans, mutes and steps
  stay, so a pattern plays the new kit. With no rack selected, a new Channel Rack track is made.
- What a sample is comes from its file and folder names, and from an SFZ file, a Hydrogen
  `drumkit.xml` or a DrumGizmo kit when the kit has one (one sample an instrument: the loudest
  layer). Loops are listed and never put on a channel by Load Kit.
- A kit's samples are listed with their class; the class keys show one class. A sample goes on
  one channel by a double-click (the pad that is lit), or by dragging it onto a pad or onto a
  channel of the rack. The triangle plays a sample and Audition the kit's first channels;
  nothing sounds otherwise.
- Folders: the library folders (any number; kept in `kit-library/settings.json` of the
  settings folder). A folder that is not there, such as a drive that is not mounted, has an
  unlit lamp and its kits are not listed. The index is read from a cache at once and made
  again in the background.
- A rack names its samples by their path where they are; nothing is copied into the project,
  as before.
- `prowavectl`: `kits.status`, `kits.rescan`, `kits.folders`, `kits.search`, `kits.get`,
  `kits.load`, `kits.assign`, `kits.favorite`; `ui.window` takes `kits`.

### 2026-11-23: plug-in menus scroll

Checked by tests and an offscreen picture only; not tried in the running app.

- The Instrument slot's menu, the empty Audio FX slot's menu and each maker's list of plug-ins
  scroll when they hold more than the window has room for (the wheel, or the scroll bar that
  shows while the pointer is over the menu). Before, the entries past the bottom of the window
  could not be seen or picked. A menu that fits is drawn as before.
### 2026-11-24: built-in instruments and effects

Checked by tests only; nothing was played or listened to.

- Instruments, in the Instrument slot's menu, each with a unit window, automation and the
  control interface's `plugin.list`, `instrument.set`, `plugin.params` and `param.set`:
  Pro Wave Macro Oscillator (24 synthesis models), Pro Wave Resonator, Pro Wave Digital
  Oscillator (47 shapes), FM6 (six-operator FM; reads voices from a .syx bank), Chip FM 2-Op,
  Chip FM 4-Op, Chip FM Preset, Chip 6581, Chip PSG, Kick, Bass Line, Triple Oscillator,
  Pluck and Wave Draw (its cycle is drawn on a display).
- Effects, in the Audio FX menu: Circuit Overdrive, Pro Wave Resonator FX, Pro Wave Spectral.
- MIDI FX: Swing (with humanize) and Euclidean Pattern.
- File > Import > SMAF File… renders a mobile-phone ringtone (.mmf) to an audio track.
- File > Bounce > Region through EffeTune Chain… runs the selected audio region through a
  chain of EffeTune effects with a Python that has EffeTune installed.
- `public/THIRD-PARTY-NOTICES.md` names every source.

### 2026-11-21: the Arpeggiator's step pattern

Checked by tests and offscreen pictures only; nothing was played or listened to.

- The Arpeggiator (Track > MIDI FX… > Add MIDI FX > Arpeggiator) plays the held keys through a
  pattern of up to 32 steps. Each step is on or off, has a velocity bar (drag across the bars
  to draw them), can be tied into the next step, moved by up to 24 semitones either way, or
  play the whole chord. Orders: Up, Down, Up and Down, Down and Up, Random, As Played, Chord,
  over 1 to 4 octaves. Rate from a dotted quarter to a 1/32 triplet, Swing, Gate up to 200 %,
  how much of the velocity comes from the bars, the pattern's length, ten built-in patterns
  and Randomize.
- Latch keeps the keys after they are let go. Trigger: Restart starts the pattern again with
  each new chord; Song Position keeps the steps on the song's grid.
- It now also plays the keys played live (a MIDI keyboard, Musical Typing, a note clicked in
  the Piano Roll), on the song's beats while the song plays and at the project tempo while it
  is stopped. Other MIDI FX still change only the notes in regions. Record > Send All Notes
  Off lets go of latched keys.
- A project saved with the Arpeggiator before this opens with the same notes: its settings
  become a pattern with every step on.
- `prowavectl`: `midi_fx.list`, `midi_fx.add`, `midi_fx.remove`, `midi_fx.set` and
  `midi_fx.arpeggiator` (every setting and step by name).

### 2026-11-18: the Singer's voices

Checked by tests only; nothing was listened to.

- CHIMERA is updated from upstream commit 09fb776 to eeb0c19 (122 upstream commits). It sounds
  different: the lungs' pressure now reaches the mouth, women's and children's vocal folds are
  modelled by their length, a phrase ending on a vowel fades as the folds part, and the Singer's
  Ring narrows the whole tube above the folds. Lyrics in Korean, Russian, pinyin with tone
  numbers and Chinese characters are sung. Phrases sung by the old engine are sung again.
- "CHIMERA (09fb776)" keeps the old engine as a singer of its own, for the old sound.
- Novox The Robot (Jonathan Mann, MIT), a formant singing voice, is a new singer for the Singer
  and Vocal Top Line, with its voice presets and nine knobs. It is built from its C++ source.

### 2026-11-18: the public edition gets more of the private build

- File > Bounce > Project or Section, File > Export > All Tracks as Audio Files, Bounce >
  Regions in Place and Tracks in Place, Join into a new file, Convert Regions to New Audio
  Files.
- The 808 (a tuned, monophonic 808 bass, synthesized or from a one-shot you choose).
- The Singer and Vocal Top Line: words typed on Piano Roll notes (Paste Lyrics…, or Return on
  a note) are sung by CHIMERA, a physical model of the voice built into the program, or by
  DiffSinger, which `tools/vocal/setup.sh` installs (its voice is downloaded by hand and is for
  non-commercial use only).
- The control interface (a Unix socket in the settings folder; `crates/prowavectl` is its
  command-line client and MCP server; `docs/CONTROL-API.md`). It is off until Help > Allow
  Control by Other Programs is chosen.
- `tools/stems` and `tools/chops`: the scripts that separate songs into stems and cut loops
  from them (Python; the models they download have their own licences, listed in the scripts).
- THIRD-PARTY-NOTICES.md, also inside the app.

### 2026-11-10: autosave and crash recovery

Checked by tests only; not yet tried in the running program.

- While a project has unsaved changes, a recovery copy is written every two minutes, and right
  after an audio recording ends. It goes to `recovery/<session>/recovery.prowave` in the
  settings folder, never over the project's own file, and holds no audio: the session's
  recordings and unpacked media now live in that same session folder instead of the temp
  folder.
- At the next start after a crash, the program asks whether to recover the project or discard
  the copy. A recovered project is unsaved and keeps its file, so Save writes there.
- Quitting or closing the window with unsaved changes asks Save / Don't Save / Cancel. After
  Don't Save, audio recorded in the session that is in no saved file is kept with a recovery
  copy and offered at the next start; discarding it there deletes it.
- A panic is written to `crash.log` in the settings folder (time, version, thread, message,
  backtrace; the last 10 are kept).
- A block a plug-in returns with a NaN or an infinity in it is replaced by silence.

### 2026-11-05 to 2026-11-09, by area

A summary of `git log --no-merges --since="2026-11-05 00:00"` (320 commits; the 22 that came
from upstream are described under 0.2.2). Built and tested on macOS only: no CI run covers this
work on Windows or Linux. The longer entries after this list describe some of it in detail.

**Audio devices and timing**
- macOS: input and output in one Core Audio callback. Output device, I/O buffer size, Low
  Latency Mode and Software Monitoring can be changed while running.
- Process Buffer Range and the RECORD / MIX switch; several playback threads, and a thread that
  falls behind leaves a gap of silence and keeps its time.
- MIDI out is scheduled on the audio clock (CoreMIDI time stamps); Send MIDI Clock; MIDI panic.
- Timing fixes (2026-11-09): the click is delay-compensated, cycle passes start on the beat,
  recorded MIDI notes are placed where the song was heard, plug-ins are told the transport
  position of the sound they are making, and audio regions are not re-cut while following an
  external MIDI clock.

**Plug-ins (macOS)**
- Audio Units and VST3 plug-ins as instruments and effects, on tracks and on Stereo Out, with
  windows, saved settings, factory presets (Audio Units), automation of their parameters, and
  the host's tempo and transport. They run inside the program: there is no sandbox.
- VST3 bundles are scanned in a helper process and the result cached. A plug-in that crashed
  the program while opening is not opened automatically again.
- Plug-in windows float over the main window, follow their view's size and can fill the screen.
  Every plug-in gets a host frame (bypass, presets, A/B) and a host-drawn editor.

**Effects and instruments**
- Native effects ported from Lemon Studio: Lemon Limiter, Lemon Soft Clipper, Lemon Transient Processor,
  Lemon Fast Distortion and Press.
- Space Designer, a convolution reverb, in place of Space D; effects on the Stereo Out strip.
- Pro Wave Tune: live pitch correction, with a Pitch Correction lane in the Piano Roll.
- Channel Rack: rows of steps that play samples.
- The built-in synthesizer and SoundFont instruments play a region's controllers.

**Editing**
- Pitch Editor: a note editor for audio regions (pitch tracking, note resynthesis).
- Piano Roll: tools and Tool menu, step input from a MIDI keyboard, controller, pitch bend and
  pressure lanes, muted notes.
- Automation as envelopes: lanes, Draw Mode, Arm, override and re-enable; targets for mute,
  sends, Stereo Out and plug-in parameters.
- Stretch to fit (Option-drag a region's end); Fit to Grid: a dropped audio file lands at the
  project tempo and on the grid.
- Tracks area: Eraser, Text, Scissors, Join, Mute and Zoom tools; Option-drag copies regions;
  New Audio Track; an audio region can play a part of its file.
- Paint Image to Audio and Show Spectrogram; Musical Typing; Window > MIDI Monitor; the tempo
  can be typed into the LCD.

**Interface**
- The Hardware look (PW-1) on every window: tokens, bundled fonts, painted controls.
- Controls that did nothing were taken out of the control bar, Mixer, Inspector, Tracks area,
  Piano Roll and Event List.

**Checks and documents**
- `AGENTS.md` rewritten to describe the code as it is.
- Tests that need files or plug-ins a clean machine lacks are `#[ignore]`d with the reason,
  instead of returning early and passing: four SoundFont tests (`PROWAVE_TEST_SF2`).
- `cargo xtask check` no longer mentions crates that do not exist.


**Not built** (so nobody assumes it): bounce or audio export of a project, autosave, a "save
changes?" question at quit, a plug-in sandbox, CLAP.

### All systems (Windows, macOS, Linux)

✅ **Process Buffer Range and RECORD / MIX:** as in Logic, live tracks (record-enabled, monitoring, the selected instrument and the Aux strips they reach) run their plug-ins in the I/O callback at the I/O buffer size, and every other track's plug-ins run on real-time playback threads in larger blocks (**Record > Process Buffer**: Small 256, Medium 512, Large 1024). Delay compensation keeps both in time; a track moving between the two fades out and in. The **RECORD / MIX** switch next to the CPU meter sets the I/O buffer, the process buffer and Low Latency Mode at once (RECORD: 64 frames, Small, Low Latency on; MIX: 1024 frames, Large, Low Latency off), each mode remembering its own values; Shift-Command-L toggles it, and arming a track in MIX switches to RECORD. The round trip in ms shows under the switch. The CPU meter shows Live and Playback loads, "System overload: N", and the heaviest plug-in on hover. Measured on a MacBook Air M2 at 64 frames with 1, 2 and 4 instances of Schwabe Gold Clip on playback tracks: Core Audio overloads per 10 s went from 4321 / 2890 / 1636 to 0 / 0 / 0 (MIX: 0 / 0 / 0).

✅ **Space D is now Logic's Space Designer:** a convolution reverb with its own window (light grey, dark top bar, Sampled IR and Synthesized IR tabs, the response drawn with its Volume Envelope, and the orange controls below). Open it from the Audio FX slot or with **Mix > Space Designer > Show**. It has six built-in impulse responses (03.0s Long Reverb is the default), Volume Env and Filter Env, Init Level, Attack, Decay with LIN or EXP, End Level (drag the three blue handles or the values), IR Offset, Reverse, Quality, Predelay, Length, Size, X-Over with Lo and Hi Spread, Input, Dry and Wet, Output EQ, Compare, Copy, Paste, Undo, Redo and View (75 to 150 %). The reverb reports its 512-sample delay for compensation.

✅ **Space Designer for mastering:** Length now cuts the response and Size stretches it, as in Logic. The Filter Env is a real low pass with a Start and End cut-off and Resonance (click its name to edit it in the display). Output EQ is a 100 Hz low shelf and a 10 kHz high shelf with their gains (-2 and -1 dB at first), on the reverb only. Hi Spread goes to 200 %. The Synthesized IR tab has Density, Diffuse and Early Reflections. The Input fader has a level meter. New responses: 01.2s Medium Hall, 00.5s Small Room, Plate, Church and 02.0s Hall. Three mastering presets (Glue - Small Room, Plate - Vocal Sheen, Wide Hall) are in the preset menu and in **Mix > Space Designer**. Projects with the old four-slider Space D open with the same balance of dry and reverb.

✅ **Effects on the Stereo Out strip (the master bus):** the Stereo Out strip now has real Audio FX slots, with the same menu as a track strip (add, replace, bypass, move, remove; double-click opens the window). The whole mix goes through them before the master fader, and they are saved with the project. **Mix > Space Designer > Mastering Glue - Small Room** (and the other two presets) puts a Space Designer there and opens it. Gain reduction and Space Designer input meters work for it. The master effects have no automation lanes yet.

✅ **Plug-ins on the Stereo Out strip:** its empty Audio FX slot has the same menu as a track's: the native effects, Space Designer and every installed Audio Unit and VST3 effect, one submenu per maker (a plug-in installed in both formats is one entry). Open, bypass, move and remove work, the plug-in window floats like the others, and the settings are saved with the project (older projects open as before). The plug-ins take each whole block of the mix before the master fader, so they add only their own delay, not a block of buffering; a slot's tooltip shows that delay. MIDI out and MIDI clock are stamped that much later, and audio recordings are moved back by it, so both stay in time with what is heard. Their parameters have automation lanes on the Master track, like a track's plug-ins. In Low Latency Mode, while a track is played live, Stereo Out plug-ins that delay the sound more than 1 ms are passed over, as the slow built-in effects there are bypassed.

### Still to do or test

❌ Space Designer: loading your own impulse response files (the six responses are made in code, none is shipped as a WAV); tempo-synced Predelay (the note button is only drawn); Quality Low cuts the highs above 8 kHz and the silent tail, it does not run at half the sample rate

## 0.2.2

Tested on Ubuntu 26.04.1 LTS (x86-64, kernel 7.0.0-42): the full project check (formatting,
clippy with warnings as errors, all tests, real-time allocation check, miri, cargo-deny and the
wasm build), the layout screenshots, and the MIDI tests through real ALSA ports. No audio device
and no MIDI hardware were used. Windows and macOS builds are not tested in this release.

### New: instruments and effects

✅ **Amp Designer:** a guitar and bass amplifier in any Audio FX slot. Five models (Clean Combo, Tweed, British Crunch, Plexi Lead, Bass Amp), tone stack, three speaker cabinets or none, and a microphone position from the edge of the cone to its centre. It reports its delay to plug-in delay compensation.
✅ **SoundFont (.sf2) instruments:** an instrument track can play a preset from a SoundFont file (General MIDI and drum kits), chosen in the Inspector. Synthesis is pure Rust (no native library).
✅ **Apple Loops tags:** the Loop Browser reads the category, genre, key and beat count of Apple Loops (`.caf`) and shows them under each name; search finds loops by them.

### New: editing

✅ **Cut, Copy and Paste regions:** Edit menu. Paste puts a copy on the selected track at the playhead, keeping the distances between several regions. Audio regions paste only onto audio tracks.
✅ **Split at Playhead:** cuts every selected MIDI region under the playhead. A note that crosses the cut ends there.
✅ **Select All and Join:** Edit > Select All selects every region. Shift or Command click adds or removes one, and dragging a selected region moves the whole selection. Join merges the selected MIDI regions of one track into one region, keeping every note in place.
✅ **Functions menu:** Move to Playhead, Quantize (the notes of selected MIDI regions go to the region's Quantize grid, 1/16 when Off), and Folder (puts the selected track into a track stack).
✅ **Track menu:** Rename Track, Assign Track Color (16 swatches), Assign Track Icon (15 icons or Automatic), Configure Track Header (hide the number, icon, name, M/S/R/I buttons, volume slider or pan knob), and Show Master Track (a Master strip with the master fader under the tracks).

### New: project and transport

✅ **Project Settings** (File > Project Settings): General (project name), Synchronization (SMPTE frame rate, start time, MIDI Time Code and MIDI Clock ports), Metronome (click volume and accent of the first beat), Recording (1 to 4 bars of count-in), Tuning (reference pitch A4 from 427 to 453 Hz), Audio (output device and sample rate, read only), MIDI (input port for live notes, and Send MIDI Panic). All of it is saved with the project.
✅ **Follow MIDI Clock:** Synchronization > Follow MIDI Clock: the song starts, continues, stops and jumps where a MIDI clock input says, and its tempo is the clock's.

### New: MIDI

✅ **MIDI outputs:** an external MIDI track sends its bank and program when it is routed (Inspector: MIDI Program, MIDI Bank). Synchronization > Send MIDI Clock sends Start, Continue, Stop, Timing Clock and Song Position to a port. Send MIDI Panic silences every note on every output the tracks use.
✅ **Pitch bend, channel pressure and SysEx in regions:** a keyboard's pitch bend and channel pressure are recorded; MIDI files keep them and their SysEx messages, both ways; external MIDI tracks send them (bend at 14 bits, each SysEx message whole).
✅ **Event list:** shows pitch bend with its full value, channel pressure as Aftertouch, and each SysEx message as one row. **SysEx…** adds a message typed in hex at the playhead.

### New: metronome and tuning

✅ The metronome level and accent, the count-in bars and the reference pitch are now real engine settings (`SetClick`, `SetTuning`), not only displayed.

### Fixed

✅ **Denormal numbers on the audio thread** are flushed to zero on x86-64, so a fading reverb or filter tail no longer causes CPU spikes (contributed by SakaBoluwatife).
✅ **Missing interface texts:** ten texts of the Amp Designer, the Guitar Amp category and the SoundFont inspector were missing from every translation, so the translation test failed. Added to all 16 languages.
✅ **Stuck MIDI notes:** a note still sounding when its track was moved to another port or channel was never released; it is now released on the port it started on.
✅ **Missing clock port:** a followed MIDI clock port that is not there is looked for every few seconds, not every frame; clearing it no longer keeps its name for the next choice.
✅ **Capture Recording** keeps the sustain pedal, pitch bend and channel pressure (it used to drop the pedal).
✅ **Master strip** no longer shares a widget ID with the transport fader.

### Known limitations

❌ **Not tested with hardware:** no real MIDI synth, clock source, audio interface or plug-in was used. The MIDI tests use ALSA virtual ports.
❌ **Plug-ins:** CLAP, VST3 and AU hosting does not exist yet. The Plug-in Manager is greyed out.
❌ **Bounce:** Bounce in Place and Bounce and Join need offline rendering, which the engine does not have yet.
❌ **Convert to New Sampler Track:** needs a sampler instrument, which the project does not have yet.
❌ **Chord and Arrangement tracks:** Show Chord Track and Show Arrangement Track need data that does not exist yet.
❌ **Split and Join on audio, Pattern, Drummer and take-folder regions:** not done. Audio needs a start offset into the file in the engine.
❌ **Folder:** the Functions menu's Folder creates a track stack, our reading of Logic's "Folder".
❌ **Audio settings:** the output device and buffer size cannot be chosen yet. Project Settings > Audio shows them read only.
❌ **MIDI:** poly aftertouch is not read; SysEx cannot be edited byte by byte and is not made for audio, Pattern or Drummer regions; pitch bend and SysEx do not reach the internal instruments; a seek while stopped sends no clock, and while playing it sends a Song Position; a MIDI clock output on the same device as the followed input would echo back.
❌ **Follow MIDI Clock:** the Play and Stop buttons still work next to a followed clock; a sudden tempo change takes about two beats to follow.
❌ **Master strip:** it has no automation lane yet.
❌ **Apple Loops:** the older AIFF-based loop format is not read; the tag reader was built from community notes on the format, not from Apple's files.
❌ **Translations:** the texts added in this release are machine-written and need a native-speaker review.
❌ **Linux graphics fallbacks:** still to be confirmed by a tester with an NVIDIA card; audio recording and sound on real Linux and macOS hardware.

## 0.2.1

### All systems (Windows, macOS, Linux)

✅ **More audio formats:** Pro Wave now opens Ogg Vorbis (`.ogg`, `.oga`), M4A and MP4 (AAC and ALAC) files, and ADPCM audio, besides WAV, AIFF, FLAC, MP3 and CAF.
✅ **Recording tells you when the input fails:** if the microphone or audio interface is unplugged or taken away by the system while you record, the take now says "Part of the recording was lost" instead of ending quietly.

### Linux

✅ **Graphics problems at launch:** when the window can't be drawn, Pro Wave now tries up to three more ways by itself before giving up: Vulkan, then the X11 window system when running under Wayland, then software OpenGL. This is for NVIDIA cards (reported on Ubuntu 26.04 with an RTX 5060) and other machines where OpenGL starts but can't draw to the window.
✅ **Wayland:** the window now has the same name as its menu entry (`prowave-studio`, or the Flatpak's own name inside a Flatpak), so the desktop shows Pro Wave's icon and groups its windows.

### Still to do or test

❌ Linux: the graphics fallbacks are written but not yet confirmed by a tester with an NVIDIA card; audio recording and sound on real Linux and macOS hardware
❌ Audio recording: cycle takes, punch in and out, amp and pedal effects for guitar and bass
❌ Apple Loops: reading their tags and tempo (the audio formats they use now open)

## 0.2.0

### All systems (Windows, macOS, Linux)

✅ **Audio recording:** record your voice or an instrument. Click the Input slot of an audio track's channel strip to pick the input (mono or a stereo pair) and, under *Input Device*, the audio interface or microphone. Press the track's R button (its meter then shows the input level), turn Record on and press Play: a red region grows while you record and becomes the waveform when you stop. Several armed tracks record their own inputs at the same time. The recording is saved inside the project, and it is lined up with the song by the audio delay the system reports.

✅ **Hear your input:** the I button on an audio track plays its input through the track's channel strip, with its effects. Use headphones to avoid feedback.
✅ **Count-in works:** with Count-in and Record on, Play gives one bar of clicks and then starts recording.
✅ **Capture Recording:** the button next to Record turns what you just played on a MIDI keyboard while the song was running into a region, without having recorded it.
✅ **The R button on instrument tracks** now picks the track MIDI records on.
✅ **Mixer:** the Setting slot copies, pastes or resets a channel strip, and the Output slot sends a track to Stereo Out or to a bus.
✅ **New panes:** Quick Help, a Toolbar, a Note Pad saved with the project, a Loop Browser for a folder of your choice, and All Files. Double-click a file in a browser to add it.
✅ **Window menu:** the Record entry is gone; it switched Record on instead of opening a window.

✅ **No more demo project:** Pro Wave now always starts with a clean project, and Demo Song is gone from Project Templates.

### Linux

✅ **Fixed a crash at launch** with the error "no graphics adapter can draw to this window", reported on Ubuntu 26.04 with an NVIDIA card: when OpenGL can't draw to the window, Pro Wave now restarts itself on Vulkan.

✅ **A project's audio file that is cut shorter while it plays** now carries on from the exact place, not from the start of the block it was in.

### Still to do or test

❌ Audio recording: cycle takes, punch in and out, amp and pedal effects for guitar and bass
❌ Mixer: automation of sends, Bezier curves with two handles, plug-in (CLAP / VST3 / AU) hosting, a multi-band compressor
❌ Flex Time: dragging Flex markers with the mouse, Flex Pitch per note (drift, vibrato, formant), Groove Track for audio tracks to follow
❌ Smart Tempo: Adapt Region Tempo while recording, a tempo that is not steady in the Editor's beat grid

## 0.1.10

### All systems (Windows, macOS, Linux)

✅ **Detect Tempo:** right-click an audio region > *Smart Tempo > Detect Tempo* says its tempo, *Adapt Project Tempo to Region* makes it the project's tempo. The file is read in the background and the onsets (where hits and notes start) are found along the way, ready for Flex Time.

✅ **Smart Tempo Editor:** right-click an audio region > *Smart Tempo > Smart Tempo Editor…* shows the beats found over the waveform; ×2 and ÷2 fix a tempo read at half or double, the arrows move the downbeat. *Adapt Project Tempo* makes the project follow the beats, including tempo changes inside the recording, and keeps every audio region at the same time. *Adapt Project Tempo to All Audio Regions* hears all the audio together (multitrack), and *Region Follows Project Tempo* moves the region's beats onto the project's.
✅ **Flex Time:** right-click an audio region > *Flex Time*: pick a mode (Slicing, Rhythmic, Monophonic, Polyphonic, Speed, Tempophone), *Quantize Audio to 1/16 or 1/8* puts every hit on the grid, *Remove Flex Edits* goes back. The recording is never changed; the edit is saved with the project.
✅ **Flex Pitch:** *Flex Pitch > Correct Pitch to Key* moves every note it hears to the nearest note of the project's key, and *Create MIDI Track from Flex Pitch* writes the notes it hears on a new software instrument track.
✅ **Groove Track:** right-click a track > *Set as Groove Track*, then *Follow Groove Track* on MIDI tracks: they take its timing (swing and push), from its notes or, for audio, from the hits in its recordings.

✅ **Real effects:** each Audio FX slot of a channel strip now holds an effect that changes the sound: Channel EQ, Compressor, Space D (reverb), Tape Delay, Chorus, Phaser, Overdrive and Filter. Click a slot to add, replace, move, bypass or remove one; double-click it for its window (sliders, the EQ curve, the compressor's gain reduction). The strip's EQ thumbnail and Gain Reduction meter show what the effects do.
✅ **Buses and sends:** a strip has up to four sends (pre or post fader, with a level knob) to Bus 1-8, and Aux strips play the bus of their number (Aux 1 plays Bus 1). *Sends on Faders* makes the faders set the sends to one bus. *Edit > Copy/Paste Channel Strip*, and *Options > Create New Auxiliary Channel Strip* and *Create New VCA*.
✅ **Automation:** the strip's Automation slot picks Off, Read, Touch, Latch or Write for the fader and the pan. Move a control while the project plays to write it; Read plays it back (the fader moves by itself). *Show Automation* opens the editor: click to add a point, drag to move, double-click to delete. Saved in the project.

✅ **Noise Gate, Limiter and Multi-Band EQ:** three more effects for the Audio FX slots. The Limiter never passes its ceiling (it looks 2 ms ahead); the Multi-Band EQ has six bands (low shelf, four bells, high shelf) and draws its curve in its window and in the strip's EQ thumbnail; the Gain Reduction meter shows the Gate and the Limiter too.
✅ **Delay compensation:** an effect that looks ahead (the Limiter) no longer puts its track out of time: every other strip, and what is sent to buses, is delayed by the same amount.
✅ **Better sums:** the strips and the buses are added in 64 bits, so many tracks add up without losing quiet detail to rounding.
✅ **Automation of effect settings and curves:** move any slider of an effect while the project plays to write it (Touch, Latch, Write), and in Read mode it moves by itself; pick the setting in the Automation editor's *Parameter* menu. A handle in the middle of each stretch of a lane bends it (double-click to straighten).
✅ **Mix Groups:** the strip's Group slot makes or picks a group (*Options > Create Group…* too). Tracks in a group move their faders and pans together by the same amount, and Mute and Solo follow, as the group's settings say. Saved in the project.

### Still to do or test

❌ Mixer: automation of sends, the Output slot, Bezier curves with two handles, plug-in (CLAP / VST3 / AU) hosting, a multi-band compressor
❌ Flex Time: dragging Flex markers with the mouse, Flex Pitch per note (drift, vibrato, formant), Groove Track for audio tracks to follow
❌ Smart Tempo: Adapt Region Tempo while recording, a tempo that is not steady in the Editor's beat grid

## 0.1.9

### All systems (Windows, macOS, Linux)

✅ **Flatpak can reach the network:** Check for Updates failed with "failed to lookup address information" in the Flatpak, whose sandbox had no network access.
✅ **Close Project:** *File > Close Project* now closes the project (after asking about unsaved changes) and leaves a new, empty one.
✅ **Safer and lighter Flex and Smart Tempo:** the sounds Flex makes are kept in your own config folder (not a shared temporary one), cleared after two weeks, and made again if the recording was replaced; a project file can no longer ask for an unbounded number of Flex markers; at most four Flex sounds are made at once; the same loop used in many regions is analysed once.
✅ **Check for Updates on slow connections:** the check now tries three times, 30 seconds each, before it reports a timeout.

✅ **Several time signatures per song:** *Track > Show Signature Track*. Double-click to add a change at a bar, double-click a block to set it, drag it to another bar, right-click to delete it. The ruler, the grids, the Piano Roll, the Event list, the display and recording follow every change, and so does the engine (the metronome accents each bar). The display shows the signature at the playhead and edits it.
✅ **Fade tool and crossfades:** the *Fade Tool* in the tool menu: drag the left half of an audio region for a fade in, the right half for a fade out. *Drag: X-Fade* (or the Inspector's *X-Fade* box) crossfades a region with the audio region it overlaps.
✅ **VCA tracks:** *Track > New VCA Track*. A VCA moves the level of the tracks you give it in their *Group* slot, and its Mute and Solo reach them too.
✅ **External MIDI tracks:** *Track > New External MIDI Track*. Pick a MIDI port and channel in the Inspector and the track's regions, and notes you play, go out to your synthesizer.
✅ **MP3 and CAF audio:** drag them onto the window like WAV, AIFF and FLAC.
✅ **1000 tracks:** the engine plays 1000 tracks (1000 audio tracks streamed for 20 seconds with no dropouts on the test PC), and the interface stays quick with them.
✅ **Lower delay on Windows:** about 12 ms instead of 32 ms between Pro Wave and the speakers on the test PC (a sound card driver with smaller buffers can do better). If it cannot be used Pro Wave falls back to the old way by itself.
✅ **Smoother meters and playhead:** the engine hands the playhead, the transport and every meter to the interface together, after each block of sound.
✅ **Tempo curves:** right-click a point of the Tempo track > *Ramp to Next Tempo Change*. The tempo moves in a straight line to the next point, and the engine follows it.
✅ **SMPTE and MIDI Time Code:** the time in the control bar follows every tempo change and curve; right-click it to pick the frame rate (24, 25, 29.97, 29.97 drop frame, 30), the start time, a MIDI port to *send* MIDI Time Code to, or a port to *follow* it from (the transport starts, jumps and stops with it).
✅ **Pattern regions and Drummer regions:** *Track > New Pattern Region* and *New Drummer Region* (or double-click an empty stretch of a Drummer track). A pattern is a grid of steps with a velocity each; a Drummer plays Rock, Pop, Funk, Jazz, Hip Hop or Electronic, busier or calmer and louder or softer as you drag its pad, with fills and variations. *Convert to MIDI Region* makes them editable note by note.
✅ **Memory-mapped audio files:** WAV and AIFF files are read straight from memory (the same samples as before). 1000 tracks take about a fifth less memory of their own.
✅ **Step Sequencer extras:** each step of a pattern can have a *Chance*, a *Repeat* count and a *Tie*, and the pattern plays *Forward*, *Backward*, *Ping-Pong* or *Random*. The *Edit* box above the grid picks what a click sets.
✅ **MIDI files:** *File > Import > MIDI File…* (or drop a `.mid` on the window) and *File > Export > Selection as MIDI File…*. The tempo changes and signatures go along.
✅ **MIDI FX:** *Track > MIDI FX…*: Transposer, Velocity Processor, Scale Quantizer, Chord Trigger, Note Repeater, Arpeggiator, Randomizer, Modulator and Scripter, in a chain on the selected track.
✅ **Modulator:** a sine, triangle, saw or square wave (its cycle in beats or bars, its depth) that moves a MIDI controller (any number, or pitch bend) of an External MIDI track while the song plays.
✅ **Scripter:** write the processing of the notes yourself in a small script (Rhai). The script is checked as you type, runs in a sandbox and stops if it takes too long.
✅ **MIDI Transform:** *Functions > MIDI Transform…* in the Piano Roll: pick the notes (all of them, or by pitch, velocity, position or length) and what to do to them (transpose, set or add velocity, shift, change length, scale, randomize). Presets: Transpose, Fixed Velocity, Legato, Humanize and more.
✅ **Sustain pedal and controllers to your synthesizer:** the pedal of a MIDI region, and the Modulator, now go out to External MIDI tracks (and are let up when you stop).
✅ **Score:** the *Score* tab of the editor shows a MIDI region as notation, with the treble and bass clefs, the key signature (pick the key at the top; minor keys too), the time signature, accidentals that last to the end of the bar, rests, and ties for notes that cross a bar line or are not one note value long. Click to add a note (in the key), right-click a note to delete it.

✅ **Safer with odd files and devices:** a MIDI file with a huge number of notes, tempo changes or tracks, or with times far past the end of a song, no longer slows Pro Wave down or breaks it (files over 128 MB are refused), very long pattern regions and MIDI FX chains stop at a size limit instead of using all the memory, and a MIDI device that floods a port can no longer fill the memory. The interface does less work each frame for pattern and Drummer regions, the Score and the Tempo track.
✅ **Tested with the pointer:** the Pattern grid, the Drummer pad, the Score and the MIDI FX window are now tried in automatic tests that click, drag and right-click like a person would.

### Still to do or test

❌ Several signatures: the Event list's *Signature* tab
❌ MIDI Time Code has not been tried with real hardware; MIDI Machine Control is not there
❌ Drummer regions: following a Groove Track
❌ Score: beams, voices, chord symbols, printing and MusicXML; MIDI files: program changes, pitch bend and other controllers
❌ External MIDI with a real synthesizer (checked with automated tests only), MIDI clock and program changes
❌ VCA tracks: automation, and a VCA for the Master
❌ Crossfades: a region wholly inside another, and *No Overlap* trimming
❌ ASIO (built, but no ASIO driver to try it with)

## 0.1.8

### All systems (Windows, macOS, Linux)

✅ **Updates from inside Pro Wave:** it looks for a newer release on GitHub when it starts (at most every 6 hours; *Help > Check for Updates…* checks at once, *Check for Updates Automatically* turns it off). A window shows what's new with *Install update*, *Download page*, *Later* and *Skip this version*. Nothing about you is sent.
✅ **Installs the right package for how you installed it:** the Windows zip, the macOS app, the AppImage, the Linux tarball, and for `.deb` and `.rpm` installs that very package, installed with the system's package manager (the system asks for your password). A Flatpak bundle is downloaded and checked, and the command to install it is shown.
✅ **Closes, updates, opens again:** *Close and update* closes Pro Wave, puts the new version in place and opens it. The download is checked against the SHA-256 that GitHub publishes for it, the old version is kept until the new one has been used once, and a failed update goes back to the old one and tells you why.
✅ **MIDI recording:** play a MIDI keyboard with *Record* and *Play* on: a red region grows on the instrument track and the notes appear in it as you play. When you stop it turns into a normal region (the track's colour), ends on a bar line and plays back. The recording is one Undo step.
✅ **Sustain pedal:** recorded with the notes, saved with the project and shown in the Event list as *Control · Sustain*. Every recording starts with where the pedal was.
✅ **Take folders:** record with the Cycle on and every pass is a take. Two or more takes make a take folder: a yellow bar with the name and a triangle, and under it the comp, side by side, with a line where one take gives way to the next. Open it to see a row for each take (latest on top), drag on a take to play it in that stretch, click to pick the whole take. *Flatten Take Folder* turns the comp into an ordinary region. Saved with the project.
✅ **List Editors (Event list):** the icon at the top right, *View > Show List Editors* or **D**. It lists the MIDI events of the selected region: position, note, velocity and length, with the notes of the other regions of the track in grey. A line marks the playhead and moves down the list at the speed of the notes while the song plays. Red while recording.
✅ **Looks like Logic:**
- Regions use Logic's own colours (measured from Logic Pro); a selected region is deeper with a yellow-green outline, and the ruler over it turns gold.
- The playhead is yellow, and red while recording; the control bar's display turns red while recording too.
- Piano Roll: notes coloured by velocity with a light edge, the region bar in the track's colour, a gold ruler, a Logic-style inspector header with the region and track, and the velocity lane drawn as dots with a line along each note.
- Track volume sliders are a dark pill with a thumb that stays inside; the level meter shows inside it. 0 dB sits two thirds up, as in Logic.
- Tracks that make no sound (muted, or another track is soloed) show their instruments, plug-ins, sends and meters in grey.
✅ **The mixer keeps its size:** the mixer panel can no longer be stretched, which only stretched the faders and meters. The Piano Roll can still be made taller.
✅ **The demo project is shown once per version** (the first time you open each new version) and then Pro Wave starts clean.
✅ **Still light:** about 330–420 ms to start and 120 MB of memory at rest (Windows), 15.5 MB program (the update check adds about 1 MB)

### Still to do or test

❌ MIDI recording with real keyboards and the sustain pedal on other computers (checked by automated tests and a simulated recording only)
❌ The pedal is recorded and shown but the engine does not play it yet
❌ Take folders: editing notes needs *Flatten* first; making a take folder from existing regions, crossfades in the comp
❌ Event list: the Marker, Tempo and Signature tabs, event type filters and editing values
❌ The update install on a real Mac, a real Linux machine and with `pkexec`; only Windows was tried end to end
❌ Help > Language texts for the new messages checked by native speakers
❌ Region colours other than green, blue, yellow and purple are an estimate

## 0.1.7

### All systems (Windows, macOS, Linux)

✅ **Tempo track:** *Track > Show Tempo Track*. Double-click to add a tempo change, drag a point to change or move it, double-click or right-click to delete. Saved with the project and undoable; the control bar shows the tempo at the playhead.
✅ **Marker track:** *Track > Show Marker Track*. Name sections (Intro, Verse, Chorus...): double-click to add, click to jump there, drag to move, double-click to rename, right-click for colour or delete. *Navigate > Create Marker*, *Delete Marker*, *Go to Next / Previous Marker*.
✅ **Audio region edits:** Region Inspector *Gain*, *Fade In*, *Fade Out*, *Curve* (Equal Power, Linear, S-Curve) and *Reverse*; right-click *Normalize Region Gain* and *Reverse Region*. The audio file is never changed. Fades show on the region and the waveform follows gain and reverse. Saved with the project.
✅ **Compact menus like Logic:** each menu is as wide as its longest entry instead of stretching across the window. The File menu follows Logic, with ▸ submenus (Open Recent, Revert to, Import, Export, Bounce, Share...), also in the macOS menu bar. Entries that aren't built yet are greyed out.
✅ **24 tracks without dropouts:** 60 seconds of 24 stereo 48 kHz / 24-bit audio tracks: 0 dropouts, 1.6 % CPU (Windows)
✅ **Lighter audio regions:** the read-ahead buffer of each audio region follows the size of the song: at most 0.68 s per region and 8 MiB for all regions together (up to 128 regions), then 0.17 s each. Before it was a fixed 0.5 MB per region (about 12 MB for 24 tracks).
✅ **More efficient reverse playback:** reuses decoded audio between blocks to reduce repeated disk seeks; automated checks cover resampling and jumping within a reversed region
✅ **Playback during large imports:** the disk worker refills active regions between batches of new-file requests and skips canceled regions before opening their files
✅ **Lighter region playback:** reuses calculated region timing between edits and skips unnecessary fade-curve calculations outside the fade
✅ **Lighter mixer updates:** during playback, volume and pan edits recalculate gain targets for the changed channel while still updating mute and solo across channels
✅ **Steadier playback:** broken values (from a damaged project or a bad setting) can no longer make noise or stop the sound
✅ **No freeze when normalizing:** *Normalize Region Gain* reads the file in the background (a long recording used to freeze the window for seconds); the status line shows "Normalizing…".
✅ **SMPTE timecode fix:** a position exactly on a frame was shown one frame early (up to 8 % of frame-exact positions at 25 fps).
✅ **Lighter transport** when the song has tempo changes; tempo ramps avoid repeated logarithm calculations between tempo boundaries
✅ **17 languages:** *Help > Language*: English (default), 简体中文, 日本語, Français, Deutsch, Español, Português (Portugal), Português (Brasil), Filipino, Bahasa Indonesia, हिन्दी, Українська, Norsk, Svenska, Suomi, Polski, Lietuvių. Each language is listed in its own name; the choice switches the whole interface at once (menus, panels, dialogs, messages, also the macOS menu bar) and is kept for the next start. Chinese, Japanese and Hindi use the system's fonts.

✅ **Still light:** about 360 ms to start and 120 MB of memory at rest (Windows), 13 MB program

### Engine groundwork (not in the interface yet)

✅ Tempo ramps (gradual tempo changes)
✅ SMPTE timecode at 24, 25, 29.97 (drop / non-drop) and 30 fps
✅ Windows ASIO for low latency, as an optional build feature

### Still to do or test

❌ Region gain, fades and reverse heard on real recordings (checked by automated tests only)
❌ Dragging fades directly on a region (Fade tool) and crossfades
❌ Tempo ramps and SMPTE display in the interface
❌ Several time signatures in one song (Signature track)
❌ ASIO in the Windows download, and its latency measured on real hardware
❌ macOS menu bar submenus tried on a real Mac
❌ Translations checked by native speakers (written by the AI, verified by automated tests for completeness, placeholders and fonts)

## 0.1.4

### All systems (Windows, macOS, Linux)

✅ **Fixed: piano sounds going to `inf`.** Mellow Grand, the other grands and Chopped Vocal made the filter blow up (meter stuck on `inf`, red clipping). The filter stays stable now and meters always reset.
✅ **Undo / Redo:** Edit menu, ⌘Z / ⇧⌘Z (Ctrl+Z / Ctrl+Y on Windows & Linux), up to 100 steps
✅ **Track > New** now adds a track
✅ **Unfinished menu items are greyed out** instead of doing nothing
✅ **Newly working:** Edit > Delete / Deselect All, Go to End, Locators, Next / Previous Region, Zoom In / Out / to Fit
✅ **Right-click menus** on regions, track headers and empty lanes (Control-click on Mac)
✅ **Small screens (13" MacBook Air):** UI scales to fit, toolbars no longer overlap, View > Bigger / Smaller Interface (⌘ = / - / 0)
✅ **Lighter:** ~100 MB less memory, faster start
✅ **Benchmarks:** 320 ms start, 125 MB idle, 3.5 % CPU for 24 tracks (Windows)

### Windows & Linux

✅ **No title bar.** Minimize / maximize / close are in the menu bar. Drag the bar to move, double-click to maximize, drag the edges to resize.
✅ The tall GNOME / Wayland header on Linux is gone
✅ **Smooth maximize and resize:** the interface no longer rescales on every frame while the window changes size, so the animation stays fluid

### macOS

✅ Menu bar items greyed out when not available yet

### Still to test

❌ Piano meter fix on a real Mac
❌ macOS benchmark
❌ Windows maximized with an always-visible taskbar

## 0.1.3

### All systems (Windows, macOS, Linux)

- **Demo song on the first launch only.** The very first time Pro Wave starts it opens the demo
  song; every later start begins with a clean project (one empty instrument track).
- **Project Chooser.** *File > New from Template…* opens a chooser with New Project, your Recent
  projects and Project Templates: Empty Project, Songwriter, Electronic, Podcast and Demo Song.
- **Window title** is now just "Pro Wave", without the project name and "Tracks".
- **Fixed:** the waveforms of audio tracks without a recording (Electric Bass and Lead Vocal in
  the demo) stayed still while the Tracks area scrolled or zoomed. They now move with their
  regions.

### macOS

- **Menus in the macOS menu bar.** The menus now sit in the menu bar at the top of the screen
  instead of inside the window, with a Pro Wave menu (About, Hide, Quit ⌘Q), the ⌘N, ⌘O, ⌘S and
  ⇧⌘S key commands, check marks in the View menu and an Open Recent submenu. Errors and warnings
  show on a thin line at the top of the window.
- **Fixed:** the Dock icon switched from the Pro Wave icon on its rounded tile to the bare logo
  once the app had started. It now keeps the app's icon.

### Windows

- The README and these notes explain what to do when an antivirus (Norton, Avast, AVG, McAfee)
  blocks `prowave.exe` with "Windows cannot access the specified device, path or file".

### Linux

- No Linux-only changes: the .deb, .rpm, AppImage, Flatpak and tarball include all the changes
  listed under *All systems*.

## 0.1.2

- **Aux and Folder tracks.** *Track > New Aux Track*, and *Create Track Stack…* / *Release Track
  Stack* to group tracks in a folder that folds open and closed, with folder Mute and Solo.
  Routing audio through them needs audio engine work that is not done yet.
- **Editable time signature.** Drag the numerator or the denominator in the display up or down.
- **File menu:** Open Recent, Save A Copy As… and Revert to Saved.
- **Windows zip** added to the release downloads, with the Pro Wave icon on `prowave.exe`.
