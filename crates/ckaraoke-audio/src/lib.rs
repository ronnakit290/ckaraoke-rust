//! Audio output for CKaraoke: the default output device, playing whatever a shared `Player` renders.
//!
//! The device's own sample rate is used, so the SoundFont is loaded at that rate. The audio callback
//! takes the player's lock for each block, as the C++ app does; the UI takes the same lock briefly.

use std::sync::{Arc, Mutex};

use ckaraoke_core::player::Player;
use ckaraoke_core::{Error, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct Output {
    // Dropping the stream stops the audio, so it lives as long as the output.
    _stream: cpal::Stream,
    sample_rate: u32,
    player: Arc<Mutex<Player>>,
}

impl Output {
    /// Opens the default output device and starts it. The player is silent until a SoundFont and a song are loaded.
    pub fn open() -> Result<Output> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| Error::new("ไม่พบอุปกรณ์เสียงออก"))?;
        let supported = device
            .default_output_config()
            .map_err(|e| Error::new(format!("อ่านค่าอุปกรณ์เสียงไม่ได้: {e}")))?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(Error::new("อุปกรณ์เสียงนี้ไม่ใช่รูปแบบ f32 (ยังไม่รองรับ)"));
        }
        let channels = supported.channels() as usize;
        let sample_rate = supported.sample_rate();
        let config: cpal::StreamConfig = supported.into();
        let player = Arc::new(Mutex::new(Player::new()));

        let audio_player = Arc::clone(&player);
        let mut scratch: Vec<f32> = Vec::new();
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [f32], _| {
                    let frames = out.len() / channels;
                    scratch.resize(frames * 2, 0.0);
                    match audio_player.lock() {
                        Ok(mut p) => p.render(&mut scratch),
                        Err(_) => scratch.fill(0.0),
                    }
                    for (f, frame) in out.chunks_mut(channels).enumerate() {
                        for (c, s) in frame.iter_mut().enumerate() {
                            *s = match c {
                                0 => scratch[2 * f],
                                1 => scratch[2 * f + 1],
                                _ => 0.0,
                            };
                        }
                    }
                },
                |err| eprintln!("เสียงขัดข้อง: {err}"),
                None,
            )
            .map_err(|e| Error::new(format!("เปิดอุปกรณ์เสียงไม่ได้: {e}")))?;
        stream.play().map_err(|e| Error::new(format!("เริ่มเสียงไม่ได้: {e}")))?;
        Ok(Output { _stream: stream, sample_rate, player })
    }

    /// The device's sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The player the audio thread renders. Lock it briefly to read state or send commands.
    pub fn player(&self) -> &Arc<Mutex<Player>> {
        &self.player
    }

    /// Loads a SoundFont into the main slot at the device's sample rate.
    pub fn load_sound_font(&self, data: &[u8]) -> Result<()> {
        let mut p = self.player.lock().map_err(|_| Error::new("เครื่องเล่นขัดข้อง"))?;
        p.load_sound_font(0, data, self.sample_rate as i32).map_err(Error::new)
    }
}
