#!/bin/bash
# Pro Wave (Yuri's Version): free plug-in installer.
#
# Downloads each plug-in from its maker's own server and installs it.
# Nothing here is re-hosted. Read it before you run it.
#
#   sh install-free-plugins.sh                 install everything it can fetch (asks first)
#   sh install-free-plugins.sh --kind synth    only synths   (--kind effect: only effects)
#   sh install-free-plugins.sh --only id,id    only these ids
#   sh install-free-plugins.sh --list          print the catalog and stop
#   sh install-free-plugins.sh --dry-run       download and unpack, install nothing
#   --gui                                      no terminal: one macOS password dialog
#   --yes                                      do not ask before starting
#
# Installers (.pkg) need the Mac password once. Loose plug-ins are copied to
# ~/Library/Audio/Plug-Ins (Components and VST3), no password.

DEFAULT_KIND="all"
UA="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15"
TAB="$(printf '\t')"

# id, kind, name, maker, mode, tag, page, description, source, pattern, pick
# mode auto: fetch `source`; with a pattern, `source` is a page and the file is the
# first (or last, see pick) link on it that matches. mode manual: open `page`.
catalog() {
cat <<'PROWAVE_CATALOG_END'
digital-suburban-dexed	synth	Dexed	Digital Suburban	auto		https://github.com/asb2m10/dexed/releases	Yamaha DX7 FM synth. Open source.	https://api.github.com/repos/asb2m10/dexed/releases	https://github\.com/[^"]+/releases/download/[^"]+/Dexed-[0-9.]+-macOS\.dmg"	first
distrho-cardinal	synth	Cardinal	DISTRHO	auto		https://github.com/DISTRHO/Cardinal/releases	Virtual modular synthesizer. Open source.	https://api.github.com/repos/DISTRHO/Cardinal/releases	https://github\.com/[^"]+/releases/download/[^"]+/Cardinal-macOS-universal-[0-9.]+\.pkg"	first
full-bucket-music-blooo	synth	Blooo	Full Bucket Music	auto		https://www.fullbucket.de/music/blooo.html	Classic subtractive synth, up to 64 voices.	https://www.fullbucket.de/music/blooo.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-brokenmini	synth	BrokenMini	Full Bucket Music	auto		https://www.fullbucket.de/music/brokenmini.html	Simulates an unknown, obscure synth from the 1970s. Broken on purpose.	https://www.fullbucket.de/music/brokenmini.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-bucket-one	synth	Bucket ONE	Full Bucket Music	auto		https://www.fullbucket.de/music/bucketone.html	Simulates the Crumar BIT 01/99 (1985).	https://www.fullbucket.de/music/bucketone.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-bucketpops	synth	BucketPops	Full Bucket Music	auto		https://www.fullbucket.de/music/bucketpops.html	Simulates the Korg Mini Pops-7 rhythm machine (1966).	https://www.fullbucket.de/music/bucketpops.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-deputy	synth	Deputy	Full Bucket Music	auto		https://www.fullbucket.de/music/deputy.html	String machine and early polyphonic synth.	https://www.fullbucket.de/music/deputy.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-drumtraqs	synth	DrumTraqs	Full Bucket Music	auto		https://www.fullbucket.de/music/drumtraqs.html	Simulates the Sequential Circuits Drumtraks drum computer (1983).	https://www.fullbucket.de/music/drumtraqs.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-fb-3100	synth	FB-3100	Full Bucket Music	auto		https://www.fullbucket.de/music/fb3100.html	Simulates the Korg PS-3100 (1977).	https://www.fullbucket.de/music/fb3100.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-fb-3200	synth	FB-3200	Full Bucket Music	auto		https://www.fullbucket.de/music/fb3200.html	Simulates the Korg PS-3200 (1978).	https://www.fullbucket.de/music/fb3200.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-fb-3300	synth	FB-3300	Full Bucket Music	auto		https://www.fullbucket.de/music/fb3300.html	Simulates the Korg PS-3300 (1977).	https://www.fullbucket.de/music/fb3300.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-fb-7999	synth	FB-7999	Full Bucket Music	auto		https://www.fullbucket.de/music/fb7999.html	Simulates the Korg DW-6000 and DW-8000.	https://www.fullbucket.de/music/fb7999.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-fb3	synth	FB3	Full Bucket Music	auto		https://www.fullbucket.de/music/fb3.html	Simulates the Kawai K3 (1986).	https://www.fullbucket.de/music/fb3.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-fury68	synth	Fury68	Full Bucket Music	auto		https://www.fullbucket.de/music/fury68.html	Simulates the Korg Poly-61 (1982).	https://www.fullbucket.de/music/fury68.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-fury800	synth	Fury800	Full Bucket Music	auto		https://www.fullbucket.de/music/fury800.html	Simulates the Korg Poly-800 (1983).	https://www.fullbucket.de/music/fury800.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-kern	synth	Kern	Full Bucket Music	auto		https://www.fullbucket.de/music/kern.html	Polyphonic synth built to be run from a MIDI keyboard controller.	https://www.fullbucket.de/music/kern.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-matriq-six	synth	Matriq-Six	Full Bucket Music	auto		https://www.fullbucket.de/music/matriqsix.html	Simulates the Oberheim Matrix 6R (1985). Prototype version.	https://www.fullbucket.de/music/matriqsix.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-modulair	synth	ModulAir	Full Bucket Music	auto		https://www.fullbucket.de/music/modulair.html	Modular polyphonic synth.	https://www.fullbucket.de/music/modulair.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-monofury	synth	MonoFury	Full Bucket Music	auto		https://www.fullbucket.de/music/monofury.html	Simulates the Korg Mono/Poly.	https://www.fullbucket.de/music/monofury.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-mps	synth	MPS	Full Bucket Music	auto		https://www.fullbucket.de/music/mps.html	Polyphonic synth with a small set of parameters.	https://www.fullbucket.de/music/mps.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-nabla	synth	Nabla	Full Bucket Music	auto		https://www.fullbucket.de/music/nabla.html	Simulates the Korg Delta DL-50 strings synth (1979).	https://www.fullbucket.de/music/nabla.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-ny	synth	NY	Full Bucket Music	auto		https://www.fullbucket.de/music/ny.html	Simulates the Korg Lambda ES-50 polyphonic ensemble (1979).	https://www.fullbucket.de/music/ny.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-oxid	synth	Oxid	Full Bucket Music	auto		https://www.fullbucket.de/music/oxid.html	Simulates the ARP Omni-2 (1970s).	https://www.fullbucket.de/music/oxid.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-paralogy	synth	Paralogy	Full Bucket Music	auto		https://www.fullbucket.de/music/paralogy.html	Simulates the Crumar Trilogy and Stratus.	https://www.fullbucket.de/music/paralogy.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-pecs	synth	PECS	Full Bucket Music	auto		https://www.fullbucket.de/music/pecs.html	Based on the Korg PE-2000 string machine (1976).	https://www.fullbucket.de/music/pecs.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-qyooo	synth	Qyooo	Full Bucket Music	auto		https://www.fullbucket.de/music/qyooo.html	Three filters with scene morphing, three oscillators.	https://www.fullbucket.de/music/qyooo.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-ragnarok2	synth	Ragnarok2	Full Bucket Music	auto		https://www.fullbucket.de/music/ragnarok.html	Simulates a one-of-a-kind custom analog synth from 1980.	https://www.fullbucket.de/music/ragnarok.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-scrooo	synth	Scrooo	Full Bucket Music	auto		https://www.fullbucket.de/music/scrooo.html	Spectral formant synth.	https://www.fullbucket.de/music/scrooo.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-sixtraq	synth	SixTraq	Full Bucket Music	auto		https://www.fullbucket.de/music/sixtraq.html	Simulates the Sequential Circuits Six-Trak (1984).	https://www.fullbucket.de/music/sixtraq.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-stigma	synth	Stigma	Full Bucket Music	auto		https://www.fullbucket.de/music/stigma.html	Simulates the Korg Sigma KP-30 (1979).	https://www.fullbucket.de/music/stigma.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-tricent	synth	Tricent	Full Bucket Music	auto		https://www.fullbucket.de/music/tricent.html	Simulates the Korg Trident mkII (1982).	https://www.fullbucket.de/music/tricent.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-whispair	synth	WhispAir	Full Bucket Music	auto		https://www.fullbucket.de/music/whispair.html	Three wavetable oscillators, all parameters on one page.	https://www.fullbucket.de/music/whispair.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
krakli-arminator2	synth	Arminator2	Krakli	auto		https://www.krakli.com/	The original Arminator.	https://www.krakli.com/	https://www\.krakli\.com/_files/archives/[a-z0-9_]+\.zip\?dn=Arminator2026_OldStyle\.zip	first
krakli-arminator26	synth	Arminator26	Krakli	auto		https://www.krakli.com/	Revised Arminator: new interface, new features, Random.	https://www.krakli.com/	https://www\.krakli\.com/_files/archives/[a-z0-9_]+\.zip\?dn=Arminator26_NewStyle\.zip	first
krakli-k1ks	synth	K1Ks	Krakli	auto		https://www.krakli.com/	3 oscillators, 2 filters, multiple envelopes and LFOs.	https://www.krakli.com/	https://www\.krakli\.com/_files/archives/[a-z0-9_]+\.zip\?dn=K1Ks\.zip	first
krakli-kxtra	synth	KXtra	Krakli	auto		https://www.krakli.com/	Hybrid FM synth with configurable operators and a randomizer.	https://www.krakli.com/	https://www\.krakli\.com/_files/archives/[a-z0-9_]+\.zip\?dn=KXtra_64bit\.zip	first
krakli-rmpx	synth	RMPx	Krakli	auto		https://www.krakli.com/	Physical modelling, FM and supersaw oscillator sections.	https://www.krakli.com/	https://www\.krakli\.com/_files/archives/[a-z0-9_]+\.zip\?dn=RMPX_64bit\.zip	first
krakli-sk7	synth	SK7	Krakli	auto		https://www.krakli.com/	Styled after a 1970s synth.	https://www.krakli.com/	https://www\.krakli\.com/_files/archives/[a-z0-9_]+\.zip\?dn=SK7_64bit\.zip	first
krakli-xakt	synth	Xakt	Krakli	auto		https://www.krakli.com/	Combines two additive waveforms. Randomize.	https://www.krakli.com/	https://www\.krakli\.com/_files/archives/[a-z0-9_]+\.zip\?dn=XAKT\.zip	first
midilab-jc303	synth	JC303	midilab	auto		https://github.com/midilab/jc303/releases	Roland TB-303 bass line clone. Open source.	https://api.github.com/repos/midilab/jc303/releases	https://github\.com/[^"]+/releases/download/[^"]+/jc303-[0-9.]+-macos[_-]universal-plugins\.zip"	first
socalabs-organ	synth	Organ	SocaLabs	auto		https://socalabs.com/synths/organ/	Tonewheel organ with drawbars, vibrato and chorus.	https://socalabs.com/files/get.php?id=Organ.pkg		first
socalabs-papu	synth	PAPU	SocaLabs	auto		https://socalabs.com/synths/papu/	Nintendo Game Boy sound chip.	https://socalabs.com/files/get.php?id=PAPU.pkg		first
socalabs-piano	synth	Piano	SocaLabs	auto		https://socalabs.com/synths/piano/	Simple sampled piano.	https://socalabs.com/files/get.php?id=Piano.pkg		first
socalabs-rp2a03	synth	RP2A03	SocaLabs	auto		https://socalabs.com/synths/rp2a03/	Nintendo Entertainment System sound chip.	https://socalabs.com/files/get.php?id=RP2A03.pkg		first
socalabs-sid	synth	SID	SocaLabs	auto		https://socalabs.com/synths/commodore-64-sid/	Commodore 64 sound chip, 8 voices.	https://socalabs.com/files/get.php?id=SID.pkg		first
socalabs-sn76489	synth	SN76489	SocaLabs	auto		https://socalabs.com/synths/sn76489/	Sega Master System sound chip.	https://socalabs.com/files/get.php?id=SN76489.pkg		first
socalabs-voc	synth	Voc	SocaLabs	auto		https://socalabs.com/synths/voc-vocal-synth/	Vocal synth: vowel sounds and vocal-like tones.	https://socalabs.com/files/get.php?id=Voc.pkg		first
socalabs-wavetable	synth	Wavetable	SocaLabs	auto		https://socalabs.com/synths/wavetable/	Wavetable synth: 2 wavetable oscillators, 150 wavetables, 5 effects.	https://socalabs.com/files/get.php?id=Wavetable.pkg		first
surge-synth-team-monique	synth	Monique	Surge Synth Team	auto		https://github.com/surge-synthesizer/monique-monosynth/releases	Monophonic synth for bass and lead. Open source.	https://api.github.com/repos/surge-synthesizer/monique-monosynth/releases	https://github\.com/[^"]+/releases/download/[^"]+/moniquemonosynth-macOS-[^"]+\.dmg"	first
surge-synth-team-ob-xf	synth	OB-Xf	Surge Synth Team	auto		https://github.com/surge-synthesizer/OB-Xf/releases	Inspired by the Oberheim OB-X. Continues OB-Xd. Open source.	https://api.github.com/repos/surge-synthesizer/OB-Xf/releases	https://github\.com/[^"]+/releases/download/[^"]+/ob-xf-macOS-v[0-9.]+\.dmg"	first
surge-synth-team-surge-xt	synth	Surge XT	Surge Synth Team	auto		https://surge-synthesizer.github.io/downloads/	Hybrid synthesizer. Open source.	https://api.github.com/repos/surge-synthesizer/releases-xt/releases	https://github\.com/[^"]+/releases/download/[^"]+/surge-xt-macOS-[0-9.]+\.dmg"	first
tal-software-tal-noisemaker	synth	TAL-NoiseMaker	TAL Software	auto		https://tal-software.com/products/tal-noisemaker	Virtual analog synth.	https://tal-software.com/products/tal-noisemaker	[^"]*/downloads/plugins/TAL-NoiseMaker-installer_macos\.zip	first
thewavewarden-odin-2	synth	Odin 2	TheWaveWarden	auto		https://thewavewarden.com/odin2	24-voice polyphonic synth. Open source.	https://api.github.com/repos/TheWaveWarden/odin2/releases	https://github\.com/[^"]+/releases/download/[^"]+/Odin2[0-9.]+MacInstaller\.pkg"	first
tilr-ripplerx	synth	RipplerX	tilr	auto		https://github.com/tiagolr/ripplerx/releases	Physically modeled synth. Open source.	https://api.github.com/repos/tiagolr/ripplerx/releases	https://github\.com/[^"]+/releases/download/[^"]+/ripplerx-macos-v[0-9.]+\.zip"	first
u-he-podolski	synth	Podolski	u-he	auto		https://u-he.com/products/podolski/	Straightforward, CPU-efficient virtual analog synth.	https://u-he.com/products/podolski/	https://dl\.u-he\.com/releases/Podolski_[^"]*_Mac\.zip	first
u-he-triple-cheese	synth	Triple Cheese	u-he	auto		https://u-he.com/products/triplecheese/	Comb filter synth.	https://u-he.com/products/triplecheese/	https://dl\.u-he\.com/releases/TripleCheese_[^"]*_Mac\.zip	first
u-he-tyrell-n6	synth	Tyrell N6	u-he	auto		https://u-he.com/products/tyrelln6/	Virtual analog synth.	https://u-he.com/products/tyrelln6/	https://dl\.u-he\.com/releases/TyrellN6_[^"]*_Mac\.zip	first
u-he-zebralette-3	synth	Zebralette 3	u-he	manual		https://u-he.com/products/zebralette3/	One-oscillator synth from u-he's Zebra line.			
adb-records-adb-smart-eq	effect	ADB Smart EQ	ADB Records	auto		https://adbrecords.com/adbsmarteq/	Adaptive EQ: four engines correct resonances and tonal balance, plus a 12-band manual EQ.	https://api.github.com/repos/adbrecords/ADB-Smart-EQ/releases	https://github\.com/[^"]+/releases/download/[^"]+/ADB_Smart_EQ_[0-9.]+_macOS\.zip"	first
airwindows-airwindows-consolidated	effect	Airwindows Consolidated	Airwindows	auto		https://github.com/baconpaul/airwin2rack/releases	The Airwindows effects in one plug-in. Open source.	https://api.github.com/repos/baconpaul/airwin2rack/releases	https://github\.com/[^"]+/releases/download/[^"]+/airwindows-consolidated-macOS-[^"]+\.dmg"	first
auburn-sounds-couture	effect	Couture	Auburn Sounds	auto	Free edition	https://www.auburnsounds.com/products/Couture.html	Dynamics: transient shaping.	https://www.auburnsounds.com/products/Couture.html	[^"]*downloads/Couture-FREE-[0-9.]+\.zip	first
auburn-sounds-graillon-3	effect	Graillon 3	Auburn Sounds	auto	Free edition	https://www.auburnsounds.com/products/Graillon.html	Live voice changer and pitch correction.	https://www.auburnsounds.com/products/Graillon.html	[^"]*downloads/Graillon-FREE-[0-9.]+\.zip	first
auburn-sounds-inner-pitch-2	effect	Inner Pitch 2	Auburn Sounds	auto	Free edition	https://www.auburnsounds.com/products/InnerPitch.html	Pitch changer.	https://www.auburnsounds.com/products/InnerPitch.html	[^"]*downloads/Inner-Pitch-FREE-[0-9.]+\.zip	first
auburn-sounds-lens	effect	Lens	Auburn Sounds	auto	Free edition	https://www.auburnsounds.com/products/Lens.html	Multiband compander.	https://www.auburnsounds.com/products/Lens.html	[^"]*downloads/Lens-FREE-[0-9.]+\.zip	first
auburn-sounds-panagement-2	effect	Panagement 2	Auburn Sounds	auto	Free edition	https://www.auburnsounds.com/products/Panagement.html	Binaural panning, distance and reverb.	https://www.auburnsounds.com/products/Panagement.html	[^"]*downloads/Panagement-FREE-[0-9.]+\.zip	first
auburn-sounds-renegate	effect	Renegate	Auburn Sounds	auto	Free edition	https://www.auburnsounds.com/products/Renegate.html	Gate.	https://www.auburnsounds.com/products/Renegate.html	[^"]*downloads/Renegate-FREE-[0-9.]+\.zip	first
blue-cat-audio-chorus	effect	Chorus	Blue Cat Audio	auto		https://www.bluecataudio.com/Products/Product_Chorus/	Chorus, from light ensemble to deep modulation.	https://www.bluecataudio.com/Products/Product_Chorus/	[^"]*BlueCatChorus\.dmg	first
blue-cat-audio-flanger	effect	Flanger	Blue Cat Audio	auto		https://www.bluecataudio.com/Products/Product_Flanger/	Flanger.	https://www.bluecataudio.com/Products/Product_Flanger/	[^"]*BlueCatFlanger\.dmg	first
blue-cat-audio-free-amp	effect	Free Amp	Blue Cat Audio	auto		https://www.bluecataudio.com/Products/Product_FreeAmp/	Guitar and bass amp simulation, four amp models.	https://www.bluecataudio.com/Products/Product_FreeAmp/	[^"]*BlueCatFreeAmp\.dmg	first
blue-cat-audio-freqanalyst	effect	FreqAnalyst	Blue Cat Audio	auto		https://www.bluecataudio.com/Products/Product_FreqAnalyst/	Spectrum analyzer.	https://www.bluecataudio.com/Products/Product_FreqAnalyst/	[^"]*BlueCatFreqAnalyst\.dmg	first
blue-cat-audio-gain	effect	Gain	Blue Cat Audio	auto		https://www.bluecataudio.com/Products/Product_GainSuite/	Gain utilities that can link several tracks.	https://www.bluecataudio.com/Products/Product_GainSuite/	[^"]*BlueCatGainSuite\.dmg	first
blue-cat-audio-phaser	effect	Phaser	Blue Cat Audio	auto		https://www.bluecataudio.com/Products/Product_Phaser/	Modeled after vintage analog phaser circuits.	https://www.bluecataudio.com/Products/Product_Phaser/	[^"]*BlueCatPhaser\.dmg	first
blue-cat-audio-triple-eq	effect	Triple EQ	Blue Cat Audio	auto		https://www.bluecataudio.com/Products/Product_TripleEQ/	3-band semi-parametric equalizer.	https://www.bluecataudio.com/Products/Product_TripleEQ/	[^"]*BlueCatTripleEQ\.dmg	first
chowdsp-byod	effect	BYOD	chowdsp	auto		https://chowdsp.com/products.html	Build-your-own guitar distortion from modules. Open source.	https://chowdsp.com/products.html	https://github\.com/[^"/]+/[^"/]+/releases/download/[^"]+/BYOD-Mac[^"]*\.dmg	first
chowdsp-chow-tape-model	effect	CHOW Tape Model	chowdsp	auto		https://chowdsp.com/products.html	Analog tape machine model. Open source.	https://chowdsp.com/products.html	https://github\.com/[^"/]+/[^"/]+/releases/download/[^"]+/ChowTapeModel-Mac[^"]*\.dmg	first
chowdsp-chowcentaur	effect	ChowCentaur	chowdsp	auto		https://chowdsp.com/products.html	Klon Centaur overdrive pedal model. Open source.	https://chowdsp.com/products.html	https://github\.com/[^"/]+/[^"/]+/releases/download/[^"]+/ChowCentaur-Mac[^"]*\.dmg	first
chowdsp-chowmatrix	effect	ChowMatrix	chowdsp	auto		https://chowdsp.com/products.html	Delay built from a tree of delay lines. Open source.	https://chowdsp.com/products.html	https://github\.com/[^"/]+/[^"/]+/releases/download/[^"]+/ChowMatrix-Mac[^"]*\.dmg	first
chowdsp-chowmultitool	effect	ChowMultiTool	chowdsp	auto		https://chowdsp.com/products.html	EQ, waveshaper, band splitter and other tools in one. Open source.	https://chowdsp.com/products.html	https://github\.com/[^"/]+/[^"/]+/releases/download/[^"]+/ChowMultiTool-Mac[^"]*\.dmg	first
chowdsp-chowphaser	effect	ChowPhaser	chowdsp	auto		https://chowdsp.com/products.html	Phaser. Open source.	https://chowdsp.com/products.html	https://github\.com/[^"/]+/[^"/]+/releases/download/[^"]+/ChowPhaser-Mac[^"]*\.dmg	first
full-bucket-music-brigade-delay	effect	Brigade Delay	Full Bucket Music	auto		https://www.fullbucket.de/music/fbdelay.html	Simulates classic analog delays.	https://www.fullbucket.de/music/fbdelay.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-fbvc	effect	FBVC	Full Bucket Music	auto		https://www.fullbucket.de/music/fbvc.html	Simulates the Korg VC-10 vocoder (1978).	https://www.fullbucket.de/music/fbvc.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-frequency-shifter	effect	Frequency Shifter	Full Bucket Music	auto		https://www.fullbucket.de/music/freqshifter.html	Frequency shifter.	https://www.fullbucket.de/music/freqshifter.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-grain-strain	effect	Grain Strain	Full Bucket Music	auto		https://www.fullbucket.de/music/grainstrain.html	Granular effect.	https://www.fullbucket.de/music/grainstrain.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
full-bucket-music-phaser	effect	Phaser	Full Bucket Music	auto		https://www.fullbucket.de/music/fbphaser.html	Simulates classic analog phasers.	https://www.fullbucket.de/music/fbphaser.html	dl\.php\?file=[a-z0-9]+(_[0-9]+)+_mac"	first
kilohearts-3-band-eq	effect	3-Band EQ	Kilohearts	manual	Needs account	https://kilohearts.com/products/3band_eq	3-band EQ.			
kilohearts-bitcrush	effect	Bitcrush	Kilohearts	manual	Needs account	https://kilohearts.com/products/bitcrush	Low sample rate and bit depth.			
kilohearts-channel-mixer	effect	Channel Mixer	Kilohearts	manual	Needs account	https://kilohearts.com/products/channel_mixer	Stereo and phase utility.			
kilohearts-chorus	effect	Chorus	Kilohearts	manual	Needs account	https://kilohearts.com/products/chorus	Chorus.			
kilohearts-clipper	effect	Clipper	Kilohearts	manual	Needs account	https://kilohearts.com/products/clipper	Peak trimming.			
kilohearts-comb-filter	effect	Comb Filter	Kilohearts	manual	Needs account	https://kilohearts.com/products/comb_filter	Cuts frequencies at intervals.			
kilohearts-compactor	effect	Compactor	Kilohearts	manual	Needs account	https://kilohearts.com/products/compactor	Sidechain ducking.			
kilohearts-compressor	effect	Compressor	Kilohearts	manual	Needs account	https://kilohearts.com/products/compressor	Compressor.			
kilohearts-delay	effect	Delay	Kilohearts	manual	Needs account	https://kilohearts.com/products/delay	Delay.			
kilohearts-distortion	effect	Distortion	Kilohearts	manual	Needs account	https://kilohearts.com/products/distortion	Distortion.			
kilohearts-dual-delay	effect	Dual Delay	Kilohearts	manual	Needs account	https://kilohearts.com/products/dual_delay	Crossfeeding echoes.			
kilohearts-dynamics	effect	Dynamics	Kilohearts	manual	Needs account	https://kilohearts.com/products/dynamics	Compression and expansion.			
kilohearts-ensemble	effect	Ensemble	Kilohearts	manual	Needs account	https://kilohearts.com/products/ensemble	Moving chorus.			
kilohearts-filter	effect	Filter	Kilohearts	manual	Needs account	https://kilohearts.com/products/filter	Common filter types.			
kilohearts-flanger	effect	Flanger	Kilohearts	manual	Needs account	https://kilohearts.com/products/flanger	Flanger.			
kilohearts-formant-filter	effect	Formant Filter	Kilohearts	manual	Needs account	https://kilohearts.com/products/formant_filter	Vocal coloring.			
kilohearts-frequency-shifter	effect	Frequency Shifter	Kilohearts	manual	Needs account	https://kilohearts.com/products/frequency_shifter	Frequency shifter.			
kilohearts-gain	effect	Gain	Kilohearts	manual	Needs account	https://kilohearts.com/products/gain	Volume control.			
kilohearts-gate	effect	Gate	Kilohearts	manual	Needs account	https://kilohearts.com/products/gate	Gate.			
kilohearts-haas	effect	Haas	Kilohearts	manual	Needs account	https://kilohearts.com/products/haas	Stereo widening.			
kilohearts-ladder-filter	effect	Ladder Filter	Kilohearts	manual	Needs account	https://kilohearts.com/products/ladder_filter	Ladder filter.			
kilohearts-limiter	effect	Limiter	Kilohearts	manual	Needs account	https://kilohearts.com/products/limiter	Limiter.			
kilohearts-nonlinear-filter	effect	Nonlinear Filter	Kilohearts	manual	Needs account	https://kilohearts.com/products/nonlinear_filter	Coloring filter.			
kilohearts-phase-distortion	effect	Phase Distortion	Kilohearts	manual	Needs account	https://kilohearts.com/products/phase_distortion	Phase modulation.			
kilohearts-phaser	effect	Phaser	Kilohearts	manual	Needs account	https://kilohearts.com/products/phaser	Phaser.			
kilohearts-pitch-shifter	effect	Pitch Shifter	Kilohearts	manual	Needs account	https://kilohearts.com/products/pitch_shifter	Pitch shifter.			
kilohearts-resonator	effect	Resonator	Kilohearts	manual	Needs account	https://kilohearts.com/products/resonator	Harmonic resonance.			
kilohearts-reverb	effect	Reverb	Kilohearts	manual	Needs account	https://kilohearts.com/products/reverb	Reverb.			
kilohearts-reverser	effect	Reverser	Kilohearts	manual	Needs account	https://kilohearts.com/products/reverser	Reversed echo.			
kilohearts-ring-mod	effect	Ring Mod	Kilohearts	manual	Needs account	https://kilohearts.com/products/ring_mod	Ring modulator.			
kilohearts-shaper	effect	Shaper	Kilohearts	manual	Needs account	https://kilohearts.com/products/shaper	Distortion with custom shapes.			
kilohearts-stereo	effect	Stereo	Kilohearts	manual	Needs account	https://kilohearts.com/products/stereo	Width and panning.			
kilohearts-tape-stop	effect	Tape Stop	Kilohearts	manual	Needs account	https://kilohearts.com/products/tape_stop	Tape speed simulation.			
kilohearts-trance-gate	effect	Trance Gate	Kilohearts	manual	Needs account	https://kilohearts.com/products/trance_gate	Sequenced gate.			
kilohearts-transient-shaper	effect	Transient Shaper	Kilohearts	manual	Needs account	https://kilohearts.com/products/transient_shaper	Attack shaping.			
klanghelm-dc1a	effect	DC1A	Klanghelm	manual		https://klanghelm.com/contents/products/DC1A.html	Character compressor.			
klanghelm-ivgi	effect	IVGI	Klanghelm	manual		https://klanghelm.com/contents/products/IVGI.html	Saturation and distortion.			
klanghelm-mjuc-jr	effect	MJUC jr.	Klanghelm	manual		https://klanghelm.com/contents/products/MJUCjr.html	Variable-tube compressor.			
onetakz-soundstage-au	effect	SoundStage AU	OneTakz	manual		https://www.onetakz.com/	Stereo field visualizer, 2D and 3D. Audio passes through unchanged.			
socalabs-abtester	effect	ABTester	SocaLabs	auto		https://socalabs.com/developer-tools/abtester/	Switches between A and B inputs for comparison.	https://socalabs.com/files/get.php?id=ABTester.pkg		first
socalabs-addinvert	effect	AddInvert	SocaLabs	auto		https://socalabs.com/developer-tools/addinvert/	Adds or inverts left and right channels.	https://socalabs.com/files/get.php?id=AddInvert.pkg		first
socalabs-bitcrusher	effect	BitCrusher	SocaLabs	auto		https://socalabs.com/effects/bitcrusher/	Bit crusher.	https://socalabs.com/files/get.php?id=BitCrusher.pkg		first
socalabs-channelmute	effect	ChannelMute	SocaLabs	auto		https://socalabs.com/developer-tools/channelmute/	Mutes left or right channel.	https://socalabs.com/files/get.php?id=ChannelMute.pkg		first
socalabs-compensateddelay	effect	CompensatedDelay	SocaLabs	auto		https://socalabs.com/developer-tools/compensateddelay/	Delay that reports itself to the host for compensation.	https://socalabs.com/files/get.php?id=CompensatedDelay.pkg		first
socalabs-compressor	effect	Compressor	SocaLabs	auto		https://socalabs.com/effects/compressor/	Mono or stereo compressor with soft knee.	https://socalabs.com/files/get.php?id=Compressor.pkg		first
socalabs-correlation	effect	Correlation	SocaLabs	auto		https://socalabs.com/analysis/correlation/	Stereo phase correlation and balance meter.	https://socalabs.com/files/get.php?id=Correlation.pkg		first
socalabs-crossfeed	effect	Crossfeed	SocaLabs	auto		https://socalabs.com/effects/crossfeed/	Crossfeed for headphones.	https://socalabs.com/files/get.php?id=Crossfeed.pkg		first
socalabs-delay	effect	Delay	SocaLabs	auto		https://socalabs.com/effects/delay/	Stereo delay with feedback and mix.	https://socalabs.com/files/get.php?id=Delay.pkg		first
socalabs-expander	effect	Expander	SocaLabs	auto		https://socalabs.com/effects/expander/	Mono or stereo expander.	https://socalabs.com/files/get.php?id=Expander.pkg		first
socalabs-gate	effect	Gate	SocaLabs	auto		https://socalabs.com/effects/gate/	Mono or stereo noise gate.	https://socalabs.com/files/get.php?id=Gate.pkg		first
socalabs-graphiceq	effect	GraphicEQ	SocaLabs	auto		https://socalabs.com/effects/graphiceq/	Graphic equalizer.	https://socalabs.com/files/get.php?id=GraphicEQ.pkg		first
socalabs-hugegain	effect	HugeGain	SocaLabs	auto		https://socalabs.com/developer-tools/hugegain/	Gain from -100 dB to +100 dB.	https://socalabs.com/files/get.php?id=HugeGain.pkg		first
socalabs-limiter	effect	Limiter	SocaLabs	auto		https://socalabs.com/effects/limiter/	Mono or stereo limiter.	https://socalabs.com/files/get.php?id=Limiter.pkg		first
socalabs-loudness	effect	Loudness	SocaLabs	auto		https://socalabs.com/analysis/loudness/	LUFS, true peak and RMS meter.	https://socalabs.com/files/get.php?id=Loudness.pkg		first
socalabs-maths	effect	Maths	SocaLabs	auto		https://socalabs.com/developer-tools/maths/	Mathematical operations on audio.	https://socalabs.com/files/get.php?id=Maths.pkg		first
socalabs-mverb2020	effect	Mverb2020	SocaLabs	auto		https://socalabs.com/effects/mverb2020/	Plate reverb based on the work of Martin Eastwood.	https://socalabs.com/files/get.php?id=Mverb2020.pkg		first
socalabs-oscilloscope	effect	Oscilloscope	SocaLabs	auto		https://socalabs.com/analysis/oscilloscope/	Stereo oscilloscope.	https://socalabs.com/files/get.php?id=Oscilloscope.pkg		first
socalabs-pitchtrack	effect	PitchTrack	SocaLabs	auto		https://socalabs.com/analysis/pitchtrack/	Shows detected pitch and cents offset.	https://socalabs.com/files/get.php?id=PitchTrack.pkg		first
socalabs-sampledelay	effect	SampleDelay	SocaLabs	auto		https://socalabs.com/effects/sampledelay/	Delays audio by a number of samples.	https://socalabs.com/files/get.php?id=SampleDelay.pkg		first
socalabs-simpleverb	effect	SimpleVerb	SocaLabs	auto		https://socalabs.com/effects/simpleverb/	CPU-efficient reverb.	https://socalabs.com/files/get.php?id=SimpleVerb.pkg		first
socalabs-spectrumanalyzer	effect	SpectrumAnalyzer	SocaLabs	auto		https://socalabs.com/analysis/spectrumanalyzer/	Spectrum analyzer.	https://socalabs.com/files/get.php?id=SpectrumAnalyzer.pkg		first
socalabs-stereoenhancer	effect	StereoEnhancer	SocaLabs	auto		https://socalabs.com/effects/stereoenhancer/	Widens or narrows the stereo image.	https://socalabs.com/files/get.php?id=StereoEnhancer.pkg		first
socalabs-stereoprocessor	effect	StereoProcessor	SocaLabs	auto		https://socalabs.com/effects/stereoprocessor/	Mid/side encoding and decoding.	https://socalabs.com/files/get.php?id=StereoProcessor.pkg		first
socalabs-vu	effect	VU	SocaLabs	auto		https://socalabs.com/analysis/vu/	Pair of analogue-style VU meters.	https://socalabs.com/files/get.php?id=VU.pkg		first
socalabs-xyscope	effect	XYScope	SocaLabs	auto		https://socalabs.com/analysis/xyscope/	XY oscilloscope for stereo signals.	https://socalabs.com/files/get.php?id=XYScope.pkg		first
sonosaurus-paulxstretch	effect	PaulXStretch	Sonosaurus	auto		https://sonosaurus.com/paulxstretch/	Extreme time stretching. Open source.	https://sonosaurus.com/paulxstretch/	releases/paulxstretch-[0-9.]+-mac\.dmg	first
steven-atkinson-neural-amp-modeler	effect	Neural Amp Modeler	Steven Atkinson	auto		https://www.neuralampmodeler.com/	Plays deep-learning models of guitar amps and pedals. Open source.	https://api.github.com/repos/sdatkinson/NeuralAmpModelerPlugin/releases	https://github\.com/[^"]+/releases/download/[^"]+/NeuralAmpModeler-v[0-9.]+-mac\.dmg"	first
surge-synth-team-surge-xt-effects	effect	Surge XT Effects	Surge Synth Team	auto		https://surge-synthesizer.github.io/downloads/	The Surge XT effects as a plug-in. Installed with Surge XT. Open source.	https://api.github.com/repos/surge-synthesizer/releases-xt/releases	https://github\.com/[^"]+/releases/download/[^"]+/surge-xt-macOS-[0-9.]+\.dmg"	first
tal-software-tal-bitcrusher	effect	TAL-Bitcrusher	TAL Software	auto		https://tal-software.com/products/tal-bitcrusher	Bit crusher.	https://tal-software.com/products/tal-bitcrusher	[^"]*/downloads/plugins/TAL-Bitcrusher-installer_macos\.zip	first
tal-software-tal-chorus-lx	effect	TAL-Chorus-LX	TAL Software	auto		https://tal-software.com/products/tal-chorus-lx	Chorus.	https://tal-software.com/products/tal-chorus-lx	[^"]*/downloads/plugins/TAL-Chorus-LX-installer_macos\.zip	first
tal-software-tal-filter-2	effect	TAL-Filter-2	TAL Software	auto		https://tal-software.com/products/tal-filter	Filter.	https://tal-software.com/products/tal-filter	[^"]*/downloads/plugins/TAL-Filter-2-installer_macos\.zip	first
tal-software-tal-reverb-3	effect	TAL-Reverb-3	TAL Software	auto		https://tal-software.com/products/tal-reverb	Reverb.	https://tal-software.com/products/tal-reverb	[^"]*/downloads/plugins/TAL-Reverb-3-installer(_macos)?\.(pkg|zip)	first
tal-software-tal-reverb-4	effect	TAL-Reverb-4	TAL Software	auto		https://tal-software.com/products/tal-reverb-4	Reverb.	https://tal-software.com/products/tal-reverb-4	[^"]*/downloads/plugins/TAL-Reverb-4-installer(_macos)?\.(pkg|zip)	first
tal-software-tal-vocoder-2	effect	TAL-Vocoder-2	TAL Software	auto		https://tal-software.com/products/tal-vocoder	Vocoder.	https://tal-software.com/products/tal-vocoder	[^"]*/downloads/plugins/TAL-Vocoder-2-installer_macos\.zip	first
tokyo-dawn-labs-tdr-kotelnikov	effect	TDR Kotelnikov	Tokyo Dawn Labs	auto		https://www.tokyodawn.net/tdr-kotelnikov/	Wideband compressor for stereo bus work.	https://www.tokyodawn.net/tdr-kotelnikov/	https://www\.tokyodawn\.net/labs/[A-Za-z]+/[0-9.]+/TDR Kotelnikov\.zip	first
tokyo-dawn-labs-tdr-molotok	effect	TDR Molotok	Tokyo Dawn Labs	auto		https://www.tokyodawn.net/tdr-molotok/	Compressor with a range of colours.	https://www.tokyodawn.net/tdr-molotok/	https://www\.tokyodawn\.net/labs/[A-Za-z]+/[0-9.]+/TDR Molotok\.zip	first
tokyo-dawn-labs-tdr-nova	effect	TDR Nova	Tokyo Dawn Labs	auto		https://www.tokyodawn.net/tdr-nova/	Dynamic equalizer.	https://www.tokyodawn.net/tdr-nova/	https://www\.tokyodawn\.net/labs/[A-Za-z]+/[0-9.]+/TDR Nova\.zip	first
tokyo-dawn-labs-tdr-prism	effect	TDR Prism	Tokyo Dawn Labs	auto		https://www.tokyodawn.net/tdr-prism/	Spectrum analyzer.	https://www.tokyodawn.net/tdr-prism/	https://www\.tokyodawn\.net/labs/[A-Za-z]+/[0-9.]+/TDR Prism\.zip	first
tokyo-dawn-labs-tdr-vos-slickeq	effect	TDR VOS SlickEQ	Tokyo Dawn Labs	auto		https://www.tokyodawn.net/tdr-vos-slickeq/	Mixing and mastering equalizer.	https://www.tokyodawn.net/tdr-vos-slickeq/	https://www\.tokyodawn\.net/labs/[A-Za-z]+/[0-9.]+/TDR VOS SlickEQ\.zip	first
u-he-protoverb	effect	Protoverb	u-he	auto		https://u-he.com/products/protoverb/	Experimental research reverb.	https://u-he.com/products/protoverb/	https://dl\.u-he\.com/releases/Protoverb_[^"]*_Mac\.zip	first
valhalla-dsp-valhalla-freqecho	effect	Valhalla FreqEcho	Valhalla DSP	manual		https://valhalladsp.com/shop/delay/valhalla-freq-echo/	Frequency shifter plus analog echo.			
valhalla-dsp-valhalla-spacemodulator	effect	Valhalla SpaceModulator	Valhalla DSP	manual		https://valhalladsp.com/shop/modulation/valhalla-space-modulator/	Flanger, eleven algorithms.			
valhalla-dsp-valhalla-supermassive	effect	Valhalla Supermassive	Valhalla DSP	manual		https://valhalladsp.com/shop/reverb/valhalla-supermassive/	Massive reverbs and delays, 22 modes.			
voxengo-anspec	effect	AnSpec	Voxengo	auto		https://www.voxengo.com/product/anspec/	Analog-style spectrum analyzer.	https://www.voxengo.com/product/anspec/	https://www\.voxengo\.com/files/VoxengoAnSpec_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-beeper	effect	Beeper	Voxengo	auto		https://www.voxengo.com/product/beeper/	Inserts beeps, noise bursts or silence.	https://www.voxengo.com/product/beeper/	https://www\.voxengo\.com/files/VoxengoBeeper_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-boogex	effect	Boogex	Voxengo	auto		https://www.voxengo.com/product/boogex/	Guitar amp with cabinet modeling, gate and reverb.	https://www.voxengo.com/product/boogex/	https://www\.voxengo\.com/files/VoxengoBoogex_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-correlometer	effect	Correlometer	Voxengo	auto		https://www.voxengo.com/product/correlometer/	Multi-band correlation meter.	https://www.voxengo.com/product/correlometer/	https://www\.voxengo\.com/files/VoxengoCorrelometer_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-latency-delay	effect	Latency Delay	Voxengo	auto		https://www.voxengo.com/product/latencydelay/	Compensates latency that other plug-ins do not report.	https://www.voxengo.com/product/latencydelay/	https://www\.voxengo\.com/files/VoxengoLatencyDelay_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-marvel-geq	effect	Marvel GEQ	Voxengo	auto		https://www.voxengo.com/product/marvelgeq/	Linear-phase 16-band graphic equalizer.	https://www.voxengo.com/product/marvelgeq/	https://www\.voxengo\.com/files/VoxengoMarvelGEQ_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-msed	effect	MSED	Voxengo	auto		https://www.voxengo.com/product/msed/	Mid/side encoder and decoder.	https://www.voxengo.com/product/msed/	https://www\.voxengo\.com/files/VoxengoMSED_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-oldskoolverb	effect	OldSkoolVerb	Voxengo	auto		https://www.voxengo.com/product/oldskoolverb/	Classic stereo reverb, plate to hall.	https://www.voxengo.com/product/oldskoolverb/	https://www\.voxengo\.com/files/VoxengoOldSkoolVerb_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-overtone-geq	effect	Overtone GEQ	Voxengo	auto		https://www.voxengo.com/product/overtonegeq/	7-band harmonic graphic equalizer.	https://www.voxengo.com/product/overtonegeq/	https://www\.voxengo\.com/files/VoxengoOvertoneGEQ_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-sound-delay	effect	Sound Delay	Voxengo	auto		https://www.voxengo.com/product/sounddelay/	Delay set in milliseconds and samples.	https://www.voxengo.com/product/sounddelay/	https://www\.voxengo\.com/files/VoxengoSoundDelay_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-span	effect	SPAN	Voxengo	auto		https://www.voxengo.com/product/span/	FFT spectrum analyzer.	https://www.voxengo.com/product/span/	https://www\.voxengo\.com/files/VoxengoSPAN_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-stereo-touch	effect	Stereo Touch	Voxengo	auto		https://www.voxengo.com/product/stereotouch/	Stereo widening for mono tracks.	https://www.voxengo.com/product/stereotouch/	https://www\.voxengo\.com/files/VoxengoStereoTouch_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-tempo-delay	effect	Tempo Delay	Voxengo	auto		https://www.voxengo.com/product/tempodelay/	Tempo-based delay with filter and tremolo.	https://www.voxengo.com/product/tempodelay/	https://www\.voxengo\.com/files/VoxengoTempoDelay_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
voxengo-teq-421	effect	TEQ-421	Voxengo	auto		https://www.voxengo.com/product/teq421/	Triple equalizer.	https://www.voxengo.com/product/teq421/	https://www\.voxengo\.com/files/VoxengoTEQ421_[0-9]+_Mac_AU_AAX_VST_setup\.dmg	first
xfer-records-ott	effect	OTT	Xfer Records	auto		https://xferrecords.com/freeware	Multiband upward and downward compressor.	https://xferrecords.com/freeware	/product_downloads/[0-9]+/freeware	first
youlean-youlean-loudness-meter-2	effect	Youlean Loudness Meter 2	Youlean	auto		https://youlean.co/youlean-loudness-meter/	Loudness meter: LUFS, LRA, PLR.	https://youlean.co/download-youlean-loudness-meter/	https://cdn\.youlean\.co/[^"]*macOS\.zip	first
PROWAVE_CATALOG_END
}

KIND="$DEFAULT_KIND"; ONLY=""; LIST=0; DRY=0; GUI=0; YES=0
while [ $# -gt 0 ]; do
  case "$1" in
    --list) LIST=1 ;;
    --dry-run) DRY=1 ;;
    --gui) GUI=1; YES=1 ;;
    --yes) YES=1 ;;
    --kind) shift; KIND="$1" ;;
    --only) shift; ONLY="$1"; KIND="" ;;
    -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ "$LIST" = 1 ]; then
  catalog | cut -f1-8
  exit 0
fi

if [ "$(uname -s)" != "Darwin" ]; then echo "macOS only" >&2; exit 2; fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/prowave-free.XXXXXXXX")" || exit 2
chmod 700 "$WORK"
MOUNTS="$WORK/mounts.txt"; : > "$MOUNTS"
ROOTSH="$WORK/root.sh"; ROOTLOG="$WORK/root.log"; DONE="$WORK/done.txt"
: > "$DONE"; : > "$ROOTLOG"
printf '#!/bin/sh\n' > "$ROOTSH"
PKGIDS="$WORK/pkgids.txt"; : > "$PKGIDS"
N_OK=0; N_FAIL=0; N_MANUAL=0

cleanup() {
  if [ -s "$MOUNTS" ]; then
    while IFS= read -r mp; do hdiutil detach "$mp" -force >/dev/null 2>&1; done < "$MOUNTS"
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT
trap 'echo "@@ done ok=$N_OK fail=$N_FAIL manual=$N_MANUAL"; exit 130' INT TERM

say() { printf '%s\n' "$*"; }
ok() { N_OK=$((N_OK+1)); say "@@ ok $1"; }
fail() { N_FAIL=$((N_FAIL+1)); say "@@ fail $1 $2"; }
manual() { N_MANUAL=$((N_MANUAL+1)); say "@@ manual $1 $2"; }

fetch_page() { curl -fsSL --compressed --max-time 60 --retry 2 -A "$UA" -w '\n@@EFFECTIVE %{url_effective}\n' "$1" 2>/dev/null; }

# resolve LINK against BASE (the page's final address)
resolve() {
  link="$1"; base="$2"
  link="$(printf '%s' "$link" | sed -e 's/&amp;/\&/g' -e 's/ /%20/g')"
  case "$link" in
    http://*|https://*) printf '%s' "$link" ;;
    //*) printf 'https:%s' "$link" ;;
    /*) printf '%s%s' "$(printf '%s' "$base" | sed -E 's#^(https?://[^/]+).*#\1#')" "$link" ;;
    *) printf '%s/%s' "$(printf '%s' "$base" | sed -E 's#[?\#].*$##; s#/[^/]*$##')" "$link" ;;
  esac
}

# find_url SOURCE PATTERN PICK -> prints the file address
find_url() {
  src="$1"; pat="$2"; pick="$3"
  if [ -z "$pat" ]; then printf '%s' "$src"; return 0; fi
  page="$(fetch_page "$src")" || return 1
  base="$(printf '%s\n' "$page" | sed -n 's/^@@EFFECTIVE //p' | tail -1)"
  [ -n "$base" ] || base="$src"
  if [ "$pick" = "last" ]; then
    hit="$(printf '%s\n' "$page" | grep -oE -- "$pat" | tail -1)"
  else
    hit="$(printf '%s\n' "$page" | grep -oE -- "$pat" | head -1)"
  fi
  hit="$(printf '%s' "$hit" | sed -e 's/["'"'"']$//')"
  [ -n "$hit" ] || return 1
  resolve "$hit" "$base"
}

is_mac_bundle() { [ -d "$1/Contents/MacOS" ]; }

# scan DIR ID DEPTH: fills $WORK/$ID.pkgs and $WORK/$ID.bundles
scan() {
  dir="$1"; id="$2"; depth="$3"
  # installers, not inside a plug-in or app bundle
  find "$dir" \( -name '*.component' -o -name '*.vst3' -o -name '*.vst' -o -name '*.app' -o -name '*.clap' -o -name '*.aaxplugin' -o -name '__MACOSX' \) -prune -o \
       \( -iname '*.pkg' -o -iname '*.mpkg' \) -print 2>/dev/null |
    grep -viE '(aax|clap|uninstall)[^/]*$|[^a-zA-Z](win|windows|win32|win64|linux)([^a-zA-Z][^/]*)?$' >> "$WORK/$id.pkgs"
  if [ ! -s "$WORK/$id.pkgs" ]; then
    find "$dir" -name '__MACOSX' -prune -o \( -name '*.component' -o -name '*.vst3' \) -print -prune 2>/dev/null |
    while IFS= read -r b; do is_mac_bundle "$b" && printf '%s\n' "$b"; done >> "$WORK/$id.bundles"
  fi
  if [ ! -s "$WORK/$id.pkgs" ] && [ ! -s "$WORK/$id.bundles" ] && [ "$depth" -lt 3 ]; then
    find "$dir" \( -name '*.app' -o -name '__MACOSX' \) -prune -o \( -iname '*.zip' -o -iname '*.dmg' \) -print 2>/dev/null |
    grep -viE '[^a-zA-Z](win|windows|win32|win64|linux)([^a-zA-Z][^/]*)?$' > "$WORK/$id.nested.$depth"
    while IFS= read -r inner; do
      unpack "$inner" "$id" $((depth+1))
    done < "$WORK/$id.nested.$depth"
  fi
}

# unpack FILE ID DEPTH
unpack() {
  f="$1"; id="$2"; depth="$3"
  magic="$(head -c 4 "$f" 2>/dev/null)"
  out="$WORK/$id.x$depth.$$.$RANDOM"
  case "$magic" in
    xar!) printf '%s\n' "$f" >> "$WORK/$id.pkgs" ;;
    PK*)  mkdir -p "$out" && ditto -x -k "$f" "$out" 2>/dev/null && scan "$out" "$id" "$depth" ;;
    *)
      mp="$WORK/$id.mnt$depth.$RANDOM"; mkdir -p "$mp"
      if yes 2>/dev/null | hdiutil attach "$f" -nobrowse -readonly -noverify -noautoopen -mountpoint "$mp" >/dev/null 2>&1; then
        printf '%s\n' "$mp" >> "$MOUNTS"
        mkdir -p "$out"
        # copy out so the image can be released straight away
        ditto "$mp" "$out" 2>/dev/null
        hdiutil detach "$mp" -force >/dev/null 2>&1
        scan "$out" "$id" "$depth"
      else
        rmdir "$mp" 2>/dev/null
        return 1
      fi ;;
  esac
}

copy_bundle() {
  b="$1"
  case "$b" in
    *.component) dest="$HOME/Library/Audio/Plug-Ins/Components" ;;
    *.vst3) dest="$HOME/Library/Audio/Plug-Ins/VST3" ;;
    *) return 0 ;;
  esac
  name="$(basename "$b")"
  if [ "$DRY" = 1 ]; then say "  would copy $name to ${dest#$HOME/}"; return 0; fi
  mkdir -p "$dest" || return 1
  rm -rf "$dest/$name.prowave-new"
  ditto "$b" "$dest/$name.prowave-new" || return 1
  rm -rf "$dest/$name" && mv "$dest/$name.prowave-new" "$dest/$name" || return 1
  xattr -dr com.apple.quarantine "$dest/$name" 2>/dev/null
  # Apple Silicon refuses unsigned code: sign for this Mac only when the maker did not
  if ! codesign --verify --deep "$dest/$name" >/dev/null 2>&1; then
    codesign --force --deep -s - "$dest/$name" >/dev/null 2>&1
  fi
  say "  copied $name to ${dest#$HOME/}"
}

squote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }

# ---- choose rows ----
ROWS="$WORK/rows.tsv"
if [ -n "$ONLY" ]; then
  catalog | awk -F"$TAB" -v only=",$ONLY," 'index(only, "," $1 ",")' > "$ROWS"
elif [ -n "$KIND" ] && [ "$KIND" != "all" ]; then
  catalog | awk -F"$TAB" -v k="$KIND" '$2 == k' > "$ROWS"
else
  catalog > "$ROWS"
fi
TOTAL="$(grep -c . "$ROWS")"
if [ "$TOTAL" = 0 ]; then echo "nothing selected" >&2; exit 2; fi

if [ "$YES" != 1 ]; then
  AUTO="$(awk -F"$TAB" '$5 == "auto"' "$ROWS" | grep -c .)"
  say "Pro Wave free plug-ins"
  say "  $AUTO to download from their makers and install"
  say "  $((TOTAL-AUTO)) to get by hand (listed at the end)"
  [ "$DRY" = 1 ] && say "  dry run: nothing will be installed"
  printf 'Press Return to start, Control-C to stop. '
  read -r _ < /dev/tty || exit 1
fi

# ---- phase 1: download and unpack, as the user ----
SEEN="$WORK/seen.tsv"; : > "$SEEN"
US="$(printf '\037')"
tr '\t' '\037' < "$ROWS" > "$ROWS.us"
while IFS="$US" read -r id kind name maker mode tag page desc src pat pick; do
  [ -n "$id" ] || continue
  say "@@ start $id"
  if [ "$mode" != "auto" ]; then manual "$id" "$page"; continue; fi
  say "$name ($maker)"
  url="$(find_url "$src" "$pat" "$pick")"
  if [ -z "$url" ]; then fail "$id" "download link not found, get it from $page"; continue; fi
  prev="$(awk -F"$TAB" -v u="$url" '$1 == u {print $2; exit}' "$SEEN")"
  if [ -n "$prev" ]; then
    # same installer as an earlier row (one installer, several plug-ins)
    if grep -q "^$prev\$" "$PKGIDS"; then printf '%s %s\n' "$prev" "$id" >> "$WORK/alias.txt"
    elif grep -q "^$prev\$" "$DONE"; then ok "$id"
    else fail "$id" "its installer failed earlier in this run"; fi
    continue
  fi
  printf '%s\t%s\n' "$url" "$id" >> "$SEEN"
  file="$WORK/$id.download"
  say "  $url"
  if ! curl -fL --max-time 1800 --retry 2 -sS -A "$UA" -e "$page" -o "$file" "$url" 2>"$WORK/$id.err"; then
    fail "$id" "download failed: $(tail -1 "$WORK/$id.err" | cut -c1-120)"; continue
  fi
  : > "$WORK/$id.pkgs"; : > "$WORK/$id.bundles"
  unpack "$file" "$id" 0
  if [ -s "$WORK/$id.pkgs" ]; then
    printf '%s\n' "$id" >> "$PKGIDS"
    printf 'r=0\n' >> "$ROOTSH"
    while IFS= read -r p; do
      say "  installer: $(basename "$p")"
      printf 'installer -pkg %s -target / >/dev/null 2>&1 || r=1\n' "$(squote "$p")" >> "$ROOTSH"
    done < "$WORK/$id.pkgs"
    printf 'if [ $r = 0 ]; then echo "@@ ok %s"; else echo "@@ fail %s the installer reported an error"; fi\n' "$id" "$id" >> "$ROOTSH"
  elif [ -s "$WORK/$id.bundles" ]; then
    bad=0
    while IFS= read -r b; do copy_bundle "$b" || bad=1; done < "$WORK/$id.bundles"
    if [ $bad = 0 ]; then printf '%s\n' "$id" >> "$DONE"; ok "$id"; else fail "$id" "could not copy into ~/Library/Audio/Plug-Ins"; fi
  else
    fail "$id" "no Mac installer or plug-in found in the download, get it from $page"
  fi
  rm -f "$file"
done < "$ROWS.us"

# ---- phase 2: run the installers, one password ----
if [ -s "$PKGIDS" ]; then
  NPKG="$(grep -c . "$PKGIDS")"
  if [ "$DRY" = 1 ]; then
    say "dry run: $NPKG installer(s) not run"
    while IFS= read -r id; do printf '@@ ok %s\n' "$id" >> "$ROOTLOG"; done < "$PKGIDS"
  elif [ "$GUI" = 1 ]; then
    say "Running $NPKG installer(s). macOS asks for the password once."
    osascript -e 'on run argv' \
      -e 'do shell script "/bin/sh " & quoted form of (item 1 of argv) & " > " & quoted form of (item 2 of argv) & " 2>&1" with prompt (item 3 of argv) with administrator privileges' \
      -e 'end run' -- "$ROOTSH" "$ROOTLOG" "Pro Wave wants to run $NPKG plug-in installer(s)." >/dev/null 2>"$WORK/osa.err" ||
      say "  $(cut -c1-160 "$WORK/osa.err")"
  else
    say "Running $NPKG installer(s). Type the Mac password if asked."
    sudo /bin/sh "$ROOTSH" > "$ROOTLOG" 2>&1 < /dev/tty || true
  fi
  while IFS= read -r id; do
    line="$(grep -E "^@@ (ok|fail) $id( |\$)" "$ROOTLOG" | head -1)"
    case "$line" in
      "@@ ok "*) r=ok ;;
      "@@ fail "*) r=fail ;;
      *) r=none ;;
    esac
    ids="$id"
    [ -f "$WORK/alias.txt" ] && ids="$id $(awk -v p="$id" '$1 == p {print $2}' "$WORK/alias.txt" | tr '\n' ' ')"
    for one in $ids; do
      case "$r" in
        ok) ok "$one" ;;
        fail) fail "$one" "the installer reported an error" ;;
        none) fail "$one" "the installer did not run (no password given)" ;;
      esac
    done
  done < "$PKGIDS"
fi

if [ "$GUI" != 1 ] && [ "$N_MANUAL" -gt 0 ]; then
  say ""
  say "Get these by hand:"
  awk -F"$TAB" '$5 != "auto" {printf "  %s (%s)%s  %s\n", $3, $4, ($6 != "" ? " [" $6 "]" : ""), $7}' "$ROWS"
fi
say "@@ done ok=$N_OK fail=$N_FAIL manual=$N_MANUAL"
exit 0
