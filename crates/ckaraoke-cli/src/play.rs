//! Live playback: the default audio output, the keyboard, and the lyric screen. Mirrors the C++ app's
//! console player (`playSong` and `draw` in src/main.cpp).

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use ckaraoke_audio::Output;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;

use ckaraoke_core::lyrics::Lyrics;
use ckaraoke_core::player::Player;
use ckaraoke_core::song::load_song;
use ckaraoke_core::text::display_width;
use ckaraoke_core::{Error, Result};

const FRAME: Duration = Duration::from_millis(33);
const SEEK_MS: f64 = 5000.0;

fn mmss(ms: f64) -> String {
    let s = (ms / 1000.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Leaves the terminal in raw mode for as long as it lives.
struct RawMode;

impl RawMode {
    fn enter() -> Result<RawMode> {
        terminal::enable_raw_mode().map_err(|e| Error::new(format!("เปิดโหมดรับปุ่มไม่ได้: {e}")))?;
        print!("\x1b[?25l\x1b[2J");
        std::io::stdout().flush().ok();
        Ok(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        println!("\x1b[0m\x1b[?25h");
        std::io::stdout().flush().ok();
    }
}

/// One line of lyrics: the sung syllables highlighted, the rest plain, centred.
fn lyric_line(line: &ckaraoke_core::lyrics::LyricLine, pos: f64, cols: usize, active: bool) -> String {
    let width: usize = line.sylls.iter().map(|s| display_width(&s.text)).sum();
    let mut out = " ".repeat(cols.saturating_sub(width) / 2);
    for s in &line.sylls {
        let sung = active && s.ms <= pos;
        out.push_str(if sung {
            "\x1b[1;33m"
        } else if active {
            "\x1b[0;97m"
        } else {
            "\x1b[0;2m"
        });
        out.push_str(&s.text);
    }
    out.push_str("\x1b[0m");
    out
}

/// The screen: title, progress bar, and the two lyric lines around the current position.
fn draw(player: &Player, lyrics: &Lyrics, name: &str, volume: f32) {
    let (cols, _) = terminal::size().map(|(c, r)| (c as usize, r)).unwrap_or((80, 24));
    let pos = player.position_ms();
    let dur = player.duration_ms();
    let bar_w = cols.saturating_sub(34).clamp(10, 50);
    let filled = if dur > 0.0 { (bar_w as f64 * pos / dur) as usize } else { 0 };
    let bar: String = (0..bar_w).map(|i| if i < filled { '█' } else { '░' }).collect();

    let mut rows: Vec<String> = Vec::new();
    let mut title = if lyrics.title.is_empty() { name.to_string() } else { lyrics.title.clone() };
    if !lyrics.artist.is_empty() {
        title.push_str(&format!(" — {}", lyrics.artist));
    }
    rows.push(format!(" \x1b[1;35m♪ CKaraoke\x1b[0m  {title}"));
    rows.push(format!(
        " {} {} {} {}  เสียง {:3}%",
        if player.paused() { "⏸" } else { "▶" },
        mmss(pos),
        bar,
        mmss(dur),
        (volume * 100.0).round() as i32
    ));
    rows.push(String::new());

    let lines = &lyrics.lines;
    if lines.is_empty() {
        rows.push(String::new());
        rows.push(format!("{}\x1b[2m(ไฟล์นี้ไม่มีเนื้อเพลง)\x1b[0m", " ".repeat(cols.saturating_sub(26) / 2)));
        rows.push(String::new());
    } else {
        // The current line is the last one that has started; the one after it is shown below.
        let cur = lines.partition_point(|l| l.start_ms <= pos) as i64 - 1;
        if cur < 0 {
            let wait = lines[0].start_ms - pos;
            rows.push(if wait < 4000.0 {
                format!(
                    "{}\x1b[1;36m{}\x1b[0m",
                    " ".repeat((cols / 2).saturating_sub(3)),
                    "*".repeat((wait / 1000.0) as usize + 1)
                )
            } else {
                String::new()
            });
            rows.push(lyric_line(&lines[0], pos, cols, true));
            rows.push(lines.get(1).map_or(String::new(), |l| lyric_line(l, pos, cols, false)));
        } else {
            let cur = cur as usize;
            rows.push(String::new());
            rows.push(lyric_line(&lines[cur], pos, cols, true));
            rows.push(lines.get(cur + 1).map_or(String::new(), |l| lyric_line(l, pos, cols, false)));
        }
    }
    rows.push(String::new());
    rows.push(
        " \x1b[2m[Space] หยุด/เล่น  [←/→] เลื่อน 5 วิ  [↑/↓] ระดับเสียง  [r] เริ่มใหม่  [q] ออก\x1b[0m".to_string(),
    );

    let mut frame = String::from("\x1b[H");
    for r in rows {
        frame.push_str("\x1b[2K");
        frame.push_str(&r);
        frame.push('\n');
    }
    print!("{frame}");
    std::io::stdout().flush().ok();
}

/// Plays one song through the default audio output until it ends or the user quits.
pub fn play(sound_font: &str, pack_key: &str, song_arg: &str) -> Result<bool> {
    let font = std::fs::read(sound_font).map_err(|_| Error::new(format!("เปิดไฟล์ไม่ได้: {sound_font}")))?;

    let output = Output::open()?;
    let loaded = load_song(song_arg, pack_key)?;
    output
        .load_sound_font(&font)
        .map_err(|e| Error::new(format!("โหลด SoundFont ไม่สำเร็จ: {sound_font} ({e})")))?;
    output
        .player()
        .lock()
        .map_err(|_| Error::new("เครื่องเล่นขัดข้อง"))?
        .load_midi(&loaded.midi)
        .map_err(Error::new)?;
    let player = Arc::clone(output.player());

    let name = std::path::Path::new(song_arg)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| song_arg.to_string());
    let _raw = RawMode::enter()?;
    let mut volume = 1.0f32;
    loop {
        if event::poll(FRAME).map_err(|e| Error::new(e.to_string()))? {
            if let Event::Key(key) = event::read().map_err(|e| Error::new(e.to_string()))? {
                if key.kind == KeyEventKind::Press {
                    let mut p = player.lock().map_err(|_| Error::new("เครื่องเล่นขัดข้อง"))?;
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                        KeyCode::Char('q') | KeyCode::Char('Q') => break,
                        KeyCode::Char(' ') => p.toggle_pause(),
                        KeyCode::Left => {
                            let target = p.position_ms() - SEEK_MS;
                            p.seek(target);
                        }
                        KeyCode::Right => {
                            let target = p.position_ms() + SEEK_MS;
                            p.seek(target);
                        }
                        KeyCode::Up | KeyCode::Char('+') | KeyCode::Char('=') => {
                            volume = (volume + 0.1).min(2.0);
                            p.set_volume(volume);
                        }
                        KeyCode::Down | KeyCode::Char('-') => {
                            volume = (volume - 0.1).max(0.0);
                            p.set_volume(volume);
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') => p.seek(0.0),
                        _ => {}
                    }
                }
            }
        }
        let finished = player.lock().map(|p| p.finished()).unwrap_or(true);
        let guard = player.lock().map_err(|_| Error::new("เครื่องเล่นขัดข้อง"))?;
        draw(&guard, &loaded.lyrics, &name, volume);
        drop(guard);
        if finished {
            break;
        }
    }
    drop(output);
    Ok(true)
}
