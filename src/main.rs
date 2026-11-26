#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod lowimpact;
mod settings;
mod worker;

use eframe::egui;
use fs2::FileExt;

fn acquire_instance_lock() -> Option<std::fs::File> {
    let path = settings::data_dir().join("instance.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .ok()?;
    match file.try_lock_exclusive() {
        Ok(()) => Some(file),
        Err(_) => None,
    }
}

fn main() -> eframe::Result {
    std::panic::set_hook(Box::new(|info| {
        let path = settings::logs_dir().join("panic.log");
        let _ = std::fs::write(path, format!("{info}\n"));
    }));

    let Some(_instance_lock) = acquire_instance_lock() else {
        rfd::MessageDialog::new()
            .set_title("Unkn0WnTorrent")
            .set_description("Unkn0WnTorrent уже запущен. Закройте другое окно программы.")
            .set_level(rfd::MessageLevel::Warning)
            .show();
        return Ok(());
    };

    let settings = settings::Settings::load();
    // Щадящий режим включаем сразу: движок не должен мешать звуку и играм.
    lowimpact::apply(lowimpact::desired(settings.low_impact, false));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let shared = worker::start(settings.clone(), rx);

    let icon = egui::IconData {
        rgba: include_bytes!("../assets/icon_64.rgba").to_vec(),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Unkn0WnTorrent")
            .with_icon(std::sync::Arc::new(icon))
            .with_inner_size([1054.0, 532.0])
            .with_min_inner_size([940.0, 480.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    let result = eframe::run_native(
        "Unkn0WnTorrent",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, settings, tx, shared)))),
    );
    let _ = std::fs::write(
        settings::logs_dir().join("exit.log"),
        match &result {
            Ok(()) => "run_native returned Ok".to_string(),
            Err(e) => format!("run_native error: {e}"),
        },
    );
    result
}
