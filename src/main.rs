#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod git;
mod icon_art;

fn main() -> eframe::Result {
    install_panic_log();

    let icon = app_icon();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 600.0])
            .with_title("GitShow")
            .with_drag_and_drop(true)
            .with_icon(icon),
        centered: true,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "GitShow",
        options,
        Box::new(|cc| Ok(Box::new(app::GitShow::new(&cc.egui_ctx)))),
    )
}

fn app_icon() -> std::sync::Arc<eframe::egui::IconData> {
    let size = 64;
    std::sync::Arc::new(eframe::egui::IconData {
        rgba: icon_art::render(size),
        width: size,
        height: size,
    })
}

fn install_panic_log() {
    let dir = app::state_dir();
    std::panic::set_hook(Box::new(move |info| {
        let path = dir.join("panic.log");
        let backtrace = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(path, format!("{info}\n{backtrace}"));
    }));
}
