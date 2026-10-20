//! Finding SoundFont files in the folders the app searches: macOS's own SoundFont folders, plus any the
//! user adds. Hidden folders and symbolic links are skipped, and the search stops at a fixed depth.

use std::path::{Path, PathBuf};

/// How many folders deep the search goes.
const MAX_DEPTH: usize = 8;

/// The folders macOS keeps SoundFonts in. Only the ones that exist are searched.
pub fn default_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    vec![home.join("Library/Audio/Sounds/Banks"), PathBuf::from("/Library/Audio/Sounds/Banks")]
}

/// Whether a file is a SoundFont the app can play (`.sf2`, or `.sfo`, which the C++ app also accepts).
pub fn is_soundfont(path: &Path) -> bool {
    path.extension()
        .map(|e| {
            let e = e.to_string_lossy().to_ascii_lowercase();
            e == "sf2" || e == "sfo"
        })
        .unwrap_or(false)
}

/// Every SoundFont file under `dirs`, sorted by path, without duplicates.
pub fn find(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        walk(dir, 0, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = path.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(false);
        // `file_type` does not follow symbolic links, so links are never walked into or counted.
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if !hidden {
                walk(&path, depth + 1, out);
            }
        } else if kind.is_file() && !hidden && is_soundfont(&path) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_soundfonts_in_nested_folders_and_skips_hidden_ones() {
        let root = std::env::temp_dir().join(format!("sf-find-{}", std::process::id()));
        let nested = root.join("a").join("b");
        let hidden = root.join(".secret");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(root.join("top.sf2"), b"x").unwrap();
        std::fs::write(nested.join("deep.SFO"), b"x").unwrap();
        std::fs::write(nested.join("notes.txt"), b"x").unwrap();
        std::fs::write(hidden.join("hidden.sf2"), b"x").unwrap();

        let found = find(std::slice::from_ref(&root));
        let names: Vec<String> = found.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["deep.SFO".to_string(), "top.sf2".to_string()]);
        assert!(find(&[root.join("missing")]).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
