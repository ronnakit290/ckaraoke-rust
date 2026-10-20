//! The song library: a catalogue of every song file under a folder (title, artist, key), a cached index
//! file, and the search used by the karaoke picker. Port of `src/library.cpp`.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::lyrics::lyrics_from_midi;
use crate::song::ncn_siblings;
use crate::text::{strip_key_suffix, thai_to_utf8, trim};
use crate::Result;

/// Catalogue entry for one song file, read without decoding its MIDI data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SongInfo {
    pub path: String,
    /// UTF-8, never empty (falls back to the file name).
    pub title: String,
    /// UTF-8, may be empty.
    pub artist: String,
    /// Musical key as the author wrote it, e.g. "Em".
    pub key: String,
}

/// Index files start with this line, then the folder they were made from.
const INDEX_MAGIC: &str = "CKARAOKE-INDEX\t2";

/// Lines of a text part, trimmed, with the same rules `std::getline` uses (no phantom line after a final
/// newline). Stops after `max` lines.
/// Trims spaces, tabs and line breaks from both ends of raw bytes (text may not be UTF-8 yet).
fn trim_bytes(s: &[u8]) -> &[u8] {
    let is_space = |b: &u8| matches!(*b, b' ' | b'\t' | b'\r' | b'\n');
    let start = s.iter().position(|b| !is_space(b)).unwrap_or(s.len());
    let end = s.iter().rposition(|b| !is_space(b)).map_or(start, |i| i + 1);
    &s[start..end.max(start)]
}

fn header_lines(bytes: &[u8], max: usize) -> Vec<Vec<u8>> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut pieces: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    if bytes.last() == Some(&b'\n') {
        pieces.pop();
    }
    pieces.into_iter().take(max).map(trim_bytes).map(<[u8]>::to_vec).collect()
}

/// Title, artist and key from an NCN `.lyr` header (its first three lines).
fn from_ncn_header(lyr: &[u8], info: &mut SongInfo) {
    let head = header_lines(lyr, 3);
    if let Some(l) = head.first() {
        info.title = thai_to_utf8(l);
    }
    if let Some(l) = head.get(1) {
        info.artist = thai_to_utf8(l);
    }
    if let Some(l) = head.get(2) {
        info.key = thai_to_utf8(l);
    }
}

/// What every entry ends with: the key is dropped from the title, and the file name stands in for an
/// empty title.
pub fn finish_song_info(path: &Path, info: &mut SongInfo) {
    info.title = strip_key_suffix(&info.title, &info.key);
    info.artist = trim(&info.artist).to_string();
    if info.title.is_empty() {
        info.title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }
}

/// Reads title, artist and key from an NCN `.mid` (with its `.lyr`), or a plain `.mid` / `.kar`.
/// Returns `None` if the file is not a readable song.
pub fn read_song_info(path: &Path) -> Option<SongInfo> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut info = SongInfo { path: path.to_string_lossy().into_owned(), ..Default::default() };
    match ext.as_str() {
        "mid" | "midi" | "kar" => {
            let sib = ncn_siblings(path);
            let ncn_lyr = sib.lyr.as_ref().and_then(|p| fs::read(p).ok());
            match ncn_lyr {
                Some(lyr) => from_ncn_header(&lyr, &mut info),
                None => {
                    let bytes = fs::read(path).ok()?;
                    if bytes.len() < 14 || &bytes[..4] != b"MThd" {
                        return None;
                    }
                    let lyrics = lyrics_from_midi(&bytes);
                    info.title = lyrics.title;
                    info.artist = lyrics.artist;
                }
            }
        }
        _ => return None,
    }
    finish_song_info(path, &mut info);
    Some(info)
}

/// Counters a scan updates as it runs, so a UI thread can show progress or cancel it.
#[derive(Default)]
pub struct ScanProgress {
    pub done: AtomicUsize,
    /// Zero while the folder is still being listed.
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
}

fn collect_song_files(dir: &Path, out: &mut Vec<PathBuf>, progress: &ScanProgress) {
    let Ok(entries) = fs::read_dir(dir) else {
        return; // unreadable folders are skipped, as the C++ scan does
    };
    for entry in entries.flatten() {
        if progress.cancel.load(Ordering::Relaxed) {
            return;
        }
        let path = entry.path();
        // Directory symlinks are not followed; file symlinks are, as the C++ scan does.
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                collect_song_files(&path, out, progress);
            }
        } else if meta.is_file() {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if matches!(ext.as_str(), "mid" | "midi" | "kar") {
                out.push(path);
            }
        }
    }
}

/// Walks `root` recursively and catalogues every MIDI or karaoke file in it, sorted by title. Files are
/// read on several threads. Returns an empty list if the scan is cancelled.
pub fn scan_library(root: &Path, progress: &ScanProgress) -> Vec<SongInfo> {
    let mut files = Vec::new();
    collect_song_files(root, &mut files, progress);
    if progress.cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }
    progress.total.store(files.len(), Ordering::Relaxed);

    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
    let mut songs: Vec<(usize, SongInfo)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= files.len() || progress.cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        if let Some(info) = read_song_info(&files[i]) {
                            local.push((i, info));
                        }
                        progress.done.fetch_add(1, Ordering::Relaxed);
                    }
                    local
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    });
    if progress.cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }
    songs.sort_by_key(|(i, _)| *i);
    let mut songs: Vec<SongInfo> = songs.into_iter().map(|(_, s)| s).collect();
    songs.sort_by(|a, b| (&a.title, &a.artist, &a.path).cmp(&(&b.title, &b.artist, &b.path)));
    songs
}

/// One field of the index file: tabs and newlines would break the format.
fn index_field(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c }).collect()
}

/// Caches a catalogue as a tab-separated text file, so a large library is not rescanned on every launch.
pub fn save_index(file: &Path, root: &str, songs: &[SongInfo]) -> Result<()> {
    let mut out = String::new();
    out.push_str(INDEX_MAGIC);
    out.push('\t');
    out.push_str(&index_field(root));
    out.push('\n');
    for s in songs {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            index_field(&s.path),
            index_field(&s.title),
            index_field(&s.artist),
            index_field(&s.key)
        ));
    }
    let mut f = fs::File::create(file)?;
    f.write_all(out.as_bytes())?;
    Ok(())
}

/// Reads an index written by `save_index`. Returns the folder it was made from and the songs, or `None`
/// if the file is missing or is not an index of this version.
pub fn load_index(file: &Path) -> Option<(String, Vec<SongInfo>)> {
    let bytes = fs::read(file).ok()?;
    let mut lines = bytes.split(|&b| b == b'\n');
    let first = lines.next()?;
    let magic = INDEX_MAGIC.as_bytes();
    if !first.starts_with(magic) || first.len() <= magic.len() {
        return None;
    }
    let root = String::from_utf8_lossy(&first[magic.len() + 1..]).into_owned();
    let songs = lines
        .filter_map(|line| {
            let mut tabs = line.iter().enumerate().filter(|&(_, &b)| b == b'\t').map(|(i, _)| i);
            let (a, b, c) = (tabs.next()?, tabs.next()?, tabs.next()?);
            let field = |from: usize, to: usize| String::from_utf8_lossy(&line[from..to]).into_owned();
            Some(SongInfo {
                path: field(0, a),
                title: field(a + 1, b),
                artist: field(b + 1, c),
                key: String::from_utf8_lossy(&line[c + 1..]).into_owned(),
            })
        })
        .collect();
    Some((root, songs))
}

/// Lower-case ASCII, which is what the C++ search compares.
fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// A catalogue with a search over it.
#[derive(Default)]
pub struct SongLibrary {
    songs: Vec<SongInfo>,
    haystack: Vec<String>, // lower-cased "title\nartist\nfile name"
    by_path: HashMap<String, usize>,
}

impl SongLibrary {
    pub fn set(&mut self, songs: Vec<SongInfo>) {
        self.haystack = songs
            .iter()
            .map(|s| {
                let stem = Path::new(&s.path).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                lower(&format!("{}\n{}\n{}", s.title, s.artist, stem))
            })
            .collect();
        self.by_path = songs.iter().enumerate().map(|(i, s)| (s.path.clone(), i)).collect();
        self.songs = songs;
    }

    pub fn songs(&self) -> &[SongInfo] {
        &self.songs
    }

    pub fn by_path(&self, path: &str) -> Option<&SongInfo> {
        self.by_path.get(path).map(|&i| &self.songs[i])
    }

    /// Indexes of songs whose title, artist or file name contain every word of `query` (ASCII
    /// case-insensitive). An exact file name comes first, then titles starting with the query, then the rest.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let lowered = lower(query);
        let words: Vec<&str> = lowered.split_ascii_whitespace().collect();
        if words.is_empty() {
            return (0..self.songs.len()).collect();
        }
        let whole = trim(&lowered);
        let (mut exact, mut prefix, mut rest) = (Vec::new(), Vec::new(), Vec::new());
        for (i, h) in self.haystack.iter().enumerate() {
            if !words.iter().all(|w| h.contains(*w)) {
                continue;
            }
            // A typed song code (the file name) is the most specific match there is.
            let is_code = h.len() > whole.len()
                && h.as_bytes()[h.len() - whole.len() - 1] == b'\n'
                && h.ends_with(whole);
            if is_code {
                exact.push(i);
            } else if h.starts_with(whole) {
                prefix.push(i);
            } else {
                rest.push(i);
            }
        }
        exact.extend(prefix);
        exact.extend(rest);
        exact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(path: &str, title: &str, artist: &str) -> SongInfo {
        SongInfo { path: path.into(), title: title.into(), artist: artist.into(), key: String::new() }
    }

    #[test]
    fn search_ranks_song_codes_then_prefixes() {
        let mut lib = SongLibrary::default();
        lib.set(vec![
            song("/m/Other Song.mid", "Something", "Artist A"),
            song("/m/A01.mid", "Lovely day", "Band"),
            song("/m/Lovely.mid", "Band of Love", "Lovely"),
        ]);
        // "a01" matches the file name exactly, so it comes first.
        assert_eq!(lib.search("a01"), vec![1]);
        // Titles starting with the query come before songs that only contain it.
        assert_eq!(lib.search("band"), vec![2, 1]);
        // Every word must match somewhere.
        assert_eq!(lib.search("lovely band"), vec![1, 2]);
        assert_eq!(lib.search("   "), vec![0, 1, 2]);
    }

    #[test]
    fn index_round_trips() {
        let dir = std::env::temp_dir().join(format!("idx-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("library.tsv");
        let songs = vec![SongInfo {
            path: "/m/x.mid".into(),
            title: "คิดถึง".into(),
            artist: "tab\there".into(),
            key: "Em".into(),
        }];
        save_index(&file, "/m", &songs).unwrap();
        let (root, loaded) = load_index(&file).unwrap();
        assert_eq!(root, "/m");
        // Tabs inside fields are written as spaces, so the row keeps its shape.
        assert_eq!(loaded[0].artist, "tab here");
        assert_eq!(loaded[0].title, "คิดถึง");
        fs::remove_dir_all(&dir).unwrap();
    }
}
