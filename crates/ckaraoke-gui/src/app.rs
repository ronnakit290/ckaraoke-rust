//! The app: what is playing, the queue and its history, and the commands the panels ask for.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, Sense};

use ckaraoke_audio::Output;
use ckaraoke_core::lyrics::Lyrics;
use ckaraoke_core::library::{read_song_info, SongInfo};
use ckaraoke_core::pack::{pack_song_path, PackReader};
use ckaraoke_core::player::Player;
use ckaraoke_core::song::load_song;

use crate::library::Library;
use crate::queue::{Entry, Queue};
use crate::search;
use crate::settings::{settings_dir, Background, Settings, PRESET_BACKGROUNDS};
use crate::soundfonts;
use crate::view::{self, ChannelView, Cmd, DrumView, Snapshot};

/// Files and folders given on the command line, for starting with a SoundFont or a library already chosen.
#[derive(Default)]
pub struct Startup {
    pub soundfont: Option<PathBuf>,
    pub folder: Option<PathBuf>,
    /// A song (or pack) to start playing right away.
    pub play: Option<PathBuf>,
}

/// The two bars that appear on hover: the top menu and the bottom transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetKind {
    Menu,
    Transport,
}

struct Now {
    entry: Entry,
    lyrics: Lyrics,
}

pub struct App {
    output: Option<Output>,
    settings: Settings,
    font_name: Option<String>,
    library: Library,
    queue: Queue,
    /// Songs played before the current one, most recent last. Going back takes the last one.
    history: Vec<Entry>,
    now: Option<Now>,
    search: String,
    /// The song highlighted in the search results.
    selected: usize,
    scroll_to_selected: bool,
    /// The search as it was last frame, and when it was last used. An idle search clears itself.
    search_seen: String,
    search_active: Instant,
    /// The background image, decoded once and kept as a texture. `bg_failed` remembers a file that would not open.
    /// SoundFonts found in the search folders, and the scan that is running (if any).
    soundfonts: Vec<PathBuf>,
    sf_scan: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    /// The path window: Some while it is open, holding what has been typed.
    sf_path_input: Option<String>,
    bg_texture: Option<(PathBuf, egui::TextureHandle)>,
    bg_failed: Option<PathBuf>,
    /// The `.ckr` passphrase. Typed each session and never written to disk.
    pack_key: String,
    show_queue: bool,
    show_mixer: bool,
    show_settings: bool,
    /// The playlist song the arrow keys are on, and when the list was last used. It hides after a few idle seconds.
    queue_sel: Option<usize>,
    queue_seen: Instant,
    fullscreen: bool,
    /// When the last message was shown, so it can fade after a few seconds.
    message_at: Instant,
    /// A right-click menu of the top menu or the transport: where it opens, and which bar it belongs to.
    widget_menu: Option<(eframe::egui::Pos2, WidgetKind)>,
    widget_menu_rect: Option<eframe::egui::Rect>,
    /// Last message for the user, and whether it is an error.
    message: Option<(String, bool)>,
    cmds: Vec<Cmd>,
    frames: u32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> App {
        if !install_thai_font(&cc.egui_ctx) {
            eprintln!("ไม่พบฟอนต์ไทยในระบบ ตัวอักษรไทยอาจแสดงไม่ถูกต้อง");
        }
        cc.egui_ctx.set_theme(eframe::egui::ThemePreference::Dark);
        // The windows are 20% transparent, so the lyrics show through them.
        cc.egui_ctx.style_mut_of(eframe::egui::Theme::Dark, |style| {
            let f = style.visuals.window_fill;
            style.visuals.window_fill = eframe::egui::Color32::from_rgba_unmultiplied(f.r(), f.g(), f.b(), UI_ALPHA);
        });
        let mut settings = Settings::load();
        if startup.soundfont.is_some() {
            settings.soundfont = startup.soundfont;
        }
        if startup.folder.is_some() {
            settings.library_folder = startup.folder;
        }
        let (output, audio_error) = match Output::open() {
            Ok(o) => (Some(o), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let mut app = App {
            output,
            settings,
            font_name: None,
            library: Library::new(settings_dir().join("library-index.tsv")),
            queue: Queue::default(),
            history: Vec::new(),
            now: None,
            search: String::new(),
            soundfonts: Vec::new(),
            sf_scan: None,
            sf_path_input: None,
            bg_texture: None,
            bg_failed: None,
            selected: 0,
            scroll_to_selected: false,
            search_seen: String::new(),
            search_active: Instant::now(),
            pack_key: String::new(),
            show_queue: false,
            show_mixer: false,
            show_settings: false,
            queue_sel: None,
            queue_seen: Instant::now(),
            fullscreen: true,
            message_at: Instant::now(),
            widget_menu: None,
            widget_menu_rect: None,
            message: audio_error.map(|e| (e, true)),
            cmds: Vec::new(),
            frames: 0,
        };
        let volume = app.settings.volume;
        app.with_player(|p| p.set_volume(volume));
        if let Some(path) = app.settings.soundfont.clone() {
            app.load_font(&path);
        }
        if let Some(folder) = app.settings.library_folder.clone() {
            app.library.set_root(folder);
        }
        app.rescan_soundfonts();
        if let Some(path) = startup.play {
            app.open_path(path);
        }
        app
    }

    /// Opens the mixer and queue windows at start, if asked for on the command line.
    pub fn show_panels(&mut self, mixer: bool, queue: bool) {
        self.show_mixer = mixer;
        self.show_queue = queue;
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), false));
        self.message_at = Instant::now();
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), true));
        self.message_at = Instant::now();
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    /// Runs `f` on the player, if audio is open and the player is free.
    fn with_player<R>(&self, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
        let output = self.output.as_ref()?;
        let mut player = output.player().lock().ok()?;
        Some(f(&mut player))
    }

    fn load_font(&mut self, path: &Path) {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) => return self.error(format!("เปิดไฟล์ไม่ได้: {e}")),
        };
        let Some(output) = self.output.as_ref() else {
            return self.error("เปิดอุปกรณ์เสียงออกไม่ได้");
        };
        match output.load_sound_font(&bytes) {
            Ok(()) => {
                self.font_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.settings.soundfont = Some(path.to_path_buf());
                self.settings.save();
                self.info("โหลด SoundFont แล้ว");
            }
            Err(e) => self.error(format!("โหลด SoundFont ไม่สำเร็จ: {e}")),
        }
    }

    fn set_folder(&mut self, folder: PathBuf) {
        self.settings.library_folder = Some(folder.clone());
        self.settings.save();
        self.library.set_root(folder);
    }

    /// Loads a song and makes it the current one. Returns false (and says why) if it cannot play.
    fn load_entry(&mut self, entry: Entry) -> bool {
        let song = match load_song(&entry.arg, &self.pack_key) {
            Ok(song) => song,
            Err(e) => {
                self.error(format!("เปิดเพลงไม่ได้: {e}"));
                return false;
            }
        };
        let loaded = self.with_player(|p| {
            if p.preset_count() == 0 {
                return Err("เลือก SoundFont ก่อนเล่นเพลง".to_string());
            }
            p.load_midi(&song.midi).map_err(|e| e.to_string())?;
            p.set_speed(1.0); // a new song starts at 100%, as in the C++ app
            Ok(())
        });
        match loaded {
            Some(Ok(())) => {
                self.now = Some(Now { entry, lyrics: song.lyrics });
                true
            }
            Some(Err(e)) => {
                self.error(e);
                false
            }
            None => {
                self.error("เปิดอุปกรณ์เสียงออกไม่ได้");
                false
            }
        }
    }

    /// Plays a song now. The song that was playing goes to the history.
    fn play_now(&mut self, entry: Entry) {
        let previous = self.now.take();
        let arg = entry.arg.clone();
        if self.load_entry(entry) {
            self.queue.remove_song(&arg);
            if let Some(prev) = previous {
                self.history.push(prev.entry);
            }
        } else {
            self.now = previous;
        }
    }

    /// Plays the next song in the queue. With an empty queue, playback stops.
    fn next_song(&mut self) {
        match self.queue.pop_front() {
            Some(entry) => self.play_now(entry),
            None => self.stop(),
        }
    }

    /// Goes back to the previous song. The song that was playing returns to the front of the queue.
    fn prev_song(&mut self) {
        let Some(prev) = self.history.pop() else { return };
        let current = self.now.take();
        if self.load_entry(prev.clone()) {
            if let Some(cur) = current {
                self.queue.push_front(cur.entry);
            }
        } else {
            self.history.push(prev);
            self.now = current;
        }
    }

    fn stop(&mut self) {
        if let Some(cur) = self.now.take() {
            self.history.push(cur.entry);
        }
        self.with_player(|p| p.stop());
    }

    fn toggle_play(&mut self) {
        if self.now.is_some() {
            self.with_player(|p| p.toggle_pause());
        } else if !self.queue.is_empty() {
            self.next_song();
        }
    }

    fn enqueue(&mut self, entry: Entry) {
        self.queue.push_back(entry);
        if self.now.is_none() && self.settings.autoplay {
            self.next_song();
        }
    }

    /// A song that has run out: play the next one, or stop when autoplay is off.
    fn check_finished(&mut self) {
        if self.now.is_none() {
            return;
        }
        if self.with_player(|p| p.finished()).unwrap_or(false) {
            if self.settings.autoplay {
                self.next_song();
            } else {
                self.stop();
            }
        }
    }

    fn open_pack(&mut self, path: &Path) {
        if self.pack_key.is_empty() {
            self.error("ตั้งคีย์ไฟล์ .ckr ก่อน (เมนู ตั้งค่า)");
            self.show_settings = true;
            return;
        }
        match PackReader::open(path, &self.pack_key) {
            Ok(reader) => {
                let file = path.to_string_lossy().into_owned();
                let songs: Vec<SongInfo> = reader
                    .entries()
                    .iter()
                    .enumerate()
                    .map(|(i, e)| SongInfo {
                        path: pack_song_path(&file, i),
                        title: if e.title.is_empty() { "(ไม่มีชื่อ)".to_string() } else { e.title.clone() },
                        artist: e.artist.clone(),
                        key: e.key.clone(),
                    })
                    .collect();
                let n = songs.len();
                self.library.add_pack_songs(songs);
                self.info(format!("เพิ่มเพลงจากไฟล์ .ckr {n} เพลงในคลัง"));
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    /// Opens a file or folder given by the user, by its kind.
    fn open_path(&mut self, path: PathBuf) {
        if path.is_dir() {
            return self.set_folder(path);
        }
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "sf2" | "sfo" => self.load_font(&path),
            "mid" | "midi" | "kar" => {
                let entry = match read_song_info(&path) {
                    Some(info) => view::entry_of(&info),
                    None => Entry {
                        arg: path.to_string_lossy().into_owned(),
                        title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                        artist: String::new(),
                        key: String::new(),
                    },
                };
                self.enqueue(entry);
            }
            "ckr" => self.open_pack(&path),
            _ => self.error("ไม่รู้จักชนิดไฟล์นี้ (รับ .sf2 .mid .kar .ckr หรือโฟลเดอร์)"),
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::TogglePlay => self.toggle_play(),
            Cmd::Next => self.next_song(),
            Cmd::Prev => self.prev_song(),
            Cmd::Seek(ms) => {
                self.with_player(|p| p.seek(ms));
            }
            Cmd::SeekBy(delta) => {
                self.with_player(|p| {
                    let target = p.position_ms() + delta;
                    p.seek(target);
                });
            }
            Cmd::Speed(s) => {
                self.with_player(|p| p.set_speed(s));
            }
            Cmd::Transpose(n) => {
                self.with_player(|p| p.set_transpose(n));
            }
            Cmd::Volume(v) => {
                self.with_player(|p| p.set_volume(v));
                self.settings.volume = v;
                self.settings.save();
            }
            Cmd::ChannelGain(ch, g) => {
                self.with_player(|p| p.set_channel_gain(ch, g));
            }
            Cmd::ChannelMute(ch, m) => {
                self.with_player(|p| p.set_channel_mute(ch, m));
            }
            Cmd::ChannelSolo(ch, s) => {
                self.with_player(|p| p.set_channel_solo(ch, s));
            }
            Cmd::ResetMixer => {
                self.with_player(|p| p.reset_mixer());
            }
            Cmd::DrumGain(k, g) => {
                self.with_player(|p| p.set_drum_gain(k, g));
            }
            Cmd::DrumMute(k, m) => {
                self.with_player(|p| p.set_drum_mute(k, m));
            }
            Cmd::ResetDrums => {
                self.with_player(|p| p.reset_drums());
            }
            Cmd::PlayNow(entry) => self.play_now(entry),
            Cmd::Enqueue(entry) => self.enqueue(entry),
            Cmd::QueuePlay(i) => {
                self.queue_sel = None;
                if let Some(entry) = self.queue.remove(i) {
                    self.play_now(entry);
                }
            }
            Cmd::QueueRemove(i) => {
                self.queue_sel = None;
                self.queue.remove(i);
            }
            Cmd::QueueClear => {
                self.queue_sel = None;
                self.queue.clear();
            }
            Cmd::ChooseSoundFont => {
                if let Some(path) = rfd::FileDialog::new().add_filter("SoundFont", &["sf2", "sfo"]).pick_file() {
                    self.load_font(&path);
                }
            }
            Cmd::ChooseFile => {
                let picked = rfd::FileDialog::new()
                    .add_filter("เพลงและ SoundFont", &["sf2", "sfo", "mid", "midi", "kar", "ckr"])
                    .pick_file();
                if let Some(path) = picked {
                    self.open_path(path);
                }
            }
            Cmd::ChooseFolder => {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    self.set_folder(folder);
                }
            }
            Cmd::Rescan => self.library.rescan(),
            Cmd::LoadSoundFont(path) => self.load_font(&path),
            Cmd::OpenSoundFontPath => self.sf_path_input = Some(String::new()),
            Cmd::RescanSoundFonts => self.rescan_soundfonts(),
            Cmd::AddSoundFontFolder => {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    self.add_soundfont_folder(folder);
                }
            }
            Cmd::RemoveSoundFontFolder(i) => {
                if i < self.settings.soundfont_dirs.len() {
                    self.settings.soundfont_dirs.remove(i);
                    self.settings.save();
                    self.rescan_soundfonts();
                }
            }
            Cmd::SearchEnter => self.search_enter(),
            Cmd::SetAutoplay(on) => {
                self.settings.autoplay = on;
                self.settings.save();
            }
        }
    }

    /// The player's state, read once per frame.
    fn snapshot(&self) -> Snapshot {
        self.with_player(|p| {
            let status = p.channel_status();
            let levels = p.channel_levels();
            let channels = (0..16)
                .map(|ch| ChannelView {
                    name: status[ch].preset.clone(),
                    used: status[ch].used,
                    drums: status[ch].drums,
                    gain: p.channel_gain(ch),
                    mute: p.channel_mute(ch),
                    solo: p.channel_solo(ch),
                    level: levels[ch],
                })
                .collect();
            let keys = p.drum_keys_used();
            let drums = (0..128)
                .filter(|&k| keys[k])
                .map(|k| DrumView { key: k, gain: p.drum_gain(k), mute: p.drum_mute(k) })
                .collect();
            Snapshot {
                pos: p.position_ms(),
                dur: p.duration_ms(),
                paused: p.paused(),
                speed: p.speed(),
                transpose: p.transpose(),
                volume: p.volume(),
                channels,
                drums,
            }
        })
        .unwrap_or_default()
    }

    /// Keyboard shortcuts, and typing into the search. Typed text goes to the search whenever no
    /// widget is taking keys, so the user never has to click the search first.
    fn handle_keys(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let texts: Vec<String> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        });
        let keys = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::Escape),
                i.modifiers.command,
                i.key_pressed(egui::Key::OpenBracket),
                i.key_pressed(egui::Key::CloseBracket),
                i.key_pressed(egui::Key::Backslash),
            )
        });
        let (space, left, right, up, down, enter, backspace, escape, command, slower, faster, reset) = keys;
        if ctx.input(|i| i.modifiers.command && i.modifiers.ctrl && i.key_pressed(egui::Key::F)) {
            self.toggle_fullscreen(ctx);
        }

        if command {
            if left {
                self.cmds.push(Cmd::Prev);
            }
            if right {
                self.cmds.push(Cmd::Next);
            }
            if slower {
                self.cmds.push(Cmd::Speed((snap.speed - 0.05).clamp(0.5, 2.0)));
            }
            if faster {
                self.cmds.push(Cmd::Speed((snap.speed + 0.05).clamp(0.5, 2.0)));
            }
            if reset {
                self.cmds.push(Cmd::Speed(1.0));
            }
            return;
        }

        // Decided before this frame's text is typed: a space that types into a search does not pause.
        let searching = !self.search.is_empty();
        if space && !searching {
            self.cmds.push(Cmd::TogglePlay);
        }
        for text in &texts {
            if search::accept_text(&mut self.search, text) {
                self.selected = 0;
            }
        }
        if !self.search.is_empty() {
            if backspace {
                search::backspace(&mut self.search);
                self.selected = 0;
            }
            if escape {
                self.search.clear();
                self.selected = 0;
            }
            if enter {
                self.cmds.push(Cmd::SearchEnter);
            }
            let len = self.library.search(&self.search).len();
            let mut moved = false;
            if up || left {
                search::move_selection(&mut self.selected, -1, len);
                moved = true;
            }
            if down || right {
                search::move_selection(&mut self.selected, 1, len);
                moved = true;
            }
            if moved {
                self.scroll_to_selected = true;
            }
        } else {
            if escape {
                self.queue_sel = None;
            }
            if (up || down) && !self.queue.is_empty() {
                self.move_queue_selection(if down { 1 } else { -1 });
            }
            if enter {
                if let Some(i) = self.queue_sel {
                    self.cmds.push(Cmd::QueuePlay(i));
                }
            }
            if left {
                self.cmds.push(Cmd::SeekBy(-5000.0));
            }
            if right {
                self.cmds.push(Cmd::SeekBy(5000.0));
            }
        }
    }

    /// Moves the playlist selection one song up or down. The first press picks the top song, or the bottom one
    /// going up. Each move keeps the list open for another few seconds.
    fn move_queue_selection(&mut self, step: isize) {
        let len = self.queue.len() as isize;
        if len == 0 {
            self.queue_sel = None;
            return;
        }
        let next = match self.queue_sel {
            None if step > 0 => 0,
            None => len - 1,
            Some(i) => (i as isize + step).clamp(0, len - 1),
        };
        self.queue_sel = Some(next as usize);
        self.queue_seen = Instant::now();
    }

    /// Queues the selected search result and clears the search, ready for the next song.
    fn search_enter(&mut self) {
        let results = self.library.search(&self.search);
        if let Some(&i) = results.get(self.selected) {
            let entry = view::entry_of(&self.library.songs()[i]);
            self.enqueue(entry);
        }
        self.search.clear();
        self.selected = 0;
    }

    /// The folders the SoundFont search covers: macOS's own, and the ones the user added.
    fn soundfont_search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = soundfonts::default_dirs();
        for dir in &self.settings.soundfont_dirs {
            if !dirs.contains(dir) {
                dirs.push(dir.clone());
            }
        }
        dirs.retain(|d| d.is_dir());
        dirs
    }

    /// Searches the SoundFont folders again, in the background.
    fn rescan_soundfonts(&mut self) {
        let dirs = self.soundfont_search_dirs();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(soundfonts::find(&dirs));
        });
        self.sf_scan = Some(rx);
    }

    /// Opens a SoundFont typed as a path. A folder becomes a place to search.
    fn open_soundfont_path(&mut self, typed: &str) {
        let expanded = match typed.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or_else(|| PathBuf::from(typed)),
            None => PathBuf::from(typed),
        };
        if expanded.is_dir() {
            self.add_soundfont_folder(expanded);
        } else if soundfonts::is_soundfont(&expanded) && expanded.is_file() {
            self.load_font(&expanded);
        } else {
            self.error(format!("ไม่พบไฟล์ SoundFont ที่ path นี้: {typed}"));
        }
    }

    fn add_soundfont_folder(&mut self, folder: PathBuf) {
        if !self.settings.soundfont_dirs.contains(&folder) {
            self.settings.soundfont_dirs.push(folder.clone());
            self.settings.save();
        }
        self.info(format!("เพิ่มโฟลเดอร์ค้นหา SoundFont: {}", folder.display()));
        self.rescan_soundfonts();
    }

    /// The background image as a texture, with its size. Decoded on first use and again only when the file changes.
    fn background_texture(&mut self, ctx: &egui::Context, path: &Path) -> Option<(egui::TextureId, [usize; 2])> {
        if let Some((cached, tex)) = &self.bg_texture {
            if cached == path {
                return Some((tex.id(), tex.size()));
            }
        }
        if self.bg_failed.as_deref() == Some(path) {
            return None;
        }
        let decoded = fs::read(path)
            .ok()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|img| img.thumbnail(2560, 2560).to_rgba8());
        let Some(img) = decoded else {
            self.error("เปิดรูปพื้นหลังไม่ได้ (ใช้สีดำแทน)");
            self.bg_failed = Some(path.to_path_buf());
            return None;
        };
        let size = [img.width() as usize, img.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let tex = ctx.load_texture("background", color, egui::TextureOptions::LINEAR);
        let result = (tex.id(), tex.size());
        self.bg_texture = Some((path.to_path_buf(), tex));
        Some(result)
    }

    fn handle_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for path in dropped {
            self.open_path(path);
        }
    }

    /// The menu: the buttons for the windows and files, and the last message.
    fn menu(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let queue_label = format!("คิว ({})", self.queue.len());
            if ui.selectable_label(self.show_queue, queue_label).clicked() {
                self.show_queue = !self.show_queue;
                self.queue_sel = None;
            }
            if ui.selectable_label(self.show_mixer, "มิกเซอร์").clicked() {
                self.show_mixer = !self.show_mixer;
            }
            if ui.button("SoundFont…").clicked() {
                self.cmds.push(Cmd::ChooseSoundFont);
            }
            if ui.button("เปิดไฟล์…").on_hover_text("เพลง (.mid .kar), ไฟล์ .ckr หรือ SoundFont").clicked() {
                self.cmds.push(Cmd::ChooseFile);
            }
            if ui.selectable_label(self.show_settings, "ตั้งค่า").clicked() {
                self.show_settings = !self.show_settings;
            }
            if ui.selectable_label(self.fullscreen, "เต็มจอ").clicked() {
                let ctx = ui.ctx().clone();
                self.toggle_fullscreen(&ctx);
            }
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut background = self.settings.background.clone();
        let mut pick_image = false;
        let mut reset_background = false;
        egui::Window::new("ตั้งค่า").open(&mut open).resizable(false).default_width(340.0).show(ctx, |ui| {
            ui.label("รหัสไฟล์ .ckr (พิมพ์ทุกครั้งที่เปิดแอป ไม่บันทึกลงเครื่อง)");
            ui.add(egui::TextEdit::singleline(&mut self.pack_key).password(true).desired_width(260.0));
            ui.separator();
            ui.heading("พื้นหลังจอเนื้อเพลง");
            ui.horizontal_wrapped(|ui| {
                for (name, rgb, dark) in PRESET_BACKGROUNDS {
                    let fill = eframe::egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                    if ui.add(egui::Button::new("   ").fill(fill).min_size(egui::vec2(34.0, 22.0))).on_hover_text(name).clicked() {
                        background = Background::Color { rgb, dark_text: dark };
                    }
                }
            });
            ui.horizontal(|ui| {
                let mut rgb = match &background {
                    Background::Color { rgb, .. } => *rgb,
                    Background::Image { .. } => [10, 10, 14],
                };
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    background = Background::Color { rgb, dark_text: background.dark_text() };
                }
                ui.label("สีกำหนดเอง");
            });
            let mut dark_text = background.dark_text();
            if ui.checkbox(&mut dark_text, "ตัวอักษรสีเข้ม (สำหรับพื้นหลังสว่าง)").changed() {
                background.set_dark_text(dark_text);
            }
            ui.horizontal(|ui| {
                if ui.button("เลือกรูปภาพ…").clicked() {
                    pick_image = true;
                }
                if ui.button("คืนค่าเริ่มต้น").clicked() {
                    reset_background = true;
                }
            });
            if let Background::Image { dim, .. } = &mut background {
                let mut percent = *dim * 100.0;
                if ui.add(egui::Slider::new(&mut percent, 0.0..=90.0).suffix("% มืด")).changed() {
                    *dim = percent / 100.0;
                }
            }
            ui.separator();
            let folder = self.settings.library_folder.as_ref().map(|p| p.display().to_string());
            ui.label(format!("โฟลเดอร์เพลง: {}", folder.unwrap_or_else(|| "ยังไม่ได้เลือก".to_string())));
            ui.label(format!(
                "อุปกรณ์เสียงออก: {}",
                if self.output.is_some() { "พร้อม" } else { "เปิดไม่ได้" }
            ));
            ui.label(format!("SoundFont: {}", self.font_name.clone().unwrap_or_else(|| "ยังไม่ได้เลือก".to_string())));
        });
        if pick_image {
            if let Some(path) = rfd::FileDialog::new().add_filter("รูปภาพ", &["png", "jpg", "jpeg"]).pick_file() {
                background = Background::Image { path, dim: 0.45, dark_text: background.dark_text() };
            }
        }
        if reset_background {
            background = Background::default();
        }
        if background != self.settings.background {
            self.settings.background = background;
            self.settings.save();
        }
        self.show_settings = open;
    }
}

/// Adds a Thai font as the fallback for every family, since egui's built-in fonts have no Thai glyphs.
fn install_thai_font(ctx: &egui::Context) -> bool {
    const CANDIDATES: [&str; 3] = [
        "/System/Library/Fonts/Supplemental/Sathu.ttf",
        "/System/Library/Fonts/Supplemental/Ayuthaya.ttf",
        "/System/Library/Fonts/Supplemental/Krungthep.ttf",
    ];
    for path in CANDIDATES {
        let Ok(bytes) = fs::read(path) else { continue };
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert("thai".to_string(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("thai".to_string());
        }
        ctx.set_fonts(fonts);
        return true;
    }
    false
}

/// The part of an image that fills `rect` without stretching it (the image is cropped, not squeezed).
fn cover_uv(image: [f32; 2], rect: eframe::egui::Rect) -> eframe::egui::Rect {
    let image_aspect = image[0] / image[1];
    let rect_aspect = rect.width() / rect.height();
    if image_aspect > rect_aspect {
        let visible = rect_aspect / image_aspect;
        let u0 = (1.0 - visible) / 2.0;
        eframe::egui::Rect::from_min_max(eframe::egui::pos2(u0, 0.0), eframe::egui::pos2(u0 + visible, 1.0))
    } else {
        let visible = image_aspect / rect_aspect;
        let v0 = (1.0 - visible) / 2.0;
        eframe::egui::Rect::from_min_max(eframe::egui::pos2(0.0, v0), eframe::egui::pos2(1.0, v0 + visible))
    }
}

/// The background behind the lyrics: a near-black, so the words stand out like a karaoke screen.
const LYRIC_BACKGROUND: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(10, 10, 14);

/// How long a search may sit without typing, or the playlist without a pick, before it clears or hides itself.
const IDLE_HIDE: Duration = Duration::from_secs(5);

/// The UI windows are 80% opaque, so they are 20% transparent.
const UI_ALPHA: u8 = 204;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(typed) = std::env::var("CKARAOKE_TEST_TYPE").ok().filter(|_| self.frames == 30) {
            // Test aid: types text as the keyboard would, to exercise typed search without a click.
            ctx.input_mut(|i| i.events.push(egui::Event::Text(typed)));
        }
        if std::env::var_os("CKARAOKE_SCREENSHOT").is_some() || std::env::var_os("CKARAOKE_TEST_TYPE").is_some() {
            self.frames += 1;
        }
        if std::env::var_os("CKARAOKE_SCREENSHOT").is_some() {
            // Test aid: after a while, save what the window shows (raw RGBA, see tools/gui/png_from_raw.py) and quit.
            let at: u32 = std::env::var("CKARAOKE_SCREENSHOT_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(180);
            if self.frames == at {
                let path = std::env::var("CKARAOKE_SCREENSHOT").unwrap_or_default();
                ctx.request_screenshot(move |image| {
                    let mut raw = format!("{} {}\n", image.size[0], image.size[1]).into_bytes();
                    for px in image.pixels.iter() {
                        raw.extend_from_slice(&px.to_array());
                    }
                    let _ = std::fs::write(&path, raw);
                    std::process::exit(0);
                });
            }
        }
        self.library.poll();
        if let Some(rx) = &self.sf_scan {
            if let Ok(found) = rx.try_recv() {
                self.soundfonts = found;
                self.sf_scan = None;
            }
        }
        // Typing, the arrow keys and the pointer count as use: a search or a playlist pick stays while it is in use,
        // including while the lyrics are dragged to sync them.
        let in_use = ctx.input(|i| {
            i.pointer.any_down()
                || [egui::Key::ArrowUp, egui::Key::ArrowDown, egui::Key::ArrowLeft, egui::Key::ArrowRight]
                    .iter()
                    .any(|k| i.key_pressed(*k))
        });
        if in_use {
            self.search_active = Instant::now();
            self.queue_seen = Instant::now();
        }
        if self.search != self.search_seen {
            self.search_seen = self.search.clone();
            self.search_active = Instant::now();
        }
        if !self.search.is_empty() && self.search_active.elapsed() > IDLE_HIDE {
            // A search that has not been typed into for a few seconds clears itself and closes the library.
            self.search.clear();
            self.search_seen.clear();
            self.selected = 0;
        }
        if self.queue_sel.is_some() && !self.show_queue && self.queue_seen.elapsed() > IDLE_HIDE {
            // The playlist that the arrow keys opened hides again when nothing has been picked for a while.
            self.queue_sel = None;
        }
        self.check_finished();
        let snap = self.snapshot();
        self.handle_keys(&ctx, &snap);
        self.handle_dropped(&ctx);

        let has_song = self.now.is_some();
        let has_font = self.font_name.is_some();

        // The top menu and the transport appear only when the mouse is near them, unless they are locked.
        let screen = ctx.content_rect();
        let top_zone = eframe::egui::Rect::from_min_size(screen.min, eframe::egui::vec2(screen.width(), 90.0));
        let bottom_zone = eframe::egui::Rect::from_min_max(
            eframe::egui::pos2(screen.min.x, screen.max.y - 150.0),
            screen.max,
        );
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let locked = self.settings.chrome_locked;
        let hovering_top = pointer.is_some_and(|p| top_zone.contains(p));
        let hovering_bottom = pointer.is_some_and(|p| bottom_zone.contains(p));
        let menu_visible = locked || hovering_top || matches!(self.widget_menu, Some((_, WidgetKind::Menu)));
        let transport_visible =
            locked || hovering_bottom || matches!(self.widget_menu, Some((_, WidgetKind::Transport)));

        // Right-click on a visible bar opens its menu. A right-click anywhere else closes it, and so does Escape.
        let over_bar = (menu_visible && hovering_top) || (transport_visible && hovering_bottom);
        let menu_rect = self.widget_menu_rect;
        if let Some(p) = pointer {
            let inside_menu = menu_rect.is_some_and(|r| r.contains(p));
            if ctx.input(|i| i.pointer.secondary_clicked()) && !inside_menu {
                if menu_visible && top_zone.contains(p) {
                    self.widget_menu = Some((p, WidgetKind::Menu));
                } else if transport_visible && bottom_zone.contains(p) {
                    self.widget_menu = Some((p, WidgetKind::Transport));
                } else {
                    self.widget_menu = None;
                    self.widget_menu_rect = None;
                }
            }
        }
        if let Some(rect) = menu_rect {
            let clicked_outside = ctx.input(|i| i.pointer.primary_clicked())
                && pointer.is_some_and(|p| !rect.contains(p));
            if clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.widget_menu = None;
                self.widget_menu_rect = None;
            }
        }

        let background = self.settings.background.clone();
        let palette = view::Palette::for_background(background.dark_text());
        let image = match &background {
            Background::Image { path, .. } => self.background_texture(&ctx, path),
            Background::Color { .. } => None,
        };
        let fill = match &background {
            Background::Color { rgb, .. } => eframe::egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
            Background::Image { .. } => LYRIC_BACKGROUND,
        };
        let menu_state = view::MenuState {
            font_name: self.font_name.clone(),
            fonts: self.soundfonts.clone(),
            extra_dirs: self.settings.soundfont_dirs.clone(),
            scanning: self.sf_scan.is_some(),
        };
        let mut lyric_offset = self.settings.lyrics_offset;
        let mut lyric_locked = self.settings.lyrics_locked;
        let mut lyric_commit = false;
        egui::CentralPanel::default().frame(egui::Frame::default().fill(fill)).show(ui, |ui| {
            // The right-click menu covers the whole screen. The lyric block is registered later, so it sits on top
            // for dragging, and right-clicks on it still reach this menu.
            // Over the top or bottom bar, right-click belongs to the bar's own menu instead.
            let screen = ui.interact(ui.max_rect(), ui.scope_id().with("screen-menu"), Sense::click());
            if !over_bar {
                screen.context_menu(|ui| {
                    if view::context_menu(ui, &menu_state, &mut lyric_locked, &mut lyric_offset, &mut self.cmds) {
                        lyric_commit = true;
                    }
                });
            }
            if let Background::Image { dim, .. } = &background {
                let rect = ui.max_rect();
                if let Some((texture, size)) = image {
                    let uv = cover_uv([size[0] as f32, size[1] as f32], rect);
                    ui.painter().image(texture, rect, uv, eframe::egui::Color32::WHITE);
                }
                let shade = (dim.clamp(0.0, 0.95) * 255.0) as u8;
                ui.painter().rect_filled(rect, 0.0, eframe::egui::Color32::from_black_alpha(shade));
            }
            match self.now.as_ref() {
                Some(now) => {
                    lyric_commit = view::lyrics(
                        ui,
                        &now.lyrics,
                        &now.entry.title,
                        snap.pos,
                        palette,
                        &mut lyric_offset,
                        &mut lyric_locked,
                    );
                }
                None => view::welcome(ui, has_font),
            }
        });
        if lyric_offset != self.settings.lyrics_offset || lyric_locked != self.settings.lyrics_locked {
            self.settings.lyrics_offset = lyric_offset;
            self.settings.lyrics_locked = lyric_locked;
            if lyric_commit {
                self.settings.save();
            }
        }

        if menu_visible {
            egui::Window::new("เมนู")
                .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
                .title_bar(false)
                .resizable(false)
                .movable(false)
                .auto_sized()
                .show(&ctx, |ui| self.menu(ui));
        }
        if transport_visible {
            let width = ctx.content_rect().width().min(980.0);
            egui::Window::new("ควบคุม")
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0])
                .title_bar(false)
                .resizable(false)
                .movable(false)
                .default_width(width)
                .show(&ctx, |ui| view::transport(ui, &snap, has_song, &mut self.cmds));
        }

        // A message stays on screen for a few seconds at the top, whether or not the menu is showing.
        if let Some((text, is_error)) = self.message.clone() {
            if self.message_at.elapsed() < Duration::from_secs(6) {
                egui::Area::new(egui::Id::unique("message"))
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 14.0])
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| view::message(ui, &text, is_error));
                    });
            }
        }

        // The menu of a bar, opened by right-click.
        if let Some((at, kind)) = self.widget_menu {
            let mut lock = self.settings.chrome_locked;
            let mut close = false;
            let area = egui::Area::new(egui::Id::unique("widget-menu")).order(egui::Order::Foreground).fixed_pos(at).show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(210.0);
                    ui.checkbox(&mut lock, "ล็อก (แสดงตลอด)");
                    ui.separator();
                    ui.label(match kind {
                        WidgetKind::Menu => "เมนูด้านบน",
                        WidgetKind::Transport => "แถบควบคุม",
                    });
                    if ui.button("ปิดเมนูนี้").clicked() {
                        close = true;
                    }
                });
            });
            self.widget_menu_rect = Some(area.response.rect);
            if lock != self.settings.chrome_locked {
                self.settings.chrome_locked = lock;
                self.settings.save();
            }
            if close {
                self.widget_menu = None;
                self.widget_menu_rect = None;
            }
        }

        if !self.search.is_empty() {
            let height = ctx.content_rect().height() - 20.0;
            let library = egui::Window::new("คลังเพลง")
                .anchor(egui::Align2::LEFT_TOP, [10.0, 10.0])
                .fixed_size([440.0, height])
                .collapsible(false)
                .title_bar(false)
                .frame(egui::Frame::default().fill(view::LIBRARY_FILL.gamma_multiply(UI_ALPHA as f32 / 255.0)).inner_margin(egui::Margin::same(8)).corner_radius(6.0))
                .show(&ctx, |ui| {
                    let folder = self.settings.library_folder.as_ref().map(|p| p.display().to_string());
                    let results = self.library.search(&self.search);
                    view::library(
                        ui,
                        self.library.songs(),
                        &results,
                        &self.search,
                        self.selected,
                        &mut self.scroll_to_selected,
                        folder.as_deref(),
                        self.library.scan_progress(),
                        &mut self.cmds,
                    );
                });
            if library.is_some_and(|w| w.response.hovered()) {
                self.search_active = Instant::now();
            }
        }

        // The playlist stays while it is pinned from the menu, or while the arrow keys have a song picked.
        if self.show_queue || self.queue_sel.is_some() {
            let height = ctx.content_rect().height() - 20.0;
            let mut open = true;
            let window = egui::Window::new("คิวเพลง")
                .open(&mut open)
                .anchor(egui::Align2::RIGHT_TOP, [-10.0, 60.0])
                .fixed_size([320.0, height - 50.0])
                .collapsible(false)
                .show(&ctx, |ui| {
                    view::queue(ui, &self.queue, self.settings.autoplay, self.queue_sel, &mut self.cmds);
                });
            if window.is_some_and(|w| w.response.hovered()) {
                self.queue_seen = Instant::now();
            }
            if !open {
                self.show_queue = false;
                self.queue_sel = None;
            }
        }

        if self.show_mixer {
            let mut open = true;
            egui::Window::new("มิกเซอร์").open(&mut open).default_width(640.0).show(&ctx, |ui| {
                view::mixer(ui, &snap, &mut self.cmds);
            });
            self.show_mixer = open;
        }
        self.settings_window(&ctx);
        if let Some(mut typed) = self.sf_path_input.take() {
            let mut open = true;
            let mut submit = false;
            let mut cancel = false;
            egui::Window::new("พิมพ์ path ของ SoundFont").open(&mut open).collapsible(false).show(&ctx, |ui| {
                ui.label("ไฟล์ .sf2 หรือโฟลเดอร์ที่มีไฟล์ (เริ่มด้วย ~/ ได้)");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut typed)
                        .hint_text("/path/to/soundfont.sf2")
                        .desired_width(460.0),
                );
                edit.request_focus();
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("โหลด").clicked() {
                        submit = true;
                    }
                    if ui.button("ยกเลิก").clicked() {
                        cancel = true;
                    }
                });
            });
            if submit {
                let text = typed.trim().to_string();
                self.open_soundfont_path(&text);
            } else if open && !cancel {
                self.sf_path_input = Some(typed);
            }
        }

        for cmd in std::mem::take(&mut self.cmds) {
            self.handle(cmd);
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}
