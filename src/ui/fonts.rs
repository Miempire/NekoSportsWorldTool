//! 中文字体安装。
//!
//! 启动时按平台加载系统中文字体并注入 egui：Windows 优先微软雅黑（msyh.ttc），
//! 失败再试 simhei / simsun；macOS 优先 PingFang.ttc，失败再试 Hiragino Sans GB /
//! Songti / Arial Unicode；Linux 试常见 Noto CJK、文泉驿路径。
//! 全部失败返回 None（不崩溃，界面中文会显示为方块但不影响功能）。

#[cfg(not(target_os = "android"))]
use std::path::PathBuf;

#[cfg(target_os = "android")]
const ANDROID_FONT: &[u8] = include_bytes!("../../assets/fonts/NotoSansSC-VF.ttf");

/// Windows 候选：系统字体目录下的常见简体中文字体。
#[cfg(target_os = "windows")]
fn candidates() -> Vec<PathBuf> {
    let dir = std::env::var("WINDIR")
        .map(|w| PathBuf::from(w).join("Fonts"))
        .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\Fonts"));
    ["msyh.ttc", "msyh.ttf", "simhei.ttf", "simsun.ttc"]
        .into_iter()
        .map(|n| dir.join(n))
        .collect()
}

/// macOS 候选：PingFang（10.11+ 自带）、冬青黑体、宋体、Arial Unicode。
/// 均为系统只读文件，TTC 取 index 0（PingFang.ttc[0] = PingFang SC）。
#[cfg(target_os = "macos")]
fn candidates() -> Vec<PathBuf> {
    [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// 其他桌面平台（Linux 等）候选：常见 Noto CJK 与文泉驿。
#[cfg(all(
    not(target_os = "android"),
    not(target_os = "windows"),
    not(target_os = "macos")
))]
fn candidates() -> Vec<PathBuf> {
    [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// 安装中文字体到 egui。返回实际加载的字体名（None = 全部失败）。
pub fn install(ctx: &egui::Context) -> Option<String> {
    #[cfg(target_os = "android")]
    {
        let name = "Noto Sans SC".to_owned();
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert(name.clone(), egui::FontData::from_static(ANDROID_FONT));
        if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            list.insert(0, name.clone());
        }
        if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            list.push(name.clone());
        }
        ctx.set_fonts(fonts);
        return Some(name);
    }

    #[cfg(not(target_os = "android"))]
    {
        for path in candidates() {
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "cjk".to_owned());
            let mut fonts = egui::FontDefinitions::default();
            // ttc 集合取 index 0（PingFang.ttc[0] = PingFang SC，msyh.ttc[0] = 微软雅黑）
            fonts
                .font_data
                .insert(name.clone(), egui::FontData::from_owned(data));
            // proportional：插到最前（中文优先由系统字体渲染）
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                list.insert(0, name.clone());
            }
            // monospace：追加（数字/ASCII 仍用默认等宽，中文回落系统字体）
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                list.push(name.clone());
            }
            ctx.set_fonts(fonts);
            return Some(name);
        }
        None
    }
}
