//! A Rust port of TinySoundFont (`third_party/tsf.h`): a SoundFont 2 synthesizer.
//!
//! The port keeps the C code's order of float and double operations, so its output matches the C build
//! bit for bit when that build is compiled without fused multiply-add (`-ffp-contract=off`).
//! Only the parts CKaraoke uses are here: SF2 files with 16-bit samples, stereo interleaved output,
//! and the channel API. Ogg Vorbis (SF3, .sfo), modulators and mono/unweaved output are not supported.

mod font;
mod math;
mod region;
mod synth;

pub use synth::Tsf;
