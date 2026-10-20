//! Renders a song while applying mixer and transport commands from a script, for comparison with the C++
//! player driven by the same script (rust/tools/crosscheck/harness_player.cpp).
//!
//!   cargo run --example player_script -- <font.sf2> <song.mid> <script.txt> <out.wav>
//!
//! Each script line is `<block> <command> [args]`; a command runs before the block with that number is
//! rendered. Blocks are 1024 frames, as in the app's WAV export.

use ckaraoke_core::player::Player;
use ckaraoke_core::song::load_song;

fn wav(samples: &[f32], frames_total: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i32;
        data.extend_from_slice(&(v as i16).to_le_bytes());
    }
    let _ = frames_total;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&44100u32.to_le_bytes());
    out.extend_from_slice(&(44100u32 * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

fn apply(p: &mut Player, cmd: &str, args: &[&str]) {
    let f = |i: usize| args[i].parse::<f64>().expect("number");
    match cmd {
        "mute" => p.set_channel_mute(f(0) as usize, f(1) != 0.0),
        "solo" => p.set_channel_solo(f(0) as usize, f(1) != 0.0),
        "gain" => p.set_channel_gain(f(0) as usize, f(1) as f32),
        "drumgain" => p.set_drum_gain(f(0) as usize, f(1) as f32),
        "drummute" => p.set_drum_mute(f(0) as usize, f(1) != 0.0),
        "transpose" => p.set_transpose(f(0) as i32),
        "speed" => p.set_speed(f(0)),
        "seek" => p.seek(f(0)),
        "pause" => p.toggle_pause(),
        "volume" => p.set_volume(f(0) as f32),
        "resetmixer" => p.reset_mixer(),
        "resetdrums" => p.reset_drums(),
        other => panic!("unknown command {other}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: player_script <font.sf2> <song.mid> <script.txt> <out.wav>");
        std::process::exit(2);
    }
    let font = std::fs::read(&args[1]).expect("read sf2");
    let mut player = Player::new();
    player.load_sound_font(0, &font, 44100).expect("load sf2");
    let song = load_song(&args[2], "").expect("load song");
    player.load_midi(&song.midi).expect("load midi");

    let script = std::fs::read_to_string(&args[3]).expect("read script");
    let commands: Vec<(usize, String, Vec<String>)> = script
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            let block: usize = it.next().unwrap().parse().unwrap();
            let cmd = it.next().unwrap().to_string();
            (block, cmd, it.map(str::to_string).collect())
        })
        .collect();

    let mut samples: Vec<f32> = Vec::new();
    let mut buf = vec![0.0f32; 1024 * 2];
    let mut block = 0usize;
    let mut frames = 0usize;
    while !player.finished() && block < 5000 {
        for (b, cmd, a) in &commands {
            if *b == block {
                let refs: Vec<&str> = a.iter().map(String::as_str).collect();
                apply(&mut player, cmd, &refs);
            }
        }
        player.render(&mut buf);
        samples.extend_from_slice(&buf);
        frames += 1024;
        block += 1;
    }
    std::fs::write(&args[4], wav(&samples, frames)).expect("write wav");
    println!("blocks={block} frames={frames}");
}
