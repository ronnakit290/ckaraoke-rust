//! Karaoke lyrics: timed syllables and lines, from a MIDI file's text events or from NCN data (the
//! `.lyr` text with its `.cur` timing table). Port of the lyric code in `src/lyrics.cpp`.

use crate::midi::{self, TextEvent, TickClock};
use crate::text::{contains_bytes, decode, decode_byte, display_width, is_valid_utf8, strip_key_suffix, Encoding};

/// One sung piece of text and when it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Syllable {
    pub ms: f64,
    pub text: String,
}

/// One line of lyrics. `start_ms` is the start of its first syllable.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    pub start_ms: f64,
    pub sylls: Vec<Syllable>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lyrics {
    /// From `@T` in `.kar` files, or the first line of an NCN `.lyr`.
    pub title: String,
    /// The second `@T` entry, or the second line of an NCN `.lyr`.
    pub artist: String,
    /// Musical key, NCN songs only.
    pub key: String,
    pub lines: Vec<LyricLine>,
}

/// The part of a song that is actually sung, in ms. Karaoke files pad their lyrics with credits and
/// "...music..." markers over the intro and outro; this skips those.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SungRange {
    pub start_ms: f64,
    pub end_ms: f64,
}

/// Longest line before a wrap, in display columns.
const MAX_LINE_WIDTH: usize = 48;

fn is_line_break(b: u8) -> bool {
    matches!(b, b'\\' | b'/' | b'\r' | b'\n')
}

/// Thai above/below vowels and tone marks: they belong to the previous glyph.
fn is_thai_combining(tis: u8) -> bool {
    tis == 0xD1 || (0xD4..=0xDA).contains(&tis) || (0xE7..=0xEE).contains(&tis)
}

/// Splits on `\n`, dropping `\r`. A trailing line without a newline is kept only if it is not empty.
fn split_lines(s: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut cur = Vec::new();
    for &c in s {
        if c == b'\n' {
            lines.push(std::mem::take(&mut cur));
        } else if c != b'\r' {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// A credit or instrumental marker rather than words to sing, e.g. "....ดนตรี....", "6...7...8>>>",
/// "Nick KARAOKE....", "*****..จบ..*****".
fn is_marker_line(line: &LyricLine) -> bool {
    let text: Vec<u8> = line.sylls.iter().flat_map(|s| s.text.as_bytes().iter().copied()).collect();
    // Keep ASCII letters (lower-cased) and every non-ASCII byte; drop ASCII digits, punctuation and spaces.
    let letters: Vec<u8> = text
        .iter()
        .filter(|&&c| c >= 0x80 || c.is_ascii_alphabetic())
        .map(|&c| c.to_ascii_lowercase())
        .collect();
    if letters.is_empty() {
        return true;
    }
    const WORDS: [&str; 16] = [
        "ดนตรี", "music", "intro", "solo", "โซโล", "karaoke", "คาราโอเกะ", "midi", "grammy", "studio",
        "professional", "ศิลปิน", "คำร้อง", "ทำนอง", "เรียบเรียง", "เพลง:",
    ];
    for word in WORDS {
        // "เพลง:" keeps its colon, so it is matched against the raw text.
        let hay = if word.ends_with(':') { &text } else { &letters };
        if contains_bytes(hay, word.as_bytes()) {
            return true;
        }
    }
    let chars = letters.iter().filter(|&&c| c & 0xC0 != 0x80).count();
    let key_line = letters.starts_with(b"key") && chars <= 6; // "Key : Dm"
    let end_line = chars <= 8 && (contains_bytes(&letters, "จบ".as_bytes()) || letters == b"end"); // "..จบ..", "จบครับ.."
    key_line || end_line
}

/// Extracts lyrics from a MIDI file: FF 05 lyric events, or FF 01 text events for `.kar` files ('@'
/// lines are metadata). Whichever kind carries more syllables is used.
pub fn lyrics_from_midi(buf: &[u8]) -> Lyrics {
    let Some(parsed) = midi::parse(buf) else {
        return Lyrics::default();
    };
    let clock = TickClock::new(parsed.division, parsed.tempos);
    let all_bytes: Vec<u8> = parsed.texts.iter().flat_map(|e| e.bytes.iter().copied()).collect();
    let enc = if is_valid_utf8(&all_bytes) {
        Encoding::Utf8
    } else {
        let high = all_bytes.iter().filter(|&&c| c >= 0x80).count();
        let thai = all_bytes.iter().filter(|&&c| (0xA1..=0xFB).contains(&c)).count();
        if thai * 10 >= high * 8 {
            Encoding::Tis620
        } else {
            Encoding::Latin1
        }
    };

    let mut result = Lyrics::default();
    let mut lyric: Vec<&TextEvent> = Vec::new();
    let mut text: Vec<&TextEvent> = Vec::new();
    for e in &parsed.texts {
        if e.kind == 0x05 {
            lyric.push(e);
        } else if e.bytes.first() == Some(&b'@') {
            if e.bytes.len() > 2 && e.bytes[1] == b'T' {
                let v = decode(&e.bytes[2..], enc);
                if result.title.is_empty() {
                    result.title = v;
                } else if result.artist.is_empty() {
                    result.artist = v;
                }
            }
        } else {
            text.push(e);
        }
    }
    let mut chosen = if lyric.len() >= text.len() { lyric } else { text };
    chosen.sort_by_key(|e| e.tick);

    let mut break_pending = true;
    let mut line_width = 0usize;
    for e in chosen {
        let mut t = decode(&e.bytes, enc);
        let mut break_before = break_pending;
        while t.as_bytes().first().is_some_and(|&b| is_line_break(b)) {
            break_before = true;
            t.remove(0);
        }
        break_pending = false;
        while matches!(t.as_bytes().last(), Some(&(b'\r' | b'\n'))) {
            break_pending = true;
            t.pop();
        }
        if t.is_empty() {
            continue;
        }

        let w = display_width(&t);
        if !break_before && line_width + w > MAX_LINE_WIDTH {
            break_before = true;
        }
        let ms = clock.ms(e.tick);
        if break_before || result.lines.is_empty() {
            // Drop the leading space a wrapped word would carry.
            if t.len() > 1 && t.as_bytes()[0] == b' ' {
                t.remove(0);
            }
            result.lines.push(LyricLine { start_ms: ms, sylls: Vec::new() });
            line_width = 0;
        }
        result.lines.last_mut().expect("a line was just ensured").sylls.push(Syllable { ms, text: t });
        line_width += w;
    }
    result
}

/// Builds lyrics from NCN karaoke data: the `.lyr` text (TIS-620 or UTF-8) and the `.cur` table, which
/// holds one little-endian time per lyric byte in MIDI clocks (24 per quarter note).
pub fn lyrics_from_ncn(midi_bytes: &[u8], lyr: &[u8], cur: &[u8]) -> Lyrics {
    let Some(parsed) = midi::parse(midi_bytes) else {
        return Lyrics::default();
    };
    if parsed.division & 0x8000 != 0 {
        return Lyrics::default();
    }
    let division = parsed.division;
    let clock = TickClock::new(division, parsed.tempos);

    // .lyr: title, artist, key, blank, then one lyric line per text line.
    let raw = split_lines(lyr);
    let enc = if is_valid_utf8(lyr) { Encoding::Utf8 } else { Encoding::Tis620 };
    let mut result = Lyrics::default();
    if let Some(l) = raw.first() {
        result.title = decode(l, enc);
    }
    if let Some(l) = raw.get(1) {
        result.artist = decode(l, enc);
    }
    if let Some(l) = raw.get(2) {
        result.key = decode(l, enc);
    }
    result.title = strip_key_suffix(&result.title, &result.key);
    let lines: Vec<&[u8]> = raw.iter().skip(4).filter(|l| !l.is_empty()).map(Vec::as_slice).collect();

    // The table starts with one entry before the first line and has one entry after each line. Entries
    // are 16-bit in most files and 32-bit in some; pick whichever count matches the text.
    let expected = 1 + lines.iter().map(|l| l.len() + 1).sum::<usize>();
    let diff = |n: usize| n.abs_diff(expected);
    let stride = if diff(cur.len() / 4) < diff(cur.len() / 2) { 4 } else { 2 };
    let count = cur.len() / stride;
    let entry = |i: usize| -> i64 {
        if i >= count {
            return -1;
        }
        let v = u32::from(cur[i * stride]) | (u32::from(cur[i * stride + 1]) << 8);
        if v == 0xFFFF {
            -1
        } else {
            i64::from(v)
        }
    };
    let ticks_per_clock = f64::from(division) / 24.0;

    let mut idx = 1usize; // entry 0 precedes the first line
    let mut last_ms = 0.0f64;
    for line in lines {
        let mut sylls: Vec<(f64, Vec<u8>)> = Vec::new();
        for &c in line {
            let clk = entry(idx);
            let ms = if clk < 0 {
                last_ms
            } else {
                last_ms.max(clock.ms((clk as f64 * ticks_per_clock) as u64))
            };
            last_ms = ms;
            let ch = decode_byte(c, enc);
            // Combining marks, and characters sharing a timestamp, stay in one syllable so the highlight
            // never splits a glyph cluster. A UTF-8 continuation byte also stays with its lead byte: the C++
            // build splits such a character into invalid fragments when its bytes have different times.
            let continues_char = enc == Encoding::Utf8 && (0x80..=0xBF).contains(&c);
            let attach = sylls.last().is_some_and(|last| is_thai_combining(c) || last.0 == ms || continues_char);
            if attach {
                sylls.last_mut().expect("attach needs a syllable").1.extend(ch);
            } else {
                sylls.push((ms, ch));
            }
            idx += 1;
        }
        idx += 1; // the line terminator's entry
        if sylls.is_empty() {
            continue;
        }
        let start_ms = sylls[0].0;
        let sylls = sylls
            .into_iter()
            .map(|(ms, bytes)| Syllable { ms, text: String::from_utf8_lossy(&bytes).into_owned() })
            .collect();
        result.lines.push(LyricLine { start_ms, sylls });
    }
    result
}

/// The sung range of a song: from the first real line to the last one, skipping intro and outro credits.
pub fn sung_range(lyrics: &Lyrics) -> SungRange {
    let lines = &lyrics.lines;
    if lines.is_empty() {
        return SungRange::default();
    }
    let n = lines.len() as i64;
    let total = lines[lines.len() - 1].start_ms;

    // Everything up to the last marker near the start is intro (this also drops unrecognised credit
    // lines that come before a "...music..." marker).
    let mut first: i64 = 0;
    let mut i: i64 = 0;
    while i < n && i < 14 && lines[i as usize].start_ms < 60000.0 {
        if is_marker_line(&lines[i as usize]) {
            first = i + 1;
        }
        i += 1;
    }
    if first >= n {
        first = 0;
    }
    // Likewise everything from the first marker near the end is outro.
    let mut last = n - 1;
    let mut i = n - 1;
    while i > first && i >= n - 8 && lines[i as usize].start_ms > total - 60000.0 {
        let line = &lines[i as usize];
        if is_marker_line(line) || line.sylls.is_empty() {
            last = i - 1;
        }
        i -= 1;
    }

    let end_line = &lines[last as usize];
    let mut range = SungRange { start_ms: lines[first as usize].start_ms, end_ms: 0.0 };
    for s in &end_line.sylls {
        range.end_ms = range.end_ms.max(s.ms);
    }
    range.end_ms = range.end_ms.max(end_line.start_ms);
    range
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::tests::smf;

    fn lyric_event(text: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0x05, text.len() as u8];
        v.extend_from_slice(text);
        v
    }

    #[test]
    fn midi_lyrics_become_lines_with_timed_syllables() {
        // Division 96 at the default 120 BPM: 96 ticks is 500 ms. A leading '\' starts a new line.
        let file = smf(
            96,
            &[
                (0, lyric_event(b"Hel")),
                (96, lyric_event(b"lo")),
                (96, lyric_event(b"\\World")),
            ],
        );
        let lyrics = lyrics_from_midi(&file);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].sylls[0].text, "Hel");
        assert_eq!(lyrics.lines[0].sylls[1].text, "lo");
        assert_eq!(lyrics.lines[0].sylls[1].ms, 500.0);
        assert_eq!(lyrics.lines[1].start_ms, 1000.0);
        assert_eq!(lyrics.lines[1].sylls[0].text, "World");
    }

    #[test]
    fn ncn_timing_table_is_read_as_16_bit_clock_values() {
        // Header lines, a blank line, then one lyric line "ab". The table has one entry before the
        // line, one per byte, and one after it (0xFFFF means "no time").
        let file = smf(96, &[]);
        let lyr = "เพลง\nศิลปิน\nEm\n\nab\n".as_bytes();
        let clocks: [u16; 4] = [0, 24, 48, 0xFFFF];
        let cur: Vec<u8> = clocks.iter().flat_map(|c| c.to_le_bytes()).collect();
        let lyrics = lyrics_from_ncn(&file, lyr, &cur);
        assert_eq!(lyrics.title, "เพลง");
        assert_eq!(lyrics.artist, "ศิลปิน");
        assert_eq!(lyrics.key, "Em");
        assert_eq!(lyrics.lines.len(), 1);
        // A clock is division / 24 = 4 ticks, so entry 24 is tick 96 = 500 ms at 120 BPM, and entry 48 is 1000 ms.
        let times: Vec<f64> = lyrics.lines[0].sylls.iter().map(|s| s.ms).collect();
        assert_eq!(times, vec![500.0, 1000.0]);
    }

    #[test]
    fn utf8_continuation_bytes_stay_with_their_character() {
        let file = smf(96, &[]);
        // One UTF-8 line "ก" (E0 B8 81), each byte with its own time: the three bytes stay one syllable.
        let lyr = "\u{0E01}\n".as_bytes();
        let mut lyr_file = b"t\na\nC\n\n".to_vec();
        lyr_file.extend_from_slice(lyr);
        let clocks: [u16; 5] = [0, 24, 48, 72, 96];
        let cur: Vec<u8> = clocks.iter().flat_map(|c| c.to_le_bytes()).collect();
        let lyrics = lyrics_from_ncn(&file, &lyr_file, &cur);
        assert_eq!(lyrics.lines[0].sylls.len(), 1);
        assert_eq!(lyrics.lines[0].sylls[0].text, "\u{0E01}");
    }

    #[test]
    fn intro_credits_and_outro_are_not_sung() {
        let line = |ms: f64, text: &str| LyricLine {
            start_ms: ms,
            sylls: vec![Syllable { ms, text: text.to_string() }],
        };
        // The intro marker at 0 s is skipped; the outro "จบ" at 61 s is skipped.
        let lyrics = Lyrics {
            lines: vec![
                line(0.0, "..ดนตรี.."),
                line(9000.0, "ท่อนแรก"),
                line(30000.0, "ท่อนสุดท้าย"),
                line(61000.0, "..จบ.."),
            ],
            ..Default::default()
        };
        let range = sung_range(&lyrics);
        assert_eq!(range.start_ms, 9000.0);
        assert_eq!(range.end_ms, 30000.0);
    }
}
