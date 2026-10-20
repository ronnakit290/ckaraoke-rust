//! Instrument regions: the sound parameters of one zone, with the instrument and the preset merged in.
//! Port of the tsf_region_* functions in tsf.h. Generator values are stored as the SF2 file gives them
//! and converted when the zone is complete (`merge`), as the C code does.

use crate::math::timecents_to_secs_f;

pub(crate) const LOOP_NONE: i32 = 0;
pub(crate) const LOOP_CONTINUOUS: i32 = 1;
pub(crate) const LOOP_SUSTAIN: i32 = 2;

/// A volume or modulation envelope: times in seconds, levels as the synth uses them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Envelope {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub keynum_to_hold: f32,
    pub keynum_to_decay: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Region {
    pub loop_mode: i32,
    pub sample_rate: u32,
    pub lokey: u8,
    pub hikey: u8,
    pub lovel: u8,
    pub hivel: u8,
    pub group: u32,
    pub offset: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub transpose: i32,
    pub tune: i32,
    pub pitch_keycenter: i32,
    pub pitch_keytrack: i32,
    pub attenuation: f32,
    pub pan: f32,
    pub ampenv: Envelope,
    pub modenv: Envelope,
    pub initial_filter_q: i32,
    pub initial_filter_fc: i32,
    pub mod_env_to_pitch: i32,
    pub mod_env_to_filter_fc: i32,
    pub mod_lfo_to_filter_fc: i32,
    pub mod_lfo_to_volume: i32,
    pub delay_mod_lfo: f32,
    pub freq_mod_lfo: i32,
    pub mod_lfo_to_pitch: i32,
    pub delay_vib_lfo: f32,
    pub freq_vib_lfo: i32,
    pub vib_lfo_to_pitch: i32,
}

impl Region {
    /// tsf_region_clear. `for_relative` is set for the preset-level and global zones, which only add to others.
    pub(crate) fn cleared(for_relative: bool) -> Region {
        let mut r = Region { hikey: 127, hivel: 127, pitch_keycenter: 60, ..Default::default() };
        if for_relative {
            return r;
        }
        r.pitch_keytrack = 100;
        r.pitch_keycenter = -1;
        // SF2 defaults, in timecents.
        for env in [&mut r.ampenv, &mut r.modenv] {
            env.delay = -12000.0;
            env.attack = -12000.0;
            env.hold = -12000.0;
            env.decay = -12000.0;
            env.release = -12000.0;
        }
        r.initial_filter_fc = 13500;
        r.delay_mod_lfo = -12000.0;
        r.delay_vib_lfo = -12000.0;
        r
    }
}

/// The value of one generator: the file stores it as 16 bits, read as a signed number, a word,
/// or a key or velocity range (low byte, high byte).
#[derive(Clone, Copy)]
pub(crate) struct Amount(pub u16);

impl Amount {
    fn short(self) -> i32 {
        i32::from(self.0 as i16)
    }

    pub(crate) fn word(self) -> u16 {
        self.0
    }

    fn lo(self) -> u8 {
        self.0 as u8
    }

    fn hi(self) -> u8 {
        (self.0 >> 8) as u8
    }
}

/// Sets one generator on a zone (tsf_region_operator with an amount).
pub(crate) fn set(r: &mut Region, oper: u16, a: Amount) {
    let s = a.short();
    match oper {
        0 => r.offset = r.offset.wrapping_add(s as u32),
        1 => r.end = r.end.wrapping_add(s as u32),
        2 => r.loop_start = r.loop_start.wrapping_add(s as u32),
        3 => r.loop_end = r.loop_end.wrapping_add(s as u32),
        4 => r.offset = r.offset.wrapping_add((s << 15) as u32),
        5 => r.mod_lfo_to_pitch = s,
        6 => r.vib_lfo_to_pitch = s,
        7 => r.mod_env_to_pitch = s,
        8 => r.initial_filter_fc = s,
        9 => r.initial_filter_q = s,
        10 => r.mod_lfo_to_filter_fc = s,
        11 => r.mod_env_to_filter_fc = s,
        12 => r.end = r.end.wrapping_add((s << 15) as u32),
        13 => r.mod_lfo_to_volume = s,
        17 => r.pan = s as f32,
        21 => r.delay_mod_lfo = s as f32,
        22 => r.freq_mod_lfo = s,
        23 => r.delay_vib_lfo = s as f32,
        24 => r.freq_vib_lfo = s,
        25 => r.modenv.delay = s as f32,
        26 => r.modenv.attack = s as f32,
        27 => r.modenv.hold = s as f32,
        28 => r.modenv.decay = s as f32,
        29 => r.modenv.sustain = s as f32,
        30 => r.modenv.release = s as f32,
        31 => r.modenv.keynum_to_hold = s as f32,
        32 => r.modenv.keynum_to_decay = s as f32,
        33 => r.ampenv.delay = s as f32,
        34 => r.ampenv.attack = s as f32,
        35 => r.ampenv.hold = s as f32,
        36 => r.ampenv.decay = s as f32,
        37 => r.ampenv.sustain = s as f32,
        38 => r.ampenv.release = s as f32,
        39 => r.ampenv.keynum_to_hold = s as f32,
        40 => r.ampenv.keynum_to_decay = s as f32,
        43 => {
            r.lokey = a.lo();
            r.hikey = a.hi();
        }
        44 => {
            r.lovel = a.lo();
            r.hivel = a.hi();
        }
        45 => r.loop_start = r.loop_start.wrapping_add((s << 15) as u32),
        48 => r.attenuation = s as f32,
        50 => r.loop_end = r.loop_end.wrapping_add((s << 15) as u32),
        51 => r.transpose = s,
        52 => r.tune = s,
        54 => {
            r.loop_mode = match a.word() & 3 {
                3 => LOOP_SUSTAIN,
                1 => LOOP_CONTINUOUS,
                _ => LOOP_NONE,
            }
        }
        56 => r.pitch_keytrack = s,
        57 => r.group = u32::from(a.word()),
        58 => r.pitch_keycenter = s,
        _ => {} // unused, reserved, special (instrument, sample id) or unsupported (effects, modulators)
    }
}

/// `*val += add`, then the scale and the limits of the generator (the float case of tsf_region_operator's merge).
fn merge_float(val: &mut f32, add: f32, vfactor: f32, vmin: f32, vmax: f32) {
    *val += add;
    *val *= vfactor;
    if *val < vmin {
        *val = vmin;
    } else if *val > vmax {
        *val = vmax;
    }
}

fn merge_int(val: &mut i32, add: i32, limits: Option<(i32, i32)>) {
    *val = val.wrapping_add(add);
    if let Some((vmin, vmax)) = limits {
        if *val < vmin {
            *val = vmin;
        } else if *val > vmax {
            *val = vmax;
        }
    }
}

/// Adds the preset-level zone `m` into the zone `r`, and clamps the result (the merge branch of tsf_region_operator).
pub(crate) fn merge(r: &mut Region, m: &Region) {
    r.offset = r.offset.wrapping_add(m.offset);
    r.end = r.end.wrapping_add(m.end);
    r.loop_start = r.loop_start.wrapping_add(m.loop_start);
    r.loop_end = r.loop_end.wrapping_add(m.loop_end);
    merge_int(&mut r.mod_lfo_to_pitch, m.mod_lfo_to_pitch, Some((-12000, 12000)));
    merge_int(&mut r.vib_lfo_to_pitch, m.vib_lfo_to_pitch, Some((-12000, 12000)));
    merge_int(&mut r.mod_env_to_pitch, m.mod_env_to_pitch, Some((-12000, 12000)));
    merge_int(&mut r.initial_filter_fc, m.initial_filter_fc, Some((1500, 13500)));
    merge_int(&mut r.initial_filter_q, m.initial_filter_q, Some((0, 960)));
    merge_int(&mut r.mod_lfo_to_filter_fc, m.mod_lfo_to_filter_fc, Some((-12000, 12000)));
    merge_int(&mut r.mod_env_to_filter_fc, m.mod_env_to_filter_fc, Some((-12000, 12000)));
    merge_int(&mut r.mod_lfo_to_volume, m.mod_lfo_to_volume, Some((-960, 960)));
    merge_float(&mut r.pan, m.pan, 0.001, -0.5, 0.5);
    merge_float(&mut r.delay_mod_lfo, m.delay_mod_lfo, 1.0, -12000.0, 5000.0);
    merge_int(&mut r.freq_mod_lfo, m.freq_mod_lfo, Some((-16000, 4500)));
    merge_float(&mut r.delay_vib_lfo, m.delay_vib_lfo, 1.0, -12000.0, 5000.0);
    merge_int(&mut r.freq_vib_lfo, m.freq_vib_lfo, Some((-16000, 4500)));

    let (me, ze) = (&m.modenv, &mut r.modenv);
    merge_float(&mut ze.delay, me.delay, 1.0, -12000.0, 5000.0);
    merge_float(&mut ze.attack, me.attack, 1.0, -12000.0, 8000.0);
    merge_float(&mut ze.hold, me.hold, 1.0, -12000.0, 5000.0);
    merge_float(&mut ze.decay, me.decay, 1.0, -12000.0, 8000.0);
    merge_float(&mut ze.sustain, me.sustain, 1.0, 0.0, 1000.0);
    merge_float(&mut ze.release, me.release, 1.0, -12000.0, 8000.0);
    merge_float(&mut ze.keynum_to_hold, me.keynum_to_hold, 1.0, -1200.0, 1200.0);
    merge_float(&mut ze.keynum_to_decay, me.keynum_to_decay, 1.0, -1200.0, 1200.0);

    let (ae, za) = (&m.ampenv, &mut r.ampenv);
    merge_float(&mut za.delay, ae.delay, 1.0, -12000.0, 5000.0);
    merge_float(&mut za.attack, ae.attack, 1.0, -12000.0, 8000.0);
    merge_float(&mut za.hold, ae.hold, 1.0, -12000.0, 5000.0);
    merge_float(&mut za.decay, ae.decay, 1.0, -12000.0, 8000.0);
    merge_float(&mut za.sustain, ae.sustain, 1.0, 0.0, 1440.0);
    merge_float(&mut za.release, ae.release, 1.0, -12000.0, 8000.0);
    merge_float(&mut za.keynum_to_hold, ae.keynum_to_hold, 1.0, -1200.0, 1200.0);
    merge_float(&mut za.keynum_to_decay, ae.keynum_to_decay, 1.0, -1200.0, 1200.0);

    merge_float(&mut r.attenuation, m.attenuation, 0.01, 0.0, 14.4);
    merge_int(&mut r.transpose, m.transpose, None);
    merge_int(&mut r.tune, m.tune, None);
    merge_int(&mut r.pitch_keytrack, m.pitch_keytrack, None);
}

/// Converts an envelope's times from timecents to seconds, and its sustain to a level (tsf_region_envtosecs).
pub(crate) fn envelope_to_secs(p: &mut Envelope, sustain_is_gain: bool) {
    // Pin very short segments: timecents never reach zero, and the envelope is happier with zero.
    p.delay = if p.delay < -11950.0 { 0.0 } else { timecents_to_secs_f(p.delay) };
    p.attack = if p.attack < -11950.0 { 0.0 } else { timecents_to_secs_f(p.attack) };
    p.release = if p.release < -11950.0 { 0.0 } else { timecents_to_secs_f(p.release) };
    // Hold and decay that depend on the key stay in timecents until the note starts.
    if p.keynum_to_hold == 0.0 {
        p.hold = if p.hold < -11950.0 { 0.0 } else { timecents_to_secs_f(p.hold) };
    }
    if p.keynum_to_decay == 0.0 {
        p.decay = if p.decay < -11950.0 { 0.0 } else { timecents_to_secs_f(p.decay) };
    }
    if p.sustain < 0.0 {
        p.sustain = 0.0;
    } else if sustain_is_gain {
        p.sustain = crate::math::decibels_to_gain(-p.sustain / 10.0);
    } else {
        p.sustain = 1.0 - (p.sustain / 1000.0);
    }
}

/// A preset: a name, its bank and number, and its zones (`region_start..region_start + region_num`).
#[derive(Clone, Debug)]
pub(crate) struct Preset {
    pub name: String,
    pub bank: u16,
    pub preset: u16,
    pub region_start: usize,
    pub region_num: usize,
}
