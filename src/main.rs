#![windows_subsystem = "windows"]
//! LoL Map Studio: an editor and inspector for League of Legends map folders.

mod gui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1600.0, 960.0]).with_min_inner_size([900.0, 600.0]).with_title("LoL Map Studio"),
        depth_buffer: 24,
        ..Default::default()
    };
    eframe::run_native("LoL Map Studio", options, Box::new(|cc| Ok(Box::new(gui::App::new(cc)))))
}
