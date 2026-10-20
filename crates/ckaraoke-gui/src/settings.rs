//! Settings the app remembers between launches: the SoundFont, the song folder and the queue options.
//! The `.ckr` key is never written here; it is typed in each session.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}

fn full_volume() -> f32 {
    1.0
}

/// What is behind the lyrics. `dark_text` makes the lyric text dark, for a light background.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Background {
    Color { rgb: [u8; 3], dark_text: bool },
    Image { path: PathBuf, dim: f32, dark_text: bool },
}

impl Default for Background {
    fn default() -> Self {
        Background::Color { rgb: [10, 10, 14], dark_text: false }
    }
}

impl Background {
    pub fn dark_text(&self) -> bool {
        match self {
            Background::Color { dark_text, .. } | Background::Image { dark_text, .. } => *dark_text,
        }
    }

    pub fn set_dark_text(&mut self, on: bool) {
        match self {
            Background::Color { dark_text, .. } | Background::Image { dark_text, .. } => *dark_text = on,
        }
    }
}

/// Ready-made backgrounds: name, colour, and whether the lyric text should be dark on it.
pub const PRESET_BACKGROUNDS: [(&str, [u8; 3], bool); 7] = [
    ("ดำ", [10, 10, 14], false),
    ("น้ำเงินเข้ม", [12, 24, 56], false),
    ("เขียวป่า", [14, 40, 28], false),
    ("ม่วง", [40, 18, 56], false),
    ("เทาถ่าน", [44, 44, 50], false),
    ("ครีม", [250, 246, 232], true),
    ("ฟ้าอ่อน", [206, 230, 250], true),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub soundfont: Option<PathBuf>,
    pub library_folder: Option<PathBuf>,
    /// A finished song starts the next one in the queue.
    #[serde(default = "yes")]
    pub autoplay: bool,
    #[serde(default = "full_volume")]
    pub volume: f32,
    #[serde(default)]
    pub background: Background,
    /// Where the lyric block sits, as a fraction of the window height from its default place (dragged up or down).
    #[serde(default)]
    pub lyrics_offset: f32,
    /// A locked lyric block cannot be dragged.
    #[serde(default)]
    pub lyrics_locked: bool,
    /// Folders the user added to the SoundFont search, on top of macOS's own SoundFont folders.
    #[serde(default)]
    pub soundfont_dirs: Vec<PathBuf>,
    /// The top menu and the transport stay visible all the time, instead of appearing when the mouse is near.
    #[serde(default)]
    pub chrome_locked: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            soundfont: None,
            library_folder: None,
            autoplay: true,
            volume: 1.0,
            background: Background::default(),
            lyrics_offset: 0.0,
            lyrics_locked: false,
            soundfont_dirs: Vec::new(),
            chrome_locked: false,
        }
    }
}

/// Where the settings file lives: the macOS application-support folder, or the XDG config folder elsewhere.
pub fn settings_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/CKaraoke")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("ckaraoke")
    }
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read_to_string(settings_dir().join("settings.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let dir = settings_dir();
        if std::fs::create_dir_all(&dir).is_ok() {
            if let Ok(text) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(dir.join("settings.json"), text);
            }
        }
    }
}
