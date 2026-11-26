//! Keep-or-remove ratings of the instruments and effects, and their type, for the Instrument
//! and Audio FX menus.
//!
//! A plug-in that emulates a piece of hardware (or a classic instrument or chip) is listed
//! under that thing's proper name ("Roland TB-303"), its own name ("JC303") in small print
//! where the maker otherwise goes; the file's `emulates` says which. Its `master_chain` puts a
//! mastering tool in the Audio FX menu's first folder, Master Chain, as well as in its type's.
//!
//! The settings file `plugin-ratings.json` (written by the owner's plug-in catalog tool,
//! `tools/rate.py` there) holds, per `plugin.list` id, a rating from 1 to 5 in halves, why,
//! and the plug-in's type ("Reverb", "Synth", ...). The menus group every instrument or effect,
//! Pro Wave's own included, into one submenu per type (alphabetical, "Other" last), list the
//! best rated first and mark each with a chip coloured by its rating. The file is read again when it changes
//! (its time is looked at at most once a second).

use egui::Color32;
use std::collections::HashMap;
use std::sync::Arc;

/// What the file says about one plug-in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rated {
    /// 1 to 5 in halves; `None`: not rated.
    pub rating: Option<f32>,
    pub why: String,
    /// The type folder it goes in ("" when the file gives none).
    pub kind: String,
    /// The proper name of what it emulates ("Roland TB-303"); "" when nothing.
    pub emulates: String,
    /// The plug-in's own proper name and maker as the catalog has them ("" when not given).
    pub name: String,
    pub maker: String,
    /// A mastering tool: also listed in the Master Chain folder.
    pub master_chain: bool,
}

/// The whole file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ratings {
    by_id: HashMap<String, Rated>,
}

impl Ratings {
    /// Reads `{"plugins": {id: {rating, why, type}}}` (a bad file gives no ratings).
    pub fn parse(text: &str) -> Self {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
            return Self::default();
        };
        let Some(map) = v.get("plugins").and_then(|p| p.as_object()) else {
            return Self::default();
        };
        let by_id = map
            .iter()
            .map(|(id, e)| {
                let s = |k: &str| {
                    e.get(k)
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string()
                };
                let rated = Rated {
                    rating: e
                        .get("rating")
                        .and_then(serde_json::Value::as_f64)
                        .map(|r| r.clamp(1.0, 5.0) as f32),
                    why: s("why"),
                    kind: s("type"),
                    emulates: s("emulates"),
                    name: s("name"),
                    maker: s("maker"),
                    master_chain: e
                        .get("master_chain")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                };
                (id.clone(), rated)
            })
            .collect();
        Self { by_id }
    }

    /// What the file says about the plug-in `id`, or its other format `alt`.
    pub fn of(&self, id: &str, alt: Option<&str>) -> Option<&Rated> {
        self.by_id
            .get(id)
            .or_else(|| alt.and_then(|a| self.by_id.get(a)))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// No plug-in in the file (or no file).
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// The colour of a rating's chip: nine steps, one per half, from the theme's raised neutral
/// (1: a low rating fades into the menu) to its accent orange (5), interpolated in OKLab.
/// On the light theme that is #f4f1ea to #ff5a14; [`chip_text`] picks the print.
pub fn color(rating: f32) -> Color32 {
    let t = (((rating.clamp(1.0, 5.0) - 1.0) * 2.0).round() / 8.0).clamp(0.0, 1.0);
    let (a, b) = (
        oklab(crate::theme::BONE_HI()),
        oklab(crate::theme::ORANGE()),
    );
    from_oklab([0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t))
}

/// The print on a chip of colour `fill`: dark ink or light, whichever contrasts more (WCAG).
pub fn chip_text(fill: Color32) -> Color32 {
    // On the dark look the middle steps are mid-tones: pure black or white reach 4.5:1 there.
    let (dark, light) = if crate::theme::is_dark() {
        (Color32::BLACK, Color32::WHITE)
    } else {
        (crate::theme::ON_ACCENT(), crate::theme::ON_DARK())
    };
    if contrast(fill, dark) >= contrast(fill, light) {
        dark
    } else {
        light
    }
}

fn linear(v: u8) -> f32 {
    let v = f32::from(v) / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let v = if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

fn luminance(c: Color32) -> f32 {
    0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
}

/// The WCAG contrast ratio of two colours.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn oklab(c: Color32) -> [f32; 3] {
    let (r, g, b) = (linear(c.r()), linear(c.g()), linear(c.b()));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3]) -> Color32 {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    Color32::from_rgb(
        encode(4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_),
        encode(-1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_),
        encode(-0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_),
    )
}

/// "4.5", "3": a rating as the menus show it.
pub fn label(rating: f32) -> String {
    if rating.fract() == 0.0 {
        format!("{rating:.0}")
    } else {
        format!("{rating:.1}")
    }
}

/// One thing a menu offers: what it is called and the ids the ratings are looked up by.
pub struct Offer<'a> {
    pub id: &'a str,
    pub alt_id: Option<&'a str>,
    pub name: &'a str,
    /// Its maker: the folder it goes in when there is no ratings file at all.
    pub maker: &'a str,
}

/// How a menu shows a plug-in called `name` by `maker`: the big name and the small print.
/// One that emulates something: that thing's proper name, then its own name (the catalog's
/// when it has one); else its own name, then the maker.
pub fn shown(name: &str, maker: &str, rated: Option<&Rated>) -> (String, String) {
    match rated.filter(|r| !r.emulates.is_empty()) {
        Some(r) => {
            let own = if r.name.is_empty() { name } else { &r.name };
            (r.emulates.clone(), own.to_string())
        }
        None => (name.to_string(), maker.to_string()),
    }
}

/// Whether a menu entry matches what was typed in the menu's search field: its own name, the
/// name of what it emulates or its maker, ignoring case.
pub fn matches(query: &str, name: &str, maker: &str, rated: Option<&Rated>) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let mut hay = vec![name, maker];
    if let Some(r) = rated {
        hay.extend([r.emulates.as_str(), r.name.as_str(), r.maker.as_str()]);
    }
    hay.iter().any(|h| h.to_lowercase().contains(&q))
}

/// The menu's folders: each type (alphabetical, the ones without a type last as `other`)
/// with the indices of its offers, best rated first, unrated last, then by the name shown.
/// Without a ratings file (none, or one that rates nothing) the folders are the makers
/// instead, alphabetical, a plug-in without a maker under `other`.
/// With `master` (the Audio FX menu), a folder of that name comes first with the mastering
/// tools, which stay in their type's folder too.
pub fn arrange(
    offers: &[Offer],
    ratings: &Ratings,
    other: &str,
    master: Option<&str>,
) -> Vec<(String, Vec<usize>)> {
    let mut folders: Vec<(String, Vec<usize>)> = Vec::new();
    let by_maker = ratings.is_empty();
    for (i, o) in offers.iter().enumerate() {
        let kind = if by_maker {
            Some(o.maker.trim())
                .filter(|m| !m.is_empty())
                .unwrap_or(other)
        } else {
            ratings
                .of(o.id, o.alt_id)
                .map(|r| r.kind.as_str())
                .filter(|k| !k.is_empty() && !k.eq_ignore_ascii_case("other"))
                .unwrap_or(other)
        };
        match folders.iter_mut().find(|(k, _)| k == kind) {
            Some((_, v)) => v.push(i),
            None => folders.push((kind.to_string(), vec![i])),
        }
    }
    let rating = |i: usize| {
        ratings
            .of(offers[i].id, offers[i].alt_id)
            .and_then(|r| r.rating)
    };
    folders.sort_by_key(|(k, _)| (k == other, k.to_lowercase()));
    if let Some(m) = master {
        let tools: Vec<usize> = (0..offers.len())
            .filter(|&i| {
                ratings
                    .of(offers[i].id, offers[i].alt_id)
                    .is_some_and(|r| r.master_chain)
            })
            .collect();
        if !tools.is_empty() {
            folders.insert(0, (m.to_string(), tools));
        }
    }
    let key = |i: usize| {
        let r = ratings.of(offers[i].id, offers[i].alt_id);
        shown(offers[i].name, "", r).0.to_lowercase()
    };
    for (_, items) in &mut folders {
        items.sort_by(|&a, &b| {
            let (ra, rb) = (rating(a), rating(b));
            rb.unwrap_or(0.0)
                .total_cmp(&ra.unwrap_or(0.0))
                .then_with(|| key(a).cmp(&key(b)))
        });
    }
    folders
}

/// A plug-in name reduced for finding Pro Wave's own plug-in builds of a built-in
/// ("Pro Wave Lemon Limiter" and "Lemon Limiter" are one; so are the private build's names
/// with an FL prefix).
pub fn own_name_key(name: &str) -> String {
    let mut s = name.trim().to_lowercase();
    for prefix in ["pro wave ", "lemon ", "fl "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }
    s.chars().filter(char::is_ascii_alphanumeric).collect()
}

static CACHE: std::sync::Mutex<Option<Cache>> = std::sync::Mutex::new(None);

struct Cache {
    looked: std::time::Instant,
    modified: Option<std::time::SystemTime>,
    ratings: Arc<Ratings>,
}

/// The ratings now (the settings file, read again when it changes).
pub fn ratings() -> Arc<Ratings> {
    if cfg!(test) {
        return Arc::new(Ratings::default());
    }
    let Ok(mut c) = CACHE.lock() else {
        return Arc::new(Ratings::default());
    };
    if let Some(now) = c.as_ref() {
        if now.looked.elapsed() < std::time::Duration::from_secs(1) {
            return now.ratings.clone();
        }
    }
    let file = crate::recent_files::config_dir().map(|d| d.join("plugin-ratings.json"));
    let modified = file
        .as_ref()
        .and_then(|f| std::fs::metadata(f).ok())
        .and_then(|m| m.modified().ok());
    let ratings = match c.as_ref() {
        Some(old) if old.modified == modified => old.ratings.clone(),
        _ => Arc::new(
            file.and_then(|f| std::fs::read_to_string(f).ok())
                .map(|t| Ratings::parse(&t))
                .unwrap_or_default(),
        ),
    };
    *c = Some(Cache {
        looked: std::time::Instant::now(),
        modified,
        ratings: ratings.clone(),
    });
    ratings
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"{"plugins": {
        "aufx:Pro3:FbFt": {"rating": 5, "why": "Standard EQ.", "type": "EQ"},
        "vst3:AB": {"rating": 2.5, "why": "Overlaps.", "type": "EQ"},
        "aufx:dely:Xyzw": {"rating": 3, "why": "", "type": "Delay"},
        "builtin:Space D": {"rating": 4, "why": "Built-in.", "type": "Reverb"},
        "aufx:odd1:Xyzw": {"rating": 1, "type": "other"}
    }}"#;

    /// Type folders alphabetical with "Other" last; inside, the best rated first and the
    /// unrated last; a plug-in in both formats is found by its VST3 id too.
    #[test]
    fn menus_group_by_type_and_sort_by_rating() {
        let r = Ratings::parse(FILE);
        assert_eq!(r.len(), 5);
        let offers = [
            Offer {
                id: "aufx:Ab12:Xyzw",
                alt_id: None,
                name: "Unrated EQ?",
                maker: "Maker",
            },
            Offer {
                id: "aufx:Ch12:Xyzw",
                alt_id: Some("vst3:AB"),
                name: "Cheap EQ",
                maker: "Maker",
            },
            Offer {
                id: "aufx:Pro3:FbFt",
                alt_id: None,
                name: "Pro-Q 3",
                maker: "Maker",
            },
            Offer {
                id: "builtin:Space D",
                alt_id: None,
                name: "Space Designer",
                maker: "Maker",
            },
            Offer {
                id: "aufx:dely:Xyzw",
                alt_id: None,
                name: "Delay",
                maker: "Maker",
            },
            Offer {
                id: "aufx:odd1:Xyzw",
                alt_id: None,
                name: "Odd",
                maker: "Maker",
            },
        ];
        let folders = arrange(&offers, &r, "Other", None);
        let names: Vec<(&str, Vec<&str>)> = folders
            .iter()
            .map(|(k, v)| (k.as_str(), v.iter().map(|&i| offers[i].name).collect()))
            .collect();
        assert_eq!(
            names,
            [
                ("Delay", vec!["Delay"]),
                ("EQ", vec!["Pro-Q 3", "Cheap EQ"]),
                ("Reverb", vec!["Space Designer"]),
                ("Other", vec!["Odd", "Unrated EQ?"]),
            ]
        );
        assert_eq!(
            r.of("aufx:Ch12:Xyzw", Some("vst3:AB"))
                .and_then(|x| x.rating),
            Some(2.5)
        );
        assert_eq!(Ratings::parse("not json").len(), 0);
    }

    /// An emulation is shown by the proper name of what it emulates with its own name small,
    /// found by either name; the mastering tools get the first folder and keep their type's.
    #[test]
    fn emulations_show_the_hardware_name_and_master_chain_comes_first() {
        let r = Ratings::parse(
            r#"{"plugins": {
            "aumu:JC33:Xyzw": {"rating": 4, "why": "Good 303.", "type": "Synth",
                "emulates": "Roland TB-303", "name": "JC303", "maker": "midilab"},
            "aufx:ProL:FbFt": {"rating": 5, "why": "Limiter.", "type": "Limiter",
                "master_chain": true},
            "aufx:Pult:Xyzw": {"rating": 3, "why": "EQ.", "type": "EQ",
                "emulates": "Pultec EQP-1A", "master_chain": true},
            "aufx:Dly1:Xyzw": {"rating": 4.5, "type": "Delay"}
        }}"#,
        );
        let jc = r.of("aumu:JC33:Xyzw", None);
        assert_eq!(
            shown("JC303", "midilab", jc),
            ("Roland TB-303".to_string(), "JC303".to_string())
        );
        let plain = r.of("aufx:Dly1:Xyzw", None);
        assert_eq!(
            shown("Echo", "Maker", plain),
            ("Echo".to_string(), "Maker".to_string())
        );
        assert!(matches("tb-303", "JC303", "midilab", jc));
        assert!(matches("jc3", "JC303", "midilab", jc));
        assert!(matches("MIDILAB", "JC303", "midilab", jc));
        assert!(!matches("808", "JC303", "midilab", jc));
        assert!(matches("  ", "JC303", "midilab", jc));
        let offers = [
            Offer {
                id: "aufx:Dly1:Xyzw",
                alt_id: None,
                name: "Echo",
                maker: "Maker",
            },
            Offer {
                id: "aufx:Pult:Xyzw",
                alt_id: None,
                name: "PTEq-X",
                maker: "Maker",
            },
            Offer {
                id: "aufx:ProL:FbFt",
                alt_id: None,
                name: "Pro-L 2",
                maker: "Maker",
            },
        ];
        let folders = arrange(&offers, &r, "Other", Some("Master Chain"));
        let names: Vec<(&str, Vec<&str>)> = folders
            .iter()
            .map(|(k, v)| (k.as_str(), v.iter().map(|&i| offers[i].name).collect()))
            .collect();
        assert_eq!(
            names,
            [
                ("Master Chain", vec!["Pro-L 2", "PTEq-X"]),
                ("Delay", vec!["Echo"]),
                ("EQ", vec!["PTEq-X"]),
                ("Limiter", vec!["Pro-L 2"]),
            ]
        );
        // The Instrument menu has no Master Chain folder.
        assert_eq!(arrange(&offers, &r, "Other", None).len(), 3);
    }

    /// Every half step has its own colour, from the bone highlight at 1 to the orange at 5.
    #[test]
    fn each_half_step_has_its_own_colour() {
        use crate::theme::{set_for_thread, Theme};
        for look in [Theme::Bone, Theme::Graphite] {
            set_for_thread(Some(look));
            let all: Vec<Color32> = (2..=10).map(|h| color(h as f32 / 2.0)).collect();
            for (i, a) in all.iter().enumerate() {
                assert!(all[i + 1..].iter().all(|b| b != a), "step {i} repeats");
            }
            assert_eq!(all[0], crate::theme::BONE_HI());
            assert_eq!(all[8], crate::theme::ORANGE());
            if look == Theme::Bone {
                // The steps the plug-in catalog page uses (light theme).
                assert_eq!(color(3.0), Color32::from_rgb(0xff, 0xac, 0x8d));
            } else {
                // Graphite Amber: from the raised grey to amber.
                assert_eq!(all[0], Color32::from_rgb(0x32, 0x32, 0x37));
                assert_eq!(all[8], Color32::from_rgb(0xff, 0xb0, 0x00));
            }
            for c in &all {
                assert!(contrast(*c, chip_text(*c)) >= 4.5, "{c:?} reads");
            }
        }
        set_for_thread(None);
        assert_eq!(label(4.5), "4.5");
        assert_eq!(label(3.0), "3");
    }

    #[test]
    fn own_builds_match_their_built_in() {
        for n in ["Pro Wave Lemon Limiter", "Lemon Limiter"] {
            assert_eq!(own_name_key(n), "limiter");
        }
        assert_eq!(
            own_name_key("Pro Wave Resonator FX"),
            own_name_key("Resonator FX")
        );
    }

    /// No ratings file: one folder per maker, alphabetical, the ones without a maker last.
    #[test]
    fn without_a_ratings_file_the_folders_are_the_makers() {
        let none = Ratings::default();
        let offers = [
            Offer {
                id: "aufx:a:Zz",
                alt_id: None,
                name: "Zed EQ",
                maker: "Zeta",
            },
            Offer {
                id: "builtin:Space D",
                alt_id: None,
                name: "Space Designer",
                maker: "Pro Wave",
            },
            Offer {
                id: "aufx:b:Aa",
                alt_id: None,
                name: "Alpha Delay",
                maker: "Alpha",
            },
            Offer {
                id: "aufx:c:Xx",
                alt_id: None,
                name: "Nobody's",
                maker: " ",
            },
            Offer {
                id: "aufx:d:Aa",
                alt_id: None,
                name: "Alpha Comp",
                maker: "Alpha",
            },
        ];
        let folders = arrange(&offers, &none, "Other", Some("Master Chain"));
        let names: Vec<(&str, Vec<&str>)> = folders
            .iter()
            .map(|(k, v)| (k.as_str(), v.iter().map(|&i| offers[i].name).collect()))
            .collect();
        assert_eq!(
            names,
            [
                ("Alpha", vec!["Alpha Comp", "Alpha Delay"]),
                ("Pro Wave", vec!["Space Designer"]),
                ("Zeta", vec!["Zed EQ"]),
                ("Other", vec!["Nobody's"]),
            ]
        );
    }
}
