//! Miempire's RUN TOOL：无参数启动 GUI，带子命令进入 CLI（青龙等定时任务用）。
//! full 构建（默认）含 GUI + CLI；`--no-default-features` 仅 CLI。

use miempires_run_tool::cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        #[cfg(feature = "gui")]
        run_gui();
        #[cfg(not(feature = "gui"))]
        {
            eprintln!("当前为 CLI 构建，子命令用法：miempires-run-tool help");
            std::process::exit(1);
        }
    } else {
        let code = cli::main(args);
        std::process::exit(code);
    }
}

#[cfg(all(windows, feature = "gui"))]
fn free_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> i32;
    }
    unsafe {
        FreeConsole();
    }
}

#[cfg(not(all(windows, feature = "gui")))]
fn free_console() {}

#[cfg(feature = "gui")]
fn run_gui() {
    free_console();
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([880.0, 720.0])
        .with_title("Miempire's RUN TOOL");
    // 窗口/任务栏图标（PNG 内嵌，桌面端通用）。
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let result = miempires_run_tool::run_gui(options);
    if let Err(e) = result {
        eprintln!("GUI 启动失败: {e}");
        std::process::exit(1);
    }
}
