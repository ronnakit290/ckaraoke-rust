//! CKaraoke desktop app. `ckaraoke-gui [--sf2 <file>] [--folder <folder>] [--play <song>] [--mixer] [--queue]`.

mod app;
mod drums;
mod library;
mod queue;
mod search;
mod settings;
mod soundfonts;
mod view;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let mut startup = app::Startup::default();
    let mut show_mixer = false;
    let mut show_queue = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sf2" => startup.soundfont = args.next().map(PathBuf::from),
            "--folder" => startup.folder = args.next().map(PathBuf::from),
            "--play" => startup.play = args.next().map(PathBuf::from),
            "--mixer" => show_mixer = true,
            "--queue" => show_queue = true,
            _ => {}
        }
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("CKaraoke")
            .with_inner_size([1180.0, 760.0])
            .with_fullscreen(true)
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CKaraoke",
        options,
        Box::new(move |cc| {
            let mut app = app::App::new(cc, startup);
            app.show_panels(show_mixer, show_queue);
            Ok(Box::new(app))
        }),
    )
}
