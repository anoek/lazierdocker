//! UI state kept between runs: the order of projects in the sidebar. It
//! lives in the platform's state directory. This is best effort: without a
//! usable directory, the state lasts for the session only.

use std::io;
use std::path::{Path, PathBuf};

/// `$XDG_STATE_HOME/lazierdocker` (default `~/.local/state/lazierdocker`);
/// `~/Library/Application Support/lazierdocker` on macOS.
fn dir() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support")
    } else {
        match std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
            Some(d) => PathBuf::from(d),
            None => home()?.join(".local/state"),
        }
    };
    Some(base.join("lazierdocker"))
}

fn project_order_path() -> Option<PathBuf> {
    Some(dir()?.join("project-order"))
}

/// The saved project order (empty when there is none, or it can't be read).
pub fn load_project_order() -> Vec<String> {
    project_order_path().map(|p| load_from(&p)).unwrap_or_default()
}

/// Save the project order.
pub fn save_project_order(order: &[String]) -> io::Result<()> {
    let path = project_order_path().ok_or_else(|| io::Error::other("no home or state directory"))?;
    save_to(&path, order)
}

/// One project name per line.
fn load_from(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|text| text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

/// Written to a temporary file and renamed into place, so a crash never
/// leaves half a file.
fn save_to(path: &Path, order: &[String]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, order.iter().map(|p| format!("{p}\n")).collect::<String>())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_order_round_trips_and_a_missing_file_is_no_order() {
        let dir = std::env::temp_dir().join(format!("lazierdocker-state-test-{}", std::process::id()));
        let path = dir.join("nested/project-order");
        assert!(load_from(&path).is_empty());
        let order = vec!["shop".to_string(), "observability".to_string()];
        save_to(&path, &order).unwrap();
        assert_eq!(load_from(&path), order);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
