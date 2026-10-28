//! The file half of the Agent's kill switch (the web's `kill_switch_file`
//! setting and `RiskGuard._kill_switch_file_present`).
//!
//! The switch has two halves and either one engages it: the stored
//! `kill_switch` setting, and this file. The file is the half that works
//! when the database or the screen does not: it can be created from a
//! file manager or a shell, and it survives a restart. The guard is told
//! whether it exists; it never looks itself (`risk` does no I/O).
//!
//! Desktop: the default is `<app data folder>/agent_kill_switch` (lead
//! decision 2, 2026-10-27), and the trading safety panel's switch creates
//! and removes it through [`set`].

use std::path::{Path, PathBuf};

/// The file's name in the data folder.
pub const FILE_NAME: &str = "agent_kill_switch";

/// The default kill switch file.
pub fn default_file(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// The setting as a path: `~` is the trader's home folder, and a relative
/// path is inside the data folder, so a value typed into the settings
/// means the same thing whatever folder the app was started from.
pub fn resolve(setting: &str, data_dir: &Path) -> PathBuf {
    let text = setting.trim();
    if text.is_empty() {
        return default_file(data_dir);
    }
    let expanded = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => match home() {
            Some(home) => home.join(rest.trim_start_matches(['/', '\\'])),
            None => PathBuf::from(text),
        },
        _ => PathBuf::from(text),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        data_dir.join(expanded)
    }
}

fn home() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Whether the file exists. A path that cannot be checked counts as absent,
/// as the web's `Path.exists()` does, and is logged.
pub fn present(path: &Path) -> bool {
    match path.try_exists() {
        Ok(found) => found,
        Err(e) => {
            tracing::error!(
                "The Agent kill switch at {} could not be checked; treating it as absent: {}",
                path.display(),
                e
            );
            false
        }
    }
}

/// What a file the panel creates says to whoever finds it.
const FILE_NOTE: &str = "While this file exists, the OpenAlgo Agent places, modifies and cancels no orders.\nDelete it, or release the kill switch in Agent settings, to allow them again.\n";

/// Engage (create the file, saying what it is) or release (remove it). A
/// file already there is left as it is; releasing a switch that is not
/// engaged is not an error. The one writer of the file: the panel's switch
/// (`settings::kill_switch::set`) calls it.
pub fn set(path: &Path, engaged: bool) -> std::io::Result<()> {
    if engaged {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => std::io::Write::write_all(&mut file, FILE_NOTE.as_bytes()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_resolves_inside_the_data_folder() {
        let data = Path::new("/data/openalgo");
        assert_eq!(resolve("", data), data.join(FILE_NAME));
        assert_eq!(resolve("stop", data), data.join("stop"));
        let absolute = std::env::temp_dir().join("stop-all");
        assert_eq!(resolve(absolute.to_str().unwrap(), data), absolute);
        if let Some(home) = home() {
            assert_eq!(resolve("~/stop", data), home.join("stop"));
        }
    }

    #[test]
    fn engaging_creates_the_file_and_releasing_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE_NAME);
        assert!(!present(&path));
        set(&path, true).unwrap();
        assert!(present(&path));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FILE_NOTE);
        std::fs::write(&path, "mine").unwrap();
        set(&path, true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
        set(&path, false).unwrap();
        assert!(!present(&path));
        set(&path, false).unwrap();
    }
}
