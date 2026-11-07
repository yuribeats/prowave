# Genre and artist templates: public edition

Genre and artist templates made with Pro Wave's own instruments and effects and free plug-ins only. The drum racks play one-shots synthesized for these templates by `tools/templates/drum_samples.py` (CC0, no recordings; `crates/prowave-app/templates/samples/`, referenced as `$TEMPLATE_SAMPLES/<file>`) with a starter beat in each drum pattern. The Sampler and audio tracks start empty.

- Native effects are stored by their stable id (`prowave:lemon-limiter`, `prowave:lemon-soft-clipper`, `prowave:tune`); the app shows the engine's current name for an id, whatever name the file holds.
- Audio Unit settings are factory patches, factory presets or named parameter values that the plug-ins themselves returned; file paths the plug-ins keep in their settings are removed.
- Built in, no download: Channel Rack, Sampler, Channel EQ, Compressor, Space D (convolution reverb with built-in responses), Tape Delay, Overdrive, Filter, Pro Wave Tune (pitch correction and pitch shift), Lemon Limiter and Lemon Soft Clipper.
- A template whose free plug-ins are not installed still opens: the slots keep their names and the status line reports each as not installed.

## Free plug-ins

| Plug-in | License | Download |
|---|---|---|
| Surge XT | GPL-3.0 | https://surge-synthesizer.github.io/ |
| OB-Xf | GPL-3.0 | https://github.com/surge-synthesizer/OB-Xf |
| CHOWTapeModel | GPL-3.0 | https://chowdsp.com/products.html |
| TAL-Chorus-LX | freeware | https://tal-software.com/products/tal-chorus-lx |
| TAL-Vocoder | freeware | https://tal-software.com/products/tal-vocoder |
| TDR VOS SlickEQ | freeware (free edition, not GE) | https://www.tokyodawn.net/tdr-vos-slickeq/ |
| TDR Kotelnikov | freeware (free edition, not GE) | https://www.tokyodawn.net/tdr-kotelnikov/ |
| TDR Nova | freeware (free edition, not GE) | https://www.tokyodawn.net/tokyo-dawn-labs/tdr-nova/ |
| ValhallaSupermassive | freeware | https://valhalladsp.com/valhallasupermassive/ |
| kHs Bitcrush | freeware (Kilohearts Essentials, free account) | https://kilohearts.com/products/kilohearts_essentials |
| JC303 | GPL-3.0 | https://github.com/midilab/jc303 |
| PaulXStretch | GPL-3.0 | https://sonosaurus.com/paulxstretch/ |
| Grain Strain | freeware | https://fullbucket.de/music/grainstrain.html |

## Genres

| Template | BPM | Sound | Free plug-ins needed |
|---|---|---|---|
| New Jack Swing | 106 | Swung 808/909 drum rack at 62%, synth bass, brass stabs, bright gated snare. | Surge XT, OB-Xf, TDR VOS SlickEQ |
| 90s Boom Bap | 90 | Swung break-style drum rack into 12-bit SP-1200-rate grit, filtered bass, dusty loops. | Surge XT, OB-Xf, kHs Bitcrush, CHOWTapeModel |
| Dilla / Neo-Soul | 91 | Swung drum rack played loose, Rhodes and Moog-style bass, warm tape. | OB-Xf, CHOWTapeModel, Surge XT |
| SP-303 Beat Tape | 86 | Sampler (Vinyl Sim off; press it for the dust), unquantized pads, swung drum rack, cassette master. | Surge XT, CHOWTapeModel |
| G-Funk | 94 | Moog-style portamento lead, talkbox (vocoder stand-in), 808 groove, P-Funk keys. | Surge XT, OB-Xf, TAL-Vocoder |
| Miami Bass / Electro | 128 | TR-808 drum rack, long sub boom, vocoder, 128 BPM electro chassis. | Surge XT, OB-Xf, TAL-Vocoder |
| Memphis / Phonk | 132 | 808 cowbell, sub boom, slowed vocal chops, cassette hiss. | OB-Xf, Surge XT, CHOWTapeModel |
| Trap | 140 | 808 sub with glide, rolling hats, half-time at 140, loud clipped master. | Surge XT, OB-Xf |
| Drill (UK / NY) | 142 | Sliding 808s, triplet hats and offbeat snares at 140, dark strings and piano. | Surge XT, OB-Xf |
| Detroit Techno | 128 | 909/808 drum rack, string-machine pads, Juno-style chords, sequenced bass. | Surge XT, TAL-Chorus-LX, OB-Xf, TDR Kotelnikov |
| Chicago House / Acid | 123 | 909 four-on-the-floor, TB-303 acid line, organ stabs, piano chords. | JC303, Surge XT, OB-Xf |
| 90s R&B Ballad | 68 | DX-style EP, Rhodes, soft LinnDrum-era kit, stacked vocals, big plate. | Surge XT, TAL-Chorus-LX, OB-Xf, TDR VOS SlickEQ |
| Dancehall / Dembow | 96 | Digital riddim drum rack at 96 BPM, square-wave bass, organ skank, dub delay. | Surge XT, OB-Xf |
| 80s Boogie / Synth-Funk | 112 | LinnDrum-style rack, Minimoog-style bass, Oberheim brass, talkbox via vocoder. | Surge XT, OB-Xf, TAL-Vocoder |

## Artist sounds

| Template | BPM | Sound | Free plug-ins needed |
|---|---|---|---|
| J Dilla (Donuts era) | 85 | Swung drum rack played loose, SP-303 chops, MPC-style swing. | Surge XT, OB-Xf, CHOWTapeModel |
| Madlib (SP-303 beat tape) | 92 | SP-303 chops unquantized, loose swung drum rack, loose loops, tape hiss. | OB-Xf, CHOWTapeModel |
| MF DOOM (SP-303) | 84 | SP-303 pads (Vinyl Sim off by default), cartoon/TV samples, dusty loops, mono-ish mix. | CHOWTapeModel |
| Timbaland (late 90s / 2000s) | 100 | Off-kilter percussion, vocal-noise hits, stutter echoes, sparse bass, wide synths. | OB-Xf, Surge XT |
| The Neptunes | 98 | Dry, bright Triton-era synths, sparse 808 and claps, clavinet, short delays. | Surge XT, OB-Xf, TDR VOS SlickEQ |
| Metro Boomin | 140 | Dark minor bells and choirs, distorted 808 with glide, hat rolls, big reverb. | Surge XT, OB-Xf, ValhallaSupermassive |
| Kanye West (2004 chipmunk soul) | 92 | Sped-up soul chops (pitch up in the Sampler), hard boom-bap drums, strings, warm compression. | OB-Xf, CHOWTapeModel |
| Burial (UK garage shuffle) | 136 | 2-step shuffle at 136, vinyl crackle, pitched vocal shards, sub, reverb fog. | Surge XT, OB-Xf, CHOWTapeModel, ValhallaSupermassive |
| Boards of Canada | 92 | Detuned, wobbling tape synths, dusty breaks, VHS haze, slow tempos. | OB-Xf, CHOWTapeModel, Surge XT, kHs Bitcrush |
| DJ Screw (chopped & screwed) | 70 | Everything slowed (pitch down), chopped repeats, heavy 808, syrupy reverb. | Surge XT, CHOWTapeModel |
| Kaytranada | 112 | Bouncy swung house/hip-hop hybrid, filtered disco chops, warm sub, pumping bus. | Surge XT, OB-Xf |
| Daft Punk (filter house) | 123 | 909 kit, looped disco sample through a swept low-pass, compressor pump, vocoder. | Surge XT, OB-Xf, TAL-Vocoder |
| Four Tet | 124 | Chopped folk/jazz samples, swung programmed drums, layered percussion, warm and simple. | Surge XT |
| Oneohtrix Point Never | 80 | Juno-style arps, Eccojams slow-and-loop echoes, granular stretch, choirs, huge spaces. | OB-Xf, TAL-Chorus-LX, PaulXStretch, Surge XT, Grain Strain, CHOWTapeModel, ValhallaSupermassive |
| Enya | 72 | Dozens of stacked vocal takes, D-50/Juno pads, no drums, a very long hall. | Surge XT, TAL-Chorus-LX, OB-Xf, TDR Nova, CHOWTapeModel |
| Drake / OVO (Noah "40" Shebib) | 74 | Low-passed "underwater" mix, sparse 808s and long subs, filtered samples, space. | Surge XT, kHs Bitcrush, OB-Xf, TDR Nova |
