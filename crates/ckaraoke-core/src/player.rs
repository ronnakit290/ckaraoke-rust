//! Sequences a MIDI file through a SoundFont synthesizer, with the mixer. Port of `src/player.cpp`.
//!
//! Sounds are rendered in blocks of 64 frames. Each drum piece (MIDI key on channel 10) plays on its own
//! synthesizer channel, numbered after the 16 MIDI channels, so its level can be set separately.

use crate::tml::{self, Message};
use tsf::Tsf;

pub const MIDI_CHANNELS: usize = 16;
/// SoundFonts loaded at once: slot 0 is the main one, the rest are extras that instrument families can be routed to.
pub const SOUND_FONT_SLOTS: usize = 8;
/// The 16 General MIDI program families (program / 8) plus the drum channel.
pub const INSTRUMENT_FAMILIES: usize = 17;
pub const DRUM_FAMILY: usize = 16;
const DRUM_CHANNEL: usize = 9;
const DRUM_PIECE_BASE: usize = MIDI_CHANNELS;
const BLOCK: usize = 64;
const LEVEL_HALF_LIFE_MS: f32 = 250.0;
const HELD_LEVEL: f32 = 0.3;
const MAX_VOICES: usize = 256;

fn piece_channel(key: usize) -> usize {
    DRUM_PIECE_BASE + key
}

/// What the mixer shows for one MIDI channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelStatus {
    /// The instrument currently selected, empty if none.
    pub preset: String,
    /// The loaded song plays notes on this channel.
    pub used: bool,
    pub drums: bool,
}

pub struct Player {
    sf: [Option<Tsf>; SOUND_FONT_SLOTS],
    family_slot: [usize; INSTRUMENT_FAMILIES],
    program: [i32; MIDI_CHANNELS],
    has_preset: [[bool; MIDI_CHANNELS]; SOUND_FONT_SLOTS],
    volume: f32,
    midi: Vec<Message>,
    cursor: usize,
    sample_rate: i32,
    msec: f64,
    duration_ms: f64,
    paused: bool,
    speed: f64,
    transpose: i32,

    gain: [f32; MIDI_CHANNELS],
    mute: [bool; MIDI_CHANNELS],
    solo: [bool; MIDI_CHANNELS],
    midi_vol: [f32; MIDI_CHANNELS],
    level: [f32; MIDI_CHANNELS],
    held: [i32; MIDI_CHANNELS],
    used: [bool; MIDI_CHANNELS],
    drum_gain: [f32; 128],
    drum_mute: [bool; 128],
    drum_used: [bool; 128],
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Player {
        Player {
            sf: std::array::from_fn(|_| None),
            family_slot: [0; INSTRUMENT_FAMILIES],
            program: [0; MIDI_CHANNELS],
            has_preset: [[false; MIDI_CHANNELS]; SOUND_FONT_SLOTS],
            volume: 1.0,
            midi: Vec::new(),
            cursor: 0,
            sample_rate: 44100,
            msec: 0.0,
            duration_ms: 0.0,
            paused: false,
            speed: 1.0,
            transpose: 0,
            gain: [1.0; MIDI_CHANNELS],
            mute: [false; MIDI_CHANNELS],
            solo: [false; MIDI_CHANNELS],
            midi_vol: [1.0; MIDI_CHANNELS],
            level: [0.0; MIDI_CHANNELS],
            held: [0; MIDI_CHANNELS],
            used: [false; MIDI_CHANNELS],
            drum_gain: [1.0; 128],
            drum_mute: [false; 128],
            drum_used: [false; 128],
        }
    }

    /// Loads a SoundFont into a slot. Slot 0 is the main one; the others are extras for instrument families.
    pub fn load_sound_font(&mut self, slot: usize, data: &[u8], sample_rate: i32) -> Result<(), String> {
        if slot >= SOUND_FONT_SLOTS {
            return Err("ช่อง SoundFont ไม่ถูกต้อง".to_string());
        }
        let mut f = Tsf::load(data)?;
        self.sample_rate = sample_rate;
        f.set_output(self.sample_rate, 0.0);
        f.set_max_voices(MAX_VOICES);
        f.set_volume(self.volume);
        self.sf[slot] = Some(f);
        self.rebuild_state(self.msec);
        Ok(())
    }

    pub fn unload_sound_font(&mut self, slot: usize) {
        if (1..SOUND_FONT_SLOTS).contains(&slot) {
            self.sf[slot] = None;
        }
    }

    pub fn set_family_slot(&mut self, family: usize, slot: usize) {
        if family < INSTRUMENT_FAMILIES && slot < SOUND_FONT_SLOTS {
            self.family_slot[family] = slot;
        }
    }

    /// Loads a MIDI file's messages and starts it from the beginning (loadMidi).
    pub fn load_midi(&mut self, data: &[u8]) -> Result<(), String> {
        let messages = tml::load(data).ok_or_else(|| "ข้อมูล MIDI เสียหายหรือไม่รองรับ".to_string())?;
        let mut used = [false; MIDI_CHANNELS];
        let mut drum_keys = [false; 128];
        let mut last = 0u32;
        for m in &messages {
            last = last.max(m.time);
            if m.kind == tml::NOTE_ON && (m.channel as usize) < MIDI_CHANNELS {
                used[m.channel as usize] = true;
            }
            if m.kind == tml::NOTE_ON && m.channel as usize == DRUM_CHANNEL && (m.key as usize) < 128 {
                drum_keys[m.key as usize] = true;
            }
        }
        self.midi = messages;
        self.cursor = 0;
        self.duration_ms = f64::from(last);
        self.msec = 0.0;
        self.paused = false;
        self.used = used;
        self.drum_used = drum_keys;
        self.level = [0.0; MIDI_CHANNELS];
        self.held = [0; MIDI_CHANNELS];
        self.reset_channels();
        Ok(())
    }

    fn set_program(&mut self, ch: usize, program: i32) {
        self.program[ch] = program;
        let drums = ch == DRUM_CHANNEL;
        for slot in 0..SOUND_FONT_SLOTS {
            let Some(f) = self.sf[slot].as_mut() else { continue };
            // The synthesizer falls back to another preset when the SoundFont lacks this one; remember whether
            // it really has it, so notes can go to the main font instead.
            f.channel_set_presetnumber(ch, program, drums);
            self.has_preset[slot][ch] = if drums {
                f.channel_get_preset_bank(ch) >= 128
            } else {
                f.channel_get_preset_number(ch) == program
            };
            if drums {
                // Each drum piece takes the same kit, resolved the same way as the drum channel.
                let bank = f.channel_get_preset_bank(ch);
                for key in 0..128 {
                    f.channel_set_bank(piece_channel(key), bank);
                    f.channel_set_presetnumber(piece_channel(key), program, true);
                }
            }
        }
    }

    fn slot_for(&self, ch: usize) -> usize {
        let family = if ch == DRUM_CHANNEL { DRUM_FAMILY } else { ((self.program[ch] & 127) / 8) as usize };
        let slot = self.family_slot[family];
        if self.sf[slot].is_some() && self.has_preset[slot][ch] {
            slot
        } else {
            0
        }
    }

    fn reset_channels(&mut self) {
        for f in self.sf.iter_mut().flatten() {
            f.reset();
            f.channel_set_bank_preset(DRUM_CHANNEL, 128, 0);
        }
        for ch in 0..MIDI_CHANNELS {
            self.set_program(ch, 0);
        }
        self.midi_vol = [1.0; MIDI_CHANNELS];
        self.apply_all_channel_gains();
    }

    fn any_solo(&self) -> bool {
        self.solo.iter().any(|&s| s)
    }

    fn apply_channel_gain(&mut self, ch: usize) {
        let silent = self.mute[ch] || (self.any_solo() && !self.solo[ch]);
        let volume = if silent { 0.0 } else { self.midi_vol[ch] * self.gain[ch] };
        for f in self.sf.iter_mut().flatten() {
            f.channel_set_volume(ch, volume);
        }
        if ch == DRUM_CHANNEL {
            self.apply_all_piece_gains(); // the pieces sit under the drum channel's level
        }
    }

    fn apply_all_channel_gains(&mut self) {
        for ch in 0..MIDI_CHANNELS {
            self.apply_channel_gain(ch);
        }
    }

    /// A drum piece's volume is the drum channel's volume times its own level; a muted piece is silent.
    fn apply_piece_gain(&mut self, key: usize) {
        let silent = self.mute[DRUM_CHANNEL] || (self.any_solo() && !self.solo[DRUM_CHANNEL]) || self.drum_mute[key];
        let volume = if silent {
            0.0
        } else {
            self.midi_vol[DRUM_CHANNEL] * self.gain[DRUM_CHANNEL] * self.drum_gain[key]
        };
        for f in self.sf.iter_mut().flatten() {
            f.channel_set_volume(piece_channel(key), volume);
        }
    }

    fn apply_all_piece_gains(&mut self) {
        for key in 0..128 {
            self.apply_piece_gain(key);
        }
    }

    /// Applies one message. Controllers go to every loaded SoundFont, so each can take over a channel;
    /// a note sounds only on the one its instrument family is routed to.
    fn apply(&mut self, m: &Message, play_notes: bool) {
        let ch = m.channel as usize;
        let key = if ch == DRUM_CHANNEL {
            m.key as usize
        } else {
            (i32::from(m.key) + self.transpose).clamp(0, 127) as usize
        };
        match m.kind {
            tml::PROGRAM_CHANGE => self.set_program(ch, i32::from(m.key)),
            tml::NOTE_ON => {
                if play_notes {
                    let target = if ch == DRUM_CHANNEL { piece_channel(key) } else { ch }; // drum notes sound on their piece's channel
                    let slot = self.slot_for(ch);
                    let vel = f32::from(m.value) / 127.0;
                    if let Some(f) = self.sf[slot].as_mut() {
                        f.channel_note_on(target, key as i32, vel);
                    }
                    self.level[ch] = self.level[ch].max(vel);
                    self.held[ch] += 1;
                }
            }
            tml::NOTE_OFF => {
                if play_notes {
                    let target = if ch == DRUM_CHANNEL { piece_channel(key) } else { ch };
                    for f in self.sf.iter_mut().flatten() {
                        f.channel_note_off(target, key as i32);
                    }
                    self.held[ch] = (self.held[ch] - 1).max(0);
                }
            }
            tml::PITCH_BEND => {
                for f in self.sf.iter_mut().flatten() {
                    f.channel_set_pitchwheel(ch, i32::from(m.pitch_bend));
                }
                if ch == DRUM_CHANNEL {
                    for f in self.sf.iter_mut().flatten() {
                        for k in 0..128 {
                            f.channel_set_pitchwheel(piece_channel(k), i32::from(m.pitch_bend));
                        }
                    }
                }
            }
            tml::CONTROL_CHANGE => {
                let control = i32::from(m.key);
                let value = i32::from(m.value);
                for f in self.sf.iter_mut().flatten() {
                    f.channel_midi_control(ch, control, value);
                }
                // The drum channel's other controllers (pan, sustain, ...) apply to every piece too.
                // Volume and bank are left out: volume is set per piece below, and the kit must stay.
                if ch == DRUM_CHANNEL && !matches!(control, 0 | 32 | 7 | 11 | 39 | 43) {
                    for f in self.sf.iter_mut().flatten() {
                        for k in 0..128 {
                            f.channel_midi_control(piece_channel(k), control, value);
                        }
                    }
                }
                if matches!(control, 7 | 39 | 11 | 43 | 121) {
                    // The synthesizer just overwrote the channel volume; put the mixer gain back on top.
                    if let Some(f) = self.sf[0].as_ref() {
                        self.midi_vol[ch] = f.channel_get_volume(ch);
                    }
                    self.apply_channel_gain(ch);
                }
            }
            _ => {}
        }
    }

    /// Renders interleaved stereo into `out` (its length is twice the number of frames).
    pub fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        if self.sf[0].is_none() || self.paused {
            out.fill(0.0);
            return;
        }
        let decay = 0.5f32.powf((BLOCK as f32 * 1000.0) / self.sample_rate as f32 / LEVEL_HALF_LIFE_MS);
        let mut left = frames;
        let mut at = 0usize;
        while left > 0 {
            let block = left.min(BLOCK);
            // Song time runs faster or slower than real time with the speed setting.
            self.msec += block as f64 * 1000.0 / f64::from(self.sample_rate) * self.speed;
            while self.cursor < self.midi.len() && f64::from(self.midi[self.cursor].time) <= self.msec {
                let m = self.midi[self.cursor];
                self.apply(&m, true);
                self.cursor += 1;
            }
            let slice = &mut out[2 * at..2 * (at + block)];
            if let Some(f) = self.sf[0].as_mut() {
                f.render_float(slice, block, false);
            }
            for slot in 1..SOUND_FONT_SLOTS {
                if let Some(f) = self.sf[slot].as_mut() {
                    f.render_float(slice, block, true); // mixed on top
                }
            }
            for ch in 0..MIDI_CHANNELS {
                let held = if self.held[ch] > 0 { HELD_LEVEL } else { 0.0 };
                self.level[ch] = (self.level[ch] * decay).max(held);
            }
            at += block;
            left -= block;
        }
    }

    /// Stops the song: no more events play, and the mixer keeps its settings. Sounds already ringing die away.
    pub fn stop(&mut self) {
        self.midi.clear();
        self.cursor = 0;
        self.msec = 0.0;
        self.duration_ms = 0.0;
        self.used = [false; MIDI_CHANNELS];
        self.drum_used = [false; 128];
        self.level = [0.0; MIDI_CHANNELS];
        self.held = [0; MIDI_CHANNELS];
    }

    pub fn seek(&mut self, ms: f64) {
        if self.sf[0].is_none() || self.midi.is_empty() {
            return;
        }
        self.rebuild_state(ms.clamp(0.0, self.duration_ms));
    }

    /// Rebuilds the controller and program state up to `ms` by replaying the controllers without the notes.
    fn rebuild_state(&mut self, ms: f64) {
        if self.sf[0].is_none() {
            return;
        }
        self.reset_channels();
        self.cursor = 0;
        while self.cursor < self.midi.len() && f64::from(self.midi[self.cursor].time) < ms {
            let m = self.midi[self.cursor];
            self.apply(&m, false);
            self.cursor += 1;
        }
        self.msec = ms;
        self.level = [0.0; MIDI_CHANNELS];
        self.held = [0; MIDI_CHANNELS];
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Main volume, 1.0 is 100%.
    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, gain: f32) {
        self.volume = gain;
        for f in self.sf.iter_mut().flatten() {
            f.set_volume(gain);
        }
    }

    /// Playback speed, 0.25 to 4.0. The pitch does not change.
    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.25, 4.0);
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// Key shift for every non-drum channel, in semitones, clamped to ±12.
    pub fn set_transpose(&mut self, semitones: i32) {
        let semitones = semitones.clamp(-12, 12);
        if semitones == self.transpose {
            return;
        }
        // Held notes would never get their shifted note-off; release them now.
        for f in self.sf.iter_mut().flatten() {
            for ch in 0..MIDI_CHANNELS {
                if ch != DRUM_CHANNEL {
                    f.channel_note_off_all(ch);
                }
            }
        }
        self.transpose = semitones;
    }

    pub fn transpose(&self) -> i32 {
        self.transpose
    }

    /// Per-channel gain on top of the song's own volume (1.0 = 100%). Settings survive seeks and song changes.
    pub fn set_channel_gain(&mut self, ch: usize, gain: f32) {
        if ch < MIDI_CHANNELS {
            self.gain[ch] = gain.max(0.0);
            self.apply_channel_gain(ch);
        }
    }

    pub fn channel_gain(&self, ch: usize) -> f32 {
        self.gain.get(ch).copied().unwrap_or(1.0)
    }

    pub fn channel_mute(&self, ch: usize) -> bool {
        self.mute.get(ch).copied().unwrap_or(false)
    }

    pub fn channel_solo(&self, ch: usize) -> bool {
        self.solo.get(ch).copied().unwrap_or(false)
    }

    pub fn set_channel_mute(&mut self, ch: usize, mute: bool) {
        if ch < MIDI_CHANNELS {
            self.mute[ch] = mute;
            self.apply_channel_gain(ch);
        }
    }

    pub fn set_channel_solo(&mut self, ch: usize, solo: bool) {
        if ch < MIDI_CHANNELS {
            self.solo[ch] = solo;
            self.apply_all_channel_gains(); // a solo changes which other channels are audible
        }
    }

    pub fn reset_mixer(&mut self) {
        self.gain = [1.0; MIDI_CHANNELS];
        self.mute = [false; MIDI_CHANNELS];
        self.solo = [false; MIDI_CHANNELS];
        self.apply_all_channel_gains();
    }

    pub fn channel_status(&self) -> [ChannelStatus; MIDI_CHANNELS] {
        std::array::from_fn(|ch| {
            let mut status = ChannelStatus {
                used: !self.midi.is_empty() && self.used[ch],
                drums: ch == DRUM_CHANNEL,
                preset: String::new(),
            };
            if let Some(f) = self.sf[0].as_ref() {
                let slot = self.slot_for(ch); // name the sound that is actually heard
                if let Some(f) = self.sf[slot].as_ref() {
                    if let Some(name) = f.preset_name(f.channel_get_preset_index(ch)) {
                        status.preset = name.to_string();
                    }
                }
                let _ = f;
            }
            status
        })
    }

    /// Note activity per channel, 0..1, for the meters.
    pub fn channel_levels(&self) -> [f32; MIDI_CHANNELS] {
        let mut out = [0.0; MIDI_CHANNELS];
        if self.sf[0].is_none() || self.paused {
            return out;
        }
        let any_solo = self.any_solo();
        for (ch, slot) in out.iter_mut().enumerate() {
            if self.mute[ch] || (any_solo && !self.solo[ch]) {
                continue;
            }
            *slot = (self.level[ch] * (self.midi_vol[ch] * self.gain[ch]).min(1.0)).min(1.0);
        }
        out
    }

    /// Level of one drum piece (MIDI key), 0..2.
    pub fn set_drum_gain(&mut self, key: usize, gain: f32) {
        if key < 128 {
            self.drum_gain[key] = gain.clamp(0.0, 2.0);
            self.apply_piece_gain(key);
        }
    }

    pub fn set_drum_mute(&mut self, key: usize, mute: bool) {
        if key < 128 {
            self.drum_mute[key] = mute;
            self.apply_piece_gain(key);
        }
    }

    pub fn reset_drums(&mut self) {
        self.drum_gain = [1.0; 128];
        self.drum_mute = [false; 128];
        self.apply_all_piece_gains();
    }

    pub fn drum_gain(&self, key: usize) -> f32 {
        if key < 128 { self.drum_gain[key] } else { 1.0 }
    }

    pub fn drum_mute(&self, key: usize) -> bool {
        key < 128 && self.drum_mute[key]
    }

    /// The drum keys the loaded song plays.
    pub fn drum_keys_used(&self) -> [bool; 128] {
        self.drum_used
    }

    /// Song position in milliseconds, at most the song's length.
    pub fn position_ms(&self) -> f64 {
        self.msec.min(self.duration_ms)
    }

    pub fn duration_ms(&self) -> f64 {
        self.duration_ms
    }

    /// All events have played and the voices have released (give long tails at most three seconds).
    pub fn finished(&self) -> bool {
        if self.sf[0].is_none() || self.midi.is_empty() {
            return true;
        }
        let voices: usize = self.sf.iter().flatten().map(|f| f.active_voice_count()).sum();
        self.cursor >= self.midi.len() && (voices == 0 || self.msec > self.duration_ms + 3000.0)
    }

    pub fn preset_count(&self) -> usize {
        self.sf[0].as_ref().map_or(0, |f| f.preset_count())
    }
}
