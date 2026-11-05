//! Shared desktop and Android application.

mod api;
pub mod battery;
pub mod cli;
pub mod geetoken;
pub mod sensors;
mod crypto;
pub mod location;
pub mod platform;
pub mod template;
mod relaunch;
mod textlog;
mod track;
mod update;

#[cfg(feature = "gui")]
mod ui;

#[cfg(all(target_os = "android", feature = "android"))]
mod android;

#[cfg(feature = "gui")]
pub fn run_gui(options: eframe::NativeOptions) -> eframe::Result<()> {
    eframe::run_native(
        "Miempire's RUN TOOL",
        options,
        Box::new(|cc| {
            #[cfg(target_os = "android")]
            android::set_context(&cc.egui_ctx);
            Ok(Box::new(ui::App::new(cc)))
        }),
    )
}
