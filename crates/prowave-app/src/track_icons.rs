//! Track icons (Logic Pro: Track > Assign Track Icon) as Hardware pictograms: line drawings on a
//! 24×24 grid in 2-unit strokes of one ink, the way a panel's silkscreen prints them (keyboard,
//! grand piano, electric piano, synth, waveform, Drummer's snare, drum kit, guitar and bass,
//! microphone, violin, aux, folder, VCA, MIDI socket). Original drawings, no Apple artwork.
use crate::model::{Track, TrackKind};
use egui::{vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    Note,
    Waveform,
    DrumKit,
    DrummerTile,
    Piano,
    WoodKeys,
    Synth,
    Bass,
    Guitar,
    Mic,
    Strings,
    /// Auxiliary channel strip: lines running into a bus.
    Aux,
    /// Folder (track stack).
    Folder,
    /// VCA: three faders on a purple tile.
    Vca,
    /// External MIDI: a five-pin MIDI socket on an orange tile.
    ExternalMidi,
}

impl IconKind {
    /// Every icon, in the order the Assign Track Icon window shows them.
    pub const ALL: [IconKind; 15] = [
        IconKind::Note,
        IconKind::Waveform,
        IconKind::DrumKit,
        IconKind::DrummerTile,
        IconKind::Piano,
        IconKind::WoodKeys,
        IconKind::Synth,
        IconKind::Bass,
        IconKind::Guitar,
        IconKind::Mic,
        IconKind::Strings,
        IconKind::Aux,
        IconKind::Folder,
        IconKind::Vca,
        IconKind::ExternalMidi,
    ];

    /// The name the project file stores this icon under.
    pub fn name(self) -> &'static str {
        match self {
            IconKind::Note => "note",
            IconKind::Waveform => "waveform",
            IconKind::DrumKit => "drum_kit",
            IconKind::DrummerTile => "drummer",
            IconKind::Piano => "piano",
            IconKind::WoodKeys => "wood_keys",
            IconKind::Synth => "synth",
            IconKind::Bass => "bass",
            IconKind::Guitar => "guitar",
            IconKind::Mic => "mic",
            IconKind::Strings => "strings",
            IconKind::Aux => "aux",
            IconKind::Folder => "folder",
            IconKind::Vca => "vca",
            IconKind::ExternalMidi => "external_midi",
        }
    }

    /// The icon a project file's name stands for (`None` for an unknown name).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// Icon drawn for a track (header, inspector): the one the user picked, if any. Otherwise a
/// software instrument track shows the generic green note tile (Pro Wave has no built-in
/// instruments to show); other tracks use [`icon_kind`].
pub fn track_icon(t: &Track) -> IconKind {
    // An icon the user picked (Track > Assign Track Icon) wins over the automatic one.
    if let Some(icon) = t.icon {
        icon
    } else if t.kind == TrackKind::Aux {
        IconKind::Aux
    } else if t.kind == TrackKind::Folder {
        IconKind::Folder
    } else if t.kind == TrackKind::Vca {
        IconKind::Vca
    } else if t.kind == TrackKind::ExternalMidi {
        IconKind::ExternalMidi
    } else if t.kind == TrackKind::Instrument {
        IconKind::Note
    } else if t.kind == TrackKind::Audio {
        // A recording shows its material: the microphone for a voice, else the waveform.
        match icon_kind(t) {
            IconKind::Mic => IconKind::Mic,
            _ => IconKind::Waveform,
        }
    } else {
        icon_kind(t)
    }
}

/// Icon suggested by the track's name.
pub fn icon_kind(t: &Track) -> IconKind {
    let n = t.name.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    if has(&["vocal", "vox", "voice", "mic", "choir"]) && !has(&["tape choir"]) {
        IconKind::Mic
    } else if t.kind == TrackKind::Drummer {
        IconKind::DrummerTile
    } else if has(&[
        "drum", "kick", "snare", "hat", "perc", " kit", "808", "909", "beat",
    ]) {
        IconKind::DrumKit
    } else if has(&["bass"]) {
        IconKind::Bass
    } else if has(&[
        "guitar",
        "acoustic",
        "strat",
        "nylon",
        "steel string",
        "twelve string",
    ]) {
        IconKind::Guitar
    } else if has(&["string", "violin", "cello", "orchestral", "pizzicato"]) {
        IconKind::Strings
    } else if has(&[
        "organ",
        "b3",
        "clav",
        "electric piano",
        "e-piano",
        "tine",
        "reed",
        "suitcase",
        "wurli",
        "mellotron",
        "tape ",
    ]) {
        IconKind::WoodKeys
    } else if has(&["piano", "grand", "steinway", "upright"]) {
        IconKind::Piano
    } else if has(&[
        "pad",
        "synth",
        "lead",
        "keys",
        "keyboard",
        "arpegg",
        "arp",
        "cinematic",
        "legacy",
        "saw",
        "pulse",
        "pluck",
    ]) {
        IconKind::Synth
    } else if t.kind == TrackKind::Audio {
        IconKind::Waveform
    } else {
        IconKind::Note
    }
}

/// Drawing context: a 24×24 design grid (origin at its top left) scaled into a square around
/// `c`; every glyph is drawn in 2-unit strokes of one colour, as the Hardware pictograms are.
struct Pen<'a> {
    g: &'a Painter,
    c: Pos2,
    s: f32,
    col: Color32,
}

impl Pen<'_> {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.c + vec2((x - 12.0) * self.s, (y - 12.0) * self.s)
    }

    fn stroke(&self) -> Stroke {
        Stroke::new((2.0 * self.s).max(1.2), self.col)
    }

    /// An open polyline.
    fn line(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.g.add(Shape::line(pts, self.stroke()));
    }

    /// A closed outline.
    fn closed(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.g.add(Shape::closed_line(pts, self.stroke()));
    }

    /// A filled convex shape.
    fn fill(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.g
            .add(Shape::convex_polygon(pts, self.col, Stroke::NONE));
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32) {
        self.closed(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]);
    }

    fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32) {
        self.fill(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]);
    }

    fn circle(&self, x: f32, y: f32, r: f32) {
        self.g
            .circle_stroke(self.p(x, y), r * self.s, self.stroke());
    }

    fn dot(&self, x: f32, y: f32, r: f32) {
        self.g.circle_filled(self.p(x, y), r * self.s, self.col);
    }

    /// An elliptical arc from `a0` to `a1` degrees (0 = right, 90 = down).
    fn arc(&self, cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32) -> Vec<(f32, f32)> {
        let n = (((a1 - a0).abs() / 10.0).ceil() as usize).max(2);
        (0..=n)
            .map(|k| {
                let a = (a0 + (a1 - a0) * k as f32 / n as f32).to_radians();
                (cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect()
    }

    fn ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32) {
        let mut pts = self.arc(cx, cy, rx, ry, 0.0, 360.0);
        pts.pop();
        self.closed(&pts);
    }
}

/// Draws a track pictogram filling `r`, in ink (the Hardware print colour).
pub fn draw_icon(g: &Painter, r: Rect, kind: IconKind) {
    draw_icon_in(g, r, kind, crate::theme::INK);
}

/// Draws a track pictogram filling `r` in `col` (bone on dark caps and plates).
pub fn draw_icon_in(g: &Painter, r: Rect, kind: IconKind, col: Color32) {
    let pen = Pen {
        g,
        c: r.center(),
        s: (r.width().min(r.height()) / 24.0).max(0.2),
        col,
    };
    match kind {
        IconKind::Note => keyboard(&pen),
        IconKind::Waveform => waveform(&pen),
        IconKind::DrumKit => drum_kit(&pen),
        IconKind::DrummerTile => drummer(&pen),
        IconKind::Piano => piano(&pen),
        IconKind::WoodKeys => electric_piano(&pen),
        IconKind::Synth => synth(&pen),
        IconKind::Bass => guitar(&pen, true),
        IconKind::Guitar => guitar(&pen, false),
        IconKind::Mic => mic(&pen),
        IconKind::Strings => violin(&pen),
        IconKind::Aux => aux(&pen),
        IconKind::Folder => folder(&pen),
        IconKind::Vca => vca(&pen),
        IconKind::ExternalMidi => midi_socket(&pen),
    }
}

/// A software instrument: an octave of keys.
fn keyboard(p: &Pen) {
    p.rect(3.0, 5.0, 18.0, 14.0);
    for x in [7.5, 12.0, 16.5] {
        p.line(&[(x, 5.0), (x, 19.0)]);
    }
    p.fill_rect(5.6, 5.0, 3.6, 8.0);
    p.fill_rect(14.8, 5.0, 3.6, 8.0);
}

/// A grand piano seen from above, its keys along the front.
fn piano(p: &Pen) {
    let mut outline = vec![(4.0, 20.0), (4.0, 4.0), (9.0, 4.0)];
    outline.extend(p.arc(9.0, 11.0, 11.0, 7.0, -90.0, 0.0));
    outline.extend([(20.0, 20.0)]);
    p.closed(&outline);
    p.line(&[(4.0, 15.5), (20.0, 15.5)]);
    for x in [7.2, 10.4, 13.6, 16.8] {
        p.line(&[(x, 15.5), (x, 20.0)]);
    }
}

/// An electric piano: a keybed on a stand.
fn electric_piano(p: &Pen) {
    p.rect(2.5, 7.0, 19.0, 8.0);
    for x in [6.3, 10.1, 13.9, 17.7] {
        p.line(&[(x, 10.5), (x, 15.0)]);
    }
    p.line(&[(2.5, 10.5), (21.5, 10.5)]);
    p.line(&[(5.0, 15.0), (5.0, 21.0)]);
    p.line(&[(19.0, 15.0), (19.0, 21.0)]);
}

/// A synthesizer: knobs over a short keyboard.
fn synth(p: &Pen) {
    p.rect(3.0, 4.0, 18.0, 16.0);
    p.line(&[(3.0, 11.0), (21.0, 11.0)]);
    for x in [7.0, 12.0, 17.0] {
        p.dot(x, 7.5, 1.6);
    }
    for x in [7.5, 12.0, 16.5] {
        p.line(&[(x, 11.0), (x, 20.0)]);
    }
    p.fill_rect(5.8, 11.0, 3.4, 4.5);
    p.fill_rect(14.8, 11.0, 3.4, 4.5);
}

/// Audio: a row of waveform columns.
fn waveform(p: &Pen) {
    p.line(&[(3.0, 12.0), (4.5, 12.0)]);
    for (x, h) in [
        (4.5, 2.0),
        (7.0, 5.0),
        (9.5, 3.0),
        (12.0, 7.0),
        (14.5, 4.0),
        (17.0, 6.0),
        (19.5, 2.5),
    ] {
        p.line(&[(x, 12.0 - h), (x, 12.0 + h)]);
    }
    p.line(&[(19.5, 12.0), (21.0, 12.0)]);
}

/// Drummer: a snare drum with a stick across it.
fn drummer(p: &Pen) {
    p.ellipse(12.0, 10.0, 8.0, 3.0);
    let mut side = vec![(4.0, 10.0), (4.0, 16.0)];
    // Round the bottom: from the left side, under the drum, to the right side.
    side.extend(p.arc(12.0, 16.0, 8.0, 3.0, 180.0, 0.0));
    side.push((20.0, 10.0));
    p.line(&side);
    for x in [8.0, 12.0, 16.0] {
        p.line(&[(x, 13.0), (x, 18.5)]);
    }
    p.line(&[(14.5, 2.5), (19.0, 7.0)]);
}

/// A drum kit: the bass drum from the front, a cymbal on its stand.
fn drum_kit(p: &Pen) {
    p.circle(14.0, 14.5, 6.5);
    p.dot(14.0, 14.5, 1.6);
    p.line(&[(2.5, 6.5), (10.0, 4.5)]);
    p.line(&[(6.0, 5.5), (6.0, 21.0)]);
    p.line(&[(3.5, 21.0), (8.5, 21.0)]);
}

/// A guitar (or, with `bass`, an electric bass: a longer neck and a pickup).
fn guitar(p: &Pen, bass: bool) {
    let (neck_top, body_y) = if bass { (1.5, 17.0) } else { (3.0, 16.5) };
    p.rect(11.0, neck_top, 2.0, body_y - 4.5 - neck_top);
    for y in [neck_top + 1.5, neck_top + 4.0] {
        p.line(&[(8.5, y), (11.0, y)]);
        p.line(&[(13.0, y), (15.5, y)]);
    }
    if bass {
        p.ellipse(12.0, body_y, 5.0, 4.5);
        p.fill_rect(10.0, body_y - 0.8, 4.0, 1.6);
    } else {
        p.ellipse(12.0, body_y, 6.0, 5.0);
        p.circle(12.0, body_y - 0.5, 1.7);
    }
}

/// A studio microphone in its cradle.
fn mic(p: &Pen) {
    let mut capsule = p.arc(12.0, 6.5, 3.5, 3.5, 180.0, 360.0);
    capsule.extend(p.arc(12.0, 10.5, 3.5, 3.5, 0.0, 180.0));
    p.closed(&capsule);
    p.line(&p.arc(12.0, 11.0, 6.5, 6.5, 0.0, 180.0));
    p.line(&[(12.0, 17.5), (12.0, 21.0)]);
    p.line(&[(8.5, 21.0), (15.5, 21.0)]);
}

/// A violin with its bow.
fn violin(p: &Pen) {
    p.ellipse(11.0, 10.5, 3.6, 3.0);
    p.ellipse(11.0, 16.5, 4.6, 4.0);
    p.line(&[(11.0, 2.5), (11.0, 7.5)]);
    p.line(&[(15.0, 3.0), (20.5, 21.0)]);
}

/// An aux: three lines running into a bus.
fn aux(p: &Pen) {
    for y in [6.0, 12.0, 18.0] {
        p.line(&[(3.0, y), (10.0, 12.0)]);
    }
    p.circle(15.5, 12.0, 5.0);
    p.dot(15.5, 12.0, 1.6);
}

/// A folder.
fn folder(p: &Pen) {
    p.closed(&[
        (3.0, 6.0),
        (9.0, 6.0),
        (11.0, 8.5),
        (21.0, 8.5),
        (21.0, 19.0),
        (3.0, 19.0),
    ]);
    p.line(&[(3.0, 11.0), (21.0, 11.0)]);
}

/// A VCA: three fader slots with their caps.
fn vca(p: &Pen) {
    for x in [6.0, 12.0, 18.0] {
        p.line(&[(x, 4.0), (x, 20.0)]);
    }
    p.fill_rect(3.5, 12.0, 5.0, 3.5);
    p.fill_rect(9.5, 6.5, 5.0, 3.5);
    p.fill_rect(15.5, 14.0, 5.0, 3.5);
}

/// External MIDI: a five-pin DIN socket.
fn midi_socket(p: &Pen) {
    p.circle(12.0, 12.0, 8.5);
    for deg in [180.0f32, 225.0, 270.0, 315.0, 0.0] {
        let a = deg.to_radians();
        p.dot(12.0 + 4.8 * a.cos(), 12.5 + 4.8 * a.sin(), 1.2);
    }
    p.fill_rect(10.8, 17.5, 2.4, 2.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    #[test]
    fn demo_tracks_get_the_expected_icons() {
        let p = Project::demo();
        let kinds: Vec<IconKind> = p.tracks.iter().map(icon_kind).collect();
        assert_eq!(
            kinds,
            vec![
                IconKind::WoodKeys,
                IconKind::Piano,
                IconKind::Synth,
                IconKind::DrummerTile,
                IconKind::Bass,
                IconKind::Mic
            ]
        );
    }

    #[test]
    fn aux_and_folder_tracks_have_their_own_icons() {
        use crate::model::{Track, TrackKind};
        // Not the note tile, whatever the name says.
        let aux = Track::new("Piano Reverb", TrackKind::Aux);
        assert_eq!(track_icon(&aux), IconKind::Aux);
        assert_eq!(
            track_icon(&Track::new("Drums", TrackKind::Folder)),
            IconKind::Folder
        );
    }

    #[test]
    fn instrument_tracks_show_the_note_tile() {
        let p = Project::demo();
        // The generic tile, even with an instrument-like name.
        assert_eq!(track_icon(&p.tracks[0]), IconKind::Note);
        assert_eq!(track_icon(&p.tracks[1]), IconKind::Note);
        // Drummer and audio tracks keep their own icons.
        assert_eq!(track_icon(&p.tracks[3]), IconKind::DrummerTile);
        assert_eq!(track_icon(&p.tracks[5]), IconKind::Mic);
        // An audio track other than a voice shows the waveform, whatever its name says.
        assert_eq!(track_icon(&p.tracks[4]), IconKind::Waveform);
    }

    #[test]
    fn every_icon_draws() {
        let mut h = crate::ui_test::Harness::new(400.0, 40.0);
        h.settle(|ui| {
            let g = ui.painter().clone();
            for (i, kind) in IconKind::ALL.into_iter().enumerate() {
                let at = egui::pos2(i as f32 * 26.0, 0.0);
                draw_icon(&g, Rect::from_min_size(at, vec2(24.0, 24.0)), kind);
            }
        });
    }
}
