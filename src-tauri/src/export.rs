//! Saving an exported flight path. The frontend builds the file (KML, GPX,
//! CSV, GeoJSON); this writes it to the Downloads folder without overwriting
//! anything there, and returns where it went so the file can be shown.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

/// Longest file name kept, in characters, before the extension.
const MAX_STEM: usize = 120;

/// A file name safe on every desktop platform: no path separators, reserved
/// characters or control characters, and not empty.
fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    // no trailing dots (Windows drops them) and no leading ones (hidden files)
    let cleaned = cleaned.trim().trim_end_matches('.');
    let (stem, ext) = match cleaned.rfind('.') {
        Some(i) => (&cleaned[..i], &cleaned[i..]),
        None => (cleaned, ""),
    };
    let stem: String = stem.trim_start_matches('.').trim().chars().take(MAX_STEM).collect();
    if stem.is_empty() {
        format!("flight-path{ext}")
    } else {
        format!("{stem}{ext}")
    }
}

/// `dir/name`, or `dir/stem (2).ext` and so on if that is taken.
fn unused_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("some numbered name is free")
}

/// Writes `contents` to the Downloads folder (the home folder when there is
/// none) as `file_name`, numbered if a file of that name exists. Returns the
/// full path written.
#[tauri::command]
pub async fn save_export(app: AppHandle, file_name: String, contents: String) -> Result<String, String> {
    let paths = app.path();
    let dir = paths
        .download_dir()
        .or_else(|_| paths.home_dir())
        .map_err(|e| format!("No Downloads folder to save to: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't open {}: {e}", dir.display()))?;
    let path = unused_path(&dir, &safe_file_name(&file_name));
    std::fs::write(&path, contents).map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(safe_file_name("BAW117 LHR-JFK.kml"), "BAW117 LHR-JFK.kml");
        assert_eq!(safe_file_name("../../etc/passwd"), "-..-etc-passwd");
        assert_eq!(safe_file_name("a:b*c?.gpx"), "a-b-c-.gpx");
        assert_eq!(safe_file_name(".kml"), "flight-path.kml");
        assert_eq!(safe_file_name("BAW117."), "BAW117");
        assert_eq!(safe_file_name(""), "flight-path");
        assert_eq!(safe_file_name(&format!("{}.csv", "x".repeat(300))).len(), MAX_STEM + 4);
    }

    #[test]
    fn taken_names_are_numbered() {
        let dir = std::env::temp_dir().join(format!("rustradar-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = unused_path(&dir, "path.kml");
        assert_eq!(first, dir.join("path.kml"));
        std::fs::write(&first, "x").unwrap();
        assert_eq!(unused_path(&dir, "path.kml"), dir.join("path (2).kml"));
        std::fs::write(dir.join("path (2).kml"), "x").unwrap();
        assert_eq!(unused_path(&dir, "path.kml"), dir.join("path (3).kml"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
