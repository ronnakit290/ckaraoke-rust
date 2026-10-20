//! The song library shown in the app: the songs under the chosen folder, scanned in the background,
//! plus the songs of opened `.ckr` packs. Keeps an index file so a large folder opens quickly.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use ckaraoke_core::library::{self, ScanProgress, SongInfo, SongLibrary};

struct Scan {
    progress: Arc<ScanProgress>,
    result: Receiver<Vec<SongInfo>>,
}

pub struct Library {
    pub root: Option<PathBuf>,
    index_file: PathBuf,
    scanned: Vec<SongInfo>,
    packs: Vec<SongInfo>,
    catalog: SongLibrary,
    scan: Option<Scan>,
}

impl Library {
    pub fn new(index_file: PathBuf) -> Library {
        Library {
            root: None,
            index_file,
            scanned: Vec::new(),
            packs: Vec::new(),
            catalog: SongLibrary::default(),
            scan: None,
        }
    }

    /// Chooses the song folder. Uses the saved index when it was made from this folder, otherwise scans.
    pub fn set_root(&mut self, root: PathBuf) {
        let cached = library::load_index(&self.index_file).filter(|(r, _)| Path::new(r) == root.as_path());
        self.root = Some(root);
        match cached {
            Some((_, songs)) => {
                self.scanned = songs;
                self.rebuild();
            }
            None => self.rescan(),
        }
    }

    /// Scans the folder again, in the background.
    pub fn rescan(&mut self) {
        let Some(root) = self.root.clone() else { return };
        let progress = Arc::new(ScanProgress::default());
        let (tx, rx) = mpsc::channel();
        let worker_progress = Arc::clone(&progress);
        std::thread::spawn(move || {
            let songs = library::scan_library(&root, &worker_progress);
            let _ = tx.send(songs);
        });
        self.scan = Some(Scan { progress, result: rx });
    }

    /// Takes the result of a finished scan, if any. Returns true when the library changed.
    pub fn poll(&mut self) -> bool {
        let Some(scan) = self.scan.as_ref() else { return false };
        match scan.result.try_recv() {
            Ok(songs) => {
                self.scan = None;
                if let Some(root) = &self.root {
                    library::save_index(&self.index_file, &root.to_string_lossy(), &songs).ok();
                }
                self.scanned = songs;
                self.rebuild();
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.scan = None;
                false
            }
        }
    }

    /// Scan progress as (done, total), while a scan runs. Total is zero while the folder is listed.
    pub fn scan_progress(&self) -> Option<(usize, usize)> {
        self.scan.as_ref().map(|s| (s.progress.done.load(Ordering::Relaxed), s.progress.total.load(Ordering::Relaxed)))
    }

    pub fn add_pack_songs(&mut self, songs: Vec<SongInfo>) {
        self.packs.extend(songs);
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let mut all = self.scanned.clone();
        all.extend(self.packs.iter().cloned());
        self.catalog.set(all);
    }

    pub fn songs(&self) -> &[SongInfo] {
        self.catalog.songs()
    }

    /// Indexes into `songs()` matching the query, best matches first.
    pub fn search(&self, query: &str) -> Vec<usize> {
        self.catalog.search(query)
    }
}
