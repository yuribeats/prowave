//! The drum-kit library the Kit Browser shows: the kits found in the library folders, kept in
//! an index that is read from a cache at once and made again in the background.
//!
//! A library folder is laid out `<source>/<kit>/…`. A folder right under it that holds audio
//! files itself, or whose folders are named for drum sounds (Kicks, Snares…), is a kit of its
//! own; its source is then the library folder. Folders whose names start with `_` or `.` are
//! passed over. A `MANIFEST.tsv` in the library folder (a header naming `source` and `kit`,
//! and optionally `license`, `url`, `audio_or_instrument_files` and
//! `other_names_same_content`) adds what it knows to the kits it lists; a `BEST.md` there ranks
//! kits: a kit is ranked by the first line that names it (`source/kit`, or its source and its
//! name in the cells or emphasis of one line).
//!
//! A scan has two parts. The folders are read first (a few milliseconds): the list of kits is
//! there to browse. Then every kit's files are read, a kit at a time on a few threads, and
//! kept ([`crate::kit_files::Listing`], one file a kit in `kit-library/kits/`), so a kit picked
//! in the browser is made from what is kept and never waits for the library's disk. A kit
//! whose folder has not changed since it was read is not read again unless the scan is a full
//! one (Rescan).
//!
//! The index holds a line a kit, so a hundred thousand kits are a few megabytes and a search
//! is a pass over their names. Two kinds of line stand for more than one folder: a source whose
//! kits are nearly all one sample each (a site that gives a sample a page) is one pool, its
//! samples listed together; and a kit the manifest says has the same files as another is left
//! out, its name found by a search for the one shown. Nothing in a library folder is ever
//! written.
//!
//! A kit list (`kit_list` in the settings, a TSV such as `BEST-PROWAVE.tsv`) narrows what is
//! shown: with it on, only the kits it names (by their folder relative to the library folder)
//! are listed, in its order and grouped by its `type` column; favourites and the kit of new
//! racks are always shown. Without one, nothing changes.
use crate::kit_classify::{classify_label, natural_cmp};
use crate::kit_files::{self, Kit, Listing};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One kit of the index (or a pool of one-sample folders).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitEntry {
    /// The kit's folder (a pool's: its source's folder).
    pub path: String,
    /// The library folder it was found in.
    pub root: String,
    pub source: String,
    pub name: String,
    /// Audio and SoundFont files in it, when known (from its listing, else the manifest).
    pub files: Option<u32>,
    pub license: String,
    /// Its place in `BEST.md`, from 1.
    pub best: Option<u32>,
    /// When its files were read: the folder's modification time then (seconds).
    pub listed: Option<u64>,
    /// The channels a load fills, once its files are read.
    pub pads: Option<u8>,
    /// Other `source/kit` names the manifest gives the same files.
    pub aliases: Vec<String>,
    /// A pool: the folders (inside `path`) of the one-sample kits it stands for.
    pub members: Vec<String>,
    /// Its place in the kit list (from 1) and its type there (an index into `Index::types`).
    pub list: Option<(u32, u16)>,
    /// Source, name and other names in small letters: what a search looks through.
    hay: String,
    name_low: String,
}

impl KitEntry {
    pub fn new(root: &str, path: String, source: &str, name: &str) -> Self {
        let mut k = Self {
            path,
            root: root.to_string(),
            source: source.to_string(),
            name: name.to_string(),
            files: None,
            license: String::new(),
            best: None,
            listed: None,
            pads: None,
            aliases: Vec::new(),
            members: Vec::new(),
            list: None,
            hay: String::new(),
            name_low: String::new(),
        };
        k.rehay();
        k
    }

    fn rehay(&mut self) {
        self.name_low = fold(&self.name);
        self.hay = format!("{} {}", fold(&self.source), self.name_low);
        for a in &self.aliases {
            self.hay.push(' ');
            self.hay.push_str(&fold(a));
        }
    }

    pub fn is_pool(&self) -> bool {
        !self.members.is_empty()
    }

    /// `source/kit`, as the manifest and `BEST.md` name a kit.
    fn key(&self) -> String {
        format!(
            "{}/{}",
            self.source.to_lowercase(),
            self.name.to_lowercase()
        )
    }
}

/// A library folder and what the last scan found there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootInfo {
    pub path: String,
    /// The folder is there (a drive that is not mounted: false).
    pub present: bool,
    pub kits: usize,
}

/// The kits of every library folder that is there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub kits: Vec<KitEntry>,
    pub roots: Vec<RootInfo>,
    /// The sources with their number of kits, sorted.
    pub sources: Vec<(String, usize)>,
    /// The kit list's types, in the order of their best-placed kit, with the number of kits
    /// of the index each holds (empty without a list).
    pub types: Vec<(String, usize)>,
}

/// A source becomes one pool when it has at least this many kits…
const POOL_MIN_KITS: usize = 20;
/// …and at least this share of them (in percent) hold one file.
const POOL_SHARE: usize = 90;

/// The lines the index shows for `raw` (the kits as found): duplicates left out (their names
/// kept for search and their rank given to the kit shown), each pool source's one-sample kits
/// made one line.
fn shown(raw: Vec<KitEntry>) -> Vec<KitEntry> {
    let by_key: HashMap<String, usize> =
        raw.iter().enumerate().map(|(i, k)| (k.key(), i)).collect();
    // Duplicates: the first kit (in the index's order) of a set of the same files is shown.
    let mut order: Vec<usize> = (0..raw.len()).collect();
    order.sort_by(|a, b| {
        let (a, b) = (&raw[*a], &raw[*b]);
        natural_cmp(&a.source, &b.source).then_with(|| natural_cmp(&a.name, &b.name))
    });
    let mut hidden: HashSet<usize> = HashSet::new();
    let mut extra: HashMap<usize, (Vec<String>, Option<u32>)> = HashMap::new();
    for &i in &order {
        if hidden.contains(&i) {
            continue;
        }
        for a in &raw[i].aliases {
            let Some(&j) = by_key.get(&a.to_lowercase()) else {
                continue;
            };
            if j == i || hidden.contains(&j) {
                continue;
            }
            hidden.insert(j);
            let e = extra.entry(i).or_default();
            e.0.push(format!("{} {}", raw[j].source, raw[j].name));
            e.1 = match (e.1, raw[j].best) {
                (Some(x), Some(y)) => Some(x.min(y)),
                (x, y) => x.or(y),
            };
        }
    }
    let mut kits: Vec<KitEntry> = Vec::with_capacity(raw.len());
    for (i, mut k) in raw.into_iter().enumerate() {
        if hidden.contains(&i) {
            continue;
        }
        if let Some((names, best)) = extra.remove(&i) {
            k.aliases = names;
            k.best = match (k.best, best) {
                (Some(x), Some(y)) => Some(x.min(y)),
                (x, y) => x.or(y),
            };
            k.rehay();
        }
        kits.push(k);
    }
    // Pools: per library folder and source.
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, k) in kits.iter().enumerate() {
        groups
            .entry((k.root.clone(), k.source.clone()))
            .or_default()
            .push(i);
    }
    let mut pooled: HashSet<usize> = HashSet::new();
    let mut pools: Vec<KitEntry> = Vec::new();
    for ((root, source), list) in groups {
        let single: Vec<usize> = list
            .iter()
            .copied()
            .filter(|i| kits[*i].files == Some(1))
            .collect();
        if list.len() < POOL_MIN_KITS || single.len() * 100 < list.len() * POOL_SHARE {
            continue;
        }
        let first = &kits[single[0]];
        let Some(dir) = Path::new(&first.path).parent() else {
            continue;
        };
        let mut pool = KitEntry::new(&root, dir.to_string_lossy().into_owned(), &source, &source);
        pool.license = first.license.clone();
        pool.files = Some(single.len() as u32);
        pool.members = single.iter().map(|i| kits[*i].name.clone()).collect();
        pool.members.sort_by(|a, b| natural_cmp(a, b));
        pool.listed = single
            .iter()
            .map(|i| kits[*i].listed)
            .collect::<Option<Vec<u64>>>()
            .map(|v| v.into_iter().max().unwrap_or(0));
        pool.best = single.iter().filter_map(|i| kits[*i].best).min();
        pools.push(pool);
        pooled.extend(single);
    }
    let mut out: Vec<KitEntry> = kits
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !pooled.contains(i))
        .map(|(_, k)| k)
        .collect();
    out.extend(pools);
    out
}

impl Index {
    /// The index of `raw`, the kits as the folders were found, and the library folders.
    pub fn of(raw: Vec<KitEntry>, roots: Vec<RootInfo>) -> Self {
        Index {
            kits: shown(raw),
            roots,
            ..Index::default()
        }
        .finish()
    }

    fn finish(mut self) -> Self {
        self.kits.sort_by(|a, b| {
            natural_cmp(&a.source, &b.source)
                .then_with(|| natural_cmp(&a.name, &b.name))
                .then_with(|| a.path.cmp(&b.path))
        });
        self.kits.dedup_by(|a, b| a.path == b.path);
        let mut count: Vec<(String, usize)> = Vec::new();
        for k in &self.kits {
            match count.last_mut() {
                Some(c) if c.0 == k.source => c.1 += 1,
                _ => count.push((k.source.clone(), 1)),
            }
        }
        self.sources = count;
        for r in &mut self.roots {
            r.kits = self.kits.iter().filter(|k| k.root == r.path).count();
        }
        self
    }

    /// The kit (or pool) whose folder is `path`.
    pub fn find(&self, path: &str) -> Option<&KitEntry> {
        self.kits.iter().find(|k| k.path == path)
    }

    /// Marks the kits `list` names with their place and type (none without a list).
    pub fn apply_list(&mut self, list: Option<&KitList>) {
        self.types.clear();
        for k in &mut self.kits {
            k.list = None;
        }
        let Some(list) = list else { return };
        self.types = list.types.iter().map(|t| (t.clone(), 0)).collect();
        let roots: Vec<String> = self.roots.iter().map(|r| r.path.clone()).collect();
        let by_path = list.by_path(&roots);
        for k in &mut self.kits {
            let rel = k
                .path
                .strip_prefix(k.root.as_str())
                .unwrap_or(&k.path)
                .trim_matches('/');
            if let Some(&i) = by_path.get(&rel.to_lowercase()) {
                let row = &list.rows[i];
                k.list = Some((row.rank, row.kind));
                self.types[row.kind as usize].1 += 1;
            }
        }
    }

    /// How many kits the list names were found.
    pub fn list_matched(&self) -> usize {
        self.types.iter().map(|t| t.1).sum()
    }
}

/// `text` in small letters with the look-alike letters of other alphabets made plain: small
/// capitals ("ᴏᴢ"), full-width, circled and mathematical letters and digits ("𝐎𝐙", "ＯＺ"), and
/// the accented Latin letters ("é"). What the search compares, so "oz" finds "ᴏᴢ".
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match fold_char(c) {
            Some(s) => out.push_str(s),
            None => out.extend(c.to_lowercase()),
        }
    }
    out
}

const LOWER: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z",
];
const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// The plain letters `c` stands for, when it is a look-alike.
fn fold_char(c: char) -> Option<&'static str> {
    let u = c as u32;
    let letter = |i: u32| Some(LOWER[(i % 26) as usize]);
    match u {
        // Full-width A-Z, a-z, 0-9.
        0xFF21..=0xFF3A => letter(u - 0xFF21),
        0xFF41..=0xFF5A => letter(u - 0xFF41),
        0xFF10..=0xFF19 => Some(DIGITS[(u - 0xFF10) as usize]),
        // Circled letters.
        0x24B6..=0x24CF => letter(u - 0x24B6),
        0x24D0..=0x24E9 => letter(u - 0x24D0),
        // Mathematical letters (bold, italic, script, fraktur, double-struck, sans, mono): 52 a
        // style, capitals then small letters; and their digits, 10 a style.
        0x1D400..=0x1D6A3 => letter((u - 0x1D400) % 52),
        0x1D7CE..=0x1D7FF => Some(DIGITS[((u - 0x1D7CE) % 10) as usize]),
        // Negative squared and squared capitals (🅰, 🄰).
        0x1F130..=0x1F149 => letter(u - 0x1F130),
        0x1F170..=0x1F189 => letter(u - 0x1F170),
        _ => {
            Some(match c {
                // Small capitals.
                'ᴀ' => "a",
                'ʙ' => "b",
                'ᴄ' => "c",
                'ᴅ' => "d",
                'ᴇ' => "e",
                'ꜰ' => "f",
                'ɢ' => "g",
                'ʜ' => "h",
                'ɪ' => "i",
                'ᴊ' => "j",
                'ᴋ' => "k",
                'ʟ' => "l",
                'ᴍ' => "m",
                'ɴ' => "n",
                'ᴏ' => "o",
                'ᴘ' => "p",
                'ǫ' => "q",
                'ʀ' => "r",
                'ꜱ' => "s",
                'ᴛ' => "t",
                'ᴜ' => "u",
                'ᴠ' => "v",
                'ᴡ' => "w",
                'ʏ' => "y",
                'ᴢ' => "z",
                // Accented Latin letters.
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'À' | 'Á' | 'Â' | 'Ã'
                | 'Ä' | 'Å' | 'Ā' => "a",
                'ç' | 'ć' | 'č' | 'Ç' | 'Ć' | 'Č' => "c",
                'ď' | 'đ' | 'Ď' | 'Đ' => "d",
                'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' | 'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ę'
                | 'Ě' => "e",
                'ğ' | 'Ğ' => "g",
                'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' | 'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' | 'İ' => {
                    "i"
                }
                'ł' | 'ľ' | 'Ł' | 'Ľ' => "l",
                'ñ' | 'ń' | 'ň' | 'Ñ' | 'Ń' | 'Ň' => "n",
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö'
                | 'Ø' | 'Ō' | 'Ő' => "o",
                'ř' | 'Ř' => "r",
                'ś' | 'š' | 'ş' | 'Ś' | 'Š' | 'Ş' => "s",
                'ť' | 'ţ' | 'Ť' | 'Ţ' => "t",
                'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů'
                | 'Ű' => "u",
                'ý' | 'ÿ' | 'Ý' | 'Ÿ' => "y",
                'ź' | 'ż' | 'ž' | 'Ź' | 'Ż' | 'Ž' => "z",
                'ß' => "ss",
                'æ' | 'Æ' => "ae",
                'œ' | 'Œ' => "oe",
                _ => return None,
            })
        }
    }
}

/// What a search keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub query: String,
    pub source: Option<String>,
    pub best: bool,
    pub favorites: bool,
    /// Only the kits of the kit list (and the favourites and the kit of new racks), in its
    /// order and grouped by type; `source` is then not used.
    pub list: bool,
    /// With `list`: only the kits of this type of the list.
    pub kind: Option<String>,
}

/// [`search_keeping`] with no kit of new racks.
#[cfg(test)]
pub fn search(index: &Index, filter: &Filter, favorites: &HashSet<String>) -> Vec<u32> {
    search_keeping(index, filter, favorites, "")
}

/// Whether `kit` is the kit of new racks `default_kit` names (its folder, or a folder in it).
fn holds_default(kit: &KitEntry, default_kit: &str) -> bool {
    !default_kit.is_empty()
        && (kit.path == default_kit
            || default_kit
                .strip_prefix(kit.path.as_str())
                .is_some_and(|rest| rest.starts_with('/')))
}

/// The kits of `index` that pass `filter`, as indices into `index.kits`: every word of the
/// query has to be in the kit's source or name (or another name of the same files). Kits
/// whose name starts with the query come first; with Best on, the order is the ranking's. Kits
/// known to hold no audio and no SoundFont are left out.
/// With the kit list on, the kit of new racks (`default_kit`) is kept too.
pub fn search_keeping(
    index: &Index,
    filter: &Filter,
    favorites: &HashSet<String>,
    default_kit: &str,
) -> Vec<u32> {
    let kind = filter
        .kind
        .as_ref()
        .filter(|_| filter.list)
        .and_then(|t| index.types.iter().position(|(n, _)| n == t))
        .map(|i| i as u16);
    let q = fold(&filter.query);
    let words: Vec<&str> = q.split_whitespace().collect();
    let first = words.first().copied().unwrap_or("");
    let mut lead: Vec<u32> = Vec::new();
    let mut rest: Vec<u32> = Vec::new();
    for (i, k) in index.kits.iter().enumerate() {
        if k.files == Some(0)
            || (filter.best && k.best.is_none())
            || (filter.favorites && !favorites.contains(&k.path))
            || (!filter.list && filter.source.as_ref().is_some_and(|s| *s != k.source))
            || !words.iter().all(|w| k.hay.contains(w))
        {
            continue;
        }
        if filter.list {
            let kept = || favorites.contains(&k.path) || holds_default(k, default_kit);
            match (k.list, kind) {
                (Some((_, t)), Some(want)) if t != want => continue,
                (None, Some(_)) => continue,
                (None, None) if !kept() => continue,
                _ => {}
            }
        }
        if !first.is_empty() && k.name_low.starts_with(first) {
            lead.push(i as u32);
        } else {
            rest.push(i as u32);
        }
    }
    lead.extend(rest);
    if filter.list {
        // The favourites and the kit of new racks not on the list first, then type by type
        // in the list's order.
        lead.sort_by_key(|i| {
            index.kits[*i as usize]
                .list
                .map_or((0, 0), |(rank, t)| (u32::from(t) + 1, rank))
        });
    } else if filter.best {
        lead.sort_by_key(|i| index.kits[*i as usize].best.unwrap_or(u32::MAX));
    }
    lead
}

// ------------------------------------------------------------------------------- the kit list

/// One kit of a kit list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRow {
    pub rank: u32,
    /// The kit's folder relative to the library folder (or a full path), as written.
    pub path: String,
    /// Its type: an index into [`KitList::types`].
    pub kind: u16,
    pub reason: String,
}

/// A kit list: a TSV with a header naming at least `path` (the kit's folder relative to the
/// library folder), and optionally `rank`, `type` (or `genre/type`, `genre`) and `reason`.
/// Rows are taken in rank order (in the file's order without a rank column).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KitList {
    /// The file it was read from.
    pub file: String,
    pub rows: Vec<ListRow>,
    /// The types, in the order of their best-placed kit.
    pub types: Vec<String>,
}

/// `path` as a key: no `./`, no slashes at the ends, small letters.
fn list_key(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_matches('/')
        .to_lowercase()
}

impl KitList {
    pub fn parse(text: &str, file: &str) -> Result<Self, String> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header = lines.next().ok_or("the kit list is empty")?;
        let cols: Vec<String> = header
            .split('\t')
            .map(|c| c.trim().to_lowercase())
            .collect();
        let col = |names: &[&str]| cols.iter().position(|c| names.contains(&c.as_str()));
        let c_path = col(&["path"]).ok_or("the kit list has no `path` column")?;
        let c_rank = col(&["rank"]);
        let c_type = col(&["type", "genre/type", "genre", "kind"]);
        let c_reason = col(&["reason", "why"]);
        let mut rows: Vec<(u32, String, String, String)> = Vec::new();
        for (n, line) in lines.enumerate() {
            let f: Vec<&str> = line.split('\t').collect();
            let get = |c: Option<usize>| c.and_then(|c| f.get(c)).map_or("", |s| s.trim());
            let path = get(Some(c_path));
            if path.is_empty() {
                continue;
            }
            let rank = get(c_rank).parse::<u32>().unwrap_or(n as u32 + 1);
            rows.push((
                rank,
                path.to_string(),
                get(c_type).to_string(),
                get(c_reason).to_string(),
            ));
        }
        rows.sort_by_key(|r| r.0);
        let mut out = KitList {
            file: file.to_string(),
            ..Self::default()
        };
        let mut seen = HashSet::new();
        for (rank, path, kind, reason) in rows {
            if !seen.insert(list_key(&path)) {
                continue;
            }
            let kind = if kind.is_empty() {
                "Other".into()
            } else {
                kind
            };
            let t = match out.types.iter().position(|t| *t == kind) {
                Some(t) => t,
                None => {
                    out.types.push(kind);
                    out.types.len() - 1
                }
            };
            out.rows.push(ListRow {
                rank,
                path,
                kind: t.min(usize::from(u16::MAX)) as u16,
                reason,
            });
        }
        Ok(out)
    }

    /// Row by key of the kit's folder relative to its library folder. A full path in the list
    /// is made relative to whichever of `roots` it is in, or else cut after the name of one of
    /// them (`/Volumes/SHARE/DRUM-KITS/x` is `x` in `~/SHARE/DRUM-KITS`).
    fn by_path(&self, roots: &[String]) -> HashMap<String, usize> {
        let mut out = HashMap::with_capacity(self.rows.len());
        for (i, row) in self.rows.iter().enumerate() {
            let p = row.path.trim();
            let rel = if p.starts_with('/') || p.starts_with('~') {
                roots
                    .iter()
                    .find_map(|r| p.strip_prefix(r.as_str()).filter(|t| t.starts_with('/')))
                    .map(str::to_string)
                    .or_else(|| {
                        roots.iter().find_map(|r| {
                            let name = Path::new(r).file_name()?.to_string_lossy().into_owned();
                            let mark = format!("/{name}/");
                            p.rfind(&mark).map(|at| p[at + mark.len()..].to_string())
                        })
                    })
                    .unwrap_or_else(|| p.to_string())
            } else {
                p.to_string()
            };
            out.entry(list_key(&rel)).or_insert(i);
        }
        out
    }
}

/// The file a `kit_list` setting names, as found here: the path itself, else (a list made on
/// another Mac, a library folder mounted elsewhere) the same file inside one of `folders`: a
/// relative path in it, the part of the path after the folder's name, or its file name.
pub fn find_list_file(setting: &str, folders: &[String]) -> Option<PathBuf> {
    let setting = setting.trim();
    if setting.is_empty() {
        return None;
    }
    let p = Path::new(setting);
    if p.is_absolute() && p.is_file() {
        return Some(p.to_path_buf());
    }
    for f in folders {
        let dir = Path::new(f);
        let mut tries: Vec<PathBuf> = Vec::new();
        if p.is_relative() {
            tries.push(dir.join(p));
        } else {
            if let Some(name) = dir.file_name() {
                let mark = format!("/{}/", name.to_string_lossy());
                if let Some(at) = setting.rfind(&mark) {
                    tries.push(dir.join(&setting[at + mark.len()..]));
                }
            }
            if let Some(name) = p.file_name() {
                tries.push(dir.join(name));
            }
        }
        if let Some(found) = tries.into_iter().find(|t| t.is_file()) {
            return Some(found);
        }
    }
    None
}

/// Reads the kit list a `kit_list` setting names (see [`find_list_file`]).
pub fn load_list(setting: &str, folders: &[String]) -> Result<KitList, String> {
    let file = find_list_file(setting, folders)
        .ok_or_else(|| format!("the kit list {setting} is not there"))?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    KitList::parse(&text, &file.to_string_lossy())
}

// ------------------------------------------------------------------------------- settings

/// The library folders and the favourite kits, kept in `kit-library/settings.json` of the
/// settings folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub folders: Vec<String>,
    pub favorites: HashSet<String>,
    /// Audition is lit: a sample or kit picked is heard.
    pub audition: bool,
    /// The kit a new Channel Rack starts with (a kit's folder; empty: none). A kit whose
    /// folder is not there leaves the new rack empty.
    pub default_kit: String,
    /// When a kit loaded into a rack is copied to this computer (`kit-library/local/`).
    pub copy_kits: crate::kit_copy::CopyMode,
    /// The kit list (a TSV; empty: none): see [`KitList`].
    pub kit_list: String,
    /// The kit list is used (on unless turned off in the browser).
    pub kit_list_on: bool,
}

/// The library folders of a new installation.
#[allow(clippy::vec_init_then_push)]
pub fn default_folders() -> Vec<String> {
    #[allow(unused_mut)]
    let mut folders = Vec::new();
    folders
}

/// The kit a new Channel Rack starts with on a new installation (empty: none).
pub fn default_kit() -> String {
    let kit = "";
    kit.to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folders: default_folders(),
            favorites: HashSet::new(),
            audition: false,
            default_kit: default_kit(),
            copy_kits: crate::kit_copy::CopyMode::default(),
            kit_list: String::new(),
            kit_list_on: true,
        }
    }
}

/// Where kits loaded from other computers are copied: `kit-library/local`.
pub fn local_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("local"))
}

/// `kit-library` in the settings folder.
pub fn data_dir() -> Option<PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join("kit-library"))
}

impl Settings {
    pub fn from_json(text: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let list = |k: &str| -> Option<Vec<String>> {
            Some(
                v[k].as_array()?
                    .iter()
                    .filter_map(|s| s.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string)
                    .collect(),
            )
        };
        let mut folders = list("folders").unwrap_or_else(default_folders);
        let mut seen = HashSet::new();
        folders.retain(|f| seen.insert(f.clone()));
        Self {
            folders,
            favorites: list("favorites").unwrap_or_default().into_iter().collect(),
            audition: v["audition"].as_bool().unwrap_or(false),
            default_kit: v["default_kit"]
                .as_str()
                .map_or_else(default_kit, |s| s.trim().to_string()),
            copy_kits: v["copy_kits"]
                .as_str()
                .and_then(crate::kit_copy::CopyMode::parse)
                .unwrap_or_default(),
            kit_list: v["kit_list"].as_str().unwrap_or("").trim().to_string(),
            kit_list_on: v["kit_list_on"].as_bool().unwrap_or(true),
        }
    }

    pub fn to_json(&self) -> String {
        let mut fav: Vec<&String> = self.favorites.iter().collect();
        fav.sort();
        serde_json::to_string_pretty(&json!({
            "folders": self.folders,
            "favorites": fav,
            "audition": self.audition,
            "default_kit": self.default_kit,
            "copy_kits": self.copy_kits.id(),
            "kit_list": self.kit_list,
            "kit_list_on": self.kit_list_on,
        }))
        .unwrap_or_default()
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join("settings.json"))
            .map(|t| Self::from_json(&t))
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("settings.json"), self.to_json())
    }
}

// ------------------------------------------------------------------------------- the scan

/// What the manifest says of one kit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestRow {
    pub files: Option<u32>,
    pub license: String,
    /// `source/kit` names of kits with the same files.
    pub aliases: Vec<String>,
}

/// A manifest's rows: (source, kit) to what it says.
pub type Manifest = HashMap<(String, String), ManifestRow>;

/// Reads a `MANIFEST.tsv`: a header line, then a row a kit. Columns are found by their names
/// in the header, so a manifest with more or fewer of them is read the same way.
pub fn parse_manifest(text: &str) -> Manifest {
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Manifest::new();
    };
    let cols: Vec<&str> = header.split('\t').map(str::trim).collect();
    let col = |name: &str| cols.iter().position(|c| *c == name);
    let (Some(c_source), Some(c_kit)) = (col("source"), col("kit")) else {
        return Manifest::new();
    };
    let c_audio = col("audio_or_instrument_files").or_else(|| col("file_count"));
    let c_license = col("license");
    let c_same = col("other_names_same_content");
    let mut out = Manifest::new();
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        let get = |c: usize| f.get(c).map(|s| s.trim()).unwrap_or("");
        let (source, kit) = (get(c_source), get(c_kit));
        if source.is_empty() || kit.is_empty() {
            continue;
        }
        let aliases = c_same
            .map(|c| aliases_of(get(c), source))
            .unwrap_or_default();
        out.insert(
            (source.to_string(), kit.to_string()),
            ManifestRow {
                files: c_audio.and_then(|c| get(c).parse::<u32>().ok()),
                license: c_license.map(get).unwrap_or("").to_string(),
                aliases,
            },
        );
    }
    out
}

/// The other names in a manifest cell: `source/kit` (or a kit of the same source), separated
/// by `;` or `|`. A cell that says nothing (empty, `-`, `none`) names none.
fn aliases_of(cell: &str, source: &str) -> Vec<String> {
    cell.split([';', '|'])
        .map(str::trim)
        .filter(|a| !a.is_empty() && *a != "-" && !a.eq_ignore_ascii_case("none"))
        .map(|a| {
            if a.contains('/') {
                a.to_string()
            } else {
                format!("{source}/{a}")
            }
        })
        .collect()
}

/// The pieces of a `BEST.md` line that can name a kit or a source: table cells, link texts,
/// emphasised and quoted text, and the line itself without its list mark.
fn line_pieces(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let clean = |s: &str| {
        s.trim()
            .trim_matches(|c: char| c == '*' || c == '_' || c == '`' || c == '"')
            .trim()
            .to_lowercase()
    };
    let body = line.trim().trim_start_matches(|c: char| {
        c.is_ascii_digit() || matches!(c, '.' | ')' | '-' | '*' | '+' | '#' | ' ')
    });
    for cell in body.split('|') {
        out.push(clean(cell));
        // "Source / Kit", "Source: Kit", "Source — Kit".
        for sep in [" / ", ": ", " — ", " – ", " - "] {
            if let Some((a, b)) = cell.split_once(sep) {
                out.push(clean(a));
                out.push(clean(b));
            }
        }
    }
    for cell in line.split('|') {
        // [text](link): the text.
        let mut rest = cell;
        while let Some(a) = rest.find('[') {
            let Some(b) = rest[a..].find(']') else { break };
            out.push(clean(&rest[a + 1..a + b]));
            rest = &rest[a + b + 1..];
        }
        for mark in ["**", "`", "\""] {
            let parts: Vec<&str> = cell.split(mark).collect();
            for p in parts.iter().skip(1).step_by(2) {
                out.push(clean(p));
            }
        }
    }
    out.retain(|p| !p.is_empty());
    out
}

/// Ranks the kits a `BEST.md` names: the first line naming a kit gives its place (1 for the
/// first kit named). A line names a kit by `source/kit`, or by the kit's name with its source
/// on the same line, or by the name alone when one kit only has that name.
pub fn parse_best(text: &str, kits: &[(String, String)]) -> HashMap<usize, u32> {
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (source, name)) in kits.iter().enumerate() {
        by_path.insert(
            format!("{}/{}", source.to_lowercase(), name.to_lowercase()),
            i,
        );
        by_name.entry(name.to_lowercase()).or_default().push(i);
    }
    let mut out: HashMap<usize, u32> = HashMap::new();
    let mut rank = 0u32;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let pieces = line_pieces(line);
        let mut hit: Option<usize> = None;
        for p in &pieces {
            if let Some(i) = by_path.get(p.trim_end_matches('/')) {
                hit = Some(*i);
                break;
            }
        }
        if hit.is_none() {
            for p in &pieces {
                let Some(list) = by_name.get(p) else { continue };
                let with_source = list
                    .iter()
                    .find(|i| pieces.iter().any(|q| *q == kits[**i].0.to_lowercase()));
                hit = with_source
                    .copied()
                    .or_else(|| (list.len() == 1).then(|| list[0]));
                if hit.is_some() {
                    break;
                }
            }
        }
        if let Some(i) = hit {
            if let std::collections::hash_map::Entry::Vacant(e) = out.entry(i) {
                rank += 1;
                e.insert(rank);
            }
        }
    }
    out
}

fn skipped(name: &str) -> bool {
    name.starts_with('_') || name.starts_with('.') || name == "__MACOSX"
}

/// The folders and whether there are audio or kit-definition files right in `dir`.
fn list_dir(dir: &Path) -> (Vec<String>, bool) {
    let mut dirs = Vec::new();
    let mut audio = false;
    if let Ok(read) = std::fs::read_dir(dir) {
        for e in read.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if skipped(&name) {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(name);
            } else if !audio {
                let low = name.to_ascii_lowercase();
                audio = crate::import::is_audio_path(Path::new(&name))
                    || low.ends_with(".sfz")
                    || low == "drumkit.xml";
            }
        }
    }
    (dirs, audio)
}

/// Whether the folders of a folder make it one kit: most are named for drum sounds or for
/// what sample packs call their parts.
fn folders_of_one_kit(dirs: &[String]) -> bool {
    let known = dirs
        .iter()
        .filter(|d| {
            classify_label(d).is_some()
                || matches!(
                    d.to_lowercase().as_str(),
                    "samples"
                        | "wav"
                        | "wavs"
                        | "one shots"
                        | "one-shots"
                        | "oneshots"
                        | "drums"
                        | "hits"
                        | "audio"
                        | "midi"
                        | "presets"
                )
        })
        .count();
    known > 0 && known * 2 >= dirs.len()
}

fn join(root: &str, rel: &str) -> String {
    Path::new(root).join(rel).to_string_lossy().into_owned()
}

/// The kits of one library folder, or `None` when the folder is not there.
pub fn scan_root(root: &str) -> Option<Vec<KitEntry>> {
    let dir = Path::new(root);
    if !dir.is_dir() {
        return None;
    }
    let manifest = std::fs::read_to_string(dir.join("MANIFEST.tsv"))
        .map(|t| parse_manifest(&t))
        .unwrap_or_default();
    let manifest_sources: HashSet<&str> = manifest.keys().map(|k| k.0.as_str()).collect();
    let own = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string());
    let mut kits: Vec<KitEntry> = Vec::new();
    let (tops, audio_here) = list_dir(dir);
    if audio_here {
        kits.push(KitEntry::new(root, root.to_string(), &own, &own));
    }
    for top in tops {
        let (subs, audio) = list_dir(&dir.join(&top));
        let listed = manifest_sources.contains(top.as_str());
        if !listed && (audio || subs.is_empty() || folders_of_one_kit(&subs)) {
            kits.push(KitEntry::new(root, join(root, &top), &own, &top));
            continue;
        }
        for sub in subs {
            let mut k = KitEntry::new(root, join(root, &format!("{top}/{sub}")), &top, &sub);
            if let Some(row) = manifest.get(&(top.clone(), sub)) {
                k.files = row.files;
                k.license = row.license.clone();
                k.aliases = row.aliases.clone();
            }
            kits.push(k);
        }
    }
    if let Ok(best) = std::fs::read_to_string(dir.join("BEST.md")) {
        let names: Vec<(String, String)> = kits
            .iter()
            .map(|k| (k.source.clone(), k.name.clone()))
            .collect();
        for (i, rank) in parse_best(&best, &names) {
            kits[i].best = Some(rank);
        }
    }
    Some(kits)
}

// ------------------------------------------------------------------------------- the cache

/// The cache: for each library folder ever scanned, its kits as found (before pools and
/// duplicates are made one line). A folder that is not there now keeps its lines, so the list
/// is back at once when its drive is. Each kit's files are kept beside it, in `kits/`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cache {
    pub roots: HashMap<String, Vec<KitEntry>>,
}

const CACHE_VERSION: u64 = 2;

impl Cache {
    pub fn to_json(&self) -> String {
        let mut roots = serde_json::Map::new();
        for (root, kits) in &self.roots {
            let rows: Vec<Value> = kits
                .iter()
                .map(|k| {
                    json!([
                        k.path.strip_prefix(root.as_str()).unwrap_or(&k.path),
                        k.source,
                        k.name,
                        k.files,
                        k.license,
                        k.best,
                        k.listed,
                        k.pads,
                        k.aliases,
                    ])
                })
                .collect();
            roots.insert(root.clone(), Value::Array(rows));
        }
        json!({ "version": CACHE_VERSION, "roots": roots }).to_string()
    }

    pub fn from_json(text: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        if v["version"].as_u64() != Some(CACHE_VERSION) {
            return Self::default();
        }
        let mut out = Self::default();
        let Some(roots) = v["roots"].as_object() else {
            return out;
        };
        for (root, rows) in roots {
            let kits = rows
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|r| {
                            let tail = r[0].as_str()?;
                            let mut k = KitEntry::new(
                                root,
                                format!("{root}{tail}"),
                                r[1].as_str()?,
                                r[2].as_str()?,
                            );
                            k.files = r[3].as_u64().map(|n| n as u32);
                            k.license = r[4].as_str().unwrap_or("").to_string();
                            k.best = r[5].as_u64().map(|n| n as u32);
                            k.listed = r[6].as_u64();
                            k.pads = r[7].as_u64().map(|n| n.min(255) as u8);
                            k.aliases = r[8]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|s| s.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default();
                            k.rehay();
                            Some(k)
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.roots.insert(root.clone(), kits);
        }
        out
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join("index.json"))
            .map(|t| Self::from_json(&t))
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("index.json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(tmp, dir.join("index.json"))
    }

    /// The index of `folders` from the cache alone: the folders that are there now.
    pub fn index(&self, folders: &[String]) -> Index {
        let mut raw = Vec::new();
        let mut roots = Vec::new();
        for f in folders {
            let present = Path::new(f).is_dir();
            if present {
                if let Some(kits) = self.roots.get(f) {
                    raw.extend(kits.iter().cloned());
                }
            }
            roots.push(RootInfo {
                path: f.clone(),
                present,
                kits: 0,
            });
        }
        Index::of(raw, roots)
    }
}

// ------------------------------------------------------------------------------- listings

/// FNV-1a: a name for a kit's listing file that stays the same from run to run.
fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The file the listing of the kit in folder `kit` is kept in.
pub fn listing_file(dir: &Path, kit: &str) -> PathBuf {
    dir.join("kits").join(format!("{:016x}.txt", fnv(kit)))
}

/// The kept listing of the kit in folder `kit`, when there is one for that folder.
pub fn load_listing(dir: &Path, kit: &str) -> Option<Listing> {
    let text = std::fs::read_to_string(listing_file(dir, kit)).ok()?;
    let (first, rest) = text.split_once('\n')?;
    // The first line names the folder: two folders may share a file name.
    if first != kit {
        return None;
    }
    Listing::from_text(rest)
}

/// Keeps the listing of the kit in folder `kit`.
pub fn save_listing(dir: &Path, kit: &str, listing: &Listing) -> std::io::Result<()> {
    let file = listing_file(dir, kit);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{kit}\n{}", listing.to_text()))?;
    std::fs::rename(tmp, file)
}

/// The folder's modification time, in seconds.
fn folder_time(path: &str) -> Option<u64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        t.duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
}

/// What the files of a kit tell the index: how many there are and how many channels a load
/// fills.
fn counts(kit: &Kit) -> (u32, u8) {
    (
        kit.samples.len() as u32,
        kit.pads.iter().flatten().count().min(255) as u8,
    )
}

/// A kit read for the browser, with the listings read from its disk on the way (to be kept).
pub struct Opened {
    pub kit: Kit,
    /// (kit folder, its folder's time, listing).
    pub fresh: Vec<(String, Option<u64>, Listing)>,
}

/// Makes the kit `entry` stands for: from the kept listings in `dir` where there are any,
/// from its folder otherwise; a pool from its members'. Given up when `stop` is set.
pub fn open(dir: Option<&Path>, entry: &KitEntry, stop: &AtomicBool) -> Opened {
    let mut fresh = Vec::new();
    let mut one = |path: &str| -> Listing {
        if let Some(l) = dir.and_then(|d| load_listing(d, path)) {
            return l;
        }
        let time = folder_time(path);
        let l = kit_files::list_kit_until(Path::new(path), stop);
        if !stop.load(Ordering::Relaxed) {
            fresh.push((path.to_string(), time, l.clone()));
        }
        l
    };
    let root = Path::new(&entry.path);
    let listing = if entry.is_pool() {
        let parts = entry
            .members
            .iter()
            .map(|m| {
                let path = join(&entry.path, m);
                (m.clone(), one(&path))
            })
            .collect();
        Listing::merged(parts)
    } else {
        one(&entry.path)
    };
    Opened {
        kit: listing.kit(root),
        fresh,
    }
}

// ------------------------------------------------------------------------------- the worker

/// Scans `folders`: the kits as found, with what the cache knew of them (their files read,
/// their counts). Returns the index and the cache to write.
pub fn scan(folders: &[String], mut cache: Cache) -> (Index, Cache) {
    let mut raw = Vec::new();
    let mut roots = Vec::new();
    for f in folders {
        let found = scan_root(f);
        if let Some(mut kits) = found.clone() {
            if let Some(old) = cache.roots.get(f) {
                let known: HashMap<&str, &KitEntry> =
                    old.iter().map(|k| (k.path.as_str(), k)).collect();
                for k in kits.iter_mut() {
                    if let Some(o) = known.get(k.path.as_str()) {
                        if o.listed.is_some() {
                            k.files = o.files;
                            k.pads = o.pads;
                            k.listed = o.listed;
                        } else if k.files.is_none() {
                            k.files = o.files;
                        }
                    }
                }
            }
            raw.extend(kits.iter().cloned());
            cache.roots.insert(f.clone(), kits);
        }
        roots.push(RootInfo {
            path: f.clone(),
            present: found.is_some(),
            kits: 0,
        });
    }
    (Index::of(raw, roots), cache)
}

/// What the background scan sends.
pub enum ScanMsg {
    /// The index from the cache (the first thing sent).
    Cached(Index),
    /// The index from the folders as they are, and how long reading them took. The kits'
    /// files are read after it.
    Scanned(Index, Duration),
    /// Kits whose files have been read so far, of those to read, and the index with them.
    Listing {
        done: usize,
        total: usize,
        index: Index,
    },
    /// Every kit's files are read: the index, how many kits were read and how long it took.
    Listed(Index, usize, Duration),
}

/// Threads that read kits' files.
const LIST_THREADS: usize = 4;
/// How often the scan sends the index while it reads kits.
const LISTING_EVERY: Duration = Duration::from_millis(400);

/// A scan running on its own threads.
pub struct ScanJob {
    pub rx: Receiver<ScanMsg>,
    stop: Arc<AtomicBool>,
}

impl ScanJob {
    /// Stops it (the threads end at the next kit).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ScanJob {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts a scan of `folders` on its own threads: the cached index first, then the scanned
/// one, then the kits' files are read (all of them when `full`, else the kits never read and
/// those whose folder has changed). The cache in `dir` is written as it goes.
pub fn spawn_scan(folders: Vec<String>, dir: Option<PathBuf>, full: bool) -> ScanJob {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("kit-library-scan".into())
        .spawn(move || {
            let cache = dir.as_deref().map(Cache::load).unwrap_or_default();
            if tx.send(ScanMsg::Cached(cache.index(&folders))).is_err() {
                return;
            }
            let started = Instant::now();
            let (index, cache) = scan(&folders, cache);
            let took = started.elapsed();
            if let Some(dir) = &dir {
                if let Err(e) = cache.save(dir) {
                    tracing::warn!("could not write the kit index: {e}");
                }
            }
            if tx.send(ScanMsg::Scanned(index, took)).is_err() {
                return;
            }
            list_all(&folders, dir.as_deref(), cache, full, &flag, &tx);
        });
    if let Err(e) = spawned {
        tracing::warn!("could not start the kit scan: {e}");
    }
    ScanJob { rx, stop }
}

/// Reads the files of the kits of `cache` that need it, on [`LIST_THREADS`] threads, keeping
/// each listing and sending the index as it goes.
fn list_all(
    folders: &[String],
    dir: Option<&Path>,
    mut cache: Cache,
    full: bool,
    stop: &AtomicBool,
    tx: &mpsc::Sender<ScanMsg>,
) {
    let started = Instant::now();
    // The kits to look at, the never-read ones and the ranked ones first.
    let mut todo: Vec<(String, String, Option<u64>)> = folders
        .iter()
        .filter(|f| Path::new(f.as_str()).is_dir())
        // A library folder on a network share is not read kit by kit in the background (it
        // fills the network and every other read waits); its kits are read when picked.
        .filter(|f| !crate::kit_copy::is_remote(Path::new(f.as_str())))
        .filter_map(|f| cache.roots.get(f))
        .flatten()
        .map(|k| (k.root.clone(), k.path.clone(), k.listed))
        .collect();
    todo.sort_by_key(|t| t.2.is_some());
    let total = todo.len();
    let queue = Mutex::new(todo.into_iter());
    let (res_tx, res_rx) = mpsc::channel::<(String, String, Option<u64>, Option<(u32, u8)>)>();
    let mut read = 0usize;
    std::thread::scope(|scope| {
        for n in 0..LIST_THREADS {
            let (queue, res_tx) = (&queue, res_tx.clone());
            let spawned = std::thread::Builder::new()
                .name(format!("kit-list-{n}"))
                .spawn_scoped(scope, move || loop {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let item = queue.lock().ok().and_then(|mut q| q.next());
                    let Some((root, path, listed)) = item else {
                        return;
                    };
                    let time = folder_time(&path);
                    let kept = !full
                        && listed.is_some()
                        && listed == time
                        && dir.is_some_and(|d| listing_file(d, &path).is_file());
                    if kept {
                        let _ = res_tx.send((root, path, time, None));
                        continue;
                    }
                    let listing = kit_files::list_kit_until(Path::new(&path), stop);
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Some(d) = dir {
                        if let Err(e) = save_listing(d, &path, &listing) {
                            tracing::warn!("could not keep the files of {path}: {e}");
                        }
                    }
                    let c = counts(&listing.kit(Path::new(&path)));
                    let _ = res_tx.send((root, path, time, Some(c)));
                });
            if let Err(e) = spawned {
                tracing::warn!("could not start a kit reader: {e}");
            }
        }
        drop(res_tx);
        let mut done = 0usize;
        let mut last = Instant::now();
        let mut slots: HashMap<String, HashMap<String, usize>> = HashMap::new();
        for (root, kits) in &cache.roots {
            slots.insert(
                root.clone(),
                kits.iter()
                    .enumerate()
                    .map(|(i, k)| (k.path.clone(), i))
                    .collect(),
            );
        }
        for (root, path, time, c) in res_rx {
            done += 1;
            if let Some((files, pads)) = c {
                read += 1;
                let at = slots.get(&root).and_then(|m| m.get(&path)).copied();
                if let (Some(i), Some(kits)) = (at, cache.roots.get_mut(&root)) {
                    kits[i].files = Some(files);
                    kits[i].pads = Some(pads);
                    kits[i].listed = Some(time.unwrap_or(0));
                }
            }
            if last.elapsed() >= LISTING_EVERY {
                last = Instant::now();
                if let Some(d) = dir {
                    let _ = cache.save(d);
                }
                let _ = tx.send(ScanMsg::Listing {
                    done,
                    total,
                    index: cache.index(folders),
                });
            }
        }
    });
    if stop.load(Ordering::Relaxed) {
        return;
    }
    if let Some(d) = dir {
        if let Err(e) = cache.save(d) {
            tracing::warn!("could not write the kit index: {e}");
        }
    }
    let _ = tx.send(ScanMsg::Listed(
        cache.index(folders),
        read,
        started.elapsed(),
    ));
}

/// Notes in the cache in `dir` what reading the kit at `path` (in library folder `root`)
/// found. Background thread.
pub fn note_listed(dir: &Path, root: &str, path: &str, time: Option<u64>, files: u32, pads: u8) {
    let mut cache = Cache::load(dir);
    let Some(k) = cache
        .roots
        .get_mut(root)
        .and_then(|kits| kits.iter_mut().find(|k| k.path == path))
    else {
        return;
    };
    k.files = Some(files);
    k.pads = Some(pads);
    k.listed = Some(time.unwrap_or(0));
    if let Err(e) = cache.save(dir) {
        tracing::warn!("could not write the kit index: {e}");
    }
}

/// [`note_listed`] for what [`open`] read: keeps the listings and their counts.
pub fn keep(dir: &Path, root: &str, fresh: &[(String, Option<u64>, Listing)]) {
    for (path, time, listing) in fresh {
        if let Err(e) = save_listing(dir, path, listing) {
            tracing::warn!("could not keep the files of {path}: {e}");
            continue;
        }
        let (files, pads) = counts(&listing.kit(Path::new(path)));
        note_listed(dir, root, path, *time, files, pads);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("prowave-kitlib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// A library laid out as the real one is.
    fn library(name: &str) -> PathBuf {
        let dir = temp(name);
        touch(
            &dir,
            "MANIFEST.tsv",
            "source\tkit\turl\tlicense\tfile_count\tbytes\tdate\taudio_or_instrument_files\n\
             kb6\tAkai MPC2000\thttps://x\tCC BY-NC-ND 4.0\t44\t1722199\t2026-11-23\t42\n\
             kb6\tAcetone Rhythm-Ace\thttps://x\tCC BY-NC-ND 4.0\t18\t175113\t2026-11-23\t16\n\
             MusicRadar\t808-samples\thttps://y\tRoyalty free\t400\t9\t2026-11-23\t398\n",
        );
        touch(
            &dir,
            "kb6/Akai MPC2000/[KB6]_Akai_MPC2000/808_HH__CL.wav",
            "",
        );
        touch(&dir, "kb6/Acetone Rhythm-Ace/a/kick.wav", "");
        touch(
            &dir,
            "MusicRadar/808-samples/x/Hits/Bass Drum [BD]/E808_BD-01.wav",
            "",
        );
        // A kit the manifest does not list yet, and working folders.
        touch(&dir, "MusicRadar/909-samples/Kick.wav", "");
        touch(&dir, "_incoming/Junk/thing/a.wav", "");
        touch(&dir, "_work/dk.py", "");
        touch(&dir, "PROGRESS.txt", "");
        dir
    }

    #[test]
    fn a_library_folder_is_scanned_into_sources_and_kits() {
        let dir = library("scan");
        let root = dir.to_string_lossy().into_owned();
        let (index, cache) = scan(std::slice::from_ref(&root), Cache::default());
        let got: Vec<(&str, &str, Option<u32>)> = index
            .kits
            .iter()
            .map(|k| (k.source.as_str(), k.name.as_str(), k.files))
            .collect();
        assert_eq!(
            got,
            [
                ("kb6", "Acetone Rhythm-Ace", Some(16)),
                ("kb6", "Akai MPC2000", Some(42)),
                ("MusicRadar", "808-samples", Some(398)),
                ("MusicRadar", "909-samples", None),
            ]
        );
        assert_eq!(index.kits[1].license, "CC BY-NC-ND 4.0");
        assert_eq!(index.kits[1].path, format!("{root}/kb6/Akai MPC2000"));
        assert_eq!(
            index.sources,
            [("kb6".to_string(), 2), ("MusicRadar".to_string(), 2)]
        );
        assert_eq!(
            index.roots,
            [RootInfo {
                path: root.clone(),
                present: true,
                kits: 4
            }]
        );
        // The cache gives the same index back.
        let again = Cache::from_json(&cache.to_json());
        assert_eq!(again, cache);
        assert_eq!(again.index(std::slice::from_ref(&root)), index);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_folder_is_no_error_and_the_others_are_shown() {
        let dir = library("missing");
        let root = dir.to_string_lossy().into_owned();
        let gone = "/Volumes/NOT-MOUNTED-HERE/DRUM-KITS".to_string();
        let folders = vec![gone.clone(), root.clone()];
        let (index, cache) = scan(&folders, Cache::default());
        assert_eq!(index.kits.len(), 4);
        assert_eq!(
            index.roots[0],
            RootInfo {
                path: gone.clone(),
                present: false,
                kits: 0
            }
        );
        assert!(index.roots[1].present);
        // A drive that goes away: its kits leave the index and stay in the cache.
        std::fs::rename(&dir, dir.with_extension("away")).unwrap();
        let (index2, cache2) = scan(&folders, cache.clone());
        assert!(index2.kits.is_empty());
        assert_eq!(cache2.roots[&root].len(), 4);
        assert!(cache2.index(&folders).kits.is_empty());
        std::fs::rename(dir.with_extension("away"), &dir).unwrap();
        assert_eq!(cache2.index(&folders).kits.len(), 4);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_folder_without_a_manifest_tells_kits_from_sources() {
        let dir = temp("plain");
        // A pack with its sounds in folders: one kit.
        touch(&dir, "OZ Pack/Kicks/k.wav", "");
        touch(&dir, "OZ Pack/Snares/s.wav", "");
        touch(&dir, "OZ Pack/Hi Hats/h.wav", "");
        // A kit with its files right in it.
        touch(&dir, "Lone Kit/kick.wav", "");
        // A maker with two kits.
        touch(&dir, "Maker/Kit A/kick.wav", "");
        touch(&dir, "Maker/Kit B/sub/kick.wav", "");
        // Loose files in the library folder itself.
        touch(&dir, "clap.wav", "");
        let root = dir.to_string_lossy().into_owned();
        let own = dir.file_name().unwrap().to_string_lossy().into_owned();
        let kits = scan_root(&root).unwrap();
        let mut got: Vec<(String, String)> = kits
            .iter()
            .map(|k| (k.source.clone(), k.name.clone()))
            .collect();
        got.sort();
        let mut want = vec![
            (own.clone(), own.clone()),
            (own.clone(), "Lone Kit".to_string()),
            (own.clone(), "OZ Pack".to_string()),
            ("Maker".to_string(), "Kit A".to_string()),
            ("Maker".to_string(), "Kit B".to_string()),
        ];
        want.sort();
        assert_eq!(got, want);
        assert!(scan_root("/no/such/folder/anywhere").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn sample_index() -> Index {
        let mut kits = Vec::new();
        for (s, n) in [
            ("kb6", "Roland TR-808"),
            ("kb6", "Roland TR-909"),
            ("kb6", "Akai MPC2000"),
            ("MusicRadar", "808-samples"),
            ("MusicRadar", "Empty Thing"),
            ("Boxed-Ear", "CR-78"),
        ] {
            kits.push(KitEntry::new("/lib", format!("/lib/{s}/{n}"), s, n));
        }
        kits[4].files = Some(0);
        kits[5].best = Some(1);
        kits[0].best = Some(2);
        Index {
            kits,
            ..Index::default()
        }
        .finish()
    }

    #[test]
    fn a_search_matches_every_word_in_names_and_sources() {
        let index = sample_index();
        let names = |f: &Filter, fav: &HashSet<String>| -> Vec<String> {
            search(&index, f, fav)
                .iter()
                .map(|i| index.kits[*i as usize].name.clone())
                .collect()
        };
        let none = HashSet::new();
        let q = |s: &str| Filter {
            query: s.into(),
            ..Filter::default()
        };
        // Everything, in the index's order, without the kit known to be empty.
        assert_eq!(
            names(&Filter::default(), &none),
            [
                "CR-78",
                "Akai MPC2000",
                "Roland TR-808",
                "Roland TR-909",
                "808-samples"
            ]
        );
        // A name that starts with the query comes first.
        assert_eq!(names(&q("808"), &none), ["808-samples", "Roland TR-808"]);
        assert_eq!(names(&q("ROLAND kb6 9"), &none), ["Roland TR-909"]);
        assert_eq!(names(&q("musicradar"), &none), ["808-samples"]);
        assert!(names(&q("zzz"), &none).is_empty());
        let by_source = Filter {
            source: Some("kb6".into()),
            ..Filter::default()
        };
        assert_eq!(names(&by_source, &none).len(), 3);
        let best = Filter {
            best: true,
            ..Filter::default()
        };
        assert_eq!(names(&best, &none), ["CR-78", "Roland TR-808"]);
        let fav: HashSet<String> = ["/lib/kb6/Akai MPC2000".to_string()].into();
        let favs = Filter {
            favorites: true,
            ..Filter::default()
        };
        assert_eq!(names(&favs, &fav), ["Akai MPC2000"]);
    }

    #[test]
    fn a_kit_list_narrows_the_kits_to_its_own_in_its_order_by_type() {
        let mut index = sample_index();
        index.roots = vec![RootInfo {
            path: "/lib".into(),
            present: true,
            kits: 0,
        }];
        let text = "rank\tsource\tkit\tpath\ttype\tfile_count\tscore\treason\n\
            3\tMusicRadar\t808-samples\tMusicRadar/808-samples\tHip-hop\t398\t70\tbig\n\
            1\tkb6\tRoland TR-909\t/Volumes/SHARE/lib/kb6/Roland TR-909/\tDrum machines\t40\t90\tthe 909\n\
            2\tBoxed-Ear\tCR-78\t./boxed-ear/cr-78\tDrum machines\t30\t80\tclean\n\
            4\tNowhere\tGone\tNowhere/Gone\tHip-hop\t1\t1\tnot here\n\
            5\tBoxed-Ear\tCR-78\t./Boxed-Ear/CR-78/\tOther\t1\t1\ttwice\n";
        let list = KitList::parse(text, "/x/list.tsv").unwrap();
        assert_eq!(list.rows.len(), 4, "a kit named twice counts once");
        assert_eq!(list.types, ["Drum machines", "Hip-hop"]);
        index.apply_list(Some(&list));
        assert_eq!(
            index.types,
            [("Drum machines".to_string(), 2), ("Hip-hop".to_string(), 1)]
        );
        assert_eq!(index.list_matched(), 3);
        let fav: HashSet<String> = ["/lib/kb6/Akai MPC2000".to_string()].into();
        let names = |f: &Filter, default_kit: &str| -> Vec<String> {
            search_keeping(&index, f, &fav, default_kit)
                .iter()
                .map(|i| index.kits[*i as usize].name.clone())
                .collect()
        };
        let on = Filter {
            list: true,
            ..Filter::default()
        };
        // The favourite first, then the list type by type in rank order: the full path from
        // another mount and the one in other letters and with ./ are found too.
        assert_eq!(
            names(&on, ""),
            ["Akai MPC2000", "Roland TR-909", "CR-78", "808-samples"]
        );
        // The kit of new racks is kept too, also when it names a folder inside a kit.
        assert_eq!(
            names(&on, "/lib/kb6/Roland TR-808/Best Of/OZ"),
            [
                "Akai MPC2000",
                "Roland TR-808",
                "Roland TR-909",
                "CR-78",
                "808-samples"
            ]
        );
        assert_eq!(
            names(&on, "/lib/kb6/Roland TR-80"),
            ["Akai MPC2000", "Roland TR-909", "CR-78", "808-samples"],
            "not a kit whose name only starts the same"
        );
        let hip = Filter {
            kind: Some("Hip-hop".into()),
            ..on.clone()
        };
        assert_eq!(names(&hip, ""), ["808-samples"]);
        let q = Filter {
            query: "roland".into(),
            source: Some("Boxed-Ear".into()),
            ..on.clone()
        };
        assert_eq!(names(&q, ""), ["Roland TR-909"], "the source is not used");
        // Off: as without a list.
        assert_eq!(names(&Filter::default(), "").len(), 5);
        let hip_off = Filter {
            list: false,
            ..hip.clone()
        };
        assert_eq!(names(&hip_off, "").len(), 5);
        // Without one, no kit has a place.
        index.apply_list(None);
        assert!(index.types.is_empty() && index.kits.iter().all(|k| k.list.is_none()));
        // Rows without a rank column are in the file's order; no path column is an error.
        let plain = KitList::parse("path\na/b\nc/d\n", "f").unwrap();
        assert_eq!(
            plain.rows.iter().map(|r| r.rank).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(plain.types, ["Other"]);
        assert!(KitList::parse("kit\tsource\nx\ty\n", "f").is_err());
        assert!(KitList::parse("", "f").is_err());
    }

    #[test]
    fn a_kit_list_set_on_one_mac_is_found_on_the_other_mount() {
        let lib = temp("list-mount");
        touch(&lib, "BEST-PROWAVE.tsv", "path\nkb6/x\n");
        let folders = vec![
            "/Volumes/NOT-MOUNTED-HERE/DRUM-KITS".to_string(),
            lib.to_string_lossy().into_owned(),
        ];
        let name = lib.file_name().unwrap().to_string_lossy().into_owned();
        let want = Some(lib.join("BEST-PROWAVE.tsv"));
        for setting in [
            lib.join("BEST-PROWAVE.tsv").to_string_lossy().into_owned(),
            format!("/Volumes/SHARE/{name}/BEST-PROWAVE.tsv"),
            "/somewhere/else/BEST-PROWAVE.tsv".to_string(),
            "BEST-PROWAVE.tsv".to_string(),
        ] {
            assert_eq!(find_list_file(&setting, &folders), want, "{setting}");
        }
        assert_eq!(find_list_file("", &folders), None);
        assert_eq!(find_list_file("/x/NOPE.tsv", &folders), None);
        assert!(load_list("/x/NOPE.tsv", &folders).is_err());
        assert_eq!(
            load_list("BEST-PROWAVE.tsv", &folders).unwrap().rows[0].path,
            "kb6/x"
        );
        // The setting is kept with the others; on unless turned off.
        let s = Settings::from_json(r#"{"kit_list":" /k/BEST-PROWAVE.tsv "}"#);
        assert_eq!(s.kit_list, "/k/BEST-PROWAVE.tsv");
        assert!(s.kit_list_on);
        assert!(Settings::default().kit_list.is_empty());
        let off = Settings {
            kit_list_on: false,
            ..s
        };
        assert_eq!(Settings::from_json(&off.to_json()), off);
        let _ = std::fs::remove_dir_all(lib);
    }

    #[test]
    fn a_search_over_a_hundred_thousand_kits_is_quick() {
        let mut kits = Vec::new();
        for i in 0..100_000 {
            let s = format!("Source {}", i % 300);
            let n = format!("Drum Kit Number {i} Deluxe");
            kits.push(KitEntry::new("/lib", format!("/lib/{s}/{n}"), &s, &n));
        }
        let index = Index {
            kits,
            ..Index::default()
        }
        .finish();
        let none = HashSet::new();
        let started = std::time::Instant::now();
        let hits = search(
            &index,
            &Filter {
                query: "kit 4242 source".into(),
                ..Filter::default()
            },
            &none,
        );
        let took = started.elapsed();
        assert_eq!(hits.len(), 20, "4242, 14242 to 94242, 42420 to 42429");
        // Debug builds on a busy machine: far above what it takes, far below a stall.
        assert!(took.as_millis() < 500, "{took:?}");
        let cache = Cache {
            roots: [("/lib".to_string(), index.kits.clone())].into(),
        };
        let text = cache.to_json();
        assert_eq!(Cache::from_json(&text).roots["/lib"].len(), 100_000);
    }

    #[test]
    fn best_md_ranks_the_kits_it_names() {
        let kits: Vec<(String, String)> = [
            ("kb6", "Roland TR-808"),
            ("kb6", "Akai MPC2000"),
            ("MusicRadar", "808-samples"),
            ("Boxed-Ear", "CR-78"),
            ("Other", "CR-78"),
        ]
        .iter()
        .map(|(s, n)| (s.to_string(), n.to_string()))
        .collect();
        let text = "# Best kits\n\nRanked shortlist.\n\n\
            | # | Source | Kit | Why |\n|---|---|---|---|\n\
            | 1 | Boxed-Ear | CR-78 | clean |\n\
            2. **Akai MPC2000** (kb6): punchy\n\
            - `MusicRadar/808-samples/` big\n\
            - kb6 / Roland TR-808\n\
            - [Akai MPC2000](https://x) again\n\
            - Nothing Known\n";
        let got = parse_best(text, &kits);
        assert_eq!(
            got.get(&3),
            Some(&1),
            "the table row, told from Other's CR-78"
        );
        assert_eq!(got.get(&1), Some(&2));
        assert_eq!(got.get(&2), Some(&3));
        assert_eq!(got.get(&0), Some(&4));
        assert_eq!(got.len(), 4);
        // A name two sources share, with no source on the line, ranks nothing.
        assert!(parse_best("- CR-78\n", &kits).is_empty());
    }

    #[test]
    fn a_search_finds_look_alike_letters_by_their_plain_ones() {
        assert_eq!(fold("Best of ᴏᴢ ᴀɴᴅ ʙᴇᴀᴢʏ"), "best of oz and beazy");
        assert_eq!(fold("𝐎𝐙 ＯＺ ⓞⓩ 𝟗𝟎𝟗"), "oz oz oz 909");
        assert_eq!(fold("Café Bøøm ß"), "cafe boom ss");
        assert_eq!(fold("ÄÖÜ ɪ"), "aou i");
        let kit = KitEntry::new(
            "/lib",
            "/lib/R/K".into(),
            "Reddit-Drumkits",
            "My Favorite Knocking Drums (Best of ᴏᴢ ᴀɴᴅ ʙᴇᴀᴢʏ) [ezulpq]",
        );
        let index = Index::of(vec![kit], Vec::new());
        let none = HashSet::new();
        for q in ["oz", "OZ", "beazy", "ᴏᴢ", "knocking oz"] {
            let f = Filter {
                query: q.into(),
                ..Filter::default()
            };
            assert_eq!(search(&index, &f, &none), [0], "{q}");
        }
    }

    #[test]
    fn settings_and_the_manifest_read_what_they_are_given() {
        let s = Settings::from_json(r#"{"folders":["/a","/b","/a"," "],"favorites":["/a/k"]}"#);
        assert_eq!(s.folders, ["/a", "/b"]);
        assert!(s.favorites.contains("/a/k"));
        assert_eq!(Settings::from_json(&s.to_json()), s);
        assert_eq!(Settings::from_json("not json"), Settings::default());
        // No folders is a choice, kept as it is.
        assert!(Settings::from_json(r#"{"folders":[]}"#).folders.is_empty());
        // The kit of new racks: the installation's when the file does not name one, none when
        // it names none.
        assert_eq!(Settings::from_json("{}").default_kit, default_kit());
        assert_eq!(Settings::from_json(r#"{"default_kit":""}"#).default_kit, "");
        let k = Settings::from_json(r#"{"default_kit":" /k/OZ "}"#);
        assert_eq!(k.default_kit, "/k/OZ");
        assert_eq!(Settings::from_json(&k.to_json()), k);
        // Copying loaded kits here: only from other computers unless set to always.
        assert_eq!(k.copy_kits, crate::kit_copy::CopyMode::Remote);
        let a = Settings::from_json(r#"{"copy_kits":"always"}"#);
        assert_eq!(a.copy_kits, crate::kit_copy::CopyMode::Always);
        assert_eq!(Settings::from_json(&a.to_json()), a);
        let dir = temp("settings");
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        assert_eq!(Settings::load(&dir.join("nothing")), Settings::default());
        assert!(parse_manifest("").is_empty());
        assert!(
            parse_manifest("a\tb\nc\td\n").is_empty(),
            "no source and kit columns"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Runs a scan to its end: the index after the folders are read, the one after every
    /// kit's files are, and how many kits were read.
    fn scan_through(folders: Vec<String>, data: &Path, full: bool) -> (Index, Index, usize) {
        let job = spawn_scan(folders, Some(data.to_path_buf()), full);
        let mut scanned = None;
        loop {
            match job.rx.recv().unwrap() {
                ScanMsg::Scanned(i, _) => scanned = Some(i),
                ScanMsg::Listed(i, read, _) => return (scanned.unwrap(), i, read),
                _ => {}
            }
        }
    }

    #[test]
    fn every_kits_files_are_read_and_kept_then_only_changed_ones_again() {
        let lib = library("listing");
        let root = lib.to_string_lossy().into_owned();
        let data = temp("listing-data");
        let (first, listed, read) = scan_through(vec![root.clone()], &data, false);
        let path = format!("{root}/MusicRadar/909-samples");
        assert_eq!(first.find(&path).unwrap().files, None, "not read yet");
        assert_eq!(read, 4);
        let k = listed.find(&path).unwrap();
        assert_eq!((k.files, k.pads), (Some(1), Some(1)));
        assert!(k.listed.is_some());
        // The manifest's count gives way to what is there.
        assert_eq!(
            listed
                .find(&format!("{root}/kb6/Akai MPC2000"))
                .unwrap()
                .files,
            Some(1)
        );
        // Nothing changed: nothing is read again, and the counts come from the cache at once.
        let (first, _, read) = scan_through(vec![root.clone()], &data, false);
        assert_eq!(read, 0);
        assert_eq!(first.find(&path).unwrap().files, Some(1));
        // A full scan reads them all.
        let (_, _, read) = scan_through(vec![root.clone()], &data, true);
        assert_eq!(read, 4);
        // A kit is made from its kept listing, with its folder gone.
        let entry = listed.find(&path).unwrap().clone();
        std::fs::rename(&lib, lib.with_extension("away")).unwrap();
        let opened = open(Some(&data), &entry, &AtomicBool::new(false));
        assert!(opened.fresh.is_empty(), "nothing read from the disk");
        assert_eq!(opened.kit.samples[0].rel, "Kick.wav");
        assert_eq!(opened.kit.pads, [Some(0)]);
        // Without a kept listing the folder is read (and there is none now).
        let opened = open(None, &entry, &AtomicBool::new(false));
        assert!(opened.kit.samples.is_empty());
        std::fs::rename(lib.with_extension("away"), &lib).unwrap();
        // A changed folder is read again.
        touch(&lib, "MusicRadar/909-samples/Snare.wav", "");
        let old = std::time::SystemTime::now() - Duration::from_secs(3600);
        let cache = Cache::load(&data);
        let mut c2 = cache.clone();
        for k in c2.roots.get_mut(&root).unwrap() {
            if k.path == path {
                k.listed = Some(old.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
            }
        }
        c2.save(&data).unwrap();
        let (_, listed, read) = scan_through(vec![root.clone()], &data, false);
        assert_eq!(read, 1);
        assert_eq!(listed.find(&path).unwrap().files, Some(2));
        let _ = std::fs::remove_dir_all(lib);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn listings_are_kept_by_folder() {
        let data = temp("keep");
        let l = Listing {
            audio: vec!["a/kick.wav".into()],
            ..Listing::empty()
        };
        save_listing(&data, "/lib/s/k", &l).unwrap();
        assert_eq!(load_listing(&data, "/lib/s/k"), Some(l));
        assert_eq!(load_listing(&data, "/lib/s/other"), None);
        // Another folder whose file name is the same is not taken for this one.
        let file = listing_file(&data, "/lib/s/k");
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text.replacen("/lib/s/k", "/lib/s/z", 1)).unwrap();
        assert_eq!(load_listing(&data, "/lib/s/k"), None);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn a_source_of_one_sample_folders_is_one_pool() {
        let dir = temp("pool");
        for i in 0..30 {
            let name = ["Kick", "Snare", "Closed Hat"][i % 3];
            touch(&dir, &format!("FreeWave/{name} {i}/{name} {i}.wav"), "");
        }
        // One folder with more than one file stays a kit of its own.
        touch(&dir, "FreeWave/Big Kit/kick.wav", "");
        touch(&dir, "FreeWave/Big Kit/snare.wav", "");
        touch(&dir, "Maker/Kit A/kick.wav", "");
        // Listed by the manifest (without counts): folders named for drum sounds would make
        // the source one kit otherwise.
        let mut manifest = String::from("source\tkit\nMaker\tKit A\nFreeWave\tBig Kit\n");
        for i in 0..30 {
            let name = ["Kick", "Snare", "Closed Hat"][i % 3];
            manifest.push_str(&format!("FreeWave\t{name} {i}\n"));
        }
        touch(&dir, "MANIFEST.tsv", &manifest);
        let root = dir.to_string_lossy().into_owned();
        let data = temp("pool-data");
        let (first, listed, _) = scan_through(vec![root.clone()], &data, false);
        assert_eq!(first.kits.len(), 32, "counts unknown: no pool yet");
        let names: Vec<(&str, &str)> = listed
            .kits
            .iter()
            .map(|k| (k.source.as_str(), k.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("FreeWave", "Big Kit"),
                ("FreeWave", "FreeWave"),
                ("Maker", "Kit A")
            ]
        );
        let pool = listed.find(&format!("{root}/FreeWave")).unwrap();
        assert!(pool.is_pool());
        assert_eq!(pool.files, Some(30));
        assert_eq!(pool.members.len(), 30);
        assert_eq!(
            listed.sources,
            [("FreeWave".to_string(), 2), ("Maker".to_string(), 1)]
        );
        let kit = open(Some(&data), pool, &AtomicBool::new(false)).kit;
        assert_eq!(kit.samples.len(), 30);
        assert_eq!(kit.samples[0].rel, "Closed Hat 2/Closed Hat 2.wav");
        assert!(kit.path_of(0).is_file());
        assert_eq!(kit.pads.len(), crate::kit_classify::KIT_PADS);
        // The pool is in the cached index the next time.
        assert!(Cache::load(&data)
            .index(&[root])
            .kits
            .iter()
            .any(|k| k.is_pool()));
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn a_manifest_of_eight_or_nine_columns_and_kits_with_the_same_files() {
        let eight =
            "source\tkit\turl\tlicense\tfile_count\tbytes\tdate\taudio_or_instrument_files\n\
                     A\tOne\thttps://x\tCC0\t3\t9\t2026-11-24\t2\n";
        let m = parse_manifest(eight);
        assert_eq!(m[&("A".to_string(), "One".to_string())].files, Some(2));
        assert!(m[&("A".to_string(), "One".to_string())].aliases.is_empty());
        let nine = "source\tkit\turl\tlicense\tfile_count\tbytes\tdate\taudio_or_instrument_files\tother_names_same_content\n\
                    A\tOne\thttps://x\tCC0\t3\t9\t2026-11-24\t2\tB/Uno; Two\n\
                    B\tUno\thttps://y\tCC0\t3\t9\t2026-11-24\t2\tA/One\n\
                    A\tTwo\thttps://y\tCC0\t3\t9\t2026-11-24\t2\t\n\
                    A\tThree\thttps://y\tCC0\t3\t9\t2026-11-24\t2\n";
        let m = parse_manifest(nine);
        assert_eq!(
            m[&("A".to_string(), "One".to_string())].aliases,
            ["B/Uno", "A/Two"]
        );
        assert!(
            m[&("A".to_string(), "Three".to_string())]
                .aliases
                .is_empty(),
            "a short row"
        );
        // Shown once: the first of them, found by the others' names, ranked by the best rank.
        let mut raw = Vec::new();
        for (s, n) in [("A", "One"), ("B", "Uno"), ("A", "Two"), ("A", "Three")] {
            let mut k = KitEntry::new("/lib", format!("/lib/{s}/{n}"), s, n);
            k.aliases = m[&(s.to_string(), n.to_string())].aliases.clone();
            raw.push(k);
        }
        raw[1].best = Some(4);
        let index = Index::of(raw, Vec::new());
        let names: Vec<&str> = index.kits.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(names, ["One", "Three"]);
        assert_eq!(index.kits[0].best, Some(4));
        let none = HashSet::new();
        let q = Filter {
            query: "uno".into(),
            ..Filter::default()
        };
        assert_eq!(search(&index, &q, &none), [0]);
    }

    #[test]
    fn best_md_as_the_library_writes_it() {
        let kits: Vec<(String, String)> = [
            ("Hyperreal-Machines", "Roland TR-808"),
            ("Goldbaby", "SP1200 Free"),
            ("sfzinstruments-GitHub", "DrumGizmo.DRSKit"),
        ]
        .iter()
        .map(|(s, n)| (s.to_string(), n.to_string()))
        .collect();
        let text = "# BEST\n\nHighest-quality sets on this drive, best first within each group.\n\n\
            Hardware drum machines\n\n\
            `Hyperreal-Machines/Roland TR-808` - Original TR-808 one-shots across tone, decay and snappy settings\n\
            `Goldbaby/SP1200 Free` - Sounds sampled through a real E-mu SP-1200 by Goldbaby\n\
            `Nobody/Not Here` - gone\n\n\
            Acoustic multisampled kits\n\n\
            `sfzinstruments-GitHub/DrumGizmo.DRSKit` - Multi-mic acoustic kit, 6,061 samples, CC-BY 4.0\n";
        let got = parse_best(text, &kits);
        assert_eq!(got.get(&0), Some(&1));
        assert_eq!(got.get(&1), Some(&2));
        assert_eq!(got.get(&2), Some(&3));
    }

    /// How the classifier does on a real library: every listing kept in
    /// `$PROWAVE_KIT_LISTINGS` (a `kit-library/kits` folder), the share of kits whose first
    /// channel is a kick, second a snare (or clap) and fourth a hat, the kits that miss, and
    /// the commonest words of the names classed "other". Written to
    /// `$PROWAVE_KIT_SURVEY_OUT` as JSON.
    #[test]
    #[ignore = "needs a real library's listings"]
    fn classifier_survey() {
        use crate::kit_classify::{words, DrumClass::*};
        let dir = std::env::var("PROWAVE_KIT_LISTINGS").expect("PROWAVE_KIT_LISTINGS");
        let mut kits = 0usize;
        let mut drum_kits = 0usize;
        let mut full = 0usize;
        let (mut no_kick, mut no_snare, mut no_hat) = (0usize, 0usize, 0usize);
        let mut other_words: HashMap<String, usize> = HashMap::new();
        let mut misses: Vec<Value> = Vec::new();
        let mut picks: Vec<Value> = Vec::new();
        let mut files = 0usize;
        let mut other_files = 0usize;
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some((folder, rest)) = text.split_once('\n') else {
                continue;
            };
            let Some(listing) = Listing::from_text(rest) else {
                continue;
            };
            let kit = listing.kit(Path::new(folder));
            kits += 1;
            let audio: Vec<&crate::kit_files::KitSample> = kit
                .samples
                .iter()
                .filter(|s| s.class != SoundFont)
                .collect();
            files += audio.len();
            for s in &audio {
                if s.class == Other {
                    other_files += 1;
                    let mut seen = HashSet::new();
                    for w in words(&s.rel) {
                        if w.chars().all(|c| c.is_ascii_digit()) || !seen.insert(w.clone()) {
                            continue;
                        }
                        *other_words.entry(w).or_default() += 1;
                    }
                }
            }
            // A kit of drum one-shots: three or more audio files, not all loops.
            let shots = audio.iter().filter(|s| s.class != Loop).count();
            if shots < 3 {
                continue;
            }
            drum_kits += 1;
            let class_at = |k: usize| {
                kit.pads
                    .get(k)
                    .copied()
                    .flatten()
                    .map(|i| kit.samples[i].class)
            };
            let kick = class_at(0) == Some(Kick);
            let snare = matches!(class_at(1), Some(Snare | Clap));
            let hat = matches!(class_at(3), Some(ClosedHat | OpenHat));
            no_kick += usize::from(!kick);
            no_snare += usize::from(!snare);
            no_hat += usize::from(!hat);
            let name_at = |k: usize| {
                kit.pads
                    .get(k)
                    .copied()
                    .flatten()
                    .map(|i| kit.samples[i].rel.clone())
            };
            if kick && snare && hat {
                full += 1;
                if picks.len() < 400 {
                    picks.push(json!([folder, name_at(0), name_at(1), name_at(3)]));
                }
            } else {
                let others: Vec<&str> = audio
                    .iter()
                    .filter(|s| s.class == Other)
                    .take(12)
                    .map(|s| s.rel.as_str())
                    .collect();
                misses.push(json!({
                    "kit": folder,
                    "shots": shots,
                    "kick": kick, "snare": snare, "hat": hat,
                    "classes": kit.classes.iter().map(|(c, n)| format!("{}:{n}", c.id())).collect::<Vec<_>>(),
                    "other_names": others,
                }));
            }
        }
        let mut top: Vec<(String, usize)> = other_words.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top.truncate(150);
        let pct = |n: usize| (n as f64 * 1000.0 / drum_kits.max(1) as f64).round() / 10.0;
        let report = json!({
            "kits": kits,
            "drum_kits": drum_kits,
            "kick_snare_hat": full,
            "kick_snare_hat_pct": pct(full),
            "no_kick": no_kick, "no_snare": no_snare, "no_hat": no_hat,
            "audio_files": files,
            "other_files": other_files,
            "other_pct": (other_files as f64 * 1000.0 / files.max(1) as f64).round() / 10.0,
            "other_words": top,
            "misses": misses,
            "picks": picks,
        });
        eprintln!(
            "kits {kits}, drum kits {drum_kits}, kick+snare+hat {full} ({}%), no kick {no_kick}, no snare {no_snare}, no hat {no_hat}, other files {other_files} of {files}",
            pct(full)
        );
        if let Some(out) = std::env::var_os("PROWAVE_KIT_SURVEY_OUT") {
            std::fs::write(out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
        }
    }
}
