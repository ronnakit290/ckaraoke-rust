//! The conversion helpers of tsf.h. Each keeps the C expression's float or double steps.

/// Timecents to seconds, in double precision (TSF_POW(2.0, tc / 1200.0)).
pub(crate) fn timecents_to_secs_d(timecents: f64) -> f64 {
    2.0f64.powf(timecents / 1200.0)
}

/// Timecents to seconds, in single precision (tsf_timecents2Secsf).
pub(crate) fn timecents_to_secs_f(timecents: f32) -> f32 {
    2.0f32.powf(timecents / 1200.0)
}

/// Absolute cents to Hertz, relative to MIDI key 0 (tsf_cents2Hertz).
pub(crate) fn cents_to_hertz(cents: f32) -> f32 {
    8.176f32 * 2.0f32.powf(cents / 1200.0)
}

/// Decibels to a linear gain; anything at or below -100 dB is silent (tsf_decibelsToGain).
pub(crate) fn decibels_to_gain(db: f32) -> f32 {
    if db > -100.0 {
        10.0f32.powf(db * 0.05)
    } else {
        0.0
    }
}

/// Linear gain to decibels (tsf_gainToDecibels).
pub(crate) fn gain_to_decibels(gain: f32) -> f32 {
    if gain <= 0.00001 {
        -100.0
    } else {
        (20.0 * f64::from(gain).log10()) as f32
    }
}
