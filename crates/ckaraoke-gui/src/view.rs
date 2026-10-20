//! Drawing. Each panel reads a `Snapshot` of the player taken at the start of the frame and records what
//! the user asked for as `Cmd`s; the app runs them after drawing, so no lock is held while widgets draw.

use std::path::PathBuf;

use eframe::egui::{self, Color32, CursorIcon, FontId, RichText, Sense};
use eframe::egui::text::{LayoutJob, TextFormat};

use ckaraoke_core::lyrics::Lyrics;

use crate::drums;
use crate::queue::{Entry, Queue};

/// Colours of the lyric screen. The sung, current and upcoming text each have a light and a dark version.
#[derive(Clone, Copy)]
pub struct Palette {
    pub sung: Color32,
    pub active: Color32,
    pub upcoming: Color32,
    pub dim: Color32,
}

impl Palette {
    pub fn for_background(dark_text: bool) -> Palette {
        if dark_text {
            Palette {
                sung: Color32::from_rgb(200, 60, 20),
                active: Color32::from_gray(20),
                upcoming: Color32::from_gray(90),
                dim: Color32::from_gray(80),
            }
        } else {
            Palette {
                sung: Color32::from_rgb(255, 214, 80),
                active: Color32::WHITE,
                upcoming: Color32::from_gray(150),
                dim: Color32::from_gray(130),
            }
        }
    }
}

const DIM: Color32 = Color32::from_gray(130);
const ERROR: Color32 = Color32::from_rgb(235, 90, 80);
const METER: Color32 = Color32::from_rgb(90, 200, 110);
/// The library window, styled like a karaoke machine's screen: a navy panel with a lighter search box.
pub const LIBRARY_FILL: Color32 = Color32::from_rgb(10, 36, 84);
const SEARCH_FILL: Color32 = Color32::from_rgb(38, 84, 150);
const SELECTED_FILL: Color32 = Color32::from_rgb(60, 110, 170);
const TITLE: Color32 = Color32::WHITE;
const KEY: Color32 = Color32::from_rgb(255, 214, 110);
const ARTIST: Color32 = Color32::from_rgb(150, 210, 255);
const HINT: Color32 = Color32::from_rgb(170, 195, 225);

/// What the user asked for this frame.
#[derive(Debug, Clone)]
pub enum Cmd {
    TogglePlay,
    Next,
    Prev,
    Seek(f64),
    SeekBy(f64),
    Speed(f64),
    Transpose(i32),
    Volume(f32),
    ChannelGain(usize, f32),
    ChannelMute(usize, bool),
    ChannelSolo(usize, bool),
    ResetMixer,
    DrumGain(usize, f32),
    DrumMute(usize, bool),
    ResetDrums,
    PlayNow(Entry),
    Enqueue(Entry),
    QueuePlay(usize),
    QueueRemove(usize),
    QueueClear,
    ChooseSoundFont,
    /// Load a SoundFont from the list the search found.
    LoadSoundFont(PathBuf),
    /// Open the window where a SoundFont path can be typed.
    OpenSoundFontPath,
    RescanSoundFonts,
    AddSoundFontFolder,
    RemoveSoundFontFolder(usize),
    ChooseFile,
    /// Enter in the search: queue the selected result and clear the search.
    SearchEnter,
    ChooseFolder,
    Rescan,
    SetAutoplay(bool),
}

#[derive(Debug, Clone)]
pub struct ChannelView {
    pub name: String,
    pub used: bool,
    pub drums: bool,
    pub gain: f32,
    pub mute: bool,
    pub solo: bool,
    pub level: f32,
}

#[derive(Debug, Clone)]
pub struct DrumView {
    pub key: usize,
    pub gain: f32,
    pub mute: bool,
}

/// The player's state at the start of a frame.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub pos: f64,
    pub dur: f64,
    pub paused: bool,
    pub speed: f64,
    pub transpose: i32,
    pub volume: f32,
    pub channels: Vec<ChannelView>,
    pub drums: Vec<DrumView>,
}

pub fn mmss(ms: f64) -> String {
    let s = (ms / 1000.0).max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// How far the lyric block may be dragged, as a fraction of the window height (up is negative).
pub const OFFSET_RANGE: (f32, f32) = (-0.2, 0.5);

/// The offset after dragging by `delta_y` pixels in a window `height` pixels tall, kept in range.
pub fn offset_after_drag(offset: f32, delta_y: f32, height: f32) -> f32 {
    (offset + delta_y / height.max(1.0)).clamp(OFFSET_RANGE.0, OFFSET_RANGE.1)
}

/// What the right-click menu shows: the state it needs, read once per frame.
#[derive(Debug, Clone, Default)]
pub struct MenuState {
    pub font_name: Option<String>,
    pub fonts: Vec<PathBuf>,
    pub extra_dirs: Vec<PathBuf>,
    pub scanning: bool,
}

/// The right-click menu of the lyric screen: the lyric lock and reset, and the SoundFont submenu.
/// Returns true when a lyric setting changed and should be saved.
pub fn context_menu(
    ui: &mut egui::Ui,
    menu: &MenuState,
    locked: &mut bool,
    offset: &mut f32,
    cmds: &mut Vec<Cmd>,
) -> bool {
    let mut commit = false;
    if ui.checkbox(locked, "ล็อกตำแหน่งเนื้อเพลง").changed() {
        commit = true;
    }
    if ui.button("รีเซ็ตตำแหน่งเนื้อเพลง").clicked() {
        *offset = 0.0;
        commit = true;
        ui.close();
    }
    ui.separator();
    ui.menu_button("SoundFont", |ui| {
        match &menu.font_name {
            Some(name) => ui.label(format!("กำลังใช้: {name}")),
            None => ui.label("ยังไม่ได้เลือก"),
        };
        ui.separator();
        if menu.scanning {
            ui.label("กำลังค้นหาไฟล์…");
        } else if menu.fonts.is_empty() {
            ui.label(RichText::new("ไม่พบไฟล์ .sf2 ในโฟลเดอร์ที่ค้นหา").color(DIM));
        } else {
            ui.label(RichText::new(format!("พบ {} ไฟล์", menu.fonts.len())).color(DIM));
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for path in &menu.fonts {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let selected = menu.font_name.as_deref() == Some(name.as_str());
                    if ui.selectable_label(selected, name).on_hover_text(path.display().to_string()).clicked() {
                        cmds.push(Cmd::LoadSoundFont(path.clone()));
                        ui.close();
                    }
                }
            });
        }
        ui.separator();
        if ui.button("พิมพ์ path…").clicked() {
            cmds.push(Cmd::OpenSoundFontPath);
            ui.close();
        }
        if ui.button("เลือกไฟล์…").clicked() {
            cmds.push(Cmd::ChooseSoundFont);
            ui.close();
        }
        if ui.add_enabled(!menu.scanning, egui::Button::new("สแกนอีกครั้ง")).clicked() {
            cmds.push(Cmd::RescanSoundFonts);
            ui.close();
        }
        ui.menu_button("โฟลเดอร์ค้นหา", |ui| {
            ui.label(RichText::new("มาตรฐานของ macOS ค้นหาเสมอ").color(DIM));
            for (i, dir) in menu.extra_dirs.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(dir.display().to_string());
                    if ui.small_button("ลบ").clicked() {
                        cmds.push(Cmd::RemoveSoundFontFolder(i));
                    }
                });
            }
            if ui.button("เพิ่มโฟลเดอร์…").clicked() {
                cmds.push(Cmd::AddSoundFontFolder);
                ui.close();
            }
        });
    });
    commit
}

/// The lyric screen, like a karaoke display: the line being sung in large type, its syllables coloured as
/// they are sung, and the next line below. The type size follows the window height.
///
/// The lyric block can be dragged up and down (never sideways). Right-click gives a menu with a lock and a
/// reset. Returns true when a change should be saved (the drag ended, or the menu was used).
#[allow(clippy::too_many_arguments)]
pub fn lyrics(
    ui: &mut egui::Ui,
    lyrics: &Lyrics,
    title: &str,
    pos: f64,
    palette: Palette,
    offset: &mut f32,
    locked: &mut bool,
) -> bool {
    let height = ui.available_height();
    let big = (height * 0.12).clamp(40.0, 130.0);
    let small = big * 0.55;
    let mut commit = false;
    ui.vertical_centered(|ui| {
        ui.add_space(height * 0.04);
        let mut heading = if lyrics.title.is_empty() { title.to_string() } else { lyrics.title.clone() };
        if !lyrics.artist.is_empty() {
            heading.push_str(&format!("  ·  {}", lyrics.artist));
        }
        ui.label(RichText::new(heading).size(16.0).color(palette.dim));
        ui.add_space((height * (0.2 + *offset)).max(0.0));

        let block = ui.scope(|ui| lyric_lines(ui, lyrics, pos, palette, big, small)).response;
        // The drag area spans the full width of the lyric block, so it is easy to catch.
        let band = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), block.rect.y_range());
        let mut response = ui.interact(band, ui.scope_id().with("lyrics-drag"), Sense::drag());
        if !*locked {
            response = response.on_hover_cursor(CursorIcon::ResizeVertical);
            if response.dragged() {
                *offset = offset_after_drag(*offset, response.drag_delta().y, height);
            }
            if response.drag_stopped() {
                commit = true;
            }
        }
    });
    commit
}

/// The lines themselves: the current line in large type and the one after it below, or the empty-lyrics note.
fn lyric_lines(ui: &mut egui::Ui, lyrics: &Lyrics, pos: f64, palette: Palette, big: f32, small: f32) {
    if lyrics.lines.is_empty() {
        ui.label(RichText::new("(ไฟล์นี้ไม่มีเนื้อเพลง)").size(22.0).color(palette.dim));
        return;
    }
    // The current line is the last one that has started; the one after it is shown below.
    let cur = lyrics.lines.partition_point(|l| l.start_ms <= pos) as i64 - 1;
    if cur < 0 {
        let wait = lyrics.lines[0].start_ms - pos;
        if wait < 4000.0 {
            let dots = "●  ".repeat((wait / 1000.0) as usize + 1);
            ui.label(RichText::new(dots.trim_end()).size(small).color(Color32::from_rgb(90, 200, 240)));
        } else {
            ui.add_space(small);
        }
        line(ui, &lyrics.lines[0], pos, big, true, palette);
        if let Some(next) = lyrics.lines.get(1) {
            ui.add_space(big * 0.2);
            line(ui, next, pos, small, false, palette);
        }
    } else {
        let cur = cur as usize;
        ui.add_space(small);
        line(ui, &lyrics.lines[cur], pos, big, true, palette);
        if let Some(next) = lyrics.lines.get(cur + 1) {
            ui.add_space(big * 0.2);
            line(ui, next, pos, small, false, palette);
        }
    }
}

/// One line of lyrics: sung syllables in yellow, the rest in white (current line) or grey (the next line).
fn line(ui: &mut egui::Ui, line: &ckaraoke_core::lyrics::LyricLine, pos: f64, size: f32, active: bool, palette: Palette) {
    let mut job = LayoutJob::default();
    let font = FontId::proportional(size);
    for s in &line.sylls {
        let color = if active && s.ms <= pos {
            palette.sung
        } else if active {
            palette.active
        } else {
            palette.upcoming
        };
        job.append(&s.text, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
    }
    job.halign = eframe::egui::Align::Center;
    job.wrap.max_width = ui.available_width();
    ui.label(job);
}

/// Shown when no song is loaded.
pub fn welcome(ui: &mut egui::Ui, has_font: bool) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.28);
        ui.label(RichText::new("CKaraoke").size(40.0).strong());
        ui.add_space(12.0);
        if has_font {
            ui.label(RichText::new("เลือกเพลงจากคลังทางซ้าย แล้วกด ▶ (เล่นเลย) หรือ + (เพิ่มเข้าคิว)").size(18.0).color(DIM));
        } else {
            ui.label(RichText::new("เลือก SoundFont (.sf2) ก่อนเล่นเพลง").size(18.0).color(DIM));
        }
        ui.add_space(8.0);
        ui.label(RichText::new("หรือลากไฟล์ .sf2 / .mid / .kar / .ckr มาวางที่หน้าต่างนี้").size(14.0).color(DIM));
    });
}

/// A vertical level meter.
fn meter(ui: &mut egui::Ui, level: f32, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, Color32::from_gray(35));
    let level = level.clamp(0.0, 1.0);
    let fill = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - rect.height() * level), rect.right_bottom());
    painter.rect_filled(fill, 2.0, METER);
}

/// The transport bar: progress, the play controls, speed, transpose and volume.
pub fn transport(ui: &mut egui::Ui, snap: &Snapshot, has_song: bool, cmds: &mut Vec<Cmd>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(mmss(snap.pos)).monospace());
        let mut seek = snap.pos;
        ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(120.0);
        let slider = egui::Slider::new(&mut seek, 0.0..=snap.dur.max(1.0)).show_value(false).trailing_fill(true);
        let response = ui.add_enabled(has_song, slider);
        if response.drag_stopped() || (response.changed() && !response.dragged()) {
            cmds.push(Cmd::Seek(seek));
        }
        ui.label(RichText::new(mmss(snap.dur)).monospace());
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(has_song, egui::Button::new("|◀")).clicked() {
            cmds.push(Cmd::Prev);
        }
        if ui.add_enabled(has_song, egui::Button::new("−5 วิ")).clicked() {
            cmds.push(Cmd::SeekBy(-5000.0));
        }
        let play_label = if snap.paused || !has_song { "▶ เล่น" } else { "⏸ หยุด" };
        if ui.add_sized([72.0, 26.0], egui::Button::new(RichText::new(play_label).strong())).clicked() {
            cmds.push(Cmd::TogglePlay);
        }
        if ui.add_enabled(has_song, egui::Button::new("+5 วิ")).clicked() {
            cmds.push(Cmd::SeekBy(5000.0));
        }
        if ui.add_enabled(has_song, egui::Button::new("▶|")).clicked() {
            cmds.push(Cmd::Next);
        }
        ui.separator();

        ui.label("ความเร็ว");
        let mut speed = (snap.speed * 100.0).round() as f32;
        let r = ui.add(egui::Slider::new(&mut speed, 50.0..=200.0).step_by(5.0).suffix("%").clamping(egui::SliderClamping::Always));
        if r.changed() {
            cmds.push(Cmd::Speed(f64::from(speed) / 100.0));
        }
        if ui.small_button("100%").clicked() {
            cmds.push(Cmd::Speed(1.0));
        }
        ui.separator();

        ui.label("คีย์");
        if ui.small_button("−").clicked() {
            cmds.push(Cmd::Transpose(snap.transpose - 1));
        }
        ui.label(format!("{:+}", snap.transpose));
        if ui.small_button("+").clicked() {
            cmds.push(Cmd::Transpose(snap.transpose + 1));
        }
        ui.separator();

        ui.label("เสียง");
        let mut vol = (snap.volume * 100.0).round();
        let r = ui.add(egui::Slider::new(&mut vol, 0.0..=200.0).suffix("%").clamping(egui::SliderClamping::Always));
        if r.changed() {
            cmds.push(Cmd::Volume(vol / 100.0));
        }
    });
}

/// The mixer: one column per MIDI channel with its meter, fader, mute and solo, and the drum pieces in use.
pub fn mixer(ui: &mut egui::Ui, snap: &Snapshot, cmds: &mut Vec<Cmd>) {
    egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            for (ch, view) in snap.channels.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.set_width(56.0);
                    let title = if view.drums { "กลอง".to_string() } else { format!("CH {}", ch + 1) };
                    ui.label(RichText::new(title).strong().color(if view.used { Color32::WHITE } else { DIM }));
                    let preset: String = view.name.chars().take(10).collect();
                    ui.label(RichText::new(preset).size(10.0).color(DIM));
                    ui.horizontal(|ui| {
                        meter(ui, view.level, 12.0, 150.0);
                        let mut gain = view.gain * 100.0;
                        let fader = egui::Slider::new(&mut gain, 0.0..=200.0).vertical().show_value(false).clamping(egui::SliderClamping::Always);
                        if ui.add(fader).changed() {
                            cmds.push(Cmd::ChannelGain(ch, gain / 100.0));
                        }
                    });
                    ui.label(RichText::new(format!("{:.0}%", view.gain * 100.0)).size(10.0));
                    ui.horizontal(|ui| {
                        if ui.selectable_label(view.mute, "M").clicked() {
                            cmds.push(Cmd::ChannelMute(ch, !view.mute));
                        }
                        if ui.selectable_label(view.solo, "S").clicked() {
                            cmds.push(Cmd::ChannelSolo(ch, !view.solo));
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
    });
    ui.horizontal(|ui| {
        if ui.button("รีเซ็ตทั้งหมด").clicked() {
            cmds.push(Cmd::ResetMixer);
        }
    });
    if !snap.drums.is_empty() {
        ui.separator();
        ui.collapsing("ชิ้นกลองที่เพลงนี้ใช้", |ui| {
            if ui.button("รีเซ็ตชิ้นกลอง").clicked() {
                cmds.push(Cmd::ResetDrums);
            }
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for d in &snap.drums {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(d.mute, "M").clicked() {
                            cmds.push(Cmd::DrumMute(d.key, !d.mute));
                        }
                        ui.add_sized([170.0, 18.0], egui::Label::new(format!("{:>2}  {}", d.key, drums::piece_name(d.key))));
                        let mut gain = d.gain * 100.0;
                        if ui.add(egui::Slider::new(&mut gain, 0.0..=200.0).suffix("%").clamping(egui::SliderClamping::Always)).changed() {
                            cmds.push(Cmd::DrumGain(d.key, gain / 100.0));
                        }
                    });
                }
            });
        });
    }
}

/// The queue: the songs waiting to play, with the options that control what happens next.
/// The play queue. `selected` is the song the arrow keys are on, while the list is open for picking.
pub fn queue(ui: &mut egui::Ui, queue: &Queue, autoplay: bool, selected: Option<usize>, cmds: &mut Vec<Cmd>) {
    ui.heading(format!("คิวเพลง ({})", queue.len()));
    let mut autoplay_now = autoplay;
    if ui.checkbox(&mut autoplay_now, "เล่นต่ออัตโนมัติ").changed() {
        cmds.push(Cmd::SetAutoplay(autoplay_now));
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(!queue.is_empty(), egui::Button::new("ล้างคิว")).clicked() {
            cmds.push(Cmd::QueueClear);
        }
    });
    ui.separator();
    if queue.is_empty() {
        ui.label(RichText::new("ยังไม่มีเพลงในคิว").color(DIM));
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, entry) in queue.iter().enumerate() {
            let fill = if selected == Some(i) { SELECTED_FILL } else { egui::Color32::TRANSPARENT };
            let row = egui::Frame::default().fill(fill).inner_margin(egui::Margin::symmetric(4, 2)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button("▶").clicked() {
                        cmds.push(Cmd::QueuePlay(i));
                    }
                    if ui.small_button("✕").clicked() {
                        cmds.push(Cmd::QueueRemove(i));
                    }
                    ui.label(format!("{}. {}", i + 1, entry.title));
                });
            });
            if selected == Some(i) {
                row.response.scroll_to_me(None);
            }
        }
    });
}

/// The library: the search and the matching songs. `results` are indexes into `songs`.
#[allow(clippy::too_many_arguments)]
pub fn library(
    ui: &mut egui::Ui,
    songs: &[ckaraoke_core::library::SongInfo],
    results: &[usize],
    search: &str,
    selected: usize,
    scroll_to_selected: &mut bool,
    folder: Option<&str>,
    scanning: Option<(usize, usize)>,
    cmds: &mut Vec<Cmd>,
) {
    // The search box, as on a karaoke machine: the typed text with a cursor after it.
    egui::Frame::default()
        .fill(SEARCH_FILL)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if search.is_empty() {
                ui.label(RichText::new("พิมพ์ชื่อเพลง_").size(22.0).color(HINT));
            } else {
                ui.label(RichText::new(format!("{search}_")).size(22.0).strong().color(TITLE));
            }
        });
    ui.label(RichText::new(format!("พบ {} เพลง  ·  ↑↓ เลือก  Enter เข้าคิว  Esc ยกเลิก", results.len())).size(12.0).color(HINT));
    ui.horizontal(|ui| {
        if ui.button("เลือกโฟลเดอร์…").clicked() {
            cmds.push(Cmd::ChooseFolder);
        }
        if ui.add_enabled(folder.is_some(), egui::Button::new("สแกนใหม่")).clicked() {
            cmds.push(Cmd::Rescan);
        }
        ui.add(egui::Label::new(RichText::new(folder.unwrap_or("ยังไม่ได้เลือกโฟลเดอร์")).size(12.0).color(DIM)).truncate());
    });
    if let Some((done, total)) = scanning {
        let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
        ui.add(egui::ProgressBar::new(frac).text(if total > 0 { format!("สแกน {done}/{total}") } else { "กำลังค้นหาไฟล์…".to_string() }));
    }
    ui.add_space(4.0);

    let row_height = 36.0;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_height, results.len(), |ui, range| {
        for row in range {
            let song = &songs[results[row]];
            let is_selected = row == selected && !search.is_empty();
            let fill = if is_selected { SELECTED_FILL } else { egui::Color32::TRANSPARENT };
            let frame = egui::Frame::default().fill(fill).inner_margin(egui::Margin::symmetric(6, 2));
            let frame_response = frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 80.0).max(120.0));
                        ui.add(egui::Label::new(song_line(song)).truncate());
                    });
                    if ui.small_button("▶").on_hover_text("เล่นเลย").clicked() {
                        cmds.push(Cmd::PlayNow(entry_of(song)));
                    }
                    if ui.small_button("+").on_hover_text("เพิ่มเข้าคิว").clicked() {
                        cmds.push(Cmd::Enqueue(entry_of(song)));
                    }
                });
            });
            if is_selected && *scroll_to_selected {
                frame_response.response.scroll_to_me(None);
                *scroll_to_selected = false;
            }
        }
    });
}

/// One song on one line: `title -(key)-> artist (FORMAT)`. The key and the format are left out when unknown.
fn song_line(song: &ckaraoke_core::library::SongInfo) -> LayoutJob {
    let mut job = LayoutJob::default();
    push_text(&mut job, &song.title, TITLE);
    if song.key.is_empty() {
        push_text(&mut job, "  ->  ", KEY);
    } else {
        push_text(&mut job, &format!("  -({})->  ", song.key), KEY);
    }
    push_text(&mut job, &song.artist, ARTIST);
    let format = format_label(&song.path);
    if !format.is_empty() {
        push_text(&mut job, &format!("  ({format})"), HINT);
    }
    job
}

fn push_text(job: &mut LayoutJob, text: &str, color: egui::Color32) {
    job.append(text, 0.0, TextFormat { font_id: FontId::proportional(18.0), color, ..Default::default() });
}

/// The format of a song file as its label shows it: `CKR`, `MID`, `KAR`… A song in a pack is `pack.ckr#n`.
pub fn format_label(path: &str) -> String {
    let file = match path.rsplit_once('#') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => path,
    };
    std::path::Path::new(file).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_uppercase()).unwrap_or_default()
}

pub fn entry_of(song: &ckaraoke_core::library::SongInfo) -> Entry {
    Entry { arg: song.path.clone(), title: song.title.clone(), artist: song.artist.clone(), key: song.key.clone() }
}

/// A one-line message, red for errors.
pub fn message(ui: &mut egui::Ui, text: &str, is_error: bool) {
    let color = if is_error { ERROR } else { DIM };
    ui.label(RichText::new(text).color(color));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_label_reads_the_extension_and_skips_the_pack_number() {
        assert_eq!(format_label("/music/a b/song.kar"), "KAR");
        assert_eq!(format_label("pack.ckr#12"), "CKR");
        assert_eq!(format_label("/music/C# songs/x.mid"), "MID");
        assert_eq!(format_label("/music/noext"), "");
    }

    #[test]
    fn dragging_moves_only_by_height_and_stays_in_range() {
        // 500 px down in a 1000 px window is half the height.
        assert!((offset_after_drag(0.0, 500.0, 1000.0) - 0.5).abs() < 1e-6);
        assert!((offset_after_drag(0.0, 5000.0, 1000.0) - OFFSET_RANGE.1).abs() < 1e-6);
        assert!((offset_after_drag(0.0, -5000.0, 1000.0) - OFFSET_RANGE.0).abs() < 1e-6);
    }
}
