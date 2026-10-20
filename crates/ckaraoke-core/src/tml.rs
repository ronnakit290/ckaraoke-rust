//! MIDI message list for playback. Port of `third_party/tml.h` (TinyMidiLoader v0.7).
//!
//! The list holds the channel messages, tempo changes and end-of-track events of a file, in the order
//! they sound, each with its time in whole milliseconds. Times and ordering follow the C loader step
//! by step, including its tick-to-millisecond rounding and the way it moves the first message to the
//! front of the list, so playback matches the C build.

pub const NOTE_OFF: u8 = 0x80;
pub const NOTE_ON: u8 = 0x90;
pub const CONTROL_CHANGE: u8 = 0xB0;
pub const PROGRAM_CHANGE: u8 = 0xC0;
pub const PITCH_BEND: u8 = 0xE0;
pub const SET_TEMPO: u8 = 0x51;
pub const END_OF_TRACK: u8 = 0x2F;

/// One message of a loaded file, in playing order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Message {
    /// When the message sounds, in milliseconds from the start.
    pub time: u32,
    /// Channel message type (NOTE_ON and so on), SET_TEMPO, or END_OF_TRACK.
    pub kind: u8,
    pub channel: u8,
    /// Key for note messages, controller for CONTROL_CHANGE, program for PROGRAM_CHANGE, pressure for CHANNEL_PRESSURE.
    pub key: u8,
    /// Velocity for note messages, value for CONTROL_CHANGE. Zero for program and channel pressure.
    pub value: u8,
    /// Pitch bend, 0..16383, for PITCH_BEND.
    pub pitch_bend: u16,
    /// Microseconds per quarter note for SET_TEMPO, big-endian.
    pub tempo: [u8; 3],
}

/// A message while loading. `next` links it to the message that follows in time, as the C list does.
#[derive(Clone, Copy, Default)]
struct Stored {
    msg: Message,
    next: Option<usize>,
}

#[derive(Clone, Copy, Default)]
struct Track {
    idx: usize,
    end: usize,
    ticks: u32,
}

/// Byte reader for one track. Errors come back as -1, as in the C parser.
struct Parser<'a> {
    buf: &'a [u8],
    pos: usize,
    last_status: u8,
}

impl Parser<'_> {
    fn read_byte(&mut self) -> i32 {
        match self.buf.get(self.pos) {
            Some(&b) => {
                self.pos += 1;
                i32::from(b)
            }
            None => -1,
        }
    }

    /// A variable-length quantity of at most four bytes (tml_readvariablelength).
    fn read_vlq(&mut self) -> i32 {
        let mut res: u32 = 0;
        for _ in 0..4 {
            let Some(&c) = self.buf.get(self.pos) else { return -1 };
            self.pos += 1;
            if c & 0x80 != 0 {
                res = (res | u32::from(c & 0x7F)) << 7;
            } else {
                return (res | u32::from(c)) as i32;
            }
        }
        -1
    }

    /// Parses one event. Returns its type (0 for events that are not kept), or -1 on bad data (tml_parsemessage).
    fn parse_message(&mut self, out: &mut Vec<Stored>) -> i32 {
        let mut delta = self.read_vlq();
        let status_byte = self.read_byte();
        if (delta as u32) & 0xFFF0_0000 != 0 {
            delta = 0; // throw away delays that are insanely high for malformed files
        }
        if status_byte < 0 {
            return -1;
        }
        let mut status = status_byte as u8;
        if status & 0x80 == 0 {
            // Running status: the same status as the last message.
            if self.last_status & 0x80 == 0 {
                return -1;
            }
            self.pos -= 1;
            status = self.last_status;
        } else {
            self.last_status = status;
        }

        let mut evt = Stored::default();
        if status == 0xF0 || status == 0xF7 {
            // System exclusive: skipped.
            let n = self.read_vlq();
            let new_pos = self.pos as i64 + i64::from(n);
            if new_pos > self.buf.len() as i64 {
                self.pos = self.buf.len();
                return -1;
            }
            self.pos = new_pos.max(0) as usize;
        } else if status == 0xFF {
            let meta_type = self.read_byte();
            let buflen = self.read_vlq();
            let data_pos = self.pos;
            if meta_type < 0 {
                return -1;
            }
            if buflen > 0 {
                self.pos += buflen as usize;
                if self.pos > self.buf.len() {
                    self.pos = self.buf.len();
                    return -1;
                }
            }
            match meta_type {
                0x2F => {
                    if buflen != 0 {
                        return -1;
                    }
                    if delta == 0 {
                        return i32::from(END_OF_TRACK); // not kept
                    }
                    evt.msg.kind = END_OF_TRACK;
                }
                0x51 => {
                    if buflen != 3 {
                        return -1;
                    }
                    evt.msg.kind = SET_TEMPO;
                    evt.msg.tempo.copy_from_slice(&self.buf[data_pos..data_pos + 3]);
                }
                _ => evt.msg.kind = 0,
            }
        } else {
            let key_byte = self.read_byte();
            if key_byte < 0 {
                return -1;
            }
            let key = (key_byte & 0x7F) as u8;
            evt.msg.key = key;
            evt.msg.channel = status & 0x0F;
            let kind = status & 0xF0;
            evt.msg.kind = kind;
            match kind {
                NOTE_OFF | NOTE_ON | 0xA0 | CONTROL_CHANGE => {
                    let v = self.read_byte();
                    if v < 0 {
                        return -1;
                    }
                    evt.msg.value = (v & 0x7F) as u8;
                }
                PITCH_BEND => {
                    let v = self.read_byte();
                    if v < 0 {
                        return -1;
                    }
                    evt.msg.pitch_bend = (((v & 0x7F) << 7) | i32::from(key)) as u16;
                }
                PROGRAM_CHANGE | 0xD0 => evt.msg.value = 0,
                _ => evt.msg.kind = 0, // system and manufacturer messages are ignored
            }
        }

        if delta != 0 || evt.msg.kind != 0 {
            evt.msg.time = delta as u32;
            out.push(evt);
        }
        i32::from(evt.msg.kind)
    }
}

/// Loads the messages of a Standard MIDI File, in playing order. Returns `None` if the data is not a MIDI
/// file or has no messages (tml_load).
pub fn load(data: &[u8]) -> Option<Vec<Message>> {
    if data.len() < 14 {
        return None;
    }
    let hdr = &data[0..14];
    if &hdr[0..4] != b"MThd" || hdr[7] != 6 || hdr[9] > 2 {
        return None;
    }
    if hdr[12] & 0x80 != 0 {
        return None; // SMPTE timing is not supported
    }
    let num_tracks = (i32::from(hdr[10]) << 8) | i32::from(hdr[11]);
    let division = (i32::from(hdr[12]) << 8) | i32::from(hdr[13]);
    if num_tracks <= 0 && division <= 0 {
        return None;
    }

    let mut tracks = vec![Track::default(); num_tracks.max(0) as usize];
    let mut messages: Vec<Stored> = Vec::new();
    let mut pos = 14usize;
    let mut parser = Parser { buf: &[], pos: 0, last_status: 0 };

    for t in tracks.iter_mut() {
        if data.len() - pos < 8 {
            break;
        }
        let header = &data[pos..pos + 8];
        pos += 8;
        if &header[0..4] != b"MTrk" {
            break;
        }
        let track_length = (i32::from(header[4]) << 24) | (i32::from(header[5]) << 16) | (i32::from(header[6]) << 8) | i32::from(header[7]);
        if track_length < 0 {
            break;
        }
        let len = track_length as usize;
        if data.len() - pos < len {
            break;
        }
        parser.buf = &data[pos..pos + len];
        parser.pos = 0;
        pos += len;

        t.idx = messages.len();
        while parser.pos != parser.buf.len() {
            let ty = parser.parse_message(&mut messages);
            if ty == i32::from(END_OF_TRACK) || ty < 0 {
                break;
            }
        }
        t.end = messages.len();
    }

    if messages.is_empty() {
        return None;
    }

    // Times in milliseconds, and the linking. A tick is converted with the tempo in force at the last tempo change.
    let mut prev: Option<usize> = None;
    let mut ticks: u32 = 0;
    let mut tempo_ticks: u32 = 0;
    let mut tempo_msec: i32 = 0;
    let mut ticks2time: f64 = 500_000.0 / (1000.0 * f64::from(division));
    let mut step_smallest: i32 = 0;
    while step_smallest != 0x7FFF_FFFF {
        step_smallest = 0x7FFF_FFFF;
        let msec = tempo_msec.wrapping_add(((ticks.wrapping_sub(tempo_ticks)) as f64 * ticks2time) as i32);
        for t in tracks.iter_mut() {
            if t.idx == t.end {
                continue;
            }
            while t.idx != t.end {
                let mi = t.idx;
                let delta = messages[mi].msg.time;
                if t.ticks.wrapping_add(delta) != ticks {
                    break;
                }
                t.ticks = t.ticks.wrapping_add(delta);
                if messages[mi].msg.kind == SET_TEMPO {
                    let tp = messages[mi].msg.tempo;
                    let us = (u32::from(tp[0]) << 16) | (u32::from(tp[1]) << 8) | u32::from(tp[2]);
                    ticks2time = f64::from(us) / (1000.0 * f64::from(division));
                    tempo_msec = msec;
                    tempo_ticks = ticks;
                }
                if messages[mi].msg.kind != 0 {
                    messages[mi].msg.time = msec as u32;
                    match prev {
                        Some(p) => {
                            messages[p].next = Some(mi);
                            prev = Some(mi);
                        }
                        None => {
                            // The first message goes to the front of the list (the C code swaps it into slot 0).
                            messages.swap(mi, 0);
                            prev = Some(0);
                        }
                    }
                }
                t.idx += 1;
            }
            if t.idx != t.end {
                let due = t.ticks.wrapping_add(messages[t.idx].msg.time);
                if due > ticks {
                    let step = (due - ticks) as i32;
                    if step < step_smallest {
                        step_smallest = step;
                    }
                }
            }
        }
        ticks = ticks.wrapping_add(step_smallest as u32);
    }

    let last = prev?;
    messages[last].next = None;

    // Walk the list from the front. A visited set guards against a malformed file closing a loop.
    let mut out = Vec::new();
    let mut seen = vec![false; messages.len()];
    let mut cur = Some(0usize);
    while let Some(i) = cur {
        if seen[i] {
            break;
        }
        seen[i] = true;
        out.push(messages[i].msg);
        cur = messages[i].next;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smf(division: u16, events: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut track = Vec::new();
        for (delta, ev) in events {
            track.extend(vlq(*delta));
            track.extend_from_slice(ev);
        }
        track.extend([0, 0xFF, 0x2F, 0]);
        let mut out = b"MThd".to_vec();
        out.extend(6u32.to_be_bytes());
        out.extend([0, 0, 0, 1]);
        out.extend(division.to_be_bytes());
        out.extend(b"MTrk");
        out.extend((track.len() as u32).to_be_bytes());
        out.extend(track);
        out
    }

    fn vlq(mut v: u32) -> Vec<u8> {
        let mut b = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            b.push((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
        b.reverse();
        b
    }

    #[test]
    fn notes_get_millisecond_times_and_tempo_changes_apply() {
        // 96 ticks per quarter at 120 BPM: a quarter is 500 ms. A tempo of 250 000 us from tick 96 halves the length.
        let file = smf(
            96,
            &[
                (0, vec![0x90, 60, 100]),
                (96, vec![0x80, 60, 0]),
                (0, vec![0xFF, 0x51, 3, 0x03, 0xD0, 0x90]),
                (96, vec![0x91, 62, 90]),
            ],
        );
        let msgs = load(&file).expect("valid file");
        let times: Vec<(u8, u32)> = msgs.iter().map(|m| (m.kind, m.time)).collect();
        assert_eq!(times, vec![(NOTE_ON, 0), (NOTE_OFF, 500), (SET_TEMPO, 500), (NOTE_ON, 750)]);
    }

    #[test]
    fn non_midi_data_is_rejected() {
        assert!(load(b"RIFF....WAVE....").is_none());
    }
}
