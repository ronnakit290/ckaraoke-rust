//! Loading one playable song: a MIDI file (with its NCN lyric siblings), or one song inside a `.ckr` pack.
//! Port of `src/song.cpp`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::lyrics::{lyrics_from_midi, lyrics_from_ncn, Lyrics};
use crate::pack::{split_pack_song_path, PackReader};
use crate::{Error, Result};

/// A playable song: the raw MIDI bytes plus timed lyrics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Song {
    pub midi: Vec<u8>,
    pub lyrics: Lyrics,
}

/// The NCN lyric and cursor files that belong to a MIDI file. Each is `None` if it is not there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NcnSiblings {
    pub lyr: Option<PathBuf>,
    pub cur: Option<PathBuf>,
}

fn is_song_folder(name: Option<&std::ffi::OsStr>) -> bool {
    name.is_some_and(|n| {
        let n = n.to_string_lossy().to_ascii_lowercase();
        n == "song" || n == "songs"
    })
}

/// For a MIDI file inside an NCN song tree (`.../Song/X/NAME.mid`), finds its lyric and cursor files
/// (`.../Lyrics/X/NAME.lyr`, `.../Cursor/X/NAME.cur`). The folders mirror the path below `Song`.
pub fn ncn_siblings(mid: &Path) -> NcnSiblings {
    // Find the innermost "Song" folder, collecting the folders between it and the file.
    let mut tail: Vec<OsString> = Vec::new();
    let mut dir = mid.parent().map(Path::to_path_buf).unwrap_or_default();
    while !dir.as_os_str().is_empty() && dir != Path::new("/") && !is_song_folder(dir.file_name()) {
        if let Some(name) = dir.file_name() {
            tail.push(name.to_owned());
        }
        dir = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    }
    if dir.as_os_str().is_empty() || dir == Path::new("/") {
        return NcnSiblings::default();
    }

    let parent = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = mid.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let sibling = |folders: &[&str], exts: &[&str]| -> Option<PathBuf> {
        for folder in folders {
            let mut base = parent.join(folder);
            for part in tail.iter().rev() {
                base.push(part);
            }
            for ext in exts {
                let p = base.join(format!("{stem}{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    };
    NcnSiblings {
        lyr: sibling(&["Lyrics", "Lyric"], &[".lyr", ".LYR"]),
        cur: sibling(&["Cursor"], &[".cur", ".CUR"]),
    }
}

/// Loads a `.mid` / `.midi` / `.kar` file (with NCN lyrics if present), or one song of a `.ckr` pack
/// written as `<pack>.ckr#<index>`, opened with `pack_key`.
pub fn load_song(path: &str, pack_key: &str) -> Result<Song> {
    if let Some((pack_file, index)) = split_pack_song_path(path) {
        if pack_key.is_empty() {
            return Err(Error::new("ยังไม่ได้ตั้งคีย์ .ckr (ตั้งได้ที่เมนู CKaraoke > ตั้งค่าคีย์ .ckr…)"));
        }
        let reader = PackReader::open(&pack_file, pack_key)?;
        let song = reader.read(index)?;
        let lyrics = if song.lyric.is_empty() {
            lyrics_from_midi(&song.midi)
        } else {
            lyrics_from_ncn(&song.midi, &song.lyric, &song.cursor)
        };
        return Ok(Song { midi: song.midi, lyrics });
    }

    let file = Path::new(path);
    let bytes = fs::read(file).map_err(|_| Error::new(format!("เปิดไฟล์ไม่ได้: {path}")))?;

    let sib = ncn_siblings(file);
    let lyrics = match &sib.lyr {
        Some(lyr_path) => {
            let lyr = fs::read(lyr_path).unwrap_or_default();
            let cur = sib.cur.as_ref().and_then(|p| fs::read(p).ok()).unwrap_or_default();
            lyrics_from_ncn(&bytes, &lyr, &cur)
        }
        None => lyrics_from_midi(&bytes),
    };
    Ok(Song { midi: bytes, lyrics })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_lyric_and_cursor_siblings_in_an_ncn_tree() {
        let root = std::env::temp_dir().join(format!("ncn-test-{}", std::process::id()));
        let song_dir = root.join("Song").join("A");
        let lyr_dir = root.join("Lyrics").join("A");
        let cur_dir = root.join("Cursor").join("A");
        for d in [&song_dir, &lyr_dir, &cur_dir] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(song_dir.join("track.mid"), b"x").unwrap();
        fs::write(lyr_dir.join("track.lyr"), b"x").unwrap();

        let sib = ncn_siblings(&song_dir.join("track.mid"));
        assert_eq!(sib.lyr, Some(lyr_dir.join("track.lyr")));
        assert_eq!(sib.cur, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_plain_midi_path_has_no_siblings() {
        assert_eq!(ncn_siblings(Path::new("/tmp/loose.mid")), NcnSiblings::default());
    }
}
