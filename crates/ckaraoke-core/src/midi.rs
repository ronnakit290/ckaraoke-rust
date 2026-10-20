//! Minimal Standard MIDI File reader for lyrics and timing: text and lyric meta events, tempo changes,
//! and the division. Port of the parser in `src/lyrics.cpp`. Playback will use its own event list.

/// A text (FF 01) or lyric (FF 05) meta event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEvent {
    pub tick: u64,
    /// 0x01 for text (`.kar` lyrics and `@T` headers), 0x05 for lyric.
    pub kind: u8,
    pub bytes: Vec<u8>,
}

/// A set-tempo meta event (FF 51).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TempoEvent {
    pub tick: u64,
    pub us_per_quarter: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMidi {
    pub division: u16,
    pub texts: Vec<TextEvent>,
    pub tempos: Vec<TempoEvent>,
}

fn be32(p: &[u8]) -> u32 {
    u32::from_be_bytes([p[0], p[1], p[2], p[3]])
}

/// Byte reader over one track that never reads past its end.
struct TrackReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl TrackReader<'_> {
    fn ok(&self) -> bool {
        self.pos < self.buf.len()
    }

    fn peek(&self) -> u8 {
        self.buf.get(self.pos).copied().unwrap_or(0)
    }

    fn u8(&mut self) -> u8 {
        let v = self.peek();
        if self.ok() {
            self.pos += 1;
        }
        v
    }

    /// Variable-length quantity, at most four bytes.
    fn vlq(&mut self) -> u32 {
        let mut v = 0u32;
        for _ in 0..4 {
            if !self.ok() {
                break;
            }
            let b = self.buf[self.pos];
            self.pos += 1;
            v = (v << 7) | u32::from(b & 0x7F);
            if b & 0x80 == 0 {
                break;
            }
        }
        v
    }

    fn skip(&mut self, n: usize) {
        self.pos = if self.buf.len() - self.pos < n { self.buf.len() } else { self.pos + n };
    }
}

fn parse_track(buf: &[u8], texts: &mut Vec<TextEvent>, tempos: &mut Vec<TempoEvent>) {
    let mut r = TrackReader { buf, pos: 0 };
    let mut tick: u64 = 0;
    let mut running: u8 = 0;
    while r.ok() {
        tick += u64::from(r.vlq());
        let mut status = r.peek();
        if status & 0x80 != 0 {
            r.u8();
        } else {
            status = running; // running status: reuse the previous channel status
            if status == 0 {
                return;
            }
        }

        if status == 0xFF {
            let kind = r.u8();
            let len = r.vlq() as usize;
            let start = r.pos;
            r.skip(len);
            if r.pos - start < len {
                return; // truncated
            }
            let data = &buf[start..start + len];
            if kind == 0x51 && len == 3 {
                let us = (u32::from(data[0]) << 16) | (u32::from(data[1]) << 8) | u32::from(data[2]);
                tempos.push(TempoEvent { tick, us_per_quarter: us });
            } else if kind == 0x01 || kind == 0x05 {
                texts.push(TextEvent { tick, kind, bytes: data.to_vec() });
            } else if kind == 0x2F {
                return;
            }
        } else if status == 0xF0 || status == 0xF7 {
            let n = r.vlq() as usize;
            r.skip(n);
            running = 0;
        } else {
            running = status;
            let hi = status & 0xF0;
            r.skip(if hi == 0xC0 || hi == 0xD0 { 1 } else { 2 });
        }
    }
}

/// Parses a Standard MIDI File. Returns `None` if it does not start with an `MThd` chunk.
pub fn parse(buf: &[u8]) -> Option<ParsedMidi> {
    if buf.len() < 14 || &buf[..4] != b"MThd" {
        return None;
    }
    let header_len = be32(&buf[4..8]) as usize;
    let division = u16::from_be_bytes([buf[12], buf[13]]);
    let mut out = ParsedMidi { division, texts: Vec::new(), tempos: Vec::new() };
    let mut off = 8 + header_len;
    while off + 8 <= buf.len() {
        let len = be32(&buf[off + 4..off + 8]) as usize;
        let start = off + 8;
        let end = buf.len().min(start + len);
        if &buf[off..off + 4] == b"MTrk" {
            parse_track(&buf[start..end], &mut out.texts, &mut out.tempos);
        }
        off = start + len;
    }
    Some(out)
}

/// Converts ticks to milliseconds using the tempo map (or SMPTE timing if the division says so).
pub struct TickClock {
    division: u16,
    segs: Vec<Seg>,
}

struct Seg {
    tick: u64,
    ms: f64,
    us_per_quarter: u32,
}

impl TickClock {
    pub fn new(division: u16, mut tempos: Vec<TempoEvent>) -> Self {
        tempos.sort_by_key(|t| t.tick); // stable, like the C++ stable_sort
        let mut clock = TickClock {
            division,
            segs: vec![Seg { tick: 0, ms: 0.0, us_per_quarter: 500_000 }], // default 120 BPM
        };
        for t in tempos {
            let last = clock.segs.last().expect("the first segment is always there");
            let ms = last.ms + clock.ticks_to_ms(t.tick - last.tick, last.us_per_quarter);
            if t.tick == last.tick {
                clock.segs.last_mut().expect("non-empty").us_per_quarter = t.us_per_quarter;
            } else {
                clock.segs.push(Seg { tick: t.tick, ms, us_per_quarter: t.us_per_quarter });
            }
        }
        clock
    }

    fn ticks_to_ms(&self, ticks: u64, us_per_quarter: u32) -> f64 {
        if self.division != 0 {
            ticks as f64 * (f64::from(us_per_quarter) / 1000.0) / f64::from(self.division)
        } else {
            0.0
        }
    }

    pub fn ms(&self, tick: u64) -> f64 {
        if self.division & 0x8000 != 0 {
            // SMPTE: the high byte is minus the frames per second, the low byte the ticks per frame.
            let fps = -i32::from((self.division >> 8) as u8 as i8);
            let tpf = i32::from(self.division & 0xFF);
            return if fps > 0 && tpf > 0 { tick as f64 * 1000.0 / f64::from(fps * tpf) } else { 0.0 };
        }
        let idx = self.segs.partition_point(|s| s.tick <= tick);
        let s = &self.segs[idx - 1];
        s.ms + self.ticks_to_ms(tick - s.tick, s.us_per_quarter)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A format-0 file with one track: `(delta, event bytes)` pairs, ending with end-of-track.
    pub(crate) fn smf(division: u16, events: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut track = Vec::new();
        for (delta, ev) in events {
            push_vlq(&mut track, *delta);
            track.extend_from_slice(ev);
        }
        push_vlq(&mut track, 0);
        track.extend_from_slice(&[0xFF, 0x2F, 0x00]);
        let mut out = b"MThd".to_vec();
        out.extend_from_slice(&6u32.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&division.to_be_bytes());
        out.extend_from_slice(b"MTrk");
        out.extend_from_slice(&(track.len() as u32).to_be_bytes());
        out.extend_from_slice(&track);
        out
    }

    fn push_vlq(b: &mut Vec<u8>, mut v: u32) {
        let mut bytes = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            bytes.push((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
        bytes.reverse();
        b.extend_from_slice(&bytes);
    }

    fn meta(kind: u8, data: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, kind, data.len() as u8];
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn tempo_changes_move_later_lyrics() {
        // 480 ticks per quarter. Tempo doubles at tick 480 (250 000 us = 240 BPM).
        let file = smf(
            480,
            &[
                (0, meta(0x05, b"A")),
                (480, meta(0x51, &[0x03, 0xD0, 0x90])), // 250 000 us
                (480, meta(0x05, b"B")),
            ],
        );
        let parsed = parse(&file).unwrap();
        assert_eq!(parsed.texts.len(), 2);
        let clock = TickClock::new(parsed.division, parsed.tempos);
        assert_eq!(clock.ms(0), 0.0);
        assert_eq!(clock.ms(480), 500.0);
        assert_eq!(clock.ms(960), 750.0);
    }

    #[test]
    fn running_status_and_channel_events_are_skipped() {
        // Note-on, then a second note-on under running status, then a lyric.
        let file = smf(
            96,
            &[(0, vec![0x90, 60, 100]), (10, vec![62, 100]), (0, meta(0x05, b"x"))],
        );
        let parsed = parse(&file).unwrap();
        assert_eq!(parsed.texts, vec![TextEvent { tick: 10, kind: 0x05, bytes: b"x".to_vec() }]);
    }

    #[test]
    fn rejects_non_midi_data() {
        assert!(parse(b"RIFF....WAVE....").is_none());
    }
}
