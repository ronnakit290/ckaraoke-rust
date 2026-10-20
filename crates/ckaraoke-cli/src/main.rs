//! `ckaraoke`: command-line tool for the Rust port. This milestone covers the tools that need no
//! audio device: pack listing, packing and unpacking, lyric inspection, and library scan and search.
//! Playback is not ported yet (see rust/README.md).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use ckaraoke_core::library::{self, ScanProgress, SongInfo, SongLibrary};
use ckaraoke_core::lyrics::{sung_range, Lyrics};
use ckaraoke_core::player::Player;

mod play;
use ckaraoke_core::pack::{self, pack_song_path, split_pack_song_path, PackReader};
use ckaraoke_core::song::load_song;
use ckaraoke_core::{Error, Result};

/// Source files `--pack` takes from a folder.
const SONG_SOURCE_EXTS: &[&str] = &["mid", "midi", "kar"];
/// Songs shown by `--search`.
const SEARCH_LIMIT: usize = 50;

const USAGE: &str = "\
ใช้งาน: ckaraoke <คำสั่ง> [ไฟล์ ...]

เล่นเพลง:
  <soundfont.sf2> <เพลง>                 เล่นเพลงสด (Space หยุด/เล่น, ← → เลื่อน, ↑ ↓ ระดับเสียง, r เริ่มใหม่, q ออก)
  <soundfont.sf2> <เพลง> --wav <out.wav>  เรนเดอร์เป็นไฟล์ WAV (44.1 kHz สเตอริโอ)
  เพลง = .mid / .kar หรือ pack.ckr#หมายเลข

คำสั่ง:
  --list <pack.ckr>...              แสดงรายการเพลงในไฟล์ .ckr
  --pack <out.ckr> <ไฟล์|โฟลเดอร์>...  แพ็กไฟล์ .mid / .kar หรือโฟลเดอร์เป็นไฟล์ .ckr ไฟล์เดียว
  --unpack <โฟลเดอร์> <pack.ckr>...   แตกไฟล์ .ckr ลงโฟลเดอร์ (Song / Lyrics / Cursor)
  --lyrics <เพลง>                    แสดงเนื้อร้องพร้อมเวลา (เพลง = .mid / .kar หรือ pack.ckr#หมายเลข)
  --scan <โฟลเดอร์> [--index <ไฟล์>]   สแกนคลังเพลง (ถ้าระบุ --index จะบันทึกผลไว้)
  --search <คำค้น> <โฟลเดอร์> [--index <ไฟล์>]  ค้นหาเพลงในคลัง

รหัสผ่านของไฟล์ .ckr รับจากตัวแปร CKARAOKE_KEY เพื่อไม่ให้ค้างในประวัติคำสั่ง
";

fn pack_key() -> String {
    std::env::var("CKARAOKE_KEY").unwrap_or_default()
}

fn has_ext(path: &str, exts: &[&str]) -> bool {
    Path::new(path).extension().is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Lists the song-source files in a folder, recursively, in sorted order.
fn song_sources_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            song_sources_under(&path, out);
        } else if path.is_file() {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if SONG_SOURCE_EXTS.contains(&ext.as_str()) {
                out.push(path);
            }
        }
    }
}

fn cmd_list(files: &[String]) -> Result<bool> {
    let key = pack_key();
    if key.is_empty() {
        return Err(Error::new("ตั้งคีย์ .ckr ที่ตัวแปรสภาพแวดล้อม CKARAOKE_KEY ก่อน"));
    }
    let mut ok = true;
    for file in files {
        let reader = match PackReader::open(file, &key) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{file}: {e}");
                ok = false;
                continue;
            }
        };
        println!("{} ({} เพลง)", file_name(file), reader.entries().len());
        for (i, e) in reader.entries().iter().enumerate() {
            let title = if e.title.is_empty() { "(ไม่มีชื่อ)" } else { &e.title };
            let mut line = format!("  {}. {title}", i + 1);
            if !e.artist.is_empty() {
                line.push_str(&format!(" — {}", e.artist));
            }
            if !e.key.is_empty() {
                line.push_str(&format!(" ({})", e.key));
            }
            println!("{line}");
        }
    }
    Ok(ok)
}

fn cmd_pack(out: &str, inputs: &[String]) -> Result<bool> {
    let key = pack_key();
    let mut sources: Vec<PathBuf> = Vec::new();
    for input in inputs {
        let path = PathBuf::from(input);
        if path.is_dir() {
            let mut found = Vec::new();
            song_sources_under(&path, &mut found);
            found.sort();
            sources.extend(found);
        } else {
            sources.push(path);
        }
    }
    let result = pack::pack_sources(&sources, out, &key, |done| {
        if done % 1000 == 0 {
            eprint!("\rแพ็กแล้ว {done} ไฟล์");
        }
    })?;
    eprintln!();
    for s in &result.skipped {
        eprintln!("ข้าม {s}");
    }
    println!("แพ็ก {} เพลง → {out}", result.packed);
    Ok(result.skipped.is_empty())
}

fn cmd_unpack(folder: &str, packs: &[String]) -> Result<bool> {
    let key = pack_key();
    let mut ok = true;
    for file in packs {
        let stem = Path::new(file).file_stem().map(|s| s.to_os_string()).unwrap_or_default();
        let dest = Path::new(folder).join(stem);
        match pack::unpack_pack(file, &key, &dest) {
            Ok(res) => {
                println!("{file} → {} ({} เพลง)", dest.display(), res.written);
                for f in &res.failures {
                    eprintln!("  {f}");
                    ok = false;
                }
            }
            Err(e) => {
                eprintln!("{file}: {e}");
                ok = false;
            }
        }
    }
    Ok(ok)
}

/// Prints lyrics in a fixed, plain format (one line per lyric line, one per syllable). Other tools in
/// this repo use the same format to compare the Rust and C++ readers.
fn print_lyrics(lyrics: &Lyrics) {
    println!("title: {}", lyrics.title);
    println!("artist: {}", lyrics.artist);
    println!("key: {}", lyrics.key);
    let sung = sung_range(lyrics);
    println!("sung {:.3} {:.3}", sung.start_ms, sung.end_ms);
    for line in &lyrics.lines {
        println!("line {:.3}", line.start_ms);
        for s in &line.sylls {
            println!("  syll {:.3} {}", s.ms, s.text);
        }
    }
}

/// `pack.ckr#N` counts songs from 1, as `--list` shows them; the core addresses them from 0.
fn song_arg(arg: &str) -> Result<String> {
    match split_pack_song_path(arg) {
        Some((_, 0)) => Err(Error::new("หมายเลขเพลงในไฟล์ .ckr เริ่มที่ 1 (ดูรายการด้วย --list)")),
        Some((file, n)) => Ok(pack_song_path(&file, n - 1)),
        None => Ok(arg.to_string()),
    }
}

fn cmd_lyrics(song: &str) -> Result<bool> {
    let loaded = load_song(&song_arg(song)?, &pack_key())?;
    print_lyrics(&loaded.lyrics);
    Ok(true)
}

fn print_songs(songs: &[SongInfo]) {
    for s in songs {
        let mut line = s.title.clone();
        if !s.artist.is_empty() {
            line.push_str(&format!(" — {}", s.artist));
        }
        if !s.key.is_empty() {
            line.push_str(&format!(" ({})", s.key));
        }
        println!("{line}");
    }
}

/// Catalogue of a folder: from the index when it exists, otherwise a fresh scan (saved to the index if asked).
fn catalogue(folder: &str, index: Option<&str>) -> Result<Vec<SongInfo>> {
    if let Some(index) = index {
        if let Some((root, songs)) = library::load_index(Path::new(index)) {
            if root == folder {
                return Ok(songs);
            }
        }
    }
    let started = Instant::now();
    let progress = ScanProgress::default();
    let songs = library::scan_library(Path::new(folder), &progress);
    eprintln!("สแกน {} ไฟล์ ใช้เวลา {:.1} วินาที", progress.total.load(std::sync::atomic::Ordering::Relaxed), started.elapsed().as_secs_f64());
    if let Some(index) = index {
        library::save_index(Path::new(index), folder, &songs)?;
    }
    Ok(songs)
}

fn cmd_scan(folder: &str, index: Option<&str>) -> Result<bool> {
    let songs = catalogue(folder, index)?;
    println!("พบ {} เพลง", songs.len());
    print_songs(&songs);
    Ok(true)
}

fn cmd_search(query: &str, folder: &str, index: Option<&str>) -> Result<bool> {
    let mut lib = SongLibrary::default();
    lib.set(catalogue(folder, index)?);
    let hits = lib.search(query);
    println!("พบ {} เพลง", hits.len());
    let shown: Vec<SongInfo> = hits.iter().take(SEARCH_LIMIT).map(|&i| lib.songs()[i].clone()).collect();
    print_songs(&shown);
    std::io::stdout().flush().ok();
    Ok(true)
}


const SAMPLE_RATE: i32 = 44100;

fn mmss(ms: f64) -> String {
    let s = (ms / 1000.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Renders a song to a 16-bit stereo WAV file, the way the C++ app's `--wav` does.
fn cmd_wav(sound_font: &str, song: &str, out: &str) -> Result<bool> {
    let font = std::fs::read(sound_font).map_err(|_| Error::new(format!("เปิดไฟล์ไม่ได้: {sound_font}")))?;
    let mut player = Player::new();
    player
        .load_sound_font(0, &font, SAMPLE_RATE)
        .map_err(|e| Error::new(format!("โหลด SoundFont ไม่สำเร็จ: {sound_font} ({e})")))?;
    println!("SoundFont: {} ({} presets)", file_name(sound_font), player.preset_count());
    let loaded = load_song(&song_arg(song)?, &pack_key())?;
    player.load_midi(&loaded.midi).map_err(Error::new)?;

    let mut data: Vec<u8> = Vec::new();
    let mut buf = vec![0.0f32; 1024 * 2];
    while !player.finished() {
        player.render(&mut buf);
        for &s in &buf {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i32;
            data.extend_from_slice(&(v as i16).to_le_bytes());
        }
    }
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
    wav.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE as u32 * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(out, &wav).map_err(|_| Error::new(format!("เขียนไฟล์ไม่ได้: {out}")))?;
    let seconds = data.len() as f64 / 4.0 / f64::from(SAMPLE_RATE) * 1000.0;
    println!("บันทึก {out} ({})", mmss(seconds));
    Ok(true)
}

fn run(args: &[String]) -> Result<bool> {
    // Flags and their values; everything else is a positional argument.
    let mut index: Option<String> = None;
    let mut wav: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--index" {
            index = Some(args.get(i + 1).cloned().ok_or_else(|| Error::new("--index ต้องมีชื่อไฟล์"))?);
            i += 2;
        } else if args[i] == "--wav" {
            wav = Some(args.get(i + 1).cloned().ok_or_else(|| Error::new("--wav ต้องมีชื่อไฟล์"))?);
            i += 2;
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }
    if let Some(out) = wav {
        // ckaraoke <soundfont.sf2> <song> --wav <out.wav>
        let font = positional.iter().find(|p| has_ext(p, &["sf2", "sfo"]));
        let song = positional.iter().find(|p| !has_ext(p, &["sf2", "sfo"]));
        return match (font, song) {
            (Some(font), Some(song)) => cmd_wav(font, song, &out),
            _ => Err(Error::new("--wav ต้องระบุ SoundFont (.sf2) และเพลง")),
        };
    }
    // ckaraoke <soundfont.sf2> <song>: play it live.
    if !positional.iter().any(|p| p.starts_with("--")) {
        if let Some(font) = positional.iter().find(|p| has_ext(p, &["sf2", "sfo"])) {
            let song = positional
                .iter()
                .find(|p| !has_ext(p, &["sf2", "sfo"]))
                .ok_or_else(|| Error::new("ระบุเพลง (.mid / .kar หรือ pack.ckr#หมายเลข)"))?;
            return play::play(font, &pack_key(), &song_arg(song)?);
        }
    }
    let command = positional.first().cloned().unwrap_or_default();
    let rest = &positional[positional.len().min(1)..];
    let need = |n: usize| -> Result<()> {
        if rest.len() < n {
            Err(Error::new("ไม่ครบอาร์กิวเมนต์ (ดู ckaraoke --help)"))
        } else {
            Ok(())
        }
    };
    match command.as_str() {
        "--list" => {
            need(1)?;
            cmd_list(rest)
        }
        "--pack" => {
            need(2)?;
            cmd_pack(&rest[0], &rest[1..])
        }
        "--unpack" => {
            need(2)?;
            cmd_unpack(&rest[0], &rest[1..])
        }
        "--lyrics" => {
            need(1)?;
            cmd_lyrics(&rest[0])
        }
        "--scan" => {
            need(1)?;
            cmd_scan(&rest[0], index.as_deref())
        }
        "--search" => {
            need(2)?;
            cmd_search(&rest[0], &rest[1], index.as_deref())
        }
        _ => Err(Error::new("ไม่รู้จักคำสั่ง (ดู ckaraoke --help)")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
