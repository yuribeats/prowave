//! Kits kept on this computer: when a kit (or one sample) from a library folder on another
//! computer (a network share such as another Mac's drive) is loaded into a Channel Rack, its
//! samples are copied into `kit-library/local/<source>/<kit>/` of the settings folder and the
//! rack plays the copies, so a project keeps its sounds when the share is not there. Browsing
//! and auditioning read the share itself; only what is loaded is copied.
//!
//! [`CopyMode::Remote`] (the default) copies only from folders on a network file system;
//! [`CopyMode::Always`] copies from every folder that is not already the local copy.
use std::path::{Path, PathBuf};

/// When samples loaded into a rack are copied to this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CopyMode {
    /// Only from folders on another computer (a network share).
    #[default]
    Remote,
    /// From every folder.
    Always,
}

impl CopyMode {
    pub fn id(self) -> &'static str {
        match self {
            CopyMode::Remote => "remote",
            CopyMode::Always => "always",
        }
    }

    pub fn parse(s: &str) -> Option<CopyMode> {
        match s.trim() {
            "remote" => Some(CopyMode::Remote),
            "always" => Some(CopyMode::Always),
            _ => None,
        }
    }
}

/// Whether `path` is on a network file system (SMB, AFP, NFS, WebDAV), as macOS reports it.
/// A path that is not there, or another system: false.
pub fn is_remote(path: &Path) -> bool {
    fs_type(path).is_some_and(|t| {
        matches!(
            t.as_str(),
            "smbfs" | "afpfs" | "nfs" | "webdav" | "cifs" | "ftp"
        )
    })
}

/// The name of the file system `path` is on.
#[cfg(target_os = "macos")]
fn fs_type(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated string and `st` is a plain struct statfs fills in;
    // nothing is read from it unless the call succeeded.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let name: Vec<u8> = st
        .f_fstypename
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    Some(String::from_utf8_lossy(&name).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn fs_type(_path: &Path) -> Option<String> {
    None
}

/// A folder name made safe: no path separators, not empty, not starting with a dot.
fn safe(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() {
        "_".to_string()
    } else {
        s
    }
}

/// Where the local copy of kit `kit` of source `source` goes in `local`.
pub fn kit_dir(local: &Path, source: &str, kit: &str) -> PathBuf {
    local.join(safe(source)).join(safe(kit))
}

/// Copies `src` to `dest` unless a file of the same size is already there (written beside it
/// first and then renamed, so a copy cut short is never taken for a whole one).
pub fn copy_file(src: &Path, dest: &Path) -> std::io::Result<()> {
    let len = std::fs::metadata(src)?.len();
    if std::fs::metadata(dest).is_ok_and(|m| m.is_file() && m.len() == len) {
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::copy(src, &part)?;
    std::fs::rename(&part, dest)
}

/// Where the rack should play `file` from, copying it first when `mode` says so. `kit`: the
/// kit it is part of (its folder, source and name); a file outside it, or with no kit, goes in
/// `_loose/<its folder's name>/`. A file already inside `local`, a copy that fails (logged), or
/// no `local`: `file` itself.
pub fn localize(
    local: Option<&Path>,
    mode: CopyMode,
    kit: Option<(&Path, &str, &str)>,
    file: &Path,
) -> PathBuf {
    let Some(local) = local else {
        return file.to_path_buf();
    };
    if file.starts_with(local) || file.as_os_str().is_empty() {
        return file.to_path_buf();
    }
    if mode == CopyMode::Remote && !is_remote(file) {
        return file.to_path_buf();
    }
    let dest = match kit
        .and_then(|(root, source, name)| Some((file.strip_prefix(root).ok()?, source, name)))
    {
        Some((rel, source, name)) => kit_dir(local, source, name).join(rel),
        None => {
            let folder = file
                .parent()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Some(name) = file.file_name() else {
                return file.to_path_buf();
            };
            kit_dir(local, "_loose", &folder).join(name)
        }
    };
    match copy_file(file, &dest) {
        Ok(()) => dest,
        Err(e) => {
            tracing::warn!("could not copy {} here: {e}", file.display());
            file.to_path_buf()
        }
    }
}

/// [`localize`] for every file of a kit load (an empty channel stays empty).
pub fn localize_pads(
    local: Option<&Path>,
    mode: CopyMode,
    kit: Option<(&Path, &str, &str)>,
    pads: &[Option<PathBuf>],
) -> Vec<Option<PathBuf>> {
    pads.iter()
        .map(|p| p.as_ref().map(|f| localize(local, mode, kit, f)))
        .collect()
}

/// Copies the whole kit in folder `dir` (its audio and SoundFont files) into `local` when
/// `mode` says so, and returns the folder to use: the copy, or `dir` itself.
pub fn localize_kit_folder(local: Option<&Path>, mode: CopyMode, dir: &Path) -> PathBuf {
    let Some(local) = local else {
        return dir.to_path_buf();
    };
    if dir.starts_with(local) || (mode == CopyMode::Remote && !is_remote(dir)) {
        return dir.to_path_buf();
    }
    let name = |p: Option<&Path>| {
        p.and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let to = kit_dir(local, &name(dir.parent()), &name(Some(dir)));
    let kit = crate::kit_files::read_kit(dir);
    let mut ok = true;
    for s in &kit.samples {
        if let Err(e) = copy_file(&kit.root.join(&s.rel), &to.join(&s.rel)) {
            tracing::warn!("could not copy {} here: {e}", s.rel);
            ok = false;
        }
    }
    if ok && !kit.samples.is_empty() {
        to
    } else {
        dir.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn a_loaded_kit_is_copied_into_the_local_folder_and_played_from_there() {
        let lib = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let kit = lib.path().join("Src/Kit One");
        let kick = kit.join("Kicks/Kick 1.wav");
        let snare = kit.join("Snare.wav");
        write(&kick, b"RIFFkick");
        write(&snare, b"RIFFsnare");
        let k = Some((kit.as_path(), "Src", "Kit/One"));
        // A local folder (the temp folder) is copied only with Always.
        assert_eq!(
            localize(Some(local.path()), CopyMode::Remote, k, &kick),
            kick
        );
        let pads = vec![Some(kick.clone()), None, Some(snare.clone())];
        let got = localize_pads(Some(local.path()), CopyMode::Always, k, &pads);
        let dir = local.path().join("Src").join("Kit_One");
        assert_eq!(
            got,
            vec![
                Some(dir.join("Kicks/Kick 1.wav")),
                None,
                Some(dir.join("Snare.wav"))
            ]
        );
        assert_eq!(
            std::fs::read(dir.join("Kicks/Kick 1.wav")).unwrap(),
            b"RIFFkick"
        );
        // Loading again copies nothing new and gives the same files; a copy is not copied.
        let again = localize_pads(Some(local.path()), CopyMode::Always, k, &pads);
        assert_eq!(again, got);
        let copy = dir.join("Snare.wav");
        assert_eq!(
            localize(Some(local.path()), CopyMode::Always, None, &copy),
            copy
        );
        // A file changed at its source (another size) is copied again.
        write(&snare, b"RIFFsnare2");
        localize(Some(local.path()), CopyMode::Always, k, &snare);
        assert_eq!(std::fs::read(&copy).unwrap(), b"RIFFsnare2");
        // One sample with no kit: _loose/<its folder>/.
        let one = lib.path().join("Elsewhere/Clap.wav");
        write(&one, b"RIFFclap");
        assert_eq!(
            localize(Some(local.path()), CopyMode::Always, None, &one),
            local.path().join("_loose/Elsewhere/Clap.wav")
        );
        // A file that can not be read, or no local folder: the file itself.
        let gone = lib.path().join("gone.wav");
        assert_eq!(
            localize(Some(local.path()), CopyMode::Always, None, &gone),
            gone
        );
        assert_eq!(localize(None, CopyMode::Always, k, &kick), kick);
        // No part files are left behind.
        let parts = walk(local.path())
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "part"))
            .count();
        assert_eq!(parts, 0);
    }

    fn walk(d: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn a_whole_kit_folder_is_copied_for_the_default_kit() {
        let lib = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let kit = lib.path().join("OZ & KBeazy/Best Of OZ");
        write(&kit.join("Kicks/Kick (A).wav"), b"RIFF1");
        write(&kit.join("Fx (Nicki).wav"), b"RIFF2");
        write(&kit.join("notes.txt"), b"not audio");
        let to = localize_kit_folder(Some(local.path()), CopyMode::Always, &kit);
        assert_eq!(to, local.path().join("OZ & KBeazy/Best Of OZ"));
        assert!(to.join("Kicks/Kick (A).wav").is_file());
        assert!(to.join("Fx (Nicki).wav").is_file());
        assert!(!to.join("notes.txt").exists(), "only the kit's sounds");
        // Already local, or local and Remote: as it is.
        assert_eq!(
            localize_kit_folder(Some(local.path()), CopyMode::Always, &to),
            to
        );
        assert_eq!(
            localize_kit_folder(Some(local.path()), CopyMode::Remote, &kit),
            kit
        );
        let none = lib.path().join("none");
        assert_eq!(
            localize_kit_folder(Some(local.path()), CopyMode::Always, &none),
            none
        );
    }

    #[test]
    fn local_folders_are_not_remote_and_modes_read_back() {
        assert!(!is_remote(Path::new("/")));
        assert!(!is_remote(&std::env::temp_dir()));
        assert!(!is_remote(Path::new("/no/such/folder")));
        for m in [CopyMode::Remote, CopyMode::Always] {
            assert_eq!(CopyMode::parse(m.id()), Some(m));
        }
        assert_eq!(CopyMode::parse("x"), None);
        assert_eq!(safe(".hidden/x"), "hidden_x");
        assert_eq!(safe(""), "_");
    }
}
