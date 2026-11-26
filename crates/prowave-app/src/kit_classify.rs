//! What a drum sample is, told from its name: kick, snare, clap, closed hat, open hat, rim,
//! toms, percussion, shaker, crash, ride, 808 bass, effect or loop. And where the samples of a
//! kit go on the Channel Rack's channels when a whole kit is loaded ([`pads`]).
//!
//! The file name is read first, then the folders it is in (the innermost first). A name is cut
//! into words at every character that is not a letter or a digit, between letters and digits,
//! and at a capital after a small letter ("ClosedHiHat01" is closed, hi, hat, 01), so the short
//! codes of drum machines (BD, SD, HH, CP, LT) are found as whole words only.

/// The kinds of drum sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DrumClass {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    Rim,
    TomLow,
    TomMid,
    TomHigh,
    Perc,
    Shaker,
    Crash,
    Ride,
    Bass808,
    Fx,
    /// A loop or a break: browsed, never put on a channel by a kit load.
    Loop,
    Other,
    /// A SoundFont file (`.sf2`): listed with a kit's samples, never put on a rack channel.
    SoundFont,
}

impl DrumClass {
    pub const ALL: [DrumClass; 18] = [
        DrumClass::Kick,
        DrumClass::Snare,
        DrumClass::Clap,
        DrumClass::ClosedHat,
        DrumClass::OpenHat,
        DrumClass::Rim,
        DrumClass::TomLow,
        DrumClass::TomMid,
        DrumClass::TomHigh,
        DrumClass::Perc,
        DrumClass::Shaker,
        DrumClass::Crash,
        DrumClass::Ride,
        DrumClass::Bass808,
        DrumClass::Fx,
        DrumClass::Loop,
        DrumClass::Other,
        DrumClass::SoundFont,
    ];

    /// The short name printed on tags and filter keys.
    pub fn tag(self) -> &'static str {
        match self {
            DrumClass::Kick => "KICK",
            DrumClass::Snare => "SNARE",
            DrumClass::Clap => "CLAP",
            DrumClass::ClosedHat => "HAT",
            DrumClass::OpenHat => "OPEN HAT",
            DrumClass::Rim => "RIM",
            DrumClass::TomLow => "TOM L",
            DrumClass::TomMid => "TOM M",
            DrumClass::TomHigh => "TOM H",
            DrumClass::Perc => "PERC",
            DrumClass::Shaker => "SHAKER",
            DrumClass::Crash => "CRASH",
            DrumClass::Ride => "RIDE",
            DrumClass::Bass808 => "808",
            DrumClass::Fx => "FX",
            DrumClass::Loop => "LOOP",
            DrumClass::Other => "OTHER",
            DrumClass::SoundFont => "SF2",
        }
    }

    /// The class a control-interface name means.
    pub fn from_id(id: &str) -> Option<DrumClass> {
        DrumClass::ALL.into_iter().find(|c| c.id() == id)
    }

    /// The name the control interface uses.
    pub fn id(self) -> &'static str {
        match self {
            DrumClass::Kick => "kick",
            DrumClass::Snare => "snare",
            DrumClass::Clap => "clap",
            DrumClass::ClosedHat => "closed_hat",
            DrumClass::OpenHat => "open_hat",
            DrumClass::Rim => "rim",
            DrumClass::TomLow => "tom_low",
            DrumClass::TomMid => "tom_mid",
            DrumClass::TomHigh => "tom_high",
            DrumClass::Perc => "perc",
            DrumClass::Shaker => "shaker",
            DrumClass::Crash => "crash",
            DrumClass::Ride => "ride",
            DrumClass::Bass808 => "808",
            DrumClass::Fx => "fx",
            DrumClass::Loop => "loop",
            DrumClass::Other => "other",
            DrumClass::SoundFont => "sf2",
        }
    }
}

/// The words of a name, in small letters.
pub fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev = None;
            continue;
        }
        let cut = prev.is_some_and(|p| {
            (p.is_ascii_digit() != c.is_ascii_digit()) || (p.is_lowercase() && c.is_uppercase())
        });
        if cut && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
        prev = Some(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn any(w: &[String], list: &[&str]) -> bool {
    w.iter().any(|t| list.contains(&t.as_str()))
}

/// A word that holds one of `list` (for the long words: "kicks", "subkick", "snaredrum").
fn holds(w: &[String], list: &[&str]) -> bool {
    w.iter().any(|t| list.iter().any(|k| t.contains(k)))
}

/// Two words side by side ("bass drum", "hi hat", "side stick").
fn pair(w: &[String], a: &[&str], b: &[&str]) -> bool {
    w.windows(2)
        .any(|p| a.contains(&p[0].as_str()) && b.contains(&p[1].as_str()))
}

fn is_loop(w: &[String]) -> bool {
    any(
        w,
        &[
            "loop", "loops", "bpm", "break", "breaks", "groove", "grooves", "fill", "fills",
        ],
    ) || w
        .iter()
        .any(|t| t.ends_with("loop") || t.ends_with("loops"))
}

/// The class a list of words names, without the loop test.
fn class_of(w: &[String]) -> Option<DrumClass> {
    use DrumClass::*;
    // Hats first: "open" and "closed" belong to them.
    let hat = any(
        w,
        &[
            "hat", "hats", "hh", "hhat", "chh", "ohh", "hhc", "hho", "cht", "oht",
        ],
    ) || holds(
        w,
        &[
            "hihat", "hi-hat", "highhat", "hithat", "clhat", "ophat", "opnhat", "chhat", "clhh",
            "ophh",
        ],
    ) || pair(w, &["hi", "high"], &["hat", "hats"])
        || w.iter().any(|t| {
            (t.ends_with("hat") || t.ends_with("hats"))
                && ["open", "closed", "close", "cl", "op", "o", "c", "pedal"]
                    .iter()
                    .any(|p| t.starts_with(p) && t.len() <= p.len() + 4)
        });
    let hat_code = any(w, &["ch", "oh"]) && !holds(w, &["tom", "conga", "bongo"]);
    if hat || hat_code {
        let open = any(w, &["open", "opn", "op", "ohh", "oh", "hho", "o", "oht"])
            || w.iter().any(|t| {
                t.starts_with("open")
                    || t.ends_with("ophat")
                    || t.ends_with("opnhat")
                    || t.ends_with("ophh")
                    || t == "ohat"
            });
        let closed = any(
            w,
            &[
                "closed", "close", "cl", "chh", "ch", "hhc", "pedal", "foot", "c",
            ],
        );
        return Some(if open && !closed { OpenHat } else { ClosedHat });
    }
    if holds(w, &["clap", "snap"]) || any(w, &["clp", "cp"]) {
        return Some(Clap);
    }
    if holds(w, &["rimshot", "sidestick", "xstick"])
        || any(w, &["rim", "rims", "rs", "stick", "sticks"])
        || pair(w, &["side", "cross", "x"], &["stick"])
    {
        return Some(Rim);
    }
    if holds(w, &["snare"]) || any(w, &["snr", "sn", "sd", "snar", "snares", "sdrum"]) {
        return Some(Snare);
    }
    if holds(w, &["kick", "bassdrum", "bdrum", "kik"])
        || any(
            w,
            &["kik", "kck", "bd", "kd", "kicks", "bds", "kdrum", "kikz"],
        )
        || pair(w, &["bass", "base"], &["drum", "drums"])
    {
        return Some(Kick);
    }
    // "hitom", "lotom", "midtom": a tom run together with where it sits.
    let joined_tom = w.iter().find_map(|t| {
        [
            "hitom", "htom", "lotom", "ltom", "midtom", "mtom", "flrtom", "ftom",
        ]
        .iter()
        .find(|j| t.ends_with(*j) || t.ends_with(&format!("{j}s")))
        .map(|j| j.chars().next().unwrap_or('m'))
    });
    if let Some(c) = joined_tom {
        return Some(match c {
            'h' => TomHigh,
            'l' | 'f' => TomLow,
            _ => TomMid,
        });
    }
    let tom_word = w
        .iter()
        .find(|t| t.starts_with("tom") && t.len() <= 6 && *t != "tomb");
    if tom_word.is_some() || any(w, &["lt", "mt", "ht", "ft"]) || holds(w, &["floortom"]) {
        let rest = tom_word.map(|t| &t[3..]).unwrap_or("");
        let low = any(w, &["low", "lo", "floor", "lt", "ft", "l"])
            || rest.starts_with('l')
            || rest.starts_with('f');
        let high = any(w, &["hi", "high", "ht", "h", "rack"]) || rest.starts_with('h');
        return Some(if low {
            TomLow
        } else if high {
            TomHigh
        } else {
            TomMid
        });
    }
    if any(w, &["ride", "rides", "rd"]) {
        return Some(Ride);
    }
    if holds(w, &["crash", "cymbal", "cymbl", "splash", "china"])
        || any(w, &["cym", "cymb", "cy", "cr"])
    {
        return Some(Crash);
    }
    if holds(w, &["shaker", "maraca", "tambourine", "cabasa"]) || any(w, &["shk", "tamb", "ma"]) {
        return Some(Shaker);
    }
    if holds(
        w,
        &[
            "perc",
            "conga",
            "bongo",
            "cowbell",
            "clave",
            "block",
            "triangle",
            "guiro",
            "timbale",
            "agogo",
            "tabla",
            "djembe",
            "cajon",
            "udu",
            "cuica",
            "whistle",
            "vibraslap",
        ],
    ) || any(
        w,
        &[
            "cb", "cl", "lc", "mc", "hc", "snap", "snaps", "click", "knock", "bell", "bells",
            "wood",
        ],
    ) {
        return Some(Perc);
    }
    if any(w, &["subbass"])
        || pair(w, &["808"], &["s"])
        || (any(w, &["808"]) && any(w, &["bass", "sub", "glide"]))
        || pair(w, &["sub"], &["bass"])
    {
        return Some(Bass808);
    }
    if any(
        w,
        &[
            "fx", "sfx", "noise", "riser", "risers", "sweep", "sweeps", "impact", "impacts", "vox",
            "vocal", "vocals", "chant", "chants", "adlib", "adlibs", "tag", "tags", "zap", "laser",
            "scratch", "reverse", "rev", "stab", "hit", "hits",
        ],
    ) {
        return Some(Fx);
    }
    None
}

/// The two-letter codes of drum machines (BD, SD, CR, MT…). They are also what machines are
/// called (CR-78, MT-32, RD-8), so they count less than whole words: see [`classify`].
const CODES: [&str; 24] = [
    "hh", "ch", "oh", "cp", "rs", "sn", "sd", "bd", "kd", "lt", "mt", "ht", "ft", "rd", "cy", "cr",
    "ma", "cb", "cl", "lc", "mc", "hc", "op", "pc",
];

/// "snares on" and "snares off" say how a drum was recorded, not what it is.
fn without_snares_on_off(w: &mut Vec<String>) {
    let mut i = 0;
    while i + 1 < w.len() {
        if (w[i] == "snares" || w[i] == "snare") && (w[i + 1] == "on" || w[i + 1] == "off") {
            w.drain(i..i + 2);
        } else {
            i += 1;
        }
    }
}

/// The class of a sample from its path inside the kit ("Hits/Bass Drum [BD]/E808_BD-09.wav").
/// A loop is a loop wherever the path says so; otherwise the file name wins and the folders
/// (the innermost first) are a hint. A file named for nothing but the number 808 is an 808 bass.
///
/// A two-letter code counts in a folder's name only when the name is one or two words ("BD",
/// "808cy"), not in a longer one ("[KB6]_Roland_CR-78" is a machine, not a crash); and in a file
/// name it does not count when a folder's longer name has the same code before the same
/// number ("CR78 Metal.wav" in that folder).
pub fn classify(path: &str) -> DrumClass {
    let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let file = parts.pop().unwrap_or_default();
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let mut fw = words(stem);
    let mut folders: Vec<Vec<String>> = parts.iter().rev().map(|p| words(p)).collect();
    if is_loop(&fw) || folders.iter().any(|w| is_loop(w)) {
        return DrumClass::Loop;
    }
    without_snares_on_off(&mut fw);
    // The machines the folders name: (code, number).
    let mut machines: Vec<(String, String)> = Vec::new();
    for w in folders.iter_mut().filter(|w| w.len() > 2) {
        for p in w.windows(2) {
            if CODES.contains(&p[0].as_str()) && p[1].chars().all(|c| c.is_ascii_digit()) {
                machines.push((p[0].clone(), p[1].clone()));
            }
        }
        w.retain(|t| !CODES.contains(&t.as_str()));
    }
    let mut i = 0;
    while i + 1 < fw.len() {
        if machines.iter().any(|(c, n)| *c == fw[i] && *n == fw[i + 1]) {
            fw.drain(i..i + 2);
        } else {
            i += 1;
        }
    }
    if let Some(c) = class_of(&fw) {
        return c;
    }
    if fw.first().is_some_and(|t| t == "808") {
        return DrumClass::Bass808;
    }
    // Stretched spellings: "haaatt", "snaaar", "kiiick".
    let squashed: Vec<String> = fw.iter().map(|t| squash(t)).collect();
    if let Some(c) = class_of(&squashed) {
        return c;
    }
    // A letter before a number: "K01", "S 2", "H03" (kick, snare, hat).
    for p in fw.windows(2) {
        if p[1].chars().all(|c| c.is_ascii_digit()) {
            match p[0].as_str() {
                "k" => return DrumClass::Kick,
                "s" => return DrumClass::Snare,
                "h" => return DrumClass::ClosedHat,
                _ => {}
            }
        }
    }
    if let Some(c) = folders.iter().find_map(|w| class_of(w)) {
        return c;
    }
    // An 808 named among other words ("BWB MINI RNB 808 (5)"), not a machine's name.
    let machine = |t: &str| matches!(t, "tr" | "e" | "td" | "rx" | "tb");
    if fw
        .iter()
        .enumerate()
        .any(|(i, t)| t == "808" && (i == 0 || !machine(&fw[i - 1])))
    {
        return DrumClass::Bass808;
    }
    DrumClass::Other
}

/// A word with every run of one letter made one ("haaatt" is "hat").
fn squash(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut last = None;
    for c in t.chars() {
        if Some(c) != last || c.is_ascii_digit() {
            out.push(c);
        }
        last = Some(c);
    }
    out
}

/// The class of a name alone (an instrument of a kit definition: "Snare Drum 1").
pub fn classify_label(label: &str) -> Option<DrumClass> {
    let w = words(label);
    if is_loop(&w) {
        return Some(DrumClass::Loop);
    }
    class_of(&w)
}

/// The class General MIDI gives a drum note (35 to 81).
pub fn gm_class(note: u8) -> Option<DrumClass> {
    use DrumClass::*;
    Some(match note {
        35 | 36 => Kick,
        37 => Rim,
        38 | 40 => Snare,
        39 => Clap,
        41 | 43 => TomLow,
        45 | 47 => TomMid,
        48 | 50 => TomHigh,
        42 | 44 => ClosedHat,
        46 => OpenHat,
        49 | 52 | 55 | 57 => Crash,
        51 | 53 | 59 => Ride,
        54 | 69 | 70 => Shaker,
        56 | 58 | 60..=68 | 71..=81 => Perc,
        _ => return None,
    })
}

/// Channels a kit load fills.
pub const KIT_PADS: usize = 16;

/// The class each channel of a loaded kit is for, from the first: the rack's usual rows
/// (Kick, Snare, Clap, Closed Hat, Open Hat) and then the rest of a kit.
pub const PAD_LAYOUT: [DrumClass; 15] = [
    DrumClass::Kick,
    DrumClass::Snare,
    DrumClass::Clap,
    DrumClass::ClosedHat,
    DrumClass::OpenHat,
    DrumClass::Rim,
    DrumClass::TomLow,
    DrumClass::TomMid,
    DrumClass::TomHigh,
    DrumClass::Perc,
    DrumClass::Shaker,
    DrumClass::Crash,
    DrumClass::Ride,
    DrumClass::Bass808,
    DrumClass::Fx,
];

/// What a channel takes when the kit has nothing of its own class.
pub fn stand_ins(c: DrumClass) -> &'static [DrumClass] {
    use DrumClass::*;
    match c {
        Clap => &[Snare, Rim],
        OpenHat => &[ClosedHat],
        Rim => &[Perc],
        TomLow => &[TomMid, TomHigh],
        TomMid => &[TomLow, TomHigh],
        TomHigh => &[TomMid, TomLow],
        Shaker => &[Perc],
        Crash => &[Ride],
        Ride => &[Crash],
        _ => &[],
    }
}

/// The order the left-over samples fill the channels still empty.
const FILL_ORDER: [DrumClass; 16] = [
    DrumClass::Kick,
    DrumClass::Snare,
    DrumClass::ClosedHat,
    DrumClass::Perc,
    DrumClass::Clap,
    DrumClass::OpenHat,
    DrumClass::TomLow,
    DrumClass::TomMid,
    DrumClass::TomHigh,
    DrumClass::Crash,
    DrumClass::Ride,
    DrumClass::Shaker,
    DrumClass::Rim,
    DrumClass::Bass808,
    DrumClass::Fx,
    DrumClass::Other,
];

/// Where the samples of a kit go: for each channel from the first, the index of its sample in
/// `classes` (the kit's samples in order). Each channel of [`PAD_LAYOUT`] takes the kit's first
/// sample of its class; a channel left empty takes a stand-in (a second snare for a missing
/// clap, a second closed hat for a missing open hat, and so on); the samples left over then
/// fill the channels still empty, a class at a time in turn, up to [`KIT_PADS`]. Loops are
/// never placed. In a small kit the channels after the fifth close up and the last of them move
/// into the first five still empty; the list ends at the last channel filled.
pub fn pads(classes: &[DrumClass]) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = vec![None; KIT_PADS];
    let mut used = vec![false; classes.len()];
    let take = |c: DrumClass, used: &mut Vec<bool>| -> Option<usize> {
        let i = (0..classes.len()).find(|&i| !used[i] && classes[i] == c)?;
        used[i] = true;
        Some(i)
    };
    for (k, c) in PAD_LAYOUT.iter().enumerate() {
        out[k] = take(*c, &mut used);
    }
    for (k, c) in PAD_LAYOUT.iter().enumerate() {
        if out[k].is_none() {
            out[k] = stand_ins(*c).iter().find_map(|s| take(*s, &mut used));
        }
    }
    // The rest: one of each class in turn, so sixteen kicks do not fill the rack before a
    // second snare gets a channel.
    let mut empty: Vec<usize> = (0..KIT_PADS).filter(|k| out[*k].is_none()).collect();
    empty.reverse();
    'fill: loop {
        let mut placed = false;
        for c in FILL_ORDER {
            if empty.is_empty() {
                break 'fill;
            }
            if let Some(i) = take(c, &mut used) {
                if let Some(k) = empty.pop() {
                    out[k] = Some(i);
                    placed = true;
                }
            }
        }
        if !placed {
            break;
        }
    }
    // A small kit: the channels after the rack's five usual rows close up, and its last
    // samples move into any of those five rows still empty, so a gap is left only in a kit
    // of fewer than five samples.
    let tail: Vec<Option<usize>> = out
        .split_off(5)
        .into_iter()
        .filter(Option::is_some)
        .collect();
    out.extend(tail);
    while out.len() > 5 {
        let Some(gap) = out[..5].iter().position(Option::is_none) else {
            break;
        };
        out[gap] = out.pop().flatten();
    }
    while out.last().is_some_and(Option::is_none) {
        out.pop();
    }
    out
}

/// Compares names as people sort them: runs of digits by their value ("Kick 2" before
/// "Kick 10"), letters without regard to case.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let strip = |s: &[u8]| {
                let z = s.iter().take_while(|c| **c == b'0').count();
                s[z.min(s.len().saturating_sub(1))..].to_vec()
            };
            let (x, y) = (strip(&a[si..i]), strip(&b[sj..j]));
            let o = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
            if o != Ordering::Equal {
                return o;
            }
        } else {
            let o = a[i].to_ascii_lowercase().cmp(&b[j].to_ascii_lowercase());
            if o != Ordering::Equal {
                return o;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::DrumClass::*;
    use super::*;

    #[test]
    fn names_are_cut_into_words() {
        assert_eq!(
            words("ClosedHiHat01-01"),
            ["closed", "hi", "hat", "01", "01"]
        );
        assert_eq!(words("808_HH__CL"), ["808", "hh", "cl"]);
        assert_eq!(words("E808_BD[long]-09"), ["e", "808", "bd", "long", "09"]);
        assert_eq!(words("KR55CYMB"), ["kr", "55", "cymb"]);
    }

    /// File names as they are in the library on 2026-11-23.
    #[test]
    fn real_file_names_get_their_class() {
        let cases: [(&str, DrumClass); 46] = [
            ("samples/Kick04.wav", Kick),
            ("samples/Snare09.wav", Snare),
            ("samples/ClosedHiHat01-01.wav", ClosedHat),
            ("samples/OpenHiHat02-01.wav", OpenHat),
            ("samples/Cymbal01-01.wav", Crash),
            ("[KB6]_Akai_MPC2000/808_HH__CL.wav", ClosedHat),
            ("Hits/Bass Drum [BD]/E808_BD[long]-09.wav", Kick),
            ("Hits/Claves [CL]/E808_CL-02.wav", Perc),
            ("Hits/Congas [HC-MC-LC]/E808_LC-06.wav", Perc),
            ("Hits/Cymbal [CY]/E808_CY-08.wav", Crash),
            ("Hits/Rimshot [RS]/E808_RS-05.wav", Rim),
            ("Hits/Tom Toms [LT-MT-HT]/E808_HT-11.wav", TomHigh),
            ("Hits/Tom Toms [LT-MT-HT]/E808_LT-01.wav", TomLow),
            ("Hits/Tom Toms [LT-MT-HT]/E808_MT-01.wav", TomMid),
            ("Loops/105bpm/E808_Loop_Hats_105-04.wav", Loop),
            ("Loops/132bpm/E808_Loop_SD_132-03.wav", Loop),
            ("110bpm/AM_Eighties08_110-01.wav", Loop),
            ("808/CB.WAV", Perc),
            ("808cy/CY1000.WAV", Crash),
            ("808lt/LT25.WAV", TomLow),
            ("808sd/SD5075.WAV", Snare),
            ("bd/BT7AADA.wav", Kick),
            ("dr55kit/DR55SNAR.WAV", Snare),
            ("kr55kit/KR55CYMB.WAV", Crash),
            ("linndrum/tomll.wav", TomLow),
            ("linndrum/tomh.wav", TomHigh),
            ("cw_blofeld_bd34.wav", Kick),
            ("cw_blofeld_perc14.wav", Perc),
            ("cw_blofeld_tom04.wav", TomMid),
            (
                "WAV-Samples/claps/Bluezone-Rhythm-Lab-Free-Techno-Claps-010.wav",
                Clap,
            ),
            ("WAV-Samples/percussions/Bluezone-Percussions-024.wav", Perc),
            ("drums-collection-1/01 1337 Kick.mp3", Kick),
            ("drums-collection-1/06 fake rim.mp3", Rim),
            ("drums-collection-1/09 swunk shaker.mp3", Shaker),
            ("drums-collection-1/11 jpeg tom.mp3", TomMid),
            ("Bass-Drum-1.wav", Kick),
            ("Boom-Kick.wav", Kick),
            ("Kicks/OZ_kick_hard.wav", Kick),
            ("808s/Spinz 808.wav", Bass808),
            ("808 Sub Bass C.wav", Bass808),
            ("808 03.wav", Bass808),
            ("Snares/808 Snare.wav", Snare),
            ("Hi Hats/Open/OH_tight 02.wav", OpenHat),
            ("Rides/Ride Bell 1.wav", Ride),
            ("FX/Riser 01.wav", Fx),
            ("WAV/Snap Break 165.wav", Loop),
        ];
        for (path, want) in cases {
            assert_eq!(classify(path), want, "{path}");
        }
        // What machines are called is not what a sample is.
        for (path, want) in [
            ("[KB6]_Roland_CR-78/[KB6]_Roland_CR-78/Guiro.wav", Perc),
            ("[KB6]_Roland_CR-78/[KB6]_Roland_CR-78/Metal.wav", Other),
            ("[KB6]_Roland_CR-78/CR78 Metal.wav", Other),
            ("[KB6]_Roland_CR-78/CR78 CY.wav", Crash),
            ("[KB6]_Casio_MT-40/Beat 1.wav", Other),
            ("Roland MT-32 Kit/MT32_Kick.wav", Kick),
            ("JazzFunkKit/bop kick - snares off - 6.flac", Kick),
            ("JazzFunkKit/rimshot - snares on - 9.flac", Rim),
            ("JazzFunkKit/snare - snares on - 2.flac", Snare),
        ] {
            assert_eq!(classify(path), want, "{path}");
        }
        // What the library's kits name them on 2026-11-24.
        for (path, want) in [
            ("[KB6]_Casio_SK1/skclhat.wav", ClosedHat),
            ("[KB6]_Casio_SK1/skophat.wav", OpenHat),
            ("[KB6]_Casio_SK1/skhitom.wav", TomHigh),
            ("[KB6]_Casio_SK1/sklotom.wav", TomLow),
            ("did it again/did it again haaatt.wav", ClosedHat),
            ("did it again/did it again ooohat.wav", OpenHat),
            ("did it again/did it again snaaar.wav", Snare),
            ("Dust-and-Grain/WAV/DG-K01-crate-weight.wav", Kick),
            ("Dust-and-Grain/WAV/DG-S02-dry.wav", Snare),
            ("Dust-and-Grain/WAV/DG-H01-needle.wav", ClosedHat),
            ("Kick/KDrum-03.wav", Kick),
            ("Lucky You Drumkit/BWB MINI RNB 808  (5).wav", Bass808),
            ("Snaps/Snap 3.wav", Clap),
            ("Snare fills/Demo Drum Fills.wav", Loop),
            ("Percussion/Cajon Slap.wav", Perc),
            ("Vox/Producer Tag 2.wav", Fx),
            ("[KB6]_Roland_TR-808/TR 808 1.wav", Other),
            ("BURNT SKIES/HITHAT CEREAL.wav", ClosedHat),
            ("Boss DR-110/DR110CHT.WAV", ClosedHat),
            ("Boss DR-110/DR110OHT.WAV", OpenHat),
            ("Korg_KPR77/KPRCLHH.WAV", ClosedHat),
            ("Korg_KPR77/KPROPHH.WAV", OpenHat),
            ("Korg_KPR77/KPRCYMBL.WAV", Crash),
            ("Broccoli Drum Kit/KWKIK093_2.wav", Kick),
        ] {
            assert_eq!(classify(path), want, "{path}");
        }
        assert_eq!(classify("armora/000_beep.wav"), Other);
        assert_eq!(
            classify("that/what.wav"),
            Other,
            "hat inside a word is not a hat"
        );
    }

    #[test]
    fn a_label_and_a_note_name_a_class() {
        assert_eq!(classify_label("Bass Drum 1"), Some(Kick));
        assert_eq!(classify_label("Closed Hi-Hat 2"), Some(ClosedHat));
        assert_eq!(classify_label("Pedal HH"), Some(ClosedHat));
        assert_eq!(classify_label("Floor Tom"), Some(TomLow));
        assert_eq!(classify_label("Side Stick"), Some(Rim));
        assert_eq!(classify_label("Instrument 4"), None);
        assert_eq!(gm_class(36), Some(Kick));
        assert_eq!(gm_class(42), Some(ClosedHat));
        assert_eq!(gm_class(46), Some(OpenHat));
        assert_eq!(gm_class(49), Some(Crash));
        assert_eq!(gm_class(20), None);
    }

    #[test]
    fn a_full_kit_lands_on_the_racks_rows() {
        // Two of everything, in an order that is not the rack's.
        let kit = [
            Ride, Crash, Fx, Bass808, Shaker, Perc, TomHigh, TomMid, TomLow, Rim, OpenHat,
            ClosedHat, Clap, Snare, Kick, Kick, Snare, Loop, Other,
        ];
        let p = pads(&kit);
        assert_eq!(p.len(), KIT_PADS);
        for (k, c) in PAD_LAYOUT.iter().enumerate() {
            assert_eq!(kit[p[k].unwrap()], *c, "channel {}", k + 1);
        }
        assert_eq!(p[15], Some(15), "the second kick takes the last channel");
        assert!(p.iter().flatten().all(|i| kit[*i] != Loop), "no loops");
        let mut seen: Vec<usize> = p.iter().flatten().copied().collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), KIT_PADS, "no sample twice");
    }

    #[test]
    fn missing_classes_get_stand_ins_then_the_rest() {
        // No clap, no open hat: the second snare and the second hat stand in.
        let kit = [Kick, Snare, Snare, ClosedHat, ClosedHat, Perc];
        assert_eq!(
            pads(&kit),
            [Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]
        );
        // Many kicks and one snare: the snare still gets its channel, kicks fill the rest.
        let mut kit = vec![Kick; 30];
        kit.push(Snare);
        let p = pads(&kit);
        assert_eq!(p[0], Some(0));
        assert_eq!(p[1], Some(30));
        assert_eq!(p.len(), KIT_PADS);
        assert!(p.iter().all(Option::is_some));
        // Only loops: nothing to load.
        assert!(pads(&[Loop, Loop]).is_empty());
        assert!(pads(&[]).is_empty());
    }

    #[test]
    fn names_sort_by_their_numbers() {
        let mut v = vec!["Kick 10", "kick 2", "Kick 1", "Snare 1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["Kick 1", "kick 2", "Kick 10", "Snare 1"]);
        assert_eq!(natural_cmp("a007", "a7"), std::cmp::Ordering::Less);
    }
}
