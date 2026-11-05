use crate::api::{Track, valid_bvid};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TRACKS: usize = 3000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayMode {
    #[default]
    Sequential,
    RepeatOne,
    RepeatAll,
    Shuffle,
}
impl PlayMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Sequential => "顺序播放",
            Self::RepeatOne => "单曲循环",
            Self::RepeatAll => "列表循环",
            Self::Shuffle => "随机播放",
        }
    }
    pub fn next(&self) -> Self {
        match self {
            Self::Sequential => Self::RepeatOne,
            Self::RepeatOne => Self::RepeatAll,
            Self::RepeatAll => Self::Shuffle,
            Self::Shuffle => Self::Sequential,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub version: u32,
    pub sync_history: bool,
    pub close_to_tray: bool,
    pub favorites: Vec<Track>,
    pub history: Vec<Track>,
    pub queue: Vec<Track>,
    pub playlists: BTreeMap<String, Vec<Track>>,
    pub volume: f64,
    pub mode: PlayMode,
}
impl Default for Library {
    fn default() -> Self {
        Self {
            version: 1,
            sync_history: true,
            close_to_tray: false,
            favorites: vec![],
            history: vec![],
            queue: vec![],
            playlists: BTreeMap::new(),
            volume: 0.65,
            mode: PlayMode::default(),
        }
    }
}
fn clean(tracks: &mut Vec<Track>, limit: usize) {
    let mut seen = HashSet::new();
    tracks.retain(|t| valid_bvid(&t.bvid) && t.id == t.bvid && seen.insert(t.id.clone()));
    tracks.truncate(limit);
    for t in tracks {
        t.title = t.title.replace('\0', "").chars().take(500).collect();
        t.artist = t.artist.replace('\0', "").chars().take(200).collect();
        t.cover = t.cover.chars().take(2000).collect();
    }
}
impl Library {
    pub fn normalize(&mut self) {
        self.version = 1;
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0., 1.)
        } else {
            0.65
        };
        clean(&mut self.favorites, MAX_TRACKS);
        clean(&mut self.history, 200);
        clean(&mut self.queue, MAX_TRACKS);
        self.playlists = std::mem::take(&mut self.playlists)
            .into_iter()
            .filter_map(|(k, mut v)| {
                let name: String = k.replace('\0', "").trim().chars().take(60).collect();
                if name.is_empty() {
                    return None;
                }
                clean(&mut v, MAX_TRACKS);
                Some((name, v))
            })
            .take(100)
            .collect();
    }
    pub fn remember(&mut self, track: Track) {
        self.history.retain(|t| t.id != track.id);
        self.history.insert(0, track);
        self.history.truncate(200);
    }
    pub fn toggle_favorite(&mut self, track: &Track) {
        if self.favorites.iter().any(|t| t.id == track.id) {
            self.favorites.retain(|t| t.id != track.id);
        } else if self.favorites.len() < MAX_TRACKS {
            self.favorites.insert(0, track.clone());
        }
    }
}

pub fn data_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("bili-music/library.json")
}
pub fn load(path: &Path) -> Result<Library, String> {
    if !path.exists() {
        return Ok(Library::default());
    }
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_BYTES {
        return Err("资料库超过 8 MiB，已保留原文件".into());
    }
    let mut lib: Library = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("资料库读取失败，已保留原文件：{e}"))?;
    lib.normalize();
    Ok(lib)
}
pub fn save(path: &Path, lib: &Library) -> Result<(), String> {
    let mut lib = lib.clone();
    lib.normalize();
    let bytes = serde_json::to_vec_pretty(&lib).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("资料库超过 8 MiB，请减少歌单内容".into());
    }
    let parent = path.parent().ok_or("无效保存路径")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        // Make rename durable as well as the file contents.
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|e| format!("保存失败：{e}"))
}

/// Automatic sequential playback stops at the end; manual next wraps.
pub fn next_index(
    index: Option<usize>,
    len: usize,
    mode: &PlayMode,
    auto: bool,
    random: u64,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let i = index.filter(|&i| i < len);
    if auto && *mode == PlayMode::RepeatOne && i.is_some() {
        return i;
    }
    if *mode == PlayMode::Shuffle && len > 1 {
        let mut n = (random % (len as u64 - 1)) as usize;
        if let Some(i) = i
            && n >= i
        {
            n += 1;
        }
        return Some(n);
    }
    let n = i.map_or(0, |i| i + 1);
    if n < len {
        Some(n)
    } else if auto && *mode != PlayMode::RepeatAll {
        None
    } else {
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            id: "BV1BDk2YCEHF".into(),
            bvid: "BV1BDk2YCEHF".into(),
            title: "歌\0名".into(),
            ..Default::default()
        }
    }
    #[test]
    fn close_to_tray_defaults_off_and_survives_roundtrip() {
        let old: Library = serde_json::from_str("{}").unwrap();
        assert!(!old.close_to_tray);
        let enabled = Library {
            close_to_tray: true,
            ..Library::default()
        };
        let restored: Library =
            serde_json::from_str(&serde_json::to_string(&enabled).unwrap()).unwrap();
        assert!(restored.close_to_tray);
    }
    #[test]
    fn normalize_untrusted_library() {
        let mut l = Library {
            favorites: vec![track(), track(), Track::default()],
            volume: f64::NAN,
            ..Default::default()
        };
        l.normalize();
        assert_eq!(l.favorites.len(), 1);
        assert_eq!(l.favorites[0].title, "歌名");
        assert_eq!(l.volume, 0.65);
    }
    #[test]
    fn queue_end_and_repeat() {
        assert_eq!(next_index(Some(2), 3, &PlayMode::Sequential, true, 0), None);
        assert_eq!(
            next_index(Some(2), 3, &PlayMode::Sequential, false, 0),
            Some(0)
        );
        assert_eq!(
            next_index(Some(2), 3, &PlayMode::RepeatOne, true, 0),
            Some(2)
        );
        assert_eq!(next_index(None, 0, &PlayMode::Shuffle, false, 0), None);
        for n in 0..20 {
            assert_ne!(next_index(Some(1), 3, &PlayMode::Shuffle, true, n), Some(1));
        }
    }
    #[test]
    fn atomic_roundtrip_and_bad_file_kept() {
        let dir = std::env::temp_dir().join(format!("bili-storage-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.json");
        let mut lib = Library::default();
        lib.remember(track());
        save(&path, &lib).unwrap();
        assert_eq!(load(&path).unwrap().history.len(), 1);
        fs::write(&path, b"not json").unwrap();
        assert!(load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not json");
        let _ = fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod repeat_tests {
    use super::*;
    #[test]
    fn repeat_modes_wrap_and_roundtrip() {
        assert_eq!(
            next_index(Some(2), 3, &PlayMode::RepeatAll, true, 0),
            Some(0)
        );
        assert_eq!(
            next_index(Some(0), 1, &PlayMode::RepeatAll, true, 0),
            Some(0)
        );
        assert_eq!(next_index(None, 0, &PlayMode::RepeatAll, true, 0), None);
        assert_eq!(
            next_index(Some(1), 3, &PlayMode::RepeatOne, false, 0),
            Some(2)
        );
        let mode = PlayMode::Sequential.next().next();
        assert_eq!(mode, PlayMode::RepeatAll);
        assert_eq!(
            serde_json::from_str::<PlayMode>(&serde_json::to_string(&mode).unwrap()).unwrap(),
            mode
        );
    }
}
