use std::{fs, path::PathBuf};

fn preference_path() -> PathBuf {
    crate::storage::data_path().with_file_name("graphics.conf")
}

pub fn cpu_only() -> bool {
    fs::read_to_string(preference_path()).is_ok_and(|value| value.trim() == "cairo")
}

pub fn save(cpu: bool) -> std::io::Result<()> {
    let path = preference_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, if cpu { "cairo\n" } else { "auto\n" })
}

// Call before initializing GTK, GStreamer or any background threads.
pub fn configure_startup() {
    let requested = std::env::var_os("BILI_MUSIC_RENDERER")
        .filter(|value| !value.is_empty())
        .or_else(|| cpu_only().then(|| "cairo".into()));
    if let Some(renderer) = requested {
        // SAFETY: this is called at the start of main, before any threads start.
        unsafe { std::env::set_var("GSK_RENDERER", renderer) };
    }
}
