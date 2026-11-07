# BOSS SP-303 Dr. Sample — reference notes

Source of truth: BOSS SP-303 Owner's Manual (English, © 2001, 77-page PDF),
downloaded from `https://static.roland.com/assets/media/pdf/SP-303_OM.pdf`
(identical copy on archive.org item `manualsbase-id-232507`).

Page references are the **printed page numbers** ("p. N"), which match the manual's own
cross-references. The printed number 27 appears twice (the effects-list footnote page
and the first page of Chapter 3), so from Chapter 3 onward **PDF page = printed page + 1**.

Labels: **[DOC]** = stated in the manual. **[PHOTO]** = seen in photos. **[FORUM]** = user
claims, not verified. **[INFERRED]** = my reading; not stated anywhere.

---

## 1. Memory, pads, banks

- 8 pads (1–8) per bank. 4 sample banks A–D. A and B are internal, C and D live on the
  SmartMedia card. That gives 16 internal and 16 card samples. [DOC p.18, p.72]
- 4 pattern banks A–D, organised the same way: 8 patterns per bank, A/B internal, C/D on card. [DOC p.43, p.72]
- Sample banks and pattern banks are independent. [DOC p.15]
- Bank A is selected at power-on. [DOC p.18]
- Card: 3.3 V SmartMedia, 8–64 MB. 1, 2 and 4 MB cards are not supported. The card also has a
  backup area holding up to 7 saved sets of the internal A+B data, using pads 1–7 as slot numbers. [DOC p.48–51, p.72]
- Polyphony is 8 voices. A stereo sample uses 2 voices. Newer notes take priority: the oldest
  pad is cut. During resampling the maximum is 4 voices (4 mono or 2 stereo). [DOC p.13, p.18]
- Pads have no velocity. Volume does not follow how hard you hit. [DOC p.13] MIDI Note On velocity *does* change volume. [DOC p.54]
- "Current pad" is the last pad played. Settings edits apply to it. [DOC p.18]

## 2. Sampling grade, rate, time

| Grade (LONG/LO-FI button) | Button state | Sample rate | Internal time (mono, A+B total) |
|---|---|---|---|
| STANDARD | unlit | 44.1 kHz | ~31 s |
| LONG | lit | 22.05 kHz | ~63 s |
| LO-FI | blinking | 11.025 kHz | ~3 min 10 s |

[DOC p.27, p.72]. Stereo halves the time. [DOC p.27] On a card: 64 MB gives 33 / 66 / 202 min. [DOC p.27]
Converters are 20-bit A/D and 20-bit D/A. [DOC p.72]

- Grade and stereo/mono are fixed when you sample and cannot be changed later. [DOC p.27]
- STEREO lit = stereo, unlit = mono. Mono sampling of a stereo input records the same sound on L and R. [DOC p.27, p.30]
- The manual never states the internal bit depth per grade. **Unknown.**
- Auto Sampling: hold CANCEL and press REC, then set the threshold with CTRL 3 in 9 steps (0–8). The display shows "-3-"; "-0-" means off. "rdY" means armed. [DOC p.31–32]
- WAV/AIFF import: 44.1 kHz only, files named `SMPL0001…0008`, minimum ~100 ms. [DOC p.52]

## 3. Buttons: what each does

| Control | Function |
|---|---|
| GATE | Lit = Gate (sound only while held). Unlit = Trigger (press to start, keeps playing). Set per pad. New samples default to Trigger. [DOC p.8, p.36–37] |
| LOOP | Lit = loop. Unlit = one-shot. Set per pad. Trigger+Loop: a second press stops. Trigger+One-shot: a second press restarts. [DOC p.8, p.36] |
| REVERSE | Per-pad reverse playback. Start and End swap. Turns off Time Modify. [DOC p.8, p.37, p.42] |
| HOLD (pad) | Press while holding a Gate pad to latch it. Press again to stop. Does not light up. No effect on Trigger pads. [DOC p.8, p.19] |
| EXT SOURCE (pad) | Passes LINE IN / MIC audio through without sampling, so it can be effected. Follows GATE (momentary) or toggles. STEREO sets the input to stereo or mono. Its level is set with START/END/LEVEL + CTRL 3. [DOC p.8, p.19–20] |
| MARK | Sets Start/End markers while the sample plays (see §4). Lit = only the marked region plays. Also starts truncate (DEL then MARK). [DOC p.8, p.38–41] |
| DEL | Delete a sample or pattern: DEL, pad, DEL. CANCEL+DEL deletes all. DEL+REC swaps two pads. During pattern recording: erase notes. [DOC p.8, p.33–34, p.46–47] |
| REC | Start and stop sampling. In pattern mode: start and stop pattern recording. Panel label "PATTERN REC". [DOC p.8] |
| RESAMPLE | Bounce a pad through the current effect to a new pad (see §7). [DOC p.8, p.32–33] |
| REMAIN (CURRENT PAD) | Shows remaining sample time (STANDARD/mono figure). REMAIN + pads chooses which pads get the effect. REMAIN + effect button applies it to all pads. [DOC p.8, p.22–23, p.35] |
| CANCEL (PATTERN STOP) | Cancels a sampling or delete operation. Stops the pattern. [DOC p.8, p.44] |
| LONG/LO-FI, STEREO | Sampling grade and channel count (see §2). |
| START/END/LEVEL | Edit mode: CTRL1 = Start, CTRL2 = End, CTRL3 = level (0–127). In pattern recording: metronome volume. [DOC p.7, p.38, p.44] |
| TIME/BPM | Edit mode: CTRL1 = Time Modify, CTRL2 = BPM. In pattern mode: pattern tempo. [DOC p.7, p.41–43] |
| TAP TEMPO (EFFECT GRAB) | Tap 4 times to set BPM to the quarter-note interval. Hold it and press an effect button for a momentary effect (on only while held). [DOC p.8, p.22, p.31, p.43] |
| PATTERN SELECT | Lit = pads and banks select patterns ("Ptn"). Unlit = pads play samples. [DOC p.7–8] |
| LENGTH / QUANTIZE | Pattern length and quantize, both set with CTRL 3 (see §6). [DOC p.8, p.45] |
| BANK A–D | Choose the sample bank, or the pattern bank when PATTERN SELECT is lit. [DOC p.8] |
| MFX | Hold MFX and turn CTRL 3 to pick MFX type 1–21. Release to switch it on. [DOC p.22] |

## 4. Start/End, Mark, Truncate, sample BPM

- **Mark, live** [DOC p.38–39]
  - Play the pad, press MARK at the start point (MARK blinks, display "---"), then press MARK at the end point (MARK lights).
  - End only: hold MARK, press the pad, release MARK, then press MARK at the end point.
  - Start only: press MARK at the start, then press the pad again (or release it if it is a Gate pad).
- **BPM snapping** [DOC p.28–31, p.39]
  - If a BPM is set before marking or sampling, the End point snaps to the nearest beat measured from the Start point at that BPM.
- **Fine edit** with START/END/LEVEL lit [DOC p.40]
  - CTRL1 and CTRL2 are centre-detent relative knobs: centre = no change, displayed as "0".
  - The maximum move is about 130 ms per pass. Repeat the pass to go further.
  - Start and End cannot be closer than 100 ms.
  - Leaving START/END/LEVEL with the knobs off-centre commits the change. Returning them to centre first cancels it. [DOC p.38]
- **Truncate** [DOC p.41]
  - Requires MARK to be lit. Press DEL, then MARK ("trC"), then DEL.
  - Throws away the audio outside Start/End.
- **Sample BPM** [DOC p.29–30, p.41]
  - Calculated automatically from the Start-to-End length.
  - May come out at half or double the real tempo. The fix is TIME/BPM + CTRL2: turning left halves, turning right doubles.
  - The manual calls the result approximate. Digits after the decimal point are rounded off, so long loops drift. [DOC p.31, p.40]
- **BPM range** 40–200. Turning CTRL2 fully left shows "---" and clears the tempo. [DOC p.30]

## 5. Time Modify (tempo change without pitch change)

- Enter with TIME/BPM, then turn CTRL1 (TIME). [DOC p.42]
- Turning left lengthens the sample (slower). Turning right shortens it (faster).
- Fully left = "oFF" (original length). Fully right = "Ptn": follows the pattern tempo.
- Range is ½× to about 1.3× of the sample's original BPM, always clamped to 40–200.
- The manual warns of "a rapid tremor or noise". [INFERRED] This suggests a granular / overlap-add stretch.
- Disabled while REVERSE is on.
- SOS (Chris Carter, Aug 2001) gives the range as "50 to 130 percent".

## 6. Pattern sequencer

- 32 patterns: 4 banks × 8 pads. Tempo is **global**; you cannot store a tempo per pattern. [DOC p.43]
- Tempo range 40–200 via CTRL2 [DOC p.43]
  - 40–60 and 160–180: even values only.
  - 180–200: only 180, 183, 186, 190, 193, 196, 200.
  - Tap tempo (4+ taps) can reach any value 40–200. [DOC p.45]
  - p.43 says "turn the knob to the left to increase the BPM". This contradicts p.30 and p.41, which say right = faster. It is probably a typo.
- Recording [DOC p.44–45]
  - Steps: PATTERN SELECT, REC (metronome starts), BANK, pad, optional settings, REC.
  - One-bar count-in shown as "-4 -3 -2 -1". Pads hit during the count-in are not recorded.
  - Recording loops over the pattern length and overdubs on each pass. Bank changes during recording are allowed.
  - REC stops recording.
- Length 1–99 bars. 1–20 in 1-bar steps, then 4-bar steps. Set with LENGTH + CTRL3. [DOC p.45, p.72]
- Quantize values: `oFF`, `4`, `8`, `8-3` (8th-note triplet), `16`. Set with QUANTIZE + CTRL3. It can be changed during recording. [DOC p.45]
- Metronome: its volume is set with START/END/LEVEL + CTRL3 in record or standby. The manual gives no other settings. [DOC p.44]
- Erase while recording: press DEL ("ErS"), then hold a pad to wipe that pad's notes for as long as it is held. [DOC p.46, p.60]
- Resolution 96 PPQN. About 7,500 notes internal and about 7,500 on the card. [DOC p.72]
- Playback: pressing a pad in pattern mode starts that pattern, and PATTERN SELECT goes out. Switching patterns is immediate (not quantized to the bar). CANCEL stops. [DOC p.15, p.44]
- MIDI sync: hold TIME/BPM at power-on and choose with CTRL1 [DOC p.55]
  - `Aut`: clock plus Start/Stop/Continue.
  - `tMp`: clock only.
  - `oFF`: no sync.
  - MIDI clock cannot change the BPM of samples. [DOC p.54]

## 7. Effects: routing and application

- 26 effects: 5 on direct buttons plus 21 in MFX. **Only one effect can be on at a time** for the whole unit. [DOC p.7, p.21]
- By default an effect applies only to the current pad. Use REMAIN + pads to add or remove pads (a long blink means on, a short blink means off). [DOC p.22]
- REMAIN + effect button applies it to **all 32 pads**; the effect button blinks. All pads still share the same single effect. [DOC p.22–23]
- EXT SOURCE can be the effect target (REMAIN + EXT SOURCE). [DOC p.19]
- Effects can be printed while sampling from mic or line. CTRL3 is then the input level, so only CTRL1 and CTRL2 adjust the effect. [DOC p.28, p.30]
- [INFERRED] It acts as one insert effect on the bus carrying the selected pads; unselected pads stay dry. The manual does not show the signal routing.
- **Resample** [DOC p.32–33]
  1. Press RESAMPLE ("LEV").
  2. Choose the source bank/pad and switch the effect on. Adjust CTRL1 and CTRL2; CTRL3 is the resample level.
  3. Press REC (blinks) and choose the destination bank/pad. It cannot be the source pad.
  4. Set grade and stereo.
  5. Press REC (lit), then press the source pad. Playback and recording start together.
  6. Press REC to stop.
  - The whole path is digital. [DOC p.6]

## 8. Effects list (CTRL1 / CTRL2 / CTRL3, display code)

[DOC p.24–27]. **The manual gives no numeric ranges** except where noted. The display
shows the 3-character code while a knob is turned. [INFERRED] The display probably shows a
value too, but the manual does not say.

Direct buttons:
| Effect | CTRL1 | CTRL2 | CTRL3 |
|---|---|---|---|
| FILTER+DRIVE (LPF + overdrive) | CUTOFF `CoF` | RESONANCE `rES` | DRIVE `drV` |
| PITCH (pitch shifter) | PITCH `Pit`, ±2 octaves | FEEDBACK `Fdb` | DIRECT/EFFECT BALANCE `dAL` |
| DELAY | TIME `t32`…`t1`, tempo-synced note values | FEEDBACK `Fdb` | E.LEVEL `LEV` |
| VINYL SIM | COMPRESSOR `CMP` | NOISE LEVEL `noS` | WOW FLUTTER `FLu` |
| ISOLATOR | LOW `Lo` | MID `Mid` | HIGH `Hi` (each one cuts or keeps its band) |

Delay note values [DOC p.24]: t32, t16, t16., t8.3, t8, t8., t4.3, t4, t4., t2.3, t2, t2., t1.
- Synced to the BPM of the longest selected sample, or to the pattern tempo during pattern playback.
- Settings over about 1450 ms are not selectable.

MFX (hold MFX and turn CTRL3 to choose 1–21):
| # | Effect | CTRL1 | CTRL2 | CTRL3 |
|---|---|---|---|---|
| 1 | REVERB | REVERB TIME `tiM` | TONE `ton` | E.LEVEL `LEV` |
| 2 | TAPE ECHO | REPEAT RATE `rAt` (tape speed) | INTENSITY `int` | E.LEVEL `LEV` |
| 3 | CHORUS | DEPTH `dPt` | RATE `rAt` | E.LEVEL `LEV` |
| 4 | FLANGER | DEPTH/MANUAL `dPt` | RATE `rAt` (fully left = LFO stopped, CTRL1 becomes MANUAL) | RESONANCE `rES` |
| 5 | PHASER | DEPTH/MANUAL `dPt` | RATE `rAt` (same rule as FLANGER) | RESONANCE `rES` |
| 6 | TREMOLO/PAN | DEPTH `dPt` | RATE `rAt` | WAVEFORM `trM`/`PAn`: left side = tremolo, right side = pan |
| 7 | DISTORTION | DRIVE `drV` | TONE `ton` | LEVEL `LEV` |
| 8 | OVERDRIVE | DRIVE `drV` | TONE `ton` | LEVEL `LEV` |
| 9 | FUZZ | DRIVE `drV` | TONE `ton` | LEVEL `LEV` |
| 10 | WAH (auto/envelope) | SENS `SnS` | FREQUENCY `Frq` | RESONANCE `rES` |
| 11 | OCTAVE | −2 OCT LEVEL `oC2` | −1 OCT LEVEL `oC1` | DIRECT LEVEL `dir` |
| 12 | COMP | SUSTAIN `SuS` | ATTACK `Att` | LEVEL `LEV` |
| 13 | EQUALIZER | LOW `Lo` | MID `Mid` | HIGH `Hi` |
| 14 | LO-FI | SAMPLE RATE `rAt` | BIT `bit` | FILTER `FLt` |
| 15 | NOISE GEN | COLOR `CoL` (hiss tone) | QUALITY `qLt` (scratch rate) | LEVEL `LEV` (hiss level) |
| 16 | RADIO TUNING | TUNING `tun` | NOISE LEVEL `noS` | FREQUENCY RANGE `Frq` |
| 17 | SLICER (includes a flanger) | TIMING PTN `P01`–`P16` | RATE `rAt`: 1, ½, ¼ or ⅛ bar per cycle | FEEDBACK `Fdb` (flanger amount) |
| 18 | RING MOD | FREQUENCY `Frq` | EFFECT LEVEL `EFF` | DIRECT LEVEL `dir` |
| 19 | CHROMATIC PS (2-voice) | PITCH 1 `-C`…`CC` (±1 oct, semitones) | PITCH 2 `-C`…`CC` | BALANCE `bAL` |
| 20 | VOICE TRANS. | FORMANT `For` | EFFECT LEVEL `EFF` | DIRECT LEVEL `dir` |
| 21 | C.CANCELER (centre cancel) | L-R BALANCE `bAL` | LOW BOOST `Lo` | HIGH BOOST `Hi` |

- The SLICER's 16 gate patterns are only drawn as a figure (fig.r02-10). The image is not readable in the text layer and I did not transcribe it. It can be rendered from PDF page 27.
- The SLICER period follows the BPM of the longest selected sample, or the pattern tempo. [DOC p.26–27]
- Knob panel legends (printed on the panel) [PHOTO, DOC p.7]:
  - CTRL1: "CUTOFF / TIME/START"
  - CTRL2: "RESONANCE / BPM/END"
  - CTRL3/MFX: "DRIVE / LEVEL"

## 9. Vinyl Sim and compressor: what is actually known

**Documented** [DOC p.24]:
- Vinyl Sim = "sonic qualities of an analog record".
- Its three knobs:
  - CTRL1 COMPRESSOR: "the sense of compression … characteristic of analog records".
  - CTRL2 NOISE LEVEL: "volume of analog record noise".
  - CTRL3 WOW FLUTTER: "rotational irregularities".
- MFX 12 COMP is a separate effect: SUSTAIN (amount), ATTACK, LEVEL.
- The manual says nothing about filtering, sample-rate reduction, compression ratio or time constants.

**Forum and secondary claims (unverified, not measurements):**
- Gearspace thread "SP303 (Vinyl) Compressor Emulation" (kwazi, 2014): https://gearspace.com/threads/sp303-vinyl-compressor-emulation.930022/
  - The Ableton rack chains: LoPass → compressor (attack/release) → Redux bit-depth reduction → crackle → "glue" compressor. **That is the emulator's design, not a teardown.**
  - Rob Lo: the crackle and compression were "close", but the wow/flutter was missing. Turning the wow/flutter knob ("the 'drive' knob on the 303", i.e. CTRL3, which matches the manual) gives "nasty warped vinyl/chewed tape" sound.
  - devinkerr (Goodhertz): Vulf Compressor is "partially based on the 303/404/505".
- SP-Forums "The character of the SP303/505 compression" (2011): https://sp-forums.com/viewtopic.php?t=11844
  - blavatsky guesses "a pretty high ratio like 8:1 or 10:1". This is a guess, not a measurement.
  - Users report more white noise on the 303 than on the 505.
  - The 303's Vinyl Sim "distorts" when tweaked live.
  - djmellow: works well on snares, less well on kicks.
  - The 303 and 505 share the same 26-effect set (citing the SOS SP-505 review).
- OP-Forums "SP303 vinyl sim emulation on OP-1" (2016): https://op-forums.com/t/sp303-vinyl-sim-emulation-on-op-1/4861
  - Describes a "tight, squashed, lo-fi sound" in which drums make the whole mix "duck" in and out (pumping).
  - The suggested OP-1 recipe is max drive plus a short release.
  - A later post: the kick must be much louder than the other parts for the mix to duck.
- Wikipedia "Boss SP-303" quotes producer Dibiase: the 303's "vinyl sound compression … has a grittier sound" than the SP-404's. https://en.wikipedia.org/wiki/Boss_SP-303
- Beatproduction.net (rack write-up): https://beatproduction.net/sp-303-vinyl-sim-ableton-rack/ (no hardware analysis).

**[INFERRED] model for building, consistent with the above but unconfirmed:**
- A full-mix compressor with high ratio, fast attack and medium release, plus makeup gain, driven by CTRL1. Loud transients (kicks) cause audible ducking.
- An additive noise / crackle bed (CTRL2).
- Wow (slow, below ~1 Hz) plus flutter (faster, ~5–15 Hz) pitch modulation through a short modulated delay line (CTRL3).
- Some HF roll-off and grit.
- I found no published measurement of the attack, release, ratio or filter values.

## 10. Panel layout and colours

Dimensions are 170 W × 241 D × 67 H mm, 850 g. [DOC p.72] The unit is portrait-oriented, with the
jacks on the rear edge and the card slot on the front edge. [DOC p.9, PHOTO]

**Rear edge (top of the photo):**
- LINE IN L/R and LINE OUT L/R (RCA)
- MIDI IN
- POWER switch
- DC IN

**Front edge:**
- PHONES (¼")
- MEMORY CARD slot (centre)
- MIC LEVEL thumbwheel
- MIC jack

**Top panel, from top to bottom** [DOC fig.P01 p.7; PHOTO]:
1. Silver strip with the "BOSS" logo (left) and "Dr.Sample SP-303" (right).
2. Black block, upper half:
   - VOLUME knob (white MIN–MAX ticks)
   - Three CTRL knobs under a red "EFFECTS CONTROL" header line, with red tick scales
   - PEAK LED (small round, right of the CTRL2 legend)
3. Black block, lower half:
   - Left: "BPM/VALUE" 3-digit red 7-segment LED display behind a smoked window.
   - Centre: "DSP EFFECTS" 3×2 button grid:
     - Top row: FILTER+DRIVE, PITCH, DELAY
     - Bottom row: MFX, VINYL SIM, ISOLATOR
   - Right: "SAMPLE EDIT" column with START/END/LEVEL above TIME/BPM, separated by a vertical white line.
   - Under the block, the MFX 1–21 list is printed in white in 6 columns.
4. Silver area:
   - A red filled rounded bar "PATTERN SEQUENCER" containing PATTERN SELECT, a white "CTRL 3" tag, LENGTH and QUANTIZE.
   - "CTRL 2 = BPM ADJ" printed below the bar.
   - TAP TEMPO to the right, with "EFFECT GRAB" printed above it.
5. Row: CANCEL ("PATTERN STOP" above), REMAIN ("CURRENT PAD" above), then a red-outlined
   "SAMPLE MODE" group: LONG/LO-FI, STEREO, GATE, LOOP, REVERSE.
   - The legend above LONG/LO-FI reads: ■ LONG = lit, ☼ LO-FI = blinking.
6. Row:
   - DEL
   - REC in a red outline, "SAMPLING" above and "PATTERN REC" below in red
   - RE-SAMPLE
   - MARK ("START POINT" above, "END POINT" below)
   - "BANK" A B | C D in a black frame, labelled "INTERNAL" under A/B and "CARD" under C/D
7. Pad block: a 4×2 grid (1–4 on top, 5–8 below) labelled "SAMPLE/PATTERN" underneath.
   - A fifth column to the right: HOLD (top) and EXT SOURCE (bottom).
   - EXT SOURCE has a **dark charcoal face with white text**; all other pads are light.

**Colours** — sampled as averaged 6×6 px patches from photos. Lighting varies, so treat these
as approximate. "Neutral" = Wikimedia Commons photo (daylight). "Warm" = Audiofanzine photo under indoor light.

| Element | Neutral photo | Warm photo | Suggested |
|---|---|---|---|
| Top plate (brushed silver/aluminium look) | #d0cdc4–#f5eee4 | #eed6bc | ~#d4d2cc |
| Black upper block | #242623–#262828 | #2f241f | ~#1e1f1f |
| Body / bezel (black plastic) | #3f4045 | #1d140f | ~#1a1a1c |
| Red sequencer bar / red print | #d14f59 / #d15861 | #aa252a / #9b2730 | ~#c8323c |
| Rubber buttons, unlit (grey) | #979690–#b0ac9b | #7a6852–#826d58 | ~#9a9890 |
| Pads 1–8 and HOLD, unlit (pale grey-beige) | #a29f8c–#c0bfac | #a68f71–#ad9472 | ~#b8b6a6 |
| EXT SOURCE pad | #2b2f2f | #695747 | ~#2a2c2d |
| Knob body | #57585d (skirt) | #130d05 | black ~#141414 |
| Knob cap | #f8eed9 | #d9b496 | off-white ~#efe9dc, black pointer line |
| LED display / lit button / lit pad | — | #e21b48, #f4295f, #f83462 (camera saturates toward magenta) | red LED ~#ff2a3a, glow ~#ff4060 |
| Display window, unlit | #2c2c28 | #1f140d | ~#1a1414 (dim ghost segments visible) |

- [PHOTO] Lit states: buttons and pads glow red through the translucent rubber. Seen on pad 7, MARK, LOOP, BANK D and FILTER+DRIVE in the Audiofanzine photo, and on pad 1 in the Roland product shot.
- Panel text: white on black and red areas, black on silver areas. The font is a bold condensed sans in caps.

**Photos used** (copies in the session scratchpad `sp303/`):
- https://commons.wikimedia.org/wiki/File:Boss_SP-303.jpg (neutral, unpowered)
- https://static.roland.com/assets/images/products/main/sp303_main.jpg (official, lit, 232×330)
- Audiofanzine user photos, powered: https://medias.audiofanzine.com/img/pictures/normal/6/6/6609562.jpeg and …/6/8/6823478.jpeg

## 11. Not verified / gaps

- Numeric ranges for effect knobs (e.g. cutoff Hz, delay feedback %, reverb time), apart from those listed in §8. **Not in the manual.**
- The internal algorithms of every effect, including Vinyl Sim and COMP. No measurements or teardown were found.
- The Time Modify algorithm.
- Internal bit depth for the LONG and LO-FI grades.
- The shapes of the SLICER timing patterns P01–P16 (only a figure on PDF p.27, not transcribed).
- The Roland "MIDI Implementation" document (byte-level) was not retrieved.
- Service notes exist at https://www.nquabayre.com/wp-content/uploads/2019/03/Boss_SP-303_Service-Manual.pdf. Not read.

## 12. Cross-reference: VU202 (vubeatz)

VU202 is an iOS / Android / VST sampler by Dariusz Chynek ("vubeatz"). It is a **modern
software reinterpretation** of the SP-202/303 workflow, not an emulation of the hardware and
not a source of hardware facts. Sources, fetched 2026-11-06:
- Manual: https://vu900.com/vu202-manual.html (written for the iPhone/iPad app; headings: Main Screen, Trim Mode, Pattern Mode, Song Mode, Context Menus And Shortcuts, Settings, Projects And Export)
- Product page: https://vu900.com/vu202.html (VST 1.3.0, Android APK 1.7.0.2)
- App Store listing and version history (1.1 to 1.8.1): https://apps.apple.com/us/app/vu202/id6760613784

Labels: **[VU202 manual §heading]**, **[VU202 page]** = product page, **[VU202 AS vN]** = App Store
release note for version N, **[VU303 AS]** = the App Store text of the successor app VU303
(a different product, shown on the same listing). The manual is a short plain-language
guide. **It gives almost no numbers.** Where a value is missing below, it is missing from all
three sources.

### 12.1 Sample quality modes (HiFi / Standard / LoFi / LoFi2)

- Four per-pad options: "Hifi / Standard / Lofi / Lofi2 — Change the sound quality for the active pad." [VU202 manual §Main Screen › Active Pad Options]
- The product page calls them "character" modes: "Move between HiFi, Standard, LoFi and LoFi2 … treat each pad as its own sound lane". [VU202 page §What it does]
- **No sample rate, bit depth or filter is stated for any of the four modes** in the manual, the product page or the App Store text. Unknown.
- "LOFI sound quality option rewritten." [VU202 AS v1.4, 2026-04-09] No detail given.
- The mode is a pad property that can be changed after loading, and is copied to slices on CHOP (§12.3). [VU202 manual §Trim Mode] This differs from the SP-303, where grade is fixed at sampling time. [DOC p.27]
- [VU303 AS] The successor app VU303 lists "the new VU303 engine (STANDARD 44.1 kHz, LONG 22.05 kHz, LO-FI 11.025 kHz) and the classic VU202 engine". These three rates are exactly the SP-303 grades in §2. They describe VU303, **not** VU202's four modes.
- Mono / Stereo per pad: "Mono sums left and right together." [VU202 manual §Active Pad Options] Left and Right channel options were added later. [VU202 AS v1.3]

### 12.2 Trigger / Gate, One Shot / Loop, Reverse, Hold

- "Trigger plays the whole sample. Gate plays only while the pad is held." [VU202 manual §Active Pad Options]
- "One Shot plays once. Loop repeats the sample in a loop." [same] The manual does not say what a second press does (the SP-303 rule is in §3: Trigger+Loop second press stops, Trigger+One-shot restarts).
- "Reverse can be used even while a sample is already playing." [same]
- HOLD: "Press HOLD, then press a pad with Gate mode to play it like Trigger." [VU202 manual §Active Pad Options] On the SP-303, HOLD latches a Gate pad until pressed again. [DOC p.19]
- Trim PLAY follows the pad's Trigger/Gate setting. [VU202 AS v1.1]
- Choke: set per bank in Settings; "every pad in that bank will mute the others". C and D are described as choke banks on the main screen. [VU202 manual §Settings › Banks; §Active Pad Options] The SP-303 has no choke setting; it has 8-voice polyphony with oldest-note stealing. [DOC p.13]
- Per-pad controls: Volume (pad, not master); Pitch −12 to +12 semitones, double-tap resets. [VU202 manual §Pad Volume And Effects]

### 12.3 Trim and chop into 8

- S / E set start / end. +/− zoom centres on the last-used point; default focus is start. TRUN truncates. AT/DC enables Attack and Decay points. [VU202 manual §Trim Mode] The App Store feature list also says "attack, decay, and release shape". [VU202 AS description]
- **CHOP**: "Slice the full sample into 8 equal parts and place them across the current bank." [VU202 manual §Trim Mode]
  - Slices are **equal length**. No transient detection and no BPM grid is mentioned.
  - "Chop ignores the trim start and end points, so truncate first if needed."
  - "Chop removes other samples already stored in that bank and writes the 8 slices to those pads."
  - Pad properties (quality, mono/stereo, effects) are copied to every slice.
  - Assignment order slice 1→pad 1 … slice 8→pad 8 is implied by "across the current bank" but not stated.
  - "non destructive CHOP for sample on 8 slices" [VU202 AS v1.2]. Later moved under OPT in Trim view with NORMALIZE (raise Start/End range to 0 dB), REMOVE (delete Start–End and join), SILENCE (replace range with digital silence), STRETCH and one-step UNDO. [VU202 AS v1.6.2]
- The SP-303 has no chop function (§4 covers only Mark/Truncate). Chop is VU202's own design.

### 12.4 Resample

- Pad context menu › Resample: "Save a new sample onto the same pad with all current effects applied, including bank and master effects." [VU202 manual §Pad Context Menu] So it writes back to the **source** pad. The SP-303 requires a different destination pad. [DOC p.32–33]
- Resample workflow mode, described as "Known resample workflow from boss and roland sp samplers": press REC, play pads, press an empty pad (bank may change), and "Everything what You played is recorded in separate sample." [VU202 AS v1.8]
- Record to empty pad: hold an empty pad more than 0.1 s to record from mic or interface. Copy pad: hold more than 2 s, then press the destination (multi-paste allowed; the menu copy cannot change bank). [VU202 manual §Context Menus And Shortcuts]

### 12.5 Effects: routing and list

- Routing has three stages: **pad, bank and master** effects. [VU202 manual §Pad Context Menu; VU202 page] "Stack pad effects, push groups harder with bank processing and keep shaping the whole instrument through the master path". [VU202 page §What it does] Each effect slot can be changed "from the pad/bank/master slot". [VU202 AS v1.5] Per-pad inserts are isolated from other pads. [VU202 AS v1.1]
- The manual's main-screen effects, each on one control knob [VU202 manual §Pad Volume And Effects]:
  - Reverb: pad reverb amount; "The center position is 0% wet"; double-tap resets.
  - Delay: enable for the pad, then set the amount with the knob.
  - Filter 1: low-pass. Filter 2: band-pass.
  - Ring Mod: "Classic ring modulation".
- Added later: "Filter HP, Vinyl Sim, Drum Compressor, Phaser, Flanger, Tape Delay". [VU202 AS v1.5]
- **No parameter list, range, time constant or algorithm is given for any VU202 effect**, including Vinyl Sim and Drum Compressor. No lo-fi/bit-crush effect is listed for VU202 itself; lo-fi character comes from the quality modes.
- This differs from the SP-303: one effect at a time for the whole unit, applied to selected pads. [DOC p.21–23] VU202 allows simultaneous per-pad, per-bank and master effects.

### 12.6 Patterns, swing, nudge, song

- Grid: 4 pads × 16 steps on screen; up/down arrows move between A1–A4, A5–A8 and the same rows in other banks. [VU202 manual §Pattern Mode]
- Length: 16, 32, 48 or 64 steps, with pages of 16. [same] 256-step patterns can be enabled in Settings. [VU202 AS v1.6] Double expands 16→32→64 "while keeping steps, velocity and nudges". [VU202 manual §Pattern Context Menu]
- Step resolution is not stated. [INFERRED] Paging in 16s and "16 / 32 / 48 / 64" suggests 16th-note steps (64 steps = 4 bars), but the manual never says so.
- Swing: **50, 55, 60, 65, 70, 75** (percent implied, not stated). [VU202 manual §Pattern Mode] The swing algorithm (which steps move, and by how much) is not described.
- Velocity: 3 levels, **100%, 70%, 30%**. [VU202 manual §Pattern Mode] Later free velocity and per-step pan by holding a step. [VU202 AS v1.6] Per-step KEY (sample pitch per step). [VU202 AS v1.6.3]
- Nudge: hold a step to show a nudge bar, drag left or right "to move the step off the grid". [VU202 manual §Pattern Mode] Range and resolution are not given. Nudges are kept by duplicate and double. [VU202 AS v1.2; manual §Pattern Context Menu]
- Live recording: REC "write[s] played pads into the current pattern". [VU202 manual §Transport] **The VU202 manual says nothing about quantize**, count-in or metronome. Erase while recording: REC+PLAY, ERASE button, hold pads. [VU202 AS v1.6]
- Erase pattern clears "all steps and settings such as pages and swing", so **swing is stored per pattern**. [VU202 manual §Pattern Context Menu]
- Duplicate copies to the first empty pattern slot. [same]
- Song mode: a 16-step chain of patterns from the pool, each step with a repeat count (R−/R+). Empty steps in the middle are ignored. [VU202 manual §Song Mode] At the time of the manual, "the transport switch for song playback is not implemented yet". [VU202 manual §Quick Operating Guide] The SP-303 has no song mode.

### 12.7 BPM, time-stretch, sync

- "Set BPM for all patterns." Double-tap the digits to edit. [VU202 manual §Pattern And Bank Area] Global tempo, as on the SP-303. [DOC p.43] No range is stated.
- PLAY/PAUSE resumes from the paused position; STOP returns to the start. [VU202 manual §Transport]
- STRETCH in Trim OPT: editable source and target BPM, with **CLASSIC** and **CLEAN** modes. [VU202 AS v1.6.2] "Time stretch got massive improvement." [VU202 AS v1.7] The modes are not described further.
- VST: "letting the DAW own transport and arrangement"; multi-out for main, banks and pads. [VU202 page §VU202 VST] No MIDI clock sync is documented for the mobile app.

### 12.8 MIDI

- **The VU202 manual, product page and App Store text contain no MIDI note mapping for pads** and no MIDI implementation. Unknown.
- [VU303 AS] The successor VU303 lists "Play the pads from any MIDI keyboard or pad controller", MIDI LEARN, channel filter and velocity curves. That is VU303, not VU202.

### 12.9 Comparison with the SP-303 facts in §1–§9

**Agree:**
- 32 pads as 4 banks × 8. [VU202 manual §Pattern And Bank Area; DOC p.18]
- Per-pad Gate vs Trigger, Loop vs one-shot, Reverse. [VU202 manual §Active Pad Options; DOC p.36–37]
- A REMAIN key tied to the current pad: VU202 uses it to change focus without sounding the pad; the SP-303 uses it to show the current pad and select effect targets. [VU202 manual; DOC p.8, p.22]
- Global BPM rather than per-pattern tempo. [VU202 manual; DOC p.43]
- Resample prints the effect into a new sample. [VU202 manual; VU202 AS v1.8; DOC p.32–33]
- Truncate exists in both. [VU202 manual §Trim Mode; DOC p.41]
- The VU303 rates (44.1 / 22.05 / 11.025 kHz) match SP-303 STANDARD / LONG / LO-FI. [VU303 AS; DOC p.27]
- Vinyl Sim and Ring Mod names appear in both effect sets. [VU202 AS v1.5; DOC p.24–26]

**VU202's own design (not SP-303 hardware behaviour):**
- Four quality modes (HiFi / Standard / LoFi / LoFi2) that can be changed after sampling. The SP-303 has three grades fixed at sampling.
- Banks C/D are choke banks. On the SP-303 they are the SmartMedia card banks, and there is no choke.
- HOLD makes a Gate pad act like Trigger, rather than latching it.
- Pitch ±12 semitones per pad. The SP-303 has a PITCH effect (±2 octaves), not a per-pad pitch.
- Attack/Decay envelope per pad. Not on the SP-303.
- CHOP into 8 equal slices. Not on the SP-303.
- Resample from the menu writes back to the same pad. The SP-303 forbids the source pad as destination.
- Pad + bank + master effects at the same time. The SP-303 has one effect at a time.
- Step-grid patterns (16–64 steps, 256 optional) with 3-level step velocity, per-step nudge, swing 50–75, per-step pan and key. The SP-303 records patterns in real time at 96 PPQN, with 1–99 bar length, quantize oFF/4/8/8-3/16, no swing setting listed in §6, and pads without velocity.
- Song mode with a 16-step chain and repeats. Not on the SP-303.
- Time-stretch to a target BPM (CLASSIC/CLEAN). The SP-303's Time Modify is limited to ½× to about 1.3×.
- Mute/solo per pad and bank, WAV and stem export.

**Not resolved by VU202:** none of the SP-303 gaps in §11 (effect ranges, Vinyl Sim/COMP internals, LONG/LO-FI bit depth, Time Modify algorithm, MIDI implementation) is answered by the VU202 sources.

---

## 13. How Pro Wave's Sampler models it

Code: `crates/prowave-engine/src/{sampler,sp_fx,sample_tools}.rs`,
`crates/prowave-app/src/{sampler,sampler_ui}.rs`.

**Follows the hardware**
- 8 pads a bank, banks A–D (A/B "INTERNAL", C/D "CARD" as printed); pad 1–8 keys of the computer keyboard play the bank's pads. [§1, §10]
- GATE (trigger vs gate), LOOP (a second hit stops a trigger loop), REVERSE per pad; HOLD latches gate pads, pressed again to stop. [§3]
- LONG/LO-FI cycles STANDARD → LONG (lit) → LO-FI (blinking): the pad's sound is resampled to 44.1 / 22.05 / 11.025 kHz with a windowed-sinc low-pass; STEREO off sums to mono. Unlike the hardware the grade can be changed after the fact (VU202 allows that too, §12). Bit depth per grade is unknown (§11), so none is imposed.
- One effect at a time, for the pads set to it (REMAIN + pad toggles a pad; REMAIN + effect key sets all pads). Five keys plus MFX 1–21, CTRL 1–3 as in §8. MFX is picked by scrolling/dragging on the MFX key (the hardware: hold MFX, turn CTRL 3) or by clicking the printed list. 24 of 26 effects are made; 17 SLICER (pattern figure missing) and 20 VOICE TRANS. are not and cannot be picked.
- DELAY TIME runs through the manual's 13 note values at the project tempo, halved past 1450 ms.
- START/END/LEVEL: CTRL 1/2 move Start/End relatively, up to ±130 ms per turn, kept 100 ms apart, the knob returning to centre on release; CTRL 3 is LEVEL 0–127. [§4]
- TIME/BPM: CTRL 1 Time Modify (left "oFF", ½× to 1.3×, right "Ptn" = follow the project tempo); CTRL 2 the pad's BPM 40–200 with the manual's step rules (left "---" clears). With PATTERN SELECT lit, CTRL 2 sets the tempo. [§4–6]
- A pad's BPM is worked out from its length as the hardware does (a power-of-two number of beats; may be half or double). [§4]
- MARK: press to play the pad's file, press at the Start, press at the End; with a BPM set the End lands on a whole beat. [§4]
- DEL, pad, DEL deletes; DEL + pad while a pattern records erases that pad's notes. CANCEL stops the pattern and backs out. [§3, §6]
- TAP TEMPO sets the tempo (or, under TIME/BPM, the pad's BPM) from the taps.
- Patterns: 32 (banks A–D × 8), LENGTH 1–20 bars then steps of 4 to 99, QUANTIZE oFF/4/8/8-3/16, recording loops and overdubs. A pattern is a MIDI region named "SP Pattern A1"… on the sampler's track, so it is in the song; PATTERN SELECT + pad plays it in a cycle on the project transport. No count-in; hits land at the UI's view of the song position (about a frame late before quantize).
- RE-SAMPLE prints the current pad through the effect (and its pitch) onto the next empty pad, writing a WAV with the session's recordings. [§7]
- The panel: layout, colours and print after the photos and p. 7 (§10); lit keys and pads are red with white print, unlit ones pale with black print, the black block's print white.

**Not like the hardware (extensions or limits)**
- 32 voices with oldest-first stealing (the SP-303 has 8). Velocity sensitivity per pad (the SP-303's pads have none; MIDI velocity does change its level, §1).
- Repitch per pad (semitones and cents) with a windowed-sinc interpolator whose cut-off follows the speed, or Stretch (Signalsmith Stretch, rendered off the audio thread). Time Modify, Fit to tempo (N bars) and Follow use Stretch.
- ADSR, low-pass with resonance, pan and a choke group per pad.
- Chop (Manual / Transients with sensitivity / Equal 2–64 / Beats) and Slices to Pads, banks E–H for big chops, Chromatic mode, Slices to MIDI Pattern, Slice to Audio Track, the file browser with tempo and key, preview in sync. Labelled as not on the SP-303 where they appear.
- EXT SOURCE is drawn but not available: an instrument slot gets no audio input. EFFECT GRAB (hold TAP TEMPO + effect key) is not made. Sampling from a live input is replaced by REC putting the editor's selection (or the browser's file) on the current pad.
