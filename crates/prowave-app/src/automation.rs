//! Automation as Ableton Live's Arrangement View does it (Live 12 manual, chapter 25,
//! "Automation and Editing Envelopes":
//! <https://www.ableton.com/en/live-manual/12/automation-and-editing-envelopes/>).
//!
//! - Every control a strip can automate has an envelope: breakpoints in beats, joined by lines
//!   that can be bent (§25.5, "Drawing and Editing Automation"). An envelope always plays; there
//!   are no Read / Touch / Latch / Write modes.
//! - Moving an automated control by hand overrides its envelope: the control keeps the value it
//!   was given, its LED turns grey and the envelope is drawn dimmed, until Re-Enable Automation
//!   (§25.4, "Overriding Automation").
//! - With Automation Arm on and the transport recording, moving a control writes breakpoints;
//!   they are thinned when the control is let go (§25.1, "Recording Automation in Arrangement
//!   View"; §25.5.4, "Simplifying Envelopes").
//! - Touching a control, in the mixer or a plug-in window, shows it in its track's parameter
//!   chooser.
//!
//! This module has the model (the lanes of a strip and their editing), the recording and the
//! override logic run once a frame by [`tick`], and the list of what each strip can automate
//! ([`devices`]). The lanes in the Tracks area are drawn and edited in `automation_ui.rs`.
use crate::i18n::{tr, trf};
use crate::mixer::{db_to_frac, frac_to_db};
use crate::model::{Project, Track};
use prowave_engine::{lane_value, AutoPoint, AutoTarget};
use std::collections::HashMap;

/// A channel strip that has automation: a track's, or the Stereo Out's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Strip {
    Track(usize),
    Master,
}

/// The lane of one parameter of an effect in a channel strip slot (positions 0..=1).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct FxLane {
    pub slot: u8,
    pub param: u8,
    pub points: Vec<AutoPoint>,
}

/// The lane of one send's level (0..=1).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SendLane {
    pub slot: u8,
    pub points: Vec<AutoPoint>,
}

/// The lane of one parameter of a plug-in (slot 0 the instrument, `k + 1` Audio FX `k`), as
/// positions 0..=1 along the parameter's range `lo..=hi`. `plugin` is the id of the plug-in it
/// was made for: a lane whose slot now holds another plug-in is not played.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PluginLane {
    pub slot: u8,
    pub param: u32,
    pub plugin: String,
    pub lo: f32,
    pub hi: f32,
    pub stepped: bool,
    pub points: Vec<AutoPoint>,
}

/// A recording pass of one control: from where it started, and the last breakpoint written.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Pass {
    pub target: AutoTarget,
    pub start: f64,
    pub last: Option<(f64, f32)>,
}

/// The automation of one strip.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Automation {
    /// The fader (dB), the pan (-1..=1) and Mute (1 muted, 0 playing).
    pub volume: Vec<AutoPoint>,
    pub pan: Vec<AutoPoint>,
    pub mute: Vec<AutoPoint>,
    pub sends: Vec<SendLane>,
    pub fx: Vec<FxLane>,
    pub plugins: Vec<PluginLane>,
    /// What the track's own lane (over its regions) shows: Ableton's parameter chooser.
    pub shown: AutoTarget,
    /// The lanes under the track (Ableton: Show Automation in New Lane), in order.
    pub extra: Vec<AutoTarget>,
    /// Envelopes overridden by hand (until Re-Enable Automation). Not saved.
    pub overridden: Vec<AutoTarget>,
    /// The control held now by a widget that says when it is let go. Not saved.
    pub held: Option<AutoTarget>,
    /// Controls moved by hand and when they last moved (seconds): they count as held for a
    /// moment, as the mouse button's release is not seen for every control. Not saved.
    pub touched: Vec<(AutoTarget, f64)>,
    /// Recording passes in progress. Not saved.
    pub passes: Vec<Pass>,
}

/// Breakpoints closer than this (in beats) are written as one unless the value moves.
const WRITE_STEP: f64 = 1.0 / 32.0;
/// A control moved by hand counts as held this long after its last move, in seconds.
const HOLD_SECS: f64 = 0.35;
/// Most lanes of effects and plug-ins one strip keeps.
pub const MAX_PARAM_LANES: usize = 512;

impl Automation {
    /// The most breakpoints one lane keeps.
    pub const MAX_POINTS: usize = prowave_engine::MAX_AUTO_POINTS;

    /// The breakpoints of the lane of `target` (none if it has no lane).
    pub fn lane(&self, target: AutoTarget) -> &[AutoPoint] {
        match target {
            AutoTarget::Volume => &self.volume,
            AutoTarget::Pan => &self.pan,
            AutoTarget::Mute => &self.mute,
            AutoTarget::Send { slot } => self
                .sends
                .iter()
                .find(|l| l.slot == slot)
                .map_or(&[], |l| &l.points),
            AutoTarget::Fx { slot, param } => self
                .fx
                .iter()
                .find(|l| l.slot == slot && l.param == param)
                .map_or(&[], |l| &l.points),
            AutoTarget::Plugin { slot, param } => self
                .plugins
                .iter()
                .find(|l| l.slot == slot && l.param == param)
                .map_or(&[], |l| &l.points),
        }
    }

    /// The breakpoints of the lane of `target`, made if it had none (see [`Automation::prune`]).
    /// A plug-in lane made here maps onto 0..=1; [`Automation::plugin_lane`] gives it its range.
    pub fn lane_mut(&mut self, target: AutoTarget) -> &mut Vec<AutoPoint> {
        match target {
            AutoTarget::Volume => &mut self.volume,
            AutoTarget::Pan => &mut self.pan,
            AutoTarget::Mute => &mut self.mute,
            AutoTarget::Send { slot } => {
                let i = match self.sends.iter().position(|l| l.slot == slot) {
                    Some(i) => i,
                    None => {
                        self.sends.push(SendLane {
                            slot,
                            points: Vec::new(),
                        });
                        self.sends.len() - 1
                    }
                };
                &mut self.sends[i].points
            }
            AutoTarget::Fx { slot, param } => {
                let i = match self
                    .fx
                    .iter()
                    .position(|l| l.slot == slot && l.param == param)
                {
                    Some(i) => i,
                    None => {
                        self.fx.push(FxLane {
                            slot,
                            param,
                            points: Vec::new(),
                        });
                        self.fx.len() - 1
                    }
                };
                &mut self.fx[i].points
            }
            AutoTarget::Plugin { slot, param } => {
                let i = self.plugin_index(slot, param, "", (0.0, 1.0), false);
                &mut self.plugins[i].points
            }
        }
    }

    fn plugin_index(
        &mut self,
        slot: u8,
        param: u32,
        plugin: &str,
        (lo, hi): (f32, f32),
        stepped: bool,
    ) -> usize {
        match self
            .plugins
            .iter()
            .position(|l| l.slot == slot && l.param == param)
        {
            Some(i) => i,
            None => {
                self.plugins.push(PluginLane {
                    slot,
                    param,
                    plugin: plugin.to_string(),
                    lo,
                    hi,
                    stepped,
                    points: Vec::new(),
                });
                self.plugins.len() - 1
            }
        }
    }

    /// Makes sure the lane of a plug-in parameter exists with the plug-in's id and range
    /// (`desc` from [`devices`]), before breakpoints are put on it.
    pub fn plugin_lane(&mut self, desc: &ParamDesc, plugin: &str) {
        if let AutoTarget::Plugin { slot, param } = desc.target {
            let i = self.plugin_index(slot, param, plugin, (desc.lo, desc.hi), desc.stepped);
            let l = &mut self.plugins[i];
            if l.points.is_empty() {
                l.plugin = plugin.to_string();
                (l.lo, l.hi, l.stepped) = (desc.lo, desc.hi, desc.stepped);
            }
        }
    }

    /// Every lane, with its breakpoints (some may have none).
    pub fn lanes(&self) -> impl Iterator<Item = (AutoTarget, &[AutoPoint])> {
        [
            (AutoTarget::Volume, &self.volume[..]),
            (AutoTarget::Pan, &self.pan[..]),
            (AutoTarget::Mute, &self.mute[..]),
        ]
        .into_iter()
        .chain(
            self.sends
                .iter()
                .map(|l| (AutoTarget::Send { slot: l.slot }, &l.points[..])),
        )
        .chain(self.fx.iter().map(|l| {
            (
                AutoTarget::Fx {
                    slot: l.slot,
                    param: l.param,
                },
                &l.points[..],
            )
        }))
        .chain(self.plugins.iter().map(|l| {
            (
                AutoTarget::Plugin {
                    slot: l.slot,
                    param: l.param,
                },
                &l.points[..],
            )
        }))
    }

    /// The targets that have breakpoints (Ableton: the automated parameters).
    pub fn automated(&self) -> Vec<AutoTarget> {
        self.lanes()
            .filter(|(_, p)| !p.is_empty())
            .map(|(t, _)| t)
            .collect()
    }

    /// Whether any lane has breakpoints.
    pub fn has_lane(&self) -> bool {
        self.lanes().any(|(_, pts)| !pts.is_empty())
    }

    /// Takes away the lanes of sends, effects and plug-ins that have no breakpoints.
    pub fn prune(&mut self) {
        self.sends.retain(|l| !l.points.is_empty());
        self.fx.retain(|l| !l.points.is_empty());
        self.plugins.retain(|l| !l.points.is_empty());
    }

    /// Whether the envelope of `target` rules its control now: not overridden, not held.
    pub fn follows(&self, target: AutoTarget) -> bool {
        !self.overridden.contains(&target)
            && self.held != Some(target)
            && !self.touched.iter().any(|(t, _)| *t == target)
    }

    /// The envelope's value at `beat`, if it rules the control and has breakpoints.
    pub fn played(&self, target: AutoTarget, beat: f64) -> Option<f32> {
        if !self.follows(target) {
            return None;
        }
        lane_value(self.lane(target), beat)
    }

    /// Whether `target` has an envelope that is overridden (its LED is grey).
    pub fn is_overridden(&self, target: AutoTarget) -> bool {
        self.overridden.contains(&target) && !self.lane(target).is_empty()
    }

    /// The range of a lane's values: dB for the fader, -1..=1 for the pan, positions 0..=1
    /// for the rest (Mute 0 or 1).
    pub fn range(target: AutoTarget) -> (f32, f32) {
        match target {
            AutoTarget::Volume => (-60.0, 6.0),
            AutoTarget::Pan => (-1.0, 1.0),
            _ => (0.0, 1.0),
        }
    }

    /// `value` made safe for `target`'s lane.
    fn tame(target: AutoTarget, value: f32) -> f32 {
        let (lo, hi) = Self::range(target);
        let v = value.clamp(lo, hi);
        if target == AutoTarget::Mute {
            v.round()
        } else {
            v
        }
    }

    /// Sets the breakpoint at `beat` (adds it, or moves the value of the one already there),
    /// keeping the lane in order.
    pub fn set_point(&mut self, target: AutoTarget, beat: f64, value: f32) {
        if !beat.is_finite() || !value.is_finite() || beat < 0.0 {
            return;
        }
        let value = Self::tame(target, value);
        let pts = self.lane_mut(target);
        let i = pts.partition_point(|p| p.beat < beat);
        if i < pts.len() && (pts[i].beat - beat).abs() < 1e-9 {
            pts[i].value = value;
        } else if pts.len() < Self::MAX_POINTS {
            pts.insert(i, AutoPoint::new(beat, value));
        }
        self.prune();
    }

    /// Records the control's value while playing: the breakpoints between this pass's last
    /// write and `beat` are replaced by the new one. Writes close together are skipped unless
    /// the value moved, or `force` (the end of a pass).
    pub fn write(&mut self, target: AutoTarget, beat: f64, value: f32, force: bool) {
        if !beat.is_finite() || !value.is_finite() || beat < 0.0 {
            return;
        }
        let k = match self.passes.iter().position(|p| p.target == target) {
            Some(k) => k,
            None => {
                self.passes.push(Pass {
                    target,
                    start: beat,
                    last: None,
                });
                self.passes.len() - 1
            }
        };
        let moved = match target {
            AutoTarget::Volume => 0.05,
            AutoTarget::Pan => 0.005,
            _ => 0.002,
        };
        let last = self.passes[k].last.filter(|(b, _)| beat >= *b);
        if let Some((lb, lv)) = last {
            if !force && beat - lb < WRITE_STEP && (value - lv).abs() < moved {
                return;
            }
            self.lane_mut(target)
                .retain(|p| p.beat <= lb || p.beat > beat);
        } else {
            // A pass that starts (or jumps back, at a cycle) replaces what was under it.
            self.lane_mut(target)
                .retain(|p| (p.beat - beat).abs() > 1e-9);
        }
        self.set_point(target, beat, value);
        if let Some(p) = self.passes.iter_mut().find(|p| p.target == target) {
            p.last = Some((beat, Self::tame(target, value)));
        }
    }

    /// Ends the recording pass of `target`: the breakpoints it wrote are thinned.
    pub fn end_pass(&mut self, target: AutoTarget) {
        let Some(k) = self.passes.iter().position(|p| p.target == target) else {
            return;
        };
        let pass = self.passes.remove(k);
        if let Some((end, _)) = pass.last {
            let tolerance = thin_tolerance(target);
            let pts = self.lane_mut(target);
            let a = pts.partition_point(|p| p.beat < pass.start);
            let b = pts.partition_point(|p| p.beat <= end);
            if b > a + 2 {
                let kept = thin(&pts[a..b], tolerance);
                pts.splice(a..b, kept);
            }
        }
        self.prune();
    }

    /// Ends every recording pass.
    pub fn end_passes(&mut self) {
        let targets: Vec<AutoTarget> = self.passes.iter().map(|p| p.target).collect();
        for t in targets {
            self.end_pass(t);
        }
    }

    /// Removes every breakpoint of every lane.
    pub fn clear(&mut self) {
        self.volume.clear();
        self.pan.clear();
        self.mute.clear();
        self.sends.clear();
        self.fx.clear();
        self.plugins.clear();
        self.passes.clear();
        self.overridden.clear();
    }

    /// Deletes the envelope of `target` (Ableton: Delete Automation).
    pub fn clear_lane(&mut self, target: AutoTarget) {
        self.lane_mut(target).clear();
        self.overridden.retain(|t| *t != target);
        self.passes.retain(|p| p.target != target);
        self.prune();
    }

    /// The effect in `slot` was taken away: its lanes go, those of later effects move up.
    pub fn insert_removed(&mut self, slot: usize) {
        self.fx.retain(|l| l.slot as usize != slot);
        for l in self.fx.iter_mut().filter(|l| l.slot as usize > slot) {
            l.slot -= 1;
        }
        self.retarget(|t| match t {
            AutoTarget::Fx { slot: s, .. } if s as usize == slot => None,
            AutoTarget::Fx { slot: s, param } if s as usize > slot => {
                Some(AutoTarget::Fx { slot: s - 1, param })
            }
            t => Some(t),
        });
    }

    /// The effects in slots `a` and `b` changed places.
    pub fn insert_swapped(&mut self, a: usize, b: usize) {
        for l in self.fx.iter_mut() {
            if l.slot as usize == a {
                l.slot = b as u8;
            } else if l.slot as usize == b {
                l.slot = a as u8;
            }
        }
        let swap = |s: u8| match s as usize {
            x if x == a => b as u8,
            x if x == b => a as u8,
            _ => s,
        };
        self.retarget(|t| match t {
            AutoTarget::Fx { slot, param } => Some(AutoTarget::Fx {
                slot: swap(slot),
                param,
            }),
            t => Some(t),
        });
    }

    /// Audio FX `k` (plug-in slot `k + 1`) was taken away: its lanes go, later ones move up.
    pub fn plugin_removed(&mut self, k: usize) {
        let slot = k + 1;
        self.plugins.retain(|l| l.slot as usize != slot);
        for l in self.plugins.iter_mut().filter(|l| l.slot as usize > slot) {
            l.slot -= 1;
        }
        self.retarget(|t| match t {
            AutoTarget::Plugin { slot: s, .. } if s as usize == slot => None,
            AutoTarget::Plugin { slot: s, param } if s as usize > slot => {
                Some(AutoTarget::Plugin { slot: s - 1, param })
            }
            t => Some(t),
        });
    }

    /// Audio FX `a` and `b` changed places.
    pub fn plugin_swapped(&mut self, a: usize, b: usize) {
        let (a, b) = (a as u8 + 1, b as u8 + 1);
        let swap = |s: u8| {
            if s == a {
                b
            } else if s == b {
                a
            } else {
                s
            }
        };
        for l in self.plugins.iter_mut() {
            l.slot = swap(l.slot);
        }
        self.retarget(|t| match t {
            AutoTarget::Plugin { slot, param } => Some(AutoTarget::Plugin {
                slot: swap(slot),
                param,
            }),
            t => Some(t),
        });
    }

    /// Send `k` was taken away: its lane goes, those of later sends move up.
    pub fn send_removed(&mut self, k: usize) {
        self.sends.retain(|l| l.slot as usize != k);
        for l in self.sends.iter_mut().filter(|l| l.slot as usize > k) {
            l.slot -= 1;
        }
        self.retarget(|t| match t {
            AutoTarget::Send { slot } if slot as usize == k => None,
            AutoTarget::Send { slot } if slot as usize > k => {
                Some(AutoTarget::Send { slot: slot - 1 })
            }
            t => Some(t),
        });
    }

    /// Moves the lane choices and the state kept about lanes as `f` says (`None`: gone).
    fn retarget(&mut self, f: impl Fn(AutoTarget) -> Option<AutoTarget>) {
        self.shown = f(self.shown).unwrap_or_default();
        self.extra = self.extra.iter().filter_map(|t| f(*t)).collect();
        self.overridden = self.overridden.iter().filter_map(|t| f(*t)).collect();
        self.held = self.held.and_then(&f);
        self.touched = self
            .touched
            .iter()
            .filter_map(|(t, at)| Some((f(*t)?, *at)))
            .collect();
        self.passes.clear();
    }
}

/// How far a thinned envelope may stray from what was recorded, in the lane's units.
fn thin_tolerance(target: AutoTarget) -> f32 {
    match target {
        AutoTarget::Volume => 0.25,
        AutoTarget::Pan => 0.01,
        AutoTarget::Mute => 0.0,
        _ => 0.004,
    }
}

/// The breakpoints of `pts` (in order, joined by straight lines) that are needed to keep the
/// envelope within `tolerance` of them all (Ramer-Douglas-Peucker on the value): the first
/// and the last are always kept, and every jump (two breakpoints at one beat).
pub fn thin(pts: &[AutoPoint], tolerance: f32) -> Vec<AutoPoint> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    for i in 1..pts.len() {
        if (pts[i].beat - pts[i - 1].beat).abs() < 1e-9 {
            keep[i] = true;
            keep[i - 1] = true;
        }
    }
    // Between every two breakpoints that must stay, keep the farthest from the line joining
    // them while it is too far, and look again on both sides.
    let fixed: Vec<usize> = (0..pts.len()).filter(|&i| keep[i]).collect();
    let mut stack: Vec<(usize, usize)> = fixed.windows(2).map(|w| (w[0], w[1])).collect();
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (pts[a], pts[b]);
        let span = pb.beat - pa.beat;
        let mut worst = (0.0f32, a);
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let line = if span > 0.0 {
                pa.value + (pb.value - pa.value) * ((p.beat - pa.beat) / span) as f32
            } else {
                pa.value
            };
            let d = (p.value - line).abs();
            if d > worst.0 {
                worst = (d, i);
            }
        }
        if worst.0 > tolerance {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    pts.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| AutoPoint { curve: 0.0, ..*p })
        .collect()
}

/// How a project file names a lane: "volume", "pan", "mute", "send:1", "fx:0:3",
/// "plugin:1:42".
pub fn target_key(t: AutoTarget) -> String {
    match t {
        AutoTarget::Volume => "volume".into(),
        AutoTarget::Pan => "pan".into(),
        AutoTarget::Mute => "mute".into(),
        AutoTarget::Send { slot } => format!("send:{slot}"),
        AutoTarget::Fx { slot, param } => format!("fx:{slot}:{param}"),
        AutoTarget::Plugin { slot, param } => format!("plugin:{slot}:{param}"),
    }
}

/// The lane a project file names (see [`target_key`]); `None` for anything else.
pub fn target_from_key(s: &str) -> Option<AutoTarget> {
    let mut it = s.split(':');
    let kind = it.next()?;
    let mut num = || it.next()?.parse::<u32>().ok();
    let t = match kind {
        "volume" => AutoTarget::Volume,
        "pan" => AutoTarget::Pan,
        "mute" => AutoTarget::Mute,
        "send" => AutoTarget::Send {
            slot: u8::try_from(num()?)
                .ok()
                .filter(|s| usize::from(*s) < prowave_engine::MAX_SENDS)?,
        },
        "fx" => AutoTarget::Fx {
            slot: u8::try_from(num()?).ok()?,
            param: u8::try_from(num()?).ok()?,
        },
        "plugin" => AutoTarget::Plugin {
            slot: u8::try_from(num()?).ok()?,
            param: num()?,
        },
        _ => return None,
    };
    Some(t)
}

// ------------------------------------------------------------------ strips and their controls

/// The automation of `strip` (none for a track that is not there).
pub fn auto_of(p: &Project, strip: Strip) -> Option<&Automation> {
    match strip {
        Strip::Track(i) => p.tracks.get(i).map(|t| &t.auto),
        Strip::Master => Some(&p.master_auto),
    }
}

pub fn auto_of_mut(p: &mut Project, strip: Strip) -> Option<&mut Automation> {
    match strip {
        Strip::Track(i) => p.tracks.get_mut(i).map(|t| &mut t.auto),
        Strip::Master => Some(&mut p.master_auto),
    }
}

/// The plug-in in slot `slot` of a track (0 the instrument, `k + 1` Audio FX `k`).
pub fn plugin_slot(t: &Track, slot: u8) -> Option<&crate::plugins::PluginSlot> {
    match slot {
        0 => t.au_instrument.as_ref(),
        k => t.au_effects.get(usize::from(k) - 1),
    }
}

/// The plug-in in slot `slot` of the Stereo Out (`k + 1`: its plug-in effect `k`; it has no
/// instrument).
pub fn master_plugin_slot(p: &Project, slot: u8) -> Option<&crate::plugins::PluginSlot> {
    usize::from(slot)
        .checked_sub(1)
        .and_then(|k| p.master_au_effects.get(k))
}

/// The plug-in in slot `slot` of `strip`.
pub fn strip_plugin_slot(
    p: &Project,
    strip: Strip,
    slot: u8,
) -> Option<&crate::plugins::PluginSlot> {
    match strip {
        Strip::Track(i) => p.tracks.get(i).and_then(|t| plugin_slot(t, slot)),
        Strip::Master => master_plugin_slot(p, slot),
    }
}

/// The strip of a mixer strip number (a track's index, or `mixer::MASTER_STRIP`).
pub fn strip_of(i: usize) -> Strip {
    if i == crate::mixer::MASTER_STRIP {
        Strip::Master
    } else {
        Strip::Track(i)
    }
}

/// The value of a control of a track in its lane's units, as the user set it (not what its
/// envelope says). `None` for a control that is not there, and for an Audio Unit's (the
/// plug-in holds those; see `LaneState::au_now`).
pub fn own_value(t: &Track, target: AutoTarget) -> Option<f32> {
    match target {
        AutoTarget::Volume => Some(t.volume_db),
        AutoTarget::Pan => Some(t.pan),
        AutoTarget::Mute => Some(if t.mute { 1.0 } else { 0.0 }),
        AutoTarget::Send { slot } => t.sends.get(usize::from(slot)).map(|s| s.level),
        AutoTarget::Fx { slot, param } => t.inserts.get(usize::from(slot)).and_then(|i| {
            let info = i.kind.params().get(usize::from(param))?;
            Some(info.to_norm(i.params[usize::from(param)]))
        }),
        AutoTarget::Plugin { slot, .. } => plugin_values(plugin_slot(t, slot)?, slot)
            .into_iter()
            .find(|(x, _)| *x == target)
            .map(|(_, v)| v),
    }
}

/// The Stereo Out's control in its lane's units: the fader in dB, an effect's parameter as a
/// position.
pub fn master_value(p: &Project, target: AutoTarget) -> Option<f32> {
    match target {
        AutoTarget::Volume => Some(crate::mixer::lin_to_db(p.master_vol).clamp(-60.0, 6.0)),
        AutoTarget::Fx { slot, param } => p.master_inserts.get(usize::from(slot)).and_then(|i| {
            let info = i.kind.params().get(usize::from(param))?;
            Some(info.to_norm(i.params[usize::from(param)]))
        }),
        AutoTarget::Plugin { slot, .. } => plugin_values(master_plugin_slot(p, slot)?, slot)
            .into_iter()
            .find(|(x, _)| *x == target)
            .map(|(_, v)| v),
        _ => None,
    }
}

/// Every parameter of a built-in plug-in (a native effect, the Channel Rack) with its
/// value as a lane position, from its settings (read once). Empty for an Audio Unit.
pub fn plugin_values(s: &crate::plugins::PluginSlot, slot: u8) -> Vec<(AutoTarget, f32)> {
    let descs = plugin_params(s, slot, None);
    if descs.is_empty() {
        return Vec::new();
    }
    let param = |d: &ParamDesc| match d.target {
        AutoTarget::Plugin { param, .. } => param,
        _ => u32::MAX,
    };
    let own: Box<dyn Fn(u32) -> Option<f32>> =
        if let Some(info) = prowave_engine::native_info(&s.id) {
            let vals = crate::native_fx_ui::values(s, info);
            Box::new(move |k| vals.get(k as usize).copied())
        } else if crate::channel_rack::is_rack(s) {
            let per = prowave_engine::RACK_PARAMS_PER_CHANNEL;
            let ch = crate::channel_rack::decode(&s.state);
            Box::new(move |k| {
                let c = ch.get((k / per) as usize)?;
                Some(match k % per {
                    0 => c.gain_db,
                    1 => c.pan,
                    _ => f32::from(u8::from(c.mute)),
                })
            })
        } else {
            instrument_values(s)
        };
    descs
        .iter()
        .filter_map(|d| Some((d.target, d.position_of(own(param(d))?))))
        .collect()
}

/// The values of a built-in instrument's parameters (none have any).
fn instrument_values(_: &crate::plugins::PluginSlot) -> Box<dyn Fn(u32) -> Option<f32>> {
    Box::new(|_| None)
}

/// Sets a control of a track from a value in its lane's units, without any automation state
/// changing (built-in plug-ins keep it in their settings; an Audio Unit is not reached here).
fn set_control(t: &mut Track, target: AutoTarget, value: f32) {
    if !value.is_finite() {
        return;
    }
    match target {
        AutoTarget::Volume => t.volume_db = value.clamp(-60.0, 6.0),
        AutoTarget::Pan => t.pan = value.clamp(-1.0, 1.0),
        AutoTarget::Mute => t.mute = value >= 0.5,
        AutoTarget::Send { slot } => {
            if let Some(s) = t.sends.get_mut(usize::from(slot)) {
                s.level = value.clamp(0.0, 1.0);
            }
        }
        AutoTarget::Fx { slot, param } => {
            if let Some(ins) = t.inserts.get_mut(usize::from(slot)) {
                if let Some(info) = ins.kind.params().get(usize::from(param)) {
                    ins.params[usize::from(param)] = value.clamp(info.min, info.max);
                }
            }
        }
        AutoTarget::Plugin { .. } => {}
    }
}

/// The user moved a control of the strip to `value` (dB for the fader, -1..1 for the pan, the
/// parameter's own value for an effect): it is held until [`released`] (it plays its own value,
/// and records when armed).
pub fn moved(t: &mut Track, target: AutoTarget, value: f32) {
    t.auto.held = Some(target);
    let v = match target {
        AutoTarget::Fx { slot, param } => t
            .inserts
            .get(usize::from(slot))
            .and_then(|i| i.kind.params().get(usize::from(param)))
            .map_or(value, |info| info.to_norm(value)),
        _ => value,
    };
    set_control(t, target, v);
}

/// The user let go of a control.
pub fn released(t: &mut Track, target: AutoTarget, _beat: f64, _playing: bool) {
    if t.auto.held == Some(target) {
        t.auto.held = None;
    }
}

/// Whether the user can move the control. Always, as in Live: moving an automated control
/// overrides its envelope (see [`tick`]).
pub fn editable(_t: &Track, _target: AutoTarget) -> bool {
    true
}

/// The value the control shows, in its lane's units: the envelope's at `beat` while it rules,
/// else the user's.
pub fn shown_value(t: &Track, target: AutoTarget, beat: f64) -> f32 {
    t.auto
        .played(target, beat)
        .or_else(|| own_value(t, target))
        .unwrap_or(0.0)
}

// ------------------------------------------------------------------ what can be automated

/// One automatable parameter, as the parameter chooser lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDesc {
    pub target: AutoTarget,
    pub name: String,
    /// The parameter's own values at lane positions 0 and 1 (Volume, Pan: the lane's range).
    pub lo: f32,
    pub hi: f32,
    pub stepped: bool,
    /// The unit of its value text.
    pub unit: String,
    /// Names for the whole values of a choice.
    pub choices: Vec<String>,
}

impl ParamDesc {
    fn plugin(slot: u8, param: u32, name: String, lo: f32, hi: f32, stepped: bool) -> Self {
        Self {
            target: AutoTarget::Plugin { slot, param },
            name,
            lo,
            hi,
            stepped,
            unit: String::new(),
            choices: Vec::new(),
        }
    }

    /// The position 0..=1 of a value in the parameter's own units.
    pub fn position_of(&self, v: f32) -> f32 {
        if self.hi > self.lo {
            ((v - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The parameter's own value at position `n`.
    pub fn value_at(&self, n: f32) -> f32 {
        let x = self.lo + (self.hi - self.lo) * n.clamp(0.0, 1.0);
        if self.stepped {
            x.round()
        } else {
            x
        }
    }
}

/// A device in the device chooser: the mixer, an effect or a plug-in, with its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub params: Vec<ParamDesc>,
}

/// The parameters of the plug-in in `slot`: a built-in one's from its own table, an Audio
/// Unit's from what the running instance listed (`au`).
pub fn plugin_params(
    s: &crate::plugins::PluginSlot,
    slot: u8,
    au: Option<&HashMap<u64, Vec<AuParamInfo>>>,
) -> Vec<ParamDesc> {
    if let Some(info) = prowave_engine::native_info(&s.id) {
        return info
            .params
            .iter()
            .enumerate()
            .map(|(k, n)| {
                let mut d =
                    ParamDesc::plugin(slot, k as u32, n.name.to_string(), n.min, n.max, n.integer);
                d.unit = n.unit.to_string();
                d.choices = n.choices.iter().map(|c| c.to_string()).collect();
                d
            })
            .collect();
    }
    if crate::channel_rack::is_rack(s) {
        let per = prowave_engine::RACK_PARAMS_PER_CHANNEL;
        return crate::channel_rack::decode(&s.state)
            .iter()
            .enumerate()
            .flat_map(|(k, c)| {
                let base = k as u32 * per;
                let name = if c.name.is_empty() {
                    trf("Channel {}", &[&(k + 1)])
                } else {
                    c.name.clone()
                };
                let mut vol = ParamDesc::plugin(
                    slot,
                    base,
                    format!("{name} · {}", tr("Volume")),
                    -96.0,
                    12.0,
                    false,
                );
                vol.unit = "dB".into();
                [
                    vol,
                    ParamDesc::plugin(
                        slot,
                        base + 1,
                        format!("{name} · {}", tr("Pan")),
                        -1.0,
                        1.0,
                        false,
                    ),
                    ParamDesc::plugin(
                        slot,
                        base + 2,
                        format!("{name} · {}", tr("Mute")),
                        0.0,
                        1.0,
                        true,
                    ),
                ]
            })
            .collect();
    }
    au.and_then(|m| m.get(&s.uid))
        .map(|list| {
            list.iter()
                .map(|a| ParamDesc::plugin(slot, a.id, a.name.clone(), a.min, a.max, a.stepped))
                .collect()
        })
        .unwrap_or_default()
}

fn insert_params(slot: usize, ins: &crate::model::Insert) -> Vec<ParamDesc> {
    ins.kind
        .params()
        .iter()
        .enumerate()
        .map(|(k, info)| ParamDesc {
            target: AutoTarget::Fx {
                slot: slot as u8,
                param: k as u8,
            },
            name: info.name.to_string(),
            lo: info.min,
            hi: info.max,
            stepped: false,
            unit: info.unit.to_string(),
            choices: Vec::new(),
        })
        .collect()
}

fn mixer_param(target: AutoTarget, name: String, unit: &str) -> ParamDesc {
    let (lo, hi) = Automation::range(target);
    ParamDesc {
        target,
        name,
        lo,
        hi,
        stepped: target == AutoTarget::Mute,
        unit: unit.into(),
        choices: Vec::new(),
    }
}

/// Everything `strip` can automate, by device: the mixer first (Volume, Pan, Mute, the
/// sends), then the effects, the instrument and the Audio FX (Ableton: the device chooser).
pub fn devices(p: &Project, strip: Strip, au: &AuParams) -> Vec<Device> {
    let mut out = Vec::new();
    match strip {
        Strip::Master => {
            out.push(Device {
                name: tr("Mixer").to_string(),
                params: vec![mixer_param(
                    AutoTarget::Volume,
                    tr("Volume").to_string(),
                    "dB",
                )],
            });
            for (k, ins) in p.master_inserts.iter().enumerate() {
                out.push(Device {
                    name: ins.name().to_string(),
                    params: insert_params(k, ins),
                });
            }
            plugin_devices(
                &mut out,
                p.master_au_effects
                    .iter()
                    .enumerate()
                    .map(|(k, s)| (k as u8 + 1, s)),
                au,
            );
        }
        Strip::Track(i) => {
            let Some(t) = p.tracks.get(i) else {
                return out;
            };
            let mut mixer = vec![
                mixer_param(AutoTarget::Volume, tr("Volume").to_string(), "dB"),
                mixer_param(AutoTarget::Pan, tr("Pan").to_string(), ""),
                mixer_param(AutoTarget::Mute, tr("Mute").to_string(), ""),
            ];
            for (k, s) in t.sends.iter().enumerate() {
                if let Some(bus) = s.bus {
                    mixer.push(mixer_param(
                        AutoTarget::Send { slot: k as u8 },
                        trf("Send to Bus {}", &[&bus]),
                        "",
                    ));
                }
            }
            out.push(Device {
                name: tr("Mixer").to_string(),
                params: mixer,
            });
            for (k, ins) in t.inserts.iter().enumerate() {
                out.push(Device {
                    name: ins.name().to_string(),
                    params: insert_params(k, ins),
                });
            }
            let slots = t.au_instrument.iter().map(|s| (0u8, s)).chain(
                t.au_effects
                    .iter()
                    .enumerate()
                    .map(|(k, s)| (k as u8 + 1, s)),
            );
            plugin_devices(&mut out, slots, au);
        }
    }
    out
}

/// The plug-ins in `slots` (with their slot numbers) as devices, those with parameters.
fn plugin_devices<'a>(
    out: &mut Vec<Device>,
    slots: impl Iterator<Item = (u8, &'a crate::plugins::PluginSlot)>,
    au: &AuParams,
) {
    for (slot, s) in slots {
        let params = plugin_params(s, slot, Some(&au.0));
        if !params.is_empty() {
            out.push(Device {
                name: prowave_engine::native_info(&s.id)
                    .map_or_else(|| s.name.clone(), |i| i.name.to_string()),
                params,
            });
        }
    }
}

/// The description of `target` on `strip`, if the strip has it.
pub fn describe(p: &Project, strip: Strip, target: AutoTarget, au: &AuParams) -> Option<ParamDesc> {
    devices(p, strip, au)
        .into_iter()
        .flat_map(|d| d.params)
        .find(|d| d.target == target)
}

/// The text of a lane value: "-3.2 dB", "L 40" / "C" / "R 40" for the pan, "On"/"Off" for
/// Mute, a parameter in its own units.
pub fn value_text(desc: &ParamDesc, v: f32) -> String {
    match desc.target {
        AutoTarget::Volume => format!("{v:.1} dB"),
        AutoTarget::Pan => {
            let n = (v * 64.0).round() as i32;
            match n {
                0 => "C".into(),
                n if n < 0 => format!("L {}", -n),
                n => format!("R {n}"),
            }
        }
        AutoTarget::Mute => {
            if v >= 0.5 {
                tr("Muted").to_string()
            } else {
                tr("Playing").to_string()
            }
        }
        AutoTarget::Send { .. } => {
            format!(
                "{:.1} dB",
                crate::mixer::lin_to_db(prowave_engine::send_gain(v))
            )
        }
        _ => {
            let x = desc.value_at(v);
            if let Some(c) = desc.choices.get(x.round().max(0.0) as usize) {
                return c.clone();
            }
            if desc.stepped && desc.hi - desc.lo <= 1.0 {
                return if x >= 0.5 { tr("On") } else { tr("Off") }.to_string();
            }
            if desc.unit.is_empty() {
                if desc.hi <= 1.0 && desc.lo >= 0.0 {
                    format!("{:.0} %", x * 100.0)
                } else {
                    format!("{x:.2}")
                }
            } else if x.abs() >= 100.0 {
                format!("{x:.0} {}", desc.unit)
            } else {
                format!("{x:.1} {}", desc.unit)
            }
        }
    }
}

/// 0 (bottom of the lane) to 1 (top) for a lane value.
pub fn frac_of(target: AutoTarget, v: f32) -> f32 {
    match target {
        AutoTarget::Volume => db_to_frac(v),
        AutoTarget::Pan => (v.clamp(-1.0, 1.0) + 1.0) / 2.0,
        _ => v.clamp(0.0, 1.0),
    }
}

/// The lane value at height `f` (0 bottom, 1 top).
pub fn value_of(target: AutoTarget, f: f32) -> f32 {
    let (lo, hi) = Automation::range(target);
    match target {
        AutoTarget::Volume => frac_to_db(f).clamp(lo, hi),
        AutoTarget::Pan => (f.clamp(0.0, 1.0) * 2.0 - 1.0).clamp(lo, hi),
        AutoTarget::Mute => f.clamp(0.0, 1.0).round(),
        _ => f.clamp(lo, hi),
    }
}

// ------------------------------------------------------------------ the frame's work

/// The parameters of the running Audio Units, by slot uid (listed when they are opened).
#[derive(Default, Clone, Debug)]
pub struct AuParams(pub HashMap<u64, Vec<AuParamInfo>>);

/// One parameter of a running Audio Unit (what `AuHandle::parameters` lists, on any platform).
#[derive(Clone, Debug, PartialEq)]
pub struct AuParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub stepped: bool,
}

/// The automation state of the interface (not saved).
pub struct AutoView {
    /// Automation Mode (A): the Tracks area shows the envelopes.
    pub mode: bool,
    /// Draw Mode (B): dragging on a lane draws the envelope.
    pub draw: bool,
    /// Automation Arm: moving controls while recording writes automation.
    pub arm: bool,
    /// Envelopes move (and are copied) with the regions over them (Live: Lock Envelopes off).
    pub with_regions: bool,
    pub au: AuParams,
    /// The last value seen of every automatable control the model holds, to tell when the user
    /// moved one (by any widget).
    seen: HashMap<(Strip, AutoTarget), f32>,
    seen_tracks: usize,
    /// The tracks whose automated lanes were laid out when Automation Mode first showed them.
    pub laid_out: Vec<usize>,
    /// The editing state of the lanes (`automation_ui.rs`).
    pub lanes: crate::automation_ui::LaneState,
}

impl Default for AutoView {
    fn default() -> Self {
        Self {
            mode: false,
            draw: false,
            arm: false,
            with_regions: true,
            au: AuParams::default(),
            seen: HashMap::new(),
            seen_tracks: 0,
            laid_out: Vec::new(),
            lanes: Default::default(),
        }
    }
}

impl AutoView {
    /// The project was replaced (opened, undone): what was seen of it does not count as a move.
    pub fn forget(&mut self) {
        self.seen.clear();
        self.laid_out.clear();
        self.lanes = Default::default();
    }
}

/// Whether any envelope of the project is overridden (Re-Enable Automation lights up).
pub fn any_overridden(p: &Project) -> bool {
    p.tracks
        .iter()
        .map(|t| &t.auto)
        .chain(std::iter::once(&p.master_auto))
        .any(|a| a.overridden.iter().any(|t| !a.lane(*t).is_empty()))
}

/// Re-Enable Automation: every overridden envelope rules its control again.
pub fn reenable(p: &mut Project) {
    for t in p.tracks.iter_mut() {
        t.auto.overridden.clear();
    }
    p.master_auto.overridden.clear();
}

/// The controls of every strip, in their lanes' units, as the user set them.
fn controls(p: &Project) -> Vec<((Strip, AutoTarget), f32)> {
    let mut out = Vec::new();
    for (i, t) in p.tracks.iter().enumerate() {
        let s = Strip::Track(i);
        for target in [AutoTarget::Volume, AutoTarget::Pan, AutoTarget::Mute] {
            if let Some(v) = own_value(t, target) {
                out.push(((s, target), v));
            }
        }
        for (k, send) in t.sends.iter().enumerate() {
            out.push(((s, AutoTarget::Send { slot: k as u8 }), send.level));
        }
        for (k, ins) in t.inserts.iter().enumerate() {
            for (n, info) in ins.kind.params().iter().enumerate() {
                let target = AutoTarget::Fx {
                    slot: k as u8,
                    param: n as u8,
                };
                out.push(((s, target), info.to_norm(ins.params[n])));
            }
        }
        let slots = t.au_instrument.iter().map(|x| (0u8, x)).chain(
            t.au_effects
                .iter()
                .enumerate()
                .map(|(k, x)| (k as u8 + 1, x)),
        );
        for (slot, x) in slots {
            for (target, v) in plugin_values(x, slot) {
                out.push(((s, target), v));
            }
        }
    }
    if let Some(v) = master_value(p, AutoTarget::Volume) {
        out.push(((Strip::Master, AutoTarget::Volume), v));
    }
    for (k, ins) in p.master_inserts.iter().enumerate() {
        for (n, info) in ins.kind.params().iter().enumerate() {
            let target = AutoTarget::Fx {
                slot: k as u8,
                param: n as u8,
            };
            out.push(((Strip::Master, target), info.to_norm(ins.params[n])));
        }
    }
    for (k, x) in p.master_au_effects.iter().enumerate() {
        for (target, v) in plugin_values(x, k as u8 + 1) {
            out.push(((Strip::Master, target), v));
        }
    }
    out
}

/// The user moved `target` of `strip` to `value` (lane units), by hand. Live: the chooser
/// shows it; armed and recording it is written; otherwise an automated control is overridden.
pub fn touch(
    p: &mut Project,
    v: &AutoView,
    strip: Strip,
    target: AutoTarget,
    value: f32,
    now: f64,
) {
    let recording = v.arm && p.recording && p.playing;
    let beat = p.position_beats;
    let desc = if let (true, AutoTarget::Plugin { slot, .. }) = (recording, target) {
        describe(p, strip, target, &v.au)
            .zip(strip_plugin_slot(p, strip, slot).map(|s| s.id.clone()))
    } else {
        None
    };
    let Some(a) = auto_of_mut(p, strip) else {
        return;
    };
    // Lanes laid out under the track keep their place; the track's own lane shows it.
    if !a.extra.contains(&target) {
        a.shown = target;
    }
    if recording {
        match a.touched.iter_mut().find(|(t, _)| *t == target) {
            Some(x) => x.1 = now,
            None => a.touched.push((target, now)),
        }
        a.overridden.retain(|t| *t != target);
        if let Some((d, id)) = desc {
            a.plugin_lane(&d, &id);
        }
        a.write(target, beat, value, false);
    } else if !a.lane(target).is_empty() && !a.overridden.contains(&target) {
        a.overridden.push(target);
    }
}

/// An Audio Unit's parameter was moved in its own window (see `AuParamWatch`): the plug-in in
/// `slot` of strip `track` (a track, or `mixer::MASTER_STRIP`), parameter `param`, to `value`
/// in the parameter's own units.
pub fn plugin_touched(
    p: &mut Project,
    v: &mut AutoView,
    track: usize,
    slot: u8,
    param: u32,
    value: f32,
    now: f64,
) {
    let target = AutoTarget::Plugin { slot, param };
    let strip = strip_of(track);
    let Some(desc) = describe(p, strip, target, &v.au) else {
        return;
    };
    let norm = desc.position_of(value);
    // An Audio Unit that reports the automation's own moves back is not a hand on a knob.
    if let Some(played) = auto_of(p, strip).and_then(|a| a.played(target, p.position_beats)) {
        if (played - norm).abs() < 1e-3 {
            return;
        }
    }
    v.lanes.au_now.insert((track, slot, param), norm);
    touch(p, v, strip, target, norm, now);
}

/// Once a frame, after the playhead was read from the engine: finds the controls the user
/// moved since the last frame (with any widget) and overrides or records them; ends the
/// touches and recording passes that are over.
pub fn tick(p: &mut Project, v: &mut AutoView, now: f64) {
    let now_controls = controls(p);
    if v.seen_tracks == p.tracks.len() {
        for (key, value) in &now_controls {
            if let Some(old) = v.seen.get(key) {
                if (old - value).abs() > 1e-6 {
                    touch(p, v, key.0, key.1, *value, now);
                }
            }
        }
    }
    v.seen = now_controls.iter().copied().collect();
    v.seen_tracks = p.tracks.len();

    let recording = v.arm && p.recording && p.playing;
    let beat = p.position_beats;
    let strips: Vec<Strip> = (0..p.tracks.len())
        .map(Strip::Track)
        .chain(std::iter::once(Strip::Master))
        .collect();
    for strip in strips {
        let held: Vec<AutoTarget> = {
            let Some(a) = auto_of_mut(p, strip) else {
                continue;
            };
            // A touch is over when its control has been still for a moment (or at once when
            // the transport stops); a widget that says when it is let go ends it itself.
            let mut over = Vec::new();
            a.touched.retain(|(t, at)| {
                let done = now - *at > HOLD_SECS || !recording;
                if done {
                    over.push(*t);
                }
                !done
            });
            if !recording {
                a.end_passes();
            } else {
                for t in over {
                    if a.held != Some(t) {
                        a.end_pass(t);
                    }
                }
                let still: Vec<AutoTarget> = a.passes.iter().map(|x| x.target).collect();
                for t in still {
                    if a.held != Some(t) && !a.touched.iter().any(|(x, _)| *x == t) {
                        a.end_pass(t);
                    }
                }
            }
            if !recording {
                Vec::new()
            } else {
                a.touched.iter().map(|(t, _)| *t).chain(a.held).collect()
            }
        };
        // Held controls keep writing their value while the playhead moves on.
        for target in held {
            let value = match strip {
                Strip::Track(i) => p.tracks.get(i).and_then(|t| own_value(t, target)),
                Strip::Master => master_value(p, target),
            }
            .or_else(|| match (strip, target) {
                (Strip::Track(i), AutoTarget::Plugin { slot, param }) => {
                    v.lanes.au_now.get(&(i, slot, param)).copied()
                }
                (Strip::Master, AutoTarget::Plugin { slot, param }) => v
                    .lanes
                    .au_now
                    .get(&(crate::mixer::MASTER_STRIP, slot, param))
                    .copied(),
                _ => None,
            });
            if let (Some(value), Some(a)) = (value, auto_of_mut(p, strip)) {
                a.write(target, beat, value, false);
            }
        }
    }
}

/// The plug-in slot (on the engine's side: running plug-ins only, instrument first) and the
/// lane's range for a plug-in lane that may be played, or `None` when its slot holds another
/// plug-in now, is bypassed, or is empty.
pub fn engine_plugin_slot(t: &Track, lane: &PluginLane) -> Option<u8> {
    engine_slot_in(t.au_instrument.as_ref(), &t.au_effects, lane)
}

/// [`engine_plugin_slot`] for the Stereo Out's plug-ins (it has no instrument).
pub fn engine_master_plugin_slot(p: &Project, lane: &PluginLane) -> Option<u8> {
    engine_slot_in(None, &p.master_au_effects, lane)
}

fn engine_slot_in(
    instrument: Option<&crate::plugins::PluginSlot>,
    effects: &[crate::plugins::PluginSlot],
    lane: &PluginLane,
) -> Option<u8> {
    let s = match lane.slot {
        0 => instrument,
        k => effects.get(usize::from(k) - 1),
    }?;
    if !lane.plugin.is_empty() && s.id != lane.plugin {
        return None;
    }
    if lane.slot == 0 {
        return Some(0);
    }
    if s.bypass {
        return None;
    }
    let k = usize::from(lane.slot) - 1;
    let running = effects[..k].iter().filter(|e| !e.bypass).count();
    Some(running as u8 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prowave_engine::FxKind;

    fn project() -> Project {
        Project::empty()
    }

    #[test]
    fn points_stay_sorted_in_range_and_bounded() {
        let mut a = Automation::default();
        a.set_point(AutoTarget::Volume, 4.0, 3.0);
        a.set_point(AutoTarget::Volume, 2.0, -500.0);
        a.set_point(AutoTarget::Volume, 4.0, -3.0);
        a.set_point(AutoTarget::Volume, f64::NAN, 0.0);
        a.set_point(AutoTarget::Pan, 1.0, 7.0);
        a.set_point(AutoTarget::Mute, 1.0, 0.7);
        assert_eq!(
            a.volume,
            [AutoPoint::new(2.0, -60.0), AutoPoint::new(4.0, -3.0)]
        );
        assert_eq!(a.pan, [AutoPoint::new(1.0, 1.0)]);
        assert_eq!(a.mute, [AutoPoint::new(1.0, 1.0)]);
        for target in [AutoTarget::Volume, AutoTarget::Pan] {
            let (lo, hi) = Automation::range(target);
            assert!((value_of(target, frac_of(target, lo)) - lo).abs() < 0.5);
            assert!((value_of(target, frac_of(target, hi)) - hi).abs() < 0.5);
        }
    }

    #[test]
    fn an_envelope_overrides_and_comes_back_with_reenable() {
        let mut p = project();
        let mut v = AutoView::default();
        p.tracks[0].auto.set_point(AutoTarget::Volume, 0.0, -12.0);
        tick(&mut p, &mut v, 0.0);
        assert_eq!(shown_value(&p.tracks[0], AutoTarget::Volume, 1.0), -12.0);
        assert!(p.tracks[0].auto.follows(AutoTarget::Volume));
        // The user drags the fader (any widget): the envelope is overridden.
        p.tracks[0].volume_db = -3.0;
        tick(&mut p, &mut v, 0.1);
        let a = &p.tracks[0].auto;
        assert!(a.is_overridden(AutoTarget::Volume) && !a.follows(AutoTarget::Volume));
        assert_eq!(shown_value(&p.tracks[0], AutoTarget::Volume, 1.0), -3.0);
        assert!(any_overridden(&p));
        // A control without an envelope is not overridden; the chooser follows the hand.
        p.tracks[0].pan = 0.5;
        tick(&mut p, &mut v, 0.2);
        assert!(!p.tracks[0].auto.is_overridden(AutoTarget::Pan));
        assert_eq!(p.tracks[0].auto.shown, AutoTarget::Pan);
        reenable(&mut p);
        assert!(!any_overridden(&p));
        assert_eq!(shown_value(&p.tracks[0], AutoTarget::Volume, 1.0), -12.0);
        // A project opened or undone is not a move.
        v.forget();
        p.tracks[0].volume_db = 2.0;
        tick(&mut p, &mut v, 0.3);
        assert!(!p.tracks[0].auto.is_overridden(AutoTarget::Volume));
    }

    #[test]
    fn armed_recording_writes_breakpoints_and_thins_them_when_let_go() {
        let mut p = project();
        let mut v = AutoView {
            arm: true,
            ..AutoView::default()
        };
        p.recording = true;
        p.playing = true;
        tick(&mut p, &mut v, 0.0);
        // A fader ramp from 0 to -10 dB over two beats, moved a little every frame, then held
        // still: a straight line needs only its ends.
        let mut now = 0.0;
        for k in 1..=40 {
            p.position_beats = k as f64 * 0.05;
            p.tracks[0].volume_db = -(k as f32) * 0.25;
            now += 1.0 / 60.0;
            tick(&mut p, &mut v, now);
            assert!(
                !p.tracks[0].auto.follows(AutoTarget::Volume),
                "held while moving"
            );
        }
        assert!(p.tracks[0].auto.volume.len() > 10, "written as it moved");
        // Still for a moment: the touch is over and the pass is thinned.
        now += 1.0;
        p.position_beats = 2.5;
        tick(&mut p, &mut v, now);
        let a = &p.tracks[0].auto;
        assert!(a.follows(AutoTarget::Volume) && a.passes.is_empty());
        assert!(a.volume.len() <= 3, "{:?}", a.volume);
        assert_eq!(
            (a.volume[0].beat, a.volume[0].value),
            (0.05, -0.25),
            "from the first move"
        );
        assert_eq!(lane_value(&a.volume, 2.0), Some(-10.0));
        // Not armed: moving it overrides instead.
        v.arm = false;
        p.tracks[0].volume_db = -1.0;
        tick(&mut p, &mut v, now + 0.1);
        assert!(p.tracks[0].auto.is_overridden(AutoTarget::Volume));
    }

    #[test]
    fn thinning_keeps_the_shape_and_its_jumps() {
        let line: Vec<AutoPoint> = (0..=10)
            .map(|k| AutoPoint::new(k as f64, k as f32))
            .collect();
        assert_eq!(thin(&line, 0.01).len(), 2);
        let mut bend = line.clone();
        bend[5].value = 9.0;
        let t = thin(&bend, 0.01);
        assert!(t.iter().any(|p| p.beat == 5.0 && p.value == 9.0), "{t:?}");
        // A jump (two breakpoints at one beat) stays a jump.
        let jump = vec![
            AutoPoint::new(0.0, 0.0),
            AutoPoint::new(1.0, 0.0),
            AutoPoint::new(2.0, 0.0),
            AutoPoint::new(2.0, 1.0),
            AutoPoint::new(3.0, 1.0),
            AutoPoint::new(4.0, 1.0),
        ];
        let t = thin(&jump, 0.01);
        assert_eq!(lane_value(&t, 1.99), Some(0.0));
        assert_eq!(lane_value(&t, 2.01), Some(1.0));
        assert_eq!(t.len(), 4, "{t:?}");
    }

    #[test]
    fn effect_lanes_follow_their_effects_and_sends_and_plugins_too() {
        let mut p = project();
        let t = &mut p.tracks[0];
        t.inserts = vec![
            crate::model::Insert::new(FxKind::Compressor),
            crate::model::Insert::new(FxKind::ChannelEq),
        ];
        let eq = AutoTarget::Fx { slot: 1, param: 0 };
        t.auto.set_point(eq, 1.0, 0.25);
        t.auto.shown = eq;
        t.auto.insert_swapped(0, 1);
        assert_eq!(t.auto.lane(AutoTarget::Fx { slot: 0, param: 0 }).len(), 1);
        assert_eq!(t.auto.shown, AutoTarget::Fx { slot: 0, param: 0 });
        t.auto.insert_removed(0);
        assert!(!t.auto.has_lane());
        assert_eq!(t.auto.shown, AutoTarget::Volume);

        t.auto.set_point(AutoTarget::Send { slot: 1 }, 0.0, 0.5);
        t.auto.send_removed(0);
        assert_eq!(t.auto.lane(AutoTarget::Send { slot: 0 }).len(), 1);

        let a = AutoTarget::Plugin { slot: 1, param: 3 };
        let b = AutoTarget::Plugin { slot: 2, param: 3 };
        t.auto.set_point(a, 0.0, 0.1);
        t.auto.set_point(b, 0.0, 0.9);
        t.auto.plugin_swapped(0, 1);
        assert_eq!(t.auto.lane(a)[0].value, 0.9);
        t.auto.plugin_removed(0);
        assert_eq!(t.auto.lane(a)[0].value, 0.1);
        assert!(t.auto.lane(b).is_empty());
    }

    #[test]
    fn native_plugin_values_are_read_from_their_settings() {
        let mut p = project();
        let info = &prowave_engine::native_effects()[0];
        p.tracks[0].au_effects = vec![
            crate::native_fx_ui::new_slot(info),
            crate::native_fx_ui::new_slot(info),
        ];
        p.tracks[0].au_effects[0].bypass = true;
        let target = AutoTarget::Plugin { slot: 2, param: 0 };
        let desc = describe(&p, Strip::Track(0), target, &AuParams::default());
        let desc = desc.unwrap_or_else(|| panic!("listed"));
        let own = own_value(&p.tracks[0], target);
        assert_eq!(own, Some(desc.position_of(info.params[0].default)));
        let mut vals = crate::native_fx_ui::values(&p.tracks[0].au_effects[1], info);
        vals[0] = info.params[0].max;
        p.tracks[0].au_effects[1].state = Some(crate::native_fx_ui::encode(&vals));
        assert_eq!(own_value(&p.tracks[0], target), Some(1.0));
        // The engine runs the second effect first (the first is bypassed).
        let mut lane = PluginLane {
            slot: 2,
            param: 0,
            plugin: info.id.to_string(),
            ..PluginLane::default()
        };
        assert_eq!(engine_plugin_slot(&p.tracks[0], &lane), Some(1));
        lane.plugin = "aufx:other:Mfr ".into();
        assert_eq!(engine_plugin_slot(&p.tracks[0], &lane), None);
    }

    #[test]
    fn stereo_out_plugins_are_automated_overridden_and_recorded() {
        let mut p = project();
        let info = &prowave_engine::native_effects()[0];
        p.master_au_effects = vec![crate::native_fx_ui::new_slot(info)];
        let mut v = AutoView::default();
        let target = AutoTarget::Plugin { slot: 1, param: 0 };
        // Listed in the Stereo Out's chooser, with its value from its settings.
        let desc = describe(&p, Strip::Master, target, &v.au).unwrap_or_else(|| panic!("listed"));
        assert_eq!(
            master_value(&p, target),
            Some(desc.position_of(info.params[0].default))
        );
        p.master_auto.plugin_lane(&desc, info.id);
        p.master_auto.set_point(target, 0.0, 0.2);
        tick(&mut p, &mut v, 0.0);
        // Its knob turned by hand (its window changes its settings): the envelope is overridden.
        let mut vals = crate::native_fx_ui::values(&p.master_au_effects[0], info);
        vals[0] = info.params[0].max;
        p.master_au_effects[0].state = Some(crate::native_fx_ui::encode(&vals));
        tick(&mut p, &mut v, 0.1);
        assert!(p.master_auto.is_overridden(target));
        reenable(&mut p);
        // An Audio Unit's window on the Stereo Out, with Automation Arm on while recording,
        // writes breakpoints.
        v.arm = true;
        p.recording = true;
        p.playing = true;
        p.position_beats = 8.0;
        plugin_touched(
            &mut p,
            &mut v,
            crate::mixer::MASTER_STRIP,
            1,
            0,
            desc.value_at(0.75),
            1.0,
        );
        let pts = p.master_auto.lane(target);
        assert!(
            pts.iter()
                .any(|x| x.beat == 8.0 && (x.value - 0.75).abs() < 1e-4),
            "{pts:?}"
        );
        assert_eq!(p.master_auto.shown, target);
    }
}
