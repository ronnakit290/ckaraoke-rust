//! Voices, channels and rendering. Port of the playback half of tsf.h (TinySoundFont v0.9).
//!
//! Every float and double step follows the C code, so the output matches the C build bit for bit when
//! both are compiled without fused multiply-add. Behaviour that CKaraoke does not use (Ogg Vorbis, mono
//! and unweaved output, the separate `channel_set_presetindex`) is left out.

use std::f64::consts::PI;

use crate::font::{self, Font};
use crate::math::*;
use crate::region::{Envelope, Preset, Region, LOOP_NONE, LOOP_SUSTAIN};

/// Samples per effect block: LFO and envelope updates happen at this rate (TSF_RENDER_EFFECTSAMPLEBLOCK).
const EFFECT_BLOCK: usize = 64;
/// Release time used when a voice is ended without one (TSF_FASTRELEASETIME).
const FAST_RELEASE_TIME: f32 = 0.01;

const SEG_NONE: u8 = 0;
const SEG_DELAY: u8 = 1;
const SEG_ATTACK: u8 = 2;
const SEG_HOLD: u8 = 3;
const SEG_DECAY: u8 = 4;
const SEG_SUSTAIN: u8 = 5;
const SEG_RELEASE: u8 = 6;
const SEG_DONE: u8 = 7;

#[derive(Clone, Copy, Default)]
struct VoiceEnvelope {
    segment: u8,
    segment_is_exponential: bool,
    is_amp_env: bool,
    midi_velocity: i16,
    level: f32,
    slope: f32,
    samples_until_next_segment: i32,
    parameters: Envelope,
}

#[derive(Clone, Copy, Default)]
struct VoiceLowpass {
    q_inv: f64,
    a0: f64,
    a1: f64,
    b1: f64,
    b2: f64,
    z1: f64,
    z2: f64,
    active: bool,
}

#[derive(Clone, Copy, Default)]
struct VoiceLfo {
    samples_until: i32,
    level: f32,
    delta: f32,
}

#[derive(Clone, Copy)]
struct Voice {
    playing_preset: i32, // -1 when the voice is free
    playing_key: i32,
    playing_channel: i32,
    held_sustain: i32,
    region: usize, // index into Tsf::regions
    pitch_input_timecents: f64,
    pitch_output_factor: f64,
    source_sample_position: f64,
    note_gain_db: f32,
    pan_factor_left: f32,
    pan_factor_right: f32,
    play_index: u32,
    loop_start: u32,
    loop_end: u32,
    ampenv: VoiceEnvelope,
    modenv: VoiceEnvelope,
    lowpass: VoiceLowpass,
    modlfo: VoiceLfo,
    viblfo: VoiceLfo,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            playing_preset: -1,
            playing_key: 0,
            playing_channel: 0,
            held_sustain: 0,
            region: 0,
            pitch_input_timecents: 0.0,
            pitch_output_factor: 0.0,
            source_sample_position: 0.0,
            note_gain_db: 0.0,
            pan_factor_left: 0.0,
            pan_factor_right: 0.0,
            play_index: 0,
            loop_start: 0,
            loop_end: 0,
            ampenv: VoiceEnvelope::default(),
            modenv: VoiceEnvelope::default(),
            lowpass: VoiceLowpass::default(),
            modlfo: VoiceLfo::default(),
            viblfo: VoiceLfo::default(),
        }
    }
}

/// The state of one MIDI channel as the synth keeps it (struct tsf_channel).
#[derive(Clone, Copy)]
struct Channel {
    preset_index: u16,
    bank: u16,
    pitch_wheel: u16,
    mid_pan: u16,
    mid_volume: u16,
    mid_expression: u16,
    mid_rpn: u16,
    mid_data: u16,
    sustain: bool,
    pan_offset: f32,
    gain_db: f32,
    pitch_range: f32,
    tuning: f32,
}

impl Channel {
    fn new() -> Channel {
        Channel {
            preset_index: 0,
            bank: 0,
            pitch_wheel: 8192,
            mid_pan: 8192,
            mid_volume: 16383,
            mid_expression: 16383,
            mid_rpn: 0xFFFF,
            mid_data: 0,
            sustain: false,
            pan_offset: 0.0,
            gain_db: 0.0,
            pitch_range: 2.0,
            tuning: 0.0,
        }
    }
}

struct Channels {
    list: Vec<Channel>,
    active: usize,
}

/// A SoundFont synthesizer: the loaded presets and samples, and the voices and channels playing them.
pub struct Tsf {
    presets: Vec<Preset>,
    regions: Vec<Region>,
    samples: Vec<f32>,
    voices: Vec<Voice>,
    max_voice_num: usize, // 0 when voices are allocated on demand
    voice_play_index: u32,
    channels: Option<Channels>,
    out_sample_rate: f32,
    global_gain_db: f32,
}

// ---------------------------------------------------------------------------
// Envelopes

fn release_samples(e: &VoiceEnvelope, sr: f32) -> i32 {
    let release = if e.parameters.release <= 0.0 { FAST_RELEASE_TIME } else { e.parameters.release };
    (release * sr) as i32
}

/// Moves an envelope to the segment after `active`, computing its level, slope and length (tsf_voice_envelope_nextsegment).
fn envelope_next_segment(e: &mut VoiceEnvelope, active: u8, sr: f32) {
    let mut seg = active;
    if seg == SEG_NONE {
        e.samples_until_next_segment = (e.parameters.delay * sr) as i32;
        if e.samples_until_next_segment > 0 {
            e.segment = SEG_DELAY;
            e.segment_is_exponential = false;
            e.level = 0.0;
            e.slope = 0.0;
            return;
        }
        seg = SEG_DELAY;
    }
    if seg == SEG_DELAY {
        e.samples_until_next_segment = (e.parameters.attack * sr) as i32;
        if e.samples_until_next_segment > 0 {
            if !e.is_amp_env {
                // Modulation envelope attacks scale with velocity: full length at velocity 1, an eighth at 127.
                e.samples_until_next_segment =
                    (e.parameters.attack * ((145 - i32::from(e.midi_velocity)) as f32 / 144.0) * sr) as i32;
            }
            e.segment = SEG_ATTACK;
            e.segment_is_exponential = false;
            e.level = 0.0;
            e.slope = 1.0 / e.samples_until_next_segment as f32;
            return;
        }
        seg = SEG_ATTACK;
    }
    if seg == SEG_ATTACK {
        e.samples_until_next_segment = (e.parameters.hold * sr) as i32;
        if e.samples_until_next_segment > 0 {
            e.segment = SEG_HOLD;
            e.segment_is_exponential = false;
            e.level = 1.0;
            e.slope = 0.0;
            return;
        }
        seg = SEG_HOLD;
    }
    if seg == SEG_HOLD {
        e.samples_until_next_segment = (e.parameters.decay * sr) as i32;
        if e.samples_until_next_segment > 0 {
            e.segment = SEG_DECAY;
            e.level = 1.0;
            if e.is_amp_env {
                // The decay follows LinuxSampler, which the C code copies.
                let mystery_slope = -9.226f32 / e.samples_until_next_segment as f32;
                e.slope = mystery_slope.exp();
                e.segment_is_exponential = true;
                if e.parameters.sustain > 0.0 {
                    e.samples_until_next_segment = ((f64::from(e.parameters.sustain)).ln() / f64::from(mystery_slope)) as i32;
                }
            } else {
                e.slope = -1.0 / e.samples_until_next_segment as f32;
                e.samples_until_next_segment = (e.parameters.decay * (1.0 - e.parameters.sustain) * sr) as i32;
                e.segment_is_exponential = false;
            }
            return;
        }
        seg = SEG_DECAY;
    }
    if seg == SEG_DECAY {
        e.segment = SEG_SUSTAIN;
        e.level = e.parameters.sustain;
        e.slope = 0.0;
        e.samples_until_next_segment = 0x7FFF_FFFF;
        e.segment_is_exponential = false;
        return;
    }
    if seg == SEG_SUSTAIN {
        e.segment = SEG_RELEASE;
        e.samples_until_next_segment = release_samples(e, sr);
        if e.is_amp_env {
            let mystery_slope = -9.226f32 / e.samples_until_next_segment as f32;
            e.slope = mystery_slope.exp();
            e.segment_is_exponential = true;
        } else {
            e.slope = -e.level / e.samples_until_next_segment as f32;
            e.segment_is_exponential = false;
        }
        return;
    }
    e.segment = SEG_DONE;
    e.segment_is_exponential = false;
    e.level = 0.0;
    e.slope = 0.0;
    e.samples_until_next_segment = 0x7FF_FFFF;
}

/// Starts an envelope for a note (tsf_voice_envelope_setup).
fn envelope_setup(e: &mut VoiceEnvelope, params: &Envelope, midi_note: i32, velocity: i16, is_amp_env: bool, sr: f32) {
    e.parameters = *params;
    if e.parameters.keynum_to_hold != 0.0 {
        e.parameters.hold += e.parameters.keynum_to_hold * (60.0 - midi_note as f32);
        e.parameters.hold = if e.parameters.hold < -10000.0 { 0.0 } else { timecents_to_secs_f(e.parameters.hold) };
    }
    if e.parameters.keynum_to_decay != 0.0 {
        e.parameters.decay += e.parameters.keynum_to_decay * (60.0 - midi_note as f32);
        e.parameters.decay = if e.parameters.decay < -10000.0 { 0.0 } else { timecents_to_secs_f(e.parameters.decay) };
    }
    e.midi_velocity = velocity;
    e.is_amp_env = is_amp_env;
    envelope_next_segment(e, SEG_NONE, sr);
}

/// Advances an envelope by a block of samples (tsf_voice_envelope_process).
fn envelope_process(e: &mut VoiceEnvelope, samples: i32, sr: f32) {
    if e.slope != 0.0 {
        if e.segment_is_exponential {
            e.level *= e.slope.powf(samples as f32);
        } else {
            e.level += e.slope * samples as f32;
        }
    }
    e.samples_until_next_segment -= samples;
    if e.samples_until_next_segment <= 0 {
        let seg = e.segment;
        envelope_next_segment(e, seg, sr);
    }
}

// ---------------------------------------------------------------------------
// Low-pass filter and LFOs

fn lowpass_setup(e: &mut VoiceLowpass, fc: f32) {
    // Biquad low-pass from earlevel.com, as in the C code.
    let k = (PI * f64::from(fc)).tan();
    let kk = k * k;
    let norm = 1.0 / (1.0 + k * e.q_inv + kk);
    e.a0 = kk * norm;
    e.a1 = 2.0 * e.a0;
    e.b1 = 2.0 * (kk - 1.0) * norm;
    e.b2 = (1.0 - k * e.q_inv + kk) * norm;
}

fn lowpass_process(e: &mut VoiceLowpass, input: f64) -> f32 {
    let out = input * e.a0 + e.z1;
    e.z1 = input * e.a1 + e.z2 - e.b1 * out;
    e.z2 = input * e.a0 - e.b2 * out;
    out as f32
}

fn lfo_setup(e: &mut VoiceLfo, delay: f32, freq_cents: i32, sr: f32) {
    e.samples_until = (delay * sr) as i32;
    e.delta = 4.0 * cents_to_hertz(freq_cents as f32) / sr;
    e.level = 0.0;
}

fn lfo_process(e: &mut VoiceLfo, block: i32) {
    if e.samples_until > block {
        e.samples_until -= block;
        return;
    }
    e.level += e.delta * block as f32;
    if e.level > 1.0 {
        e.delta = -e.delta;
        e.level = 2.0 - e.level;
    } else if e.level < -1.0 {
        e.delta = -e.delta;
        e.level = -2.0 - e.level;
    }
}

// ---------------------------------------------------------------------------
// Pitch and pan

/// Sets a voice's pitch ratio from its key, region and a pitch shift in semitones (tsf_voice_calcpitchratio).
fn calc_pitch_ratio(v: &mut Voice, r: &Region, pitch_shift: f32, sr: f32) {
    let note = f64::from(v.playing_key + r.transpose) + f64::from(r.tune) / 100.0;
    let mut adjusted_pitch = f64::from(r.pitch_keycenter)
        + (note - f64::from(r.pitch_keycenter)) * (f64::from(r.pitch_keytrack) / 100.0);
    if pitch_shift != 0.0 {
        adjusted_pitch += f64::from(pitch_shift);
    }
    v.pitch_input_timecents = adjusted_pitch * 100.0;
    v.pitch_output_factor =
        f64::from(r.sample_rate) / (timecents_to_secs_d(f64::from(r.pitch_keycenter) * 100.0) * f64::from(sr));
}

/// Pan factors for a pan position in -0.5..0.5, with a constant-power curve in between (the C code's sqrt curve).
fn pan_factors(pan: f32) -> (f32, f32) {
    if pan <= -0.5 {
        (1.0, 0.0)
    } else if pan >= 0.5 {
        (0.0, 1.0)
    } else {
        ((0.5 - pan).sqrt(), (0.5 + pan).sqrt())
    }
}

/// The pitch shift a channel applies: its pitch-bend range and tuning (tsf_channel_setup_voice and friends).
fn channel_pitch_shift(c: &Channel) -> f32 {
    if c.pitch_wheel == 8192 {
        c.tuning
    } else {
        (f32::from(c.pitch_wheel) / 16383.0 * c.pitch_range * 2.0) - c.pitch_range + c.tuning
    }
}

/// Sets up a voice that starts on a channel (tsf_channel_setup_voice).
fn channel_setup_voice(v: &mut Voice, r: &Region, c: &Channel, channel: usize, sr: f32) {
    let newpan = r.pan + c.pan_offset;
    v.playing_channel = channel as i32;
    v.note_gain_db += c.gain_db;
    calc_pitch_ratio(v, r, channel_pitch_shift(c), sr);
    let (l, rt) = pan_factors(newpan);
    v.pan_factor_left = l;
    v.pan_factor_right = rt;
}

// ---------------------------------------------------------------------------
// Rendering a voice

/// Adds one voice's output into the interleaved stereo buffer `out` (tsf_voice_render, interleaved mode).
fn render_voice(v: &mut Voice, r: &Region, input: &[f32], sr: f32, out: &mut [f32], frames: usize) {
    let update_mod_env = r.mod_env_to_pitch != 0 || r.mod_env_to_filter_fc != 0;
    let update_mod_lfo =
        v.modlfo.delta != 0.0 && (r.mod_lfo_to_pitch != 0 || r.mod_lfo_to_filter_fc != 0 || r.mod_lfo_to_volume != 0);
    let update_vib_lfo = v.viblfo.delta != 0.0 && r.vib_lfo_to_pitch != 0;
    let is_looping = v.loop_start < v.loop_end;
    let tmp_loop_start = v.loop_start;
    let tmp_loop_end = v.loop_end;
    let tmp_sample_end = f64::from(r.end);
    let tmp_loop_end_dbl = f64::from(tmp_loop_end) + 1.0;
    let mut tmp_src = v.source_sample_position;
    let mut tmp_lowpass = v.lowpass;

    let dynamic_lowpass = r.mod_lfo_to_filter_fc != 0 || r.mod_env_to_filter_fc != 0;
    let (tmp_init_fc, tmp_lfo_fc, tmp_env_fc) = if dynamic_lowpass {
        (r.initial_filter_fc as f32, r.mod_lfo_to_filter_fc as f32, r.mod_env_to_filter_fc as f32)
    } else {
        (0.0, 0.0, 0.0)
    };

    let dynamic_pitch = r.mod_lfo_to_pitch != 0 || r.mod_env_to_pitch != 0 || r.vib_lfo_to_pitch != 0;
    let (mut pitch_ratio, tmp_lfo_pitch, tmp_vib_pitch, tmp_env_pitch) = if dynamic_pitch {
        (0.0f64, r.mod_lfo_to_pitch as f32, r.vib_lfo_to_pitch as f32, r.mod_env_to_pitch as f32)
    } else {
        (timecents_to_secs_d(v.pitch_input_timecents) * v.pitch_output_factor, 0.0, 0.0, 0.0)
    };

    let dynamic_gain = r.mod_lfo_to_volume != 0;
    let mut note_gain: f32 = 0.0;
    let tmp_lfo_volume: f32 = if dynamic_gain {
        r.mod_lfo_to_volume as f32 * 0.1
    } else {
        note_gain = decibels_to_gain(v.note_gain_db);
        0.0
    };

    let (pan_left, pan_right) = (v.pan_factor_left, v.pan_factor_right);
    let mut j = 0usize; // frame index into `out`
    let mut left = frames;
    while left > 0 {
        let block = left.min(EFFECT_BLOCK);
        left -= block;

        if dynamic_lowpass {
            let fres = tmp_init_fc + v.modlfo.level * tmp_lfo_fc + v.modenv.level * tmp_env_fc;
            let lowpass_fc = if fres <= 13500.0 { cents_to_hertz(fres) / sr } else { 1.0 };
            tmp_lowpass.active = lowpass_fc < 0.499;
            if tmp_lowpass.active {
                lowpass_setup(&mut tmp_lowpass, lowpass_fc);
            }
        }

        if dynamic_pitch {
            let mods = v.modlfo.level * tmp_lfo_pitch + v.viblfo.level * tmp_vib_pitch + v.modenv.level * tmp_env_pitch;
            pitch_ratio = timecents_to_secs_d(v.pitch_input_timecents + f64::from(mods)) * v.pitch_output_factor;
        }

        if dynamic_gain {
            note_gain = decibels_to_gain(v.note_gain_db + v.modlfo.level * tmp_lfo_volume);
        }

        let gain_mono = note_gain * v.ampenv.level;

        envelope_process(&mut v.ampenv, block as i32, sr);
        if update_mod_env {
            envelope_process(&mut v.modenv, block as i32, sr);
        }
        if update_mod_lfo {
            lfo_process(&mut v.modlfo, block as i32);
        }
        if update_vib_lfo {
            lfo_process(&mut v.viblfo, block as i32);
        }

        let gain_left = gain_mono * pan_left;
        let gain_right = gain_mono * pan_right;
        let mut blk = block;
        while blk > 0 && tmp_src < tmp_sample_end {
            blk -= 1;
            let pos = tmp_src as u32;
            let next_pos = if pos >= tmp_loop_end && is_looping { tmp_loop_start } else { pos + 1 };
            // Linear interpolation between neighbouring samples. A read past the end counts as silence.
            let alpha = (tmp_src - f64::from(pos)) as f32;
            let a = input.get(pos as usize).copied().unwrap_or(0.0);
            let b = input.get(next_pos as usize).copied().unwrap_or(0.0);
            let mut val = a * (1.0 - alpha) + b * alpha;
            if tmp_lowpass.active {
                val = lowpass_process(&mut tmp_lowpass, f64::from(val));
            }
            out[2 * j] += val * gain_left;
            out[2 * j + 1] += val * gain_right;
            j += 1;

            tmp_src += pitch_ratio;
            if tmp_src >= tmp_loop_end_dbl && is_looping {
                tmp_src -= f64::from(tmp_loop_end.wrapping_sub(tmp_loop_start)) + 1.0;
            }
        }

        if tmp_src >= tmp_sample_end || v.ampenv.segment == SEG_DONE {
            v.playing_preset = -1;
            return;
        }
    }

    v.source_sample_position = tmp_src;
    if tmp_lowpass.active || dynamic_lowpass {
        v.lowpass = tmp_lowpass;
    }
}

// ---------------------------------------------------------------------------
// The synthesizer

impl Tsf {
    /// Loads a SoundFont 2 file from memory (tsf_load_memory). The output is stereo, 44100 Hz, until `set_output`.
    pub fn load(data: &[u8]) -> Result<Tsf, String> {
        let Font { presets, regions, samples } = font::load(data)?;
        Ok(Tsf {
            presets,
            regions,
            samples,
            voices: Vec::new(),
            max_voice_num: 0,
            voice_play_index: 0,
            channels: None,
            out_sample_rate: 44100.0,
            global_gain_db: 0.0,
        })
    }

    /// Sets the output sample rate and the global gain (tsf_set_output, stereo interleaved).
    pub fn set_output(&mut self, sample_rate: i32, global_gain_db: f32) {
        self.out_sample_rate = if sample_rate >= 1 { sample_rate as f32 } else { 44100.0 };
        self.global_gain_db = global_gain_db;
    }

    /// Sets the global volume as a linear factor (tsf_set_volume).
    pub fn set_volume(&mut self, volume: f32) {
        self.global_gain_db = if volume == 1.0 { 0.0 } else { -gain_to_decibels(1.0 / volume) };
    }

    /// Allocates the voices up front. With a fixed voice count, a new note steals the voice furthest into its release (tsf_set_max_voices).
    pub fn set_max_voices(&mut self, max_voices: usize) {
        let new_num = self.voices.len().max(max_voices);
        self.voices.resize(new_num, Voice::default());
        self.max_voice_num = new_num;
    }

    /// Ends the sounding voices quickly and forgets the channels (tsf_reset).
    pub fn reset(&mut self) {
        for i in 0..self.voices.len() {
            let v = &self.voices[i];
            if v.playing_preset != -1 && (v.ampenv.segment < SEG_RELEASE || v.ampenv.parameters.release != 0.0) {
                self.voice_end_quick(i);
            }
        }
        self.channels = None;
    }

    pub fn preset_count(&self) -> usize {
        self.presets.len()
    }

    /// The name of a preset by its index in the file's sorted order.
    pub fn preset_name(&self, index: usize) -> Option<&str> {
        self.presets.get(index).map(|p| p.name.as_str())
    }

    /// The index of the preset with this bank and number (tsf_get_presetindex).
    pub fn preset_index(&self, bank: i32, number: i32) -> Option<usize> {
        self.presets.iter().position(|p| i32::from(p.preset) == number && i32::from(p.bank) == bank)
    }

    /// Number of voices currently sounding or in release (tsf_active_voice_count).
    pub fn active_voice_count(&self) -> usize {
        self.voices.iter().filter(|v| v.playing_preset != -1).count()
    }

    /// Renders `frames` stereo frames into `out` (tsf_render_float, interleaved). With `mixing`, the sound is added to `out`.
    pub fn render_float(&mut self, out: &mut [f32], frames: usize, mixing: bool) {
        if !mixing {
            out[..2 * frames].fill(0.0);
        }
        let sr = self.out_sample_rate;
        let Tsf { voices, regions, samples, .. } = self;
        for v in voices.iter_mut() {
            if v.playing_preset != -1 {
                let r = regions[v.region];
                render_voice(v, &r, samples, sr, out, frames);
            }
        }
    }

    // -- voices ---------------------------------------------------------------

    fn voice_end(&mut self, i: usize) {
        let repeats = if self.max_voice_num > 0 { 2 } else { 1 };
        let sr = self.out_sample_rate;
        let region_loop_mode = self.regions[self.voices[i].region].loop_mode;
        for _ in 0..repeats {
            let v = &mut self.voices[i];
            envelope_next_segment(&mut v.ampenv, SEG_SUSTAIN, sr);
            envelope_next_segment(&mut v.modenv, SEG_SUSTAIN, sr);
            if region_loop_mode == LOOP_SUSTAIN {
                // Keep playing after the note-off, but stop looping.
                v.loop_end = v.loop_start;
            }
        }
    }

    fn voice_end_quick(&mut self, i: usize) {
        let repeats = if self.max_voice_num > 0 { 2 } else { 1 };
        let sr = self.out_sample_rate;
        for _ in 0..repeats {
            let v = &mut self.voices[i];
            v.ampenv.parameters.release = 0.0;
            envelope_next_segment(&mut v.ampenv, SEG_SUSTAIN, sr);
            v.modenv.parameters.release = 0.0;
            envelope_next_segment(&mut v.modenv, SEG_SUSTAIN, sr);
        }
    }

    /// Starts `preset_index` on `key` at velocity `vel` (0..1). Returns false if no voice was free (tsf_note_on).
    pub fn note_on(&mut self, preset_index: usize, key: i32, vel: f32) -> bool {
        let mid_vel = (vel * 127.0) as i16;
        if preset_index >= self.presets.len() {
            return true;
        }
        if vel <= 0.0 {
            self.note_off(preset_index, key);
            return true;
        }
        let pi = preset_index as i32;
        let play_index = self.voice_play_index;
        self.voice_play_index = self.voice_play_index.wrapping_add(1);
        let (start, num) = (self.presets[preset_index].region_start, self.presets[preset_index].region_num);
        let sr = self.out_sample_rate;

        for ri in start..start + num {
            let r = self.regions[ri];
            if key < i32::from(r.lokey) || key > i32::from(r.hikey) || mid_vel < i16::from(r.lovel) || mid_vel > i16::from(r.hivel) {
                continue;
            }

            // A zone in an exclusive group ends the group's other voices.
            let mut voice = None;
            if r.group != 0 {
                for i in 0..self.voices.len() {
                    let (playing, vr) = (self.voices[i].playing_preset, self.voices[i].region);
                    if playing == pi && self.regions[vr].group == r.group {
                        self.voice_end_quick(i);
                    } else if playing == -1 && voice.is_none() {
                        voice = Some(i);
                    }
                }
            } else {
                voice = self.voices.iter().position(|v| v.playing_preset == -1);
            }

            let voice_index = match voice {
                Some(i) => i,
                None if self.max_voice_num > 0 => {
                    // All voices are taken: steal the one furthest into its release.
                    let mut best_pos: i32 = -999_999_999;
                    let mut best = None;
                    for (i, v) in self.voices.iter().enumerate() {
                        if v.ampenv.segment == SEG_RELEASE {
                            let done = release_samples(&v.ampenv, sr) - v.ampenv.samples_until_next_segment;
                            if done > best_pos {
                                best_pos = done;
                                best = Some(i);
                            }
                        }
                    }
                    match best {
                        None => continue,
                        Some(i) => {
                            self.voices[i].playing_preset = -1;
                            i
                        }
                    }
                }
                None => {
                    // Grow the voice list by four, so a new note never has to steal one.
                    let old = self.voices.len();
                    self.voices.extend(std::iter::repeat_n(Voice::default(), 4));
                    old
                }
            };

            let channel = self.channels.as_ref().map(|c| (c.list[c.active], c.active));
            let v = &mut self.voices[voice_index];
            v.region = ri;
            v.playing_preset = pi;
            v.playing_key = key;
            v.play_index = play_index;
            v.held_sustain = 0;
            v.note_gain_db = self.global_gain_db - r.attenuation - gain_to_decibels(1.0 / vel);
            match channel {
                Some((c, active)) => channel_setup_voice(v, &r, &c, active, sr),
                None => {
                    calc_pitch_ratio(v, &r, 0.0, sr);
                    let (l, rt) = pan_factors(r.pan);
                    v.pan_factor_left = l;
                    v.pan_factor_right = rt;
                }
            }

            v.source_sample_position = f64::from(r.offset);
            let do_loop = r.loop_mode != LOOP_NONE && r.loop_start < r.loop_end;
            v.loop_start = if do_loop { r.loop_start } else { 0 };
            v.loop_end = if do_loop { r.loop_end } else { 0 };

            envelope_setup(&mut v.ampenv, &r.ampenv, key, mid_vel, true, sr);
            envelope_setup(&mut v.modenv, &r.modenv, key, mid_vel, false, sr);

            let lowpass_fc = if r.initial_filter_fc <= 13500 { cents_to_hertz(r.initial_filter_fc as f32) / sr } else { 1.0 };
            let lowpass_filter_q_db = r.initial_filter_q as f32 / 10.0;
            v.lowpass.q_inv = 1.0 / 10.0f64.powf(f64::from(lowpass_filter_q_db) / 20.0);
            v.lowpass.z1 = 0.0;
            v.lowpass.z2 = 0.0;
            v.lowpass.active = lowpass_fc < 0.499;
            if v.lowpass.active {
                lowpass_setup(&mut v.lowpass, lowpass_fc);
            }

            lfo_setup(&mut v.modlfo, r.delay_mod_lfo, r.freq_mod_lfo, sr);
            lfo_setup(&mut v.viblfo, r.delay_vib_lfo, r.freq_vib_lfo, sr);
        }
        true
    }

    /// Releases the first-started voices for a preset and key (tsf_note_off).
    pub fn note_off(&mut self, preset_index: usize, key: i32) {
        let pi = preset_index as i32;
        let (first, last) = match self.match_range(|v| v.playing_preset == pi && v.playing_key == key) {
            Some(range) => range,
            None => return,
        };
        for i in first..=last {
            let (f, cur) = (&self.voices[first], &self.voices[i]);
            if i != first && i != last
                && (cur.play_index != f.play_index || cur.playing_preset != pi || cur.playing_key != key || cur.ampenv.segment >= SEG_RELEASE)
            {
                continue;
            }
            self.voice_end(i);
        }
    }

    /// The first and last voice (by index) matching `pred` and not yet released, that have the smallest play index.
    fn match_range(&self, pred: impl Fn(&Voice) -> bool) -> Option<(usize, usize)> {
        let mut first: Option<usize> = None;
        let mut last: Option<usize> = None;
        for (i, v) in self.voices.iter().enumerate() {
            if !pred(v) || v.ampenv.segment >= SEG_RELEASE {
                continue;
            }
            match first {
                None => {
                    first = Some(i);
                    last = Some(i);
                }
                Some(f) if v.play_index < self.voices[f].play_index => {
                    first = Some(i);
                    last = Some(i);
                }
                Some(f) if v.play_index == self.voices[f].play_index => last = Some(i),
                _ => {}
            }
        }
        Some((first?, last?))
    }

    // -- channels -------------------------------------------------------------

    /// Makes sure channel `ch` exists, creating the channels up to it with their defaults (tsf_channel_init).
    fn channel_init(&mut self, ch: usize) -> bool {
        let chs = self.channels.get_or_insert_with(|| Channels { list: Vec::new(), active: 0 });
        while chs.list.len() <= ch {
            chs.list.push(Channel::new());
        }
        true
    }

    fn channel_mut(&mut self, ch: usize) -> &mut Channel {
        &mut self.channels.as_mut().expect("channels exist after channel_init").list[ch]
    }

    fn channel_copy(&self, ch: usize) -> Option<Channel> {
        self.channels.as_ref().and_then(|c| c.list.get(ch).copied())
    }

    fn channel_apply_pitch(&mut self, ch: usize) {
        let Some(c) = self.channel_copy(ch) else { return };
        let shift = channel_pitch_shift(&c);
        let sr = self.out_sample_rate;
        let Tsf { voices, regions, .. } = self;
        for v in voices.iter_mut() {
            if v.playing_preset != -1 && v.playing_channel == ch as i32 {
                let r = regions[v.region];
                calc_pitch_ratio(v, &r, shift, sr);
            }
        }
    }

    /// Chooses the preset a channel plays by number, with the drum fallbacks (tsf_channel_set_presetnumber).
    pub fn channel_set_presetnumber(&mut self, ch: usize, number: i32, drums: bool) -> bool {
        self.channel_init(ch);
        let bank = i32::from(self.channel_copy(ch).map_or(0, |c| c.bank) & 0x7FFF);
        let mut index = if drums {
            self.preset_index(128 | bank, number)
                .or_else(|| self.preset_index(128, number))
                .or_else(|| self.preset_index(128, 0))
                .or_else(|| self.preset_index(bank, number))
        } else {
            self.preset_index(bank, number)
        };
        if index.is_none() {
            index = self.preset_index(0, number);
        }
        match index {
            Some(i) => {
                self.channel_mut(ch).preset_index = i as u16;
                true
            }
            None => false,
        }
    }

    pub fn channel_set_bank(&mut self, ch: usize, bank: i32) -> bool {
        self.channel_init(ch);
        self.channel_mut(ch).bank = bank as u16;
        true
    }

    pub fn channel_set_bank_preset(&mut self, ch: usize, bank: i32, number: i32) -> bool {
        self.channel_init(ch);
        let Some(index) = self.preset_index(bank, number) else { return false };
        let c = self.channel_mut(ch);
        c.preset_index = index as u16;
        c.bank = bank as u16;
        true
    }

    /// Sets a channel's pan, 0 (left) to 1 (right), and moves its sounding voices (tsf_channel_set_pan).
    pub fn channel_set_pan(&mut self, ch: usize, pan: f32) -> bool {
        self.channel_init(ch);
        let Tsf { voices, regions, .. } = self;
        for v in voices.iter_mut() {
            if v.playing_preset != -1 && v.playing_channel == ch as i32 {
                let r = regions[v.region];
                let newpan = r.pan + pan - 0.5;
                let (l, rt) = pan_factors(newpan);
                v.pan_factor_left = l;
                v.pan_factor_right = rt;
            }
        }
        self.channel_mut(ch).pan_offset = pan - 0.5;
        true
    }

    /// Sets a channel's volume as a linear gain (tsf_channel_set_volume).
    pub fn channel_set_volume(&mut self, ch: usize, volume: f32) -> bool {
        self.channel_init(ch);
        let gain_db = gain_to_decibels(volume);
        let current = self.channel_mut(ch).gain_db;
        if gain_db == current {
            return true;
        }
        let change = gain_db - current;
        for v in self.voices.iter_mut() {
            if v.playing_preset != -1 && v.playing_channel == ch as i32 {
                v.note_gain_db += change;
            }
        }
        self.channel_mut(ch).gain_db = gain_db;
        true
    }

    pub fn channel_set_pitchwheel(&mut self, ch: usize, pitch_wheel: i32) -> bool {
        self.channel_init(ch);
        if i32::from(self.channel_mut(ch).pitch_wheel) == pitch_wheel {
            return true;
        }
        self.channel_mut(ch).pitch_wheel = pitch_wheel as u16;
        self.channel_apply_pitch(ch);
        true
    }

    pub fn channel_set_pitchrange(&mut self, ch: usize, pitch_range: f32) -> bool {
        self.channel_init(ch);
        if self.channel_mut(ch).pitch_range == pitch_range {
            return true;
        }
        self.channel_mut(ch).pitch_range = pitch_range;
        if self.channel_mut(ch).pitch_wheel != 8192 {
            self.channel_apply_pitch(ch);
        }
        true
    }

    pub fn channel_set_tuning(&mut self, ch: usize, tuning: f32) -> bool {
        self.channel_init(ch);
        if self.channel_mut(ch).tuning == tuning {
            return true;
        }
        self.channel_mut(ch).tuning = tuning;
        self.channel_apply_pitch(ch);
        true
    }

    /// Sustain pedal on or off. Releasing it ends the voices the pedal was holding (tsf_channel_set_sustain).
    pub fn channel_set_sustain(&mut self, ch: usize, on: bool) -> bool {
        self.channel_init(ch);
        if self.channel_mut(ch).sustain == on {
            return true;
        }
        self.channel_mut(ch).sustain = on;
        if on {
            return true;
        }
        for i in 0..self.voices.len() {
            let v = &self.voices[i];
            if v.playing_preset != -1 && v.playing_channel == ch as i32 && v.ampenv.segment < SEG_RELEASE && v.held_sustain != 0 {
                self.voice_end(i);
            }
        }
        true
    }

    /// Plays a note on a channel with its current preset (tsf_channel_note_on).
    pub fn channel_note_on(&mut self, ch: usize, key: i32, vel: f32) -> bool {
        let Some(chs) = self.channels.as_mut() else { return true };
        if ch >= chs.list.len() {
            return true;
        }
        chs.active = ch;
        if vel == 0.0 {
            self.channel_note_off(ch, key);
            return true;
        }
        let preset = self.channels.as_ref().map_or(0, |c| c.list[ch].preset_index as usize);
        self.note_on(preset, key, vel)
    }

    /// Releases the note on a channel. With the sustain pedal down the voice is only marked (tsf_channel_note_off).
    pub fn channel_note_off(&mut self, ch: usize, key: i32) {
        let Some(sustain) = self.channel_copy(ch).map(|c| c.sustain) else { return };
        let ch_i = ch as i32;
        let Some((first, last)) = self.match_range(|v| v.playing_preset != -1 && v.playing_channel == ch_i && v.playing_key == key && v.held_sustain == 0) else {
            return;
        };
        for i in first..=last {
            let (f, cur) = (&self.voices[first], &self.voices[i]);
            if i != first && i != last
                && (cur.play_index != f.play_index || cur.playing_preset == -1 || cur.playing_channel != ch_i || cur.playing_key != key || cur.ampenv.segment >= SEG_RELEASE)
            {
                continue;
            }
            if sustain {
                self.voices[i].held_sustain = 1;
            } else {
                self.voice_end(i);
            }
        }
    }

    pub fn channel_note_off_all(&mut self, ch: usize) {
        for i in 0..self.voices.len() {
            let v = &self.voices[i];
            if v.playing_preset != -1 && v.playing_channel == ch as i32 && v.ampenv.segment < SEG_RELEASE {
                self.voice_end(i);
            }
        }
    }

    pub fn channel_sounds_off_all(&mut self, ch: usize) {
        for i in 0..self.voices.len() {
            let v = &self.voices[i];
            if v.playing_preset != -1 && v.playing_channel == ch as i32 && (v.ampenv.segment < SEG_RELEASE || v.ampenv.parameters.release != 0.0) {
                self.voice_end_quick(i);
            }
        }
    }

    /// A MIDI controller change on a channel (tsf_channel_midi_control).
    pub fn channel_midi_control(&mut self, ch: usize, controller: i32, value: i32) -> bool {
        self.channel_init(ch);
        enum Next {
            None,
            Volume,
            Pan,
            Data,
            Sustain(bool),
            SoundsOff,
            NotesOff,
            ControllersOff,
        }
        let next = {
            let c = self.channel_mut(ch);
            match controller {
                7 => { c.mid_volume = ((i32::from(c.mid_volume) & 0x7F) | (value << 7)) as u16; Next::Volume }
                39 => { c.mid_volume = ((i32::from(c.mid_volume) & 0x3F80) | value) as u16; Next::Volume }
                11 => { c.mid_expression = ((i32::from(c.mid_expression) & 0x7F) | (value << 7)) as u16; Next::Volume }
                43 => { c.mid_expression = ((i32::from(c.mid_expression) & 0x3F80) | value) as u16; Next::Volume }
                10 => { c.mid_pan = ((i32::from(c.mid_pan) & 0x7F) | (value << 7)) as u16; Next::Pan }
                42 => { c.mid_pan = ((i32::from(c.mid_pan) & 0x3F80) | value) as u16; Next::Pan }
                6 => { c.mid_data = ((i32::from(c.mid_data) & 0x7F) | (value << 7)) as u16; Next::Data }
                38 => { c.mid_data = ((i32::from(c.mid_data) & 0x3F80) | value) as u16; Next::Data }
                0 => { c.bank = (0x8000 | value) as u16; Next::None }
                32 => {
                    let high = if c.bank & 0x8000 != 0 { (i32::from(c.bank) & 0x7F) << 7 } else { 0 };
                    c.bank = (high | value) as u16;
                    Next::None
                }
                101 => {
                    let rpn = if c.mid_rpn == 0xFFFF { 0 } else { i32::from(c.mid_rpn) };
                    c.mid_rpn = ((rpn & 0x7F) | (value << 7)) as u16;
                    Next::None
                }
                100 => {
                    let rpn = if c.mid_rpn == 0xFFFF { 0 } else { i32::from(c.mid_rpn) };
                    c.mid_rpn = ((rpn & 0x3F80) | value) as u16;
                    Next::None
                }
                98 | 99 => {
                    c.mid_rpn = 0xFFFF;
                    Next::None
                }
                64 => Next::Sustain(value >= 64),
                120 => Next::SoundsOff,
                123 => Next::NotesOff,
                121 => {
                    c.mid_volume = 16383;
                    c.mid_expression = 16383;
                    c.mid_pan = 8192;
                    c.bank = 0;
                    c.mid_rpn = 0xFFFF;
                    c.mid_data = 0;
                    Next::ControllersOff
                }
                _ => Next::None,
            }
        };
        match next {
            Next::None => {}
            Next::Volume => {
                let c = self.channel_copy(ch).expect("channel exists");
                let level = (f32::from(c.mid_volume) / 16383.0) * (f32::from(c.mid_expression) / 16383.0);
                self.channel_set_volume(ch, level.powf(3.0));
            }
            Next::Pan => {
                let c = self.channel_copy(ch).expect("channel exists");
                self.channel_set_pan(ch, f32::from(c.mid_pan) / 16383.0);
            }
            Next::Data => {
                let c = self.channel_copy(ch).expect("channel exists");
                if c.mid_rpn == 0 {
                    let range = (c.mid_data >> 7) as f32 + 0.01 * f32::from(c.mid_data & 0x7F);
                    self.channel_set_pitchrange(ch, range);
                } else if c.mid_rpn == 1 {
                    let tuning = (c.tuning as i32) as f32 + (f32::from(c.mid_data) - 8192.0) / 8192.0;
                    self.channel_set_tuning(ch, tuning);
                } else if c.mid_rpn == 2 && controller == 6 {
                    let tuning = (value as f32 - 64.0) + (c.tuning - (c.tuning as i32) as f32);
                    self.channel_set_tuning(ch, tuning);
                }
            }
            Next::Sustain(on) => {
                self.channel_set_sustain(ch, on);
            }
            Next::SoundsOff => self.channel_sounds_off_all(ch),
            Next::NotesOff => self.channel_note_off_all(ch),
            Next::ControllersOff => {
                self.channel_set_volume(ch, 1.0);
                self.channel_set_pan(ch, 0.5);
                self.channel_set_pitchrange(ch, 2.0);
                self.channel_set_tuning(ch, 0.0);
            }
        }
        true
    }

    /// The preset index a channel plays (0 if the channel does not exist).
    pub fn channel_get_preset_index(&self, ch: usize) -> usize {
        self.channel_copy(ch).map_or(0, |c| c.preset_index as usize)
    }

    /// The bank a channel plays from, without the "drum" flag bit.
    pub fn channel_get_preset_bank(&self, ch: usize) -> i32 {
        self.channel_copy(ch).map_or(0, |c| i32::from(c.bank & 0x7FFF))
    }

    /// The preset number a channel plays.
    pub fn channel_get_preset_number(&self, ch: usize) -> i32 {
        self.channel_copy(ch).map_or(0, |c| i32::from(self.presets[c.preset_index as usize].preset))
    }

    /// The channel's volume as a linear gain.
    pub fn channel_get_volume(&self, ch: usize) -> f32 {
        self.channel_copy(ch).map_or(1.0, |c| decibels_to_gain(c.gain_db))
    }
}
