use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

fn preference_path() -> PathBuf {
    crate::storage::data_path().with_file_name("graphics.conf")
}

pub fn cpu_only() -> bool {
    read_cpu_only(&preference_path())
}

fn read_cpu_only(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|value| value.trim() == "cairo")
}

pub fn save(cpu: bool) -> std::io::Result<()> {
    write_preference(&preference_path(), cpu)
}

fn write_preference(path: &Path, cpu: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, if cpu { "cairo\n" } else { "auto\n" })
}

fn requested_renderer(override_value: Option<OsString>, path: &Path) -> Option<OsString> {
    override_value
        .filter(|value| !value.is_empty())
        .or_else(|| read_cpu_only(path).then(|| "cairo".into()))
}

// Call before initializing GTK, GStreamer or any background threads.
pub fn configure_startup() {
    let requested = requested_renderer(std::env::var_os("BILI_MUSIC_RENDERER"), &preference_path());
    if let Some(renderer) = requested {
        // SAFETY: this is called at the start of main, before any threads start.
        unsafe { std::env::set_var("GSK_RENDERER", renderer) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_preference_roundtrip_and_startup_override() {
        let dir = std::env::temp_dir().join(format!("bili-graphics-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("bili-music/graphics.conf");

        assert!(!read_cpu_only(&path));
        assert_eq!(requested_renderer(None, &path), None);

        write_preference(&path, true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "cairo\n");
        assert!(read_cpu_only(&path));
        assert_eq!(requested_renderer(None, &path), Some("cairo".into()));
        assert_eq!(
            requested_renderer(Some(OsString::new()), &path),
            Some("cairo".into())
        );
        assert_eq!(
            requested_renderer(Some("vulkan".into()), &path),
            Some("vulkan".into())
        );

        fs::write(&path, " cairo \n").unwrap();
        assert!(read_cpu_only(&path));

        write_preference(&path, false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "auto\n");
        assert!(!read_cpu_only(&path));
        assert_eq!(requested_renderer(None, &path), None);

        fs::write(&path, "unknown\n").unwrap();
        assert_eq!(requested_renderer(None, &path), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
