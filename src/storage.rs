use crate::api::{Track, valid_bvid};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DownloadIndex(BTreeMap<String, DownloadRecord>);

#[derive(Clone, Debug, PartialEq)]
struct DownloadRecord {
    path: PathBuf,
    track: Track,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct DownloadIndexFile {
    version: u32,
    downloads: Vec<DownloadEntry>,
}

impl Default for DownloadIndexFile {
    fn default() -> Self {
        Self {
            version: 1,
            downloads: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct DownloadEntry {
    path: StoredPath,
    track: Track,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredPath {
    Utf8(String),
    Os(OsString),
}

impl StoredPath {
    fn from_path(path: &Path) -> Self {
        match path.to_str() {
            Some(path) => Self::Utf8(path.to_owned()),
            None => Self::Os(path.as_os_str().to_owned()),
        }
    }

    fn into_path(self) -> PathBuf {
        match self {
            Self::Utf8(path) => PathBuf::from(path),
            Self::Os(path) => PathBuf::from(path),
        }
    }
}

fn valid_download_path(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        !bytes.is_empty() && !bytes.contains(&0)
    }
    #[cfg(not(unix))]
    {
        !path.as_os_str().is_empty() && !path.to_string_lossy().contains('\0')
    }
}

impl DownloadIndex {
    fn normalize(&mut self) {
        let entries = std::mem::take(&mut self.0);
        let mut paths = HashSet::new();
        for (bvid, record) in entries {
            let mut tracks = vec![record.track];
            clean(&mut tracks, 1);
            let Some(track) = tracks.pop() else {
                continue;
            };
            if bvid == track.bvid
                && valid_download_path(&record.path)
                && paths.insert(record.path.clone())
            {
                self.0.insert(
                    bvid,
                    DownloadRecord {
                        path: record.path,
                        track,
                    },
                );
            }
        }
    }

    pub fn path_for(&self, bvid: &str) -> Option<&Path> {
        if !valid_bvid(bvid) {
            return None;
        }
        self.0.get(bvid).map(|record| record.path.as_path())
    }

    pub fn link(&mut self, track: &Track, path: PathBuf) {
        let mut tracks = vec![track.clone()];
        clean(&mut tracks, 1);
        let Some(track) = tracks.pop() else {
            return;
        };
        if !valid_download_path(&path) {
            return;
        }
        let bvid = track.bvid.clone();
        self.0.insert(
            bvid.clone(),
            DownloadRecord {
                path: path.clone(),
                track,
            },
        );
        self.0
            .retain(|other, record| other == &bvid || record.path != path);
    }

    pub fn tracks(&self) -> Vec<Track> {
        self.0.values().map(|record| record.track.clone()).collect()
    }

    pub fn remove(&mut self, bvid: &str) -> Option<PathBuf> {
        self.0.remove(bvid).map(|record| record.path)
    }
}

impl Serialize for DownloadIndex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut index = self.clone();
        index.normalize();
        let file = DownloadIndexFile {
            version: 1,
            downloads: index
                .0
                .into_values()
                .map(|record| DownloadEntry {
                    path: StoredPath::from_path(&record.path),
                    track: record.track,
                })
                .collect(),
        };
        file.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DownloadIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let file = DownloadIndexFile::deserialize(deserializer)?;
        let mut index = Self::default();
        for entry in file.downloads {
            index.0.insert(
                entry.track.bvid.clone(),
                DownloadRecord {
                    path: entry.path.into_path(),
                    track: entry.track,
                },
            );
        }
        index.normalize();
        Ok(index)
    }
}

pub fn data_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("bili-music/library.json")
}

fn load_limited(path: &Path, description: &str) -> Result<Option<Vec<u8>>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_BYTES {
        return Err(format!("{description}超过 8 MiB，已保留原文件"));
    }
    Ok(Some(fs::read(path).map_err(|e| e.to_string())?))
}

fn save_atomic(path: &Path, bytes: &[u8], description: &str) -> Result<(), String> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{description}超过 8 MiB，请减少内容"));
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
        file.write_all(bytes)?;
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

pub fn load(path: &Path) -> Result<Library, String> {
    let Some(bytes) = load_limited(path, "资料库")? else {
        return Ok(Library::default());
    };
    let mut lib: Library =
        serde_json::from_slice(&bytes).map_err(|e| format!("资料库读取失败，已保留原文件：{e}"))?;
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
    save_atomic(path, &bytes, "资料库")
}

pub fn load_downloads(path: &Path) -> Result<DownloadIndex, String> {
    let Some(bytes) = load_limited(path, "下载索引")? else {
        return Ok(DownloadIndex::default());
    };
    serde_json::from_slice(&bytes).map_err(|e| format!("下载索引读取失败，已保留原文件：{e}"))
}

pub fn save_downloads(path: &Path, downloads: &DownloadIndex) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(downloads).map_err(|e| e.to_string())?;
    save_atomic(path, &bytes, "下载索引")
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

    fn download_track(bvid: &str, title: &str, artist: &str) -> Track {
        Track {
            id: bvid.into(),
            bvid: bvid.into(),
            title: title.into(),
            artist: artist.into(),
            cover: "https://example.com/cover.jpg".into(),
            duration: 180,
            play_count: 42,
        }
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bili-storage-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
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
        let dir = test_dir("library");
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

    #[test]
    fn download_index_roundtrip_filters_invalid_entries() {
        let dir = test_dir("downloads-persistence");
        let path = dir.join("downloads.json");
        let invalid_bvid = download_track("not-a-bvid", "无效", "歌手");
        let mismatched_id = Track {
            id: "BV1xx411c7mD".into(),
            ..download_track("BV1BDk2YCEHF", "错配", "歌手")
        };
        let nul_path = download_track("BV1xx411c7mD", "空路径", "歌手");
        let payload = serde_json::json!({
            "version": 1,
            "downloads": [
                {"path": "/tmp/a.mp3", "track": invalid_bvid},
                {"path": "", "track": download_track("BV1BDk2YCEHF", "空路径", "歌手")},
                {"path": "\0", "track": nul_path},
                {"path": "/tmp/b.mp3", "track": mismatched_id}
            ]
        });
        fs::write(&path, serde_json::to_vec(&payload).unwrap()).unwrap();

        let mut index = load_downloads(&path).unwrap();
        assert!(index.tracks().is_empty());
        assert!(index.path_for("BV1BDk2YCEHF").is_none());
        assert!(index.path_for("BV1xx411c7mD").is_none());

        let downloaded = download_track("BV1BDk2YCEHF", "夜航", "歌手");
        let file = dir.join("song.mp3");
        index.link(&downloaded, file.clone());
        save_downloads(&path, &index).unwrap();
        let restored = load_downloads(&path).unwrap();
        assert_eq!(restored.path_for("BV1BDk2YCEHF"), Some(file.as_path()));
        assert_eq!(restored.tracks(), vec![downloaded]);

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            saved["downloads"][0]["path"],
            file.to_string_lossy().as_ref()
        );
        assert_eq!(saved["downloads"][0]["track"]["title"], "夜航");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn linking_same_bvid_replaces_its_path() {
        let mut index = DownloadIndex::default();
        let track = download_track("BV1BDk2YCEHF", "同一首歌", "歌手");
        let first = PathBuf::from("/tmp/first.mp3");
        let second = PathBuf::from("/tmp/second.mp3");

        index.link(&track, first);
        index.link(&track, second.clone());

        assert_eq!(index.path_for("BV1BDk2YCEHF"), Some(second.as_path()));
        assert_eq!(index.tracks(), vec![track]);
        assert_eq!(index.remove("BV1BDk2YCEHF"), Some(second));
        assert!(index.path_for("BV1BDk2YCEHF").is_none());
        assert!(index.tracks().is_empty());
    }

    #[test]
    fn linking_shared_path_removes_other_bvid() {
        let mut index = DownloadIndex::default();
        let first = download_track("BV1BDk2YCEHF", "第一首", "歌手甲");
        let second = download_track("BV1xx411c7mD", "第二首", "歌手乙");
        let shared = PathBuf::from("/tmp/shared.mp3");

        index.link(&first, shared.clone());
        index.link(&second, shared.clone());

        assert!(index.path_for("BV1BDk2YCEHF").is_none());
        assert_eq!(index.path_for("BV1xx411c7mD"), Some(shared.as_path()));
        assert_eq!(index.tracks(), vec![second]);
    }

    #[cfg(unix)]
    #[test]
    fn download_index_roundtrip_preserves_non_utf8_path() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let dir = test_dir("downloads-non-utf8");
        let path = dir.join("downloads.json");
        let file = PathBuf::from(OsString::from_vec(b"/tmp/music-\xff.mp3".to_vec()));
        let track = download_track("BV1BDk2YCEHF", "非 UTF-8 路径", "歌手");
        let mut index = DownloadIndex::default();
        index.link(&track, file.clone());

        save_downloads(&path, &index).unwrap();
        let restored = load_downloads(&path).unwrap();

        assert_eq!(restored.path_for("BV1BDk2YCEHF"), Some(file.as_path()));
        assert_eq!(restored.tracks(), vec![track]);
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(saved["downloads"][0]["path"]["Unix"].is_array());
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
