use eframe::egui;
use egui::Color32;
use std::cell::Cell;

/// 调色板：Legacy = 登录门禁页原有蓝青科技风；Sakura = 主界面粉色二次元风。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    Legacy,
    Sakura,
}

#[derive(Clone, Copy)]
struct Colors {
    bg: Color32,
    panel: Color32,
    header_bg: Color32,
    window: Color32,
    widget: Color32,
    widget_hover: Color32,
    border: Color32,
    border_strong: Color32,
    accent: Color32,
    accent_bright: Color32,
    accent_dim: Color32,
    text: Color32,
    text_dim: Color32,
    text_faint: Color32,
    tab_idle: Color32,
    tab_active: Color32,
    extreme_bg: Color32,
    faint_bg: Color32,
    btn_text: Color32,
    grid_line: Color32,
    map_bg: Color32,
    map_road: Color32,
    map_building: Color32,
    map_route: Color32,
    map_label: Color32,
    map_muted: Color32,
}

const LEGACY: Colors = Colors {
    bg: Color32::from_rgb(8, 13, 22),
    panel: Color32::from_rgb(11, 18, 31),
    header_bg: Color32::from_rgb(9, 16, 28),
    window: Color32::from_rgb(13, 21, 35),
    widget: Color32::from_rgb(17, 28, 46),
    widget_hover: Color32::from_rgb(24, 40, 63),
    border: Color32::from_rgb(30, 58, 95),
    border_strong: Color32::from_rgb(46, 111, 163),
    accent: Color32::from_rgb(0, 196, 255),
    accent_bright: Color32::from_rgb(63, 220, 255),
    accent_dim: Color32::from_rgb(9, 96, 128),
    text: Color32::from_rgb(223, 235, 250),
    text_dim: Color32::from_rgb(122, 145, 175),
    text_faint: Color32::from_rgb(78, 100, 130),
    tab_idle: Color32::from_rgb(13, 23, 38),
    tab_active: Color32::from_rgb(17, 51, 78),
    extreme_bg: Color32::from_rgb(7, 13, 23),
    faint_bg: Color32::from_rgb(14, 24, 40),
    btn_text: Color32::from_rgb(4, 18, 30),
    grid_line: Color32::from_rgba_unmultiplied(64, 150, 210, 9),
    map_bg: Color32::from_rgb(6, 11, 18),
    map_road: Color32::from_rgb(58, 103, 143),
    map_building: Color32::from_rgb(30, 60, 92),
    map_route: Color32::from_rgb(38, 198, 255),
    map_label: Color32::from_rgb(143, 179, 217),
    map_muted: Color32::from_rgb(91, 117, 149),
};

/// 粉色二次元：深夜樱色底 + 樱花粉 accent，辅以品红/淡紫高光。
const SAKURA: Colors = Colors {
    bg: Color32::from_rgb(22, 13, 20),
    panel: Color32::from_rgb(29, 18, 27),
    header_bg: Color32::from_rgb(27, 16, 25),
    window: Color32::from_rgb(33, 21, 31),
    widget: Color32::from_rgb(42, 27, 40),
    widget_hover: Color32::from_rgb(58, 36, 54),
    border: Color32::from_rgb(94, 54, 84),
    border_strong: Color32::from_rgb(199, 108, 168),
    accent: Color32::from_rgb(255, 133, 197),
    accent_bright: Color32::from_rgb(255, 176, 218),
    accent_dim: Color32::from_rgb(130, 48, 94),
    text: Color32::from_rgb(252, 238, 246),
    text_dim: Color32::from_rgb(202, 162, 190),
    text_faint: Color32::from_rgb(146, 108, 135),
    tab_idle: Color32::from_rgb(35, 22, 33),
    tab_active: Color32::from_rgb(70, 33, 57),
    extreme_bg: Color32::from_rgb(19, 12, 18),
    faint_bg: Color32::from_rgb(36, 22, 34),
    btn_text: Color32::from_rgb(46, 10, 32),
    grid_line: Color32::from_rgba_unmultiplied(215, 125, 185, 9),
    map_bg: Color32::from_rgb(18, 10, 16),
    map_road: Color32::from_rgb(112, 70, 102),
    map_building: Color32::from_rgb(60, 38, 55),
    map_route: Color32::from_rgb(255, 122, 202),
    map_label: Color32::from_rgb(219, 152, 200),
    map_muted: Color32::from_rgb(150, 96, 134),
};

thread_local! {
    static CURRENT: Cell<Palette> = const { Cell::new(Palette::Legacy) };
}

pub fn palette() -> Palette {
    CURRENT.with(|c| c.get())
}

fn colors() -> Colors {
    match palette() {
        Palette::Legacy => LEGACY,
        Palette::Sakura => SAKURA,
    }
}

/// 功能色两套共用（语义色不随主题漂移）。
pub fn ok() -> Color32 {
    Color32::from_rgb(58, 211, 146)
}
pub fn warn() -> Color32 {
    Color32::from_rgb(255, 190, 70)
}
pub fn err() -> Color32 {
    Color32::from_rgb(255, 92, 115)
}
pub fn accent() -> Color32 {
    colors().accent
}
pub fn accent_dim() -> Color32 {
    colors().accent_dim
}
pub fn text_dim() -> Color32 {
    colors().text_dim
}
pub fn plain() -> Color32 {
    colors().text_dim
}
pub fn text() -> Color32 {
    colors().text
}

pub fn map_bg() -> Color32 {
    colors().map_bg
}
pub fn map_road() -> Color32 {
    colors().map_road
}
pub fn map_building() -> Color32 {
    colors().map_building
}
pub fn map_route() -> Color32 {
    colors().map_route
}
pub fn map_checkpoint() -> Color32 {
    Color32::from_rgb(255, 194, 77)
}
pub fn map_start() -> Color32 {
    Color32::from_rgb(55, 227, 155)
}
pub fn map_end() -> Color32 {
    Color32::from_rgb(255, 92, 122)
}
pub fn map_outline() -> Color32 {
    Color32::from_rgb(255, 154, 60)
}
pub fn map_fence() -> Color32 {
    Color32::from_rgb(255, 176, 64)
}
pub fn map_label() -> Color32 {
    colors().map_label
}
pub fn map_muted() -> Color32 {
    colors().map_muted
}

pub fn primary_btn(text: &str) -> egui::Button<'static> {
    let c = colors();
    egui::Button::new(
        egui::RichText::new(text.to_owned())
            .color(c.btn_text)
            .strong(),
    )
    .fill(c.accent_bright)
    .stroke(egui::Stroke::new(1.0_f32, c.accent))
    .rounding(egui::Rounding::same(4.0))
}

pub fn tab_button(selected: bool, label: &str) -> egui::Button<'static> {
    let c = colors();
    let text = if selected {
        egui::RichText::new(label.to_owned())
            .color(c.accent_bright)
            .strong()
    } else {
        egui::RichText::new(label.to_owned()).color(c.text_dim)
    };
    egui::Button::new(text)
        .fill(if selected { c.tab_active } else { c.tab_idle })
        .stroke(egui::Stroke::new(
            1.0_f32,
            if selected { c.accent } else { c.border },
        ))
        .rounding(egui::Rounding::same(3.0))
}

pub fn top_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(colors().header_bg)
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
}

pub fn status_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(colors().header_bg)
        .inner_margin(egui::Margin::symmetric(12.0, 2.0))
}

pub fn brand(ui: &mut egui::Ui, online: bool) {
    let c = colors();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("MIEMPIRE'S RUN TOOL")
                .color(c.accent_bright)
                .strong()
                .size(16.0),
        );
        ui.label(
            egui::RichText::new("// RUN CONSOLE")
                .color(c.text_faint)
                .size(10.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (dot_rect, _) =
                ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(
                dot_rect.center(),
                3.0,
                if online { ok() } else { warn() },
            );
            ui.label(
                egui::RichText::new(if online { "LINK ONLINE" } else { "STANDBY" })
                    .color(if online { ok() } else { warn() })
                    .size(10.0),
            );
        });
    });
}

pub fn paint_bottom_line(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let y = rect.bottom() - 0.5;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), y),
        ],
        egui::Stroke::new(1.0_f32, colors().border_strong),
    );
}

pub fn paint_top_line(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let y = rect.top() + 0.5;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), y),
        ],
        egui::Stroke::new(1.0_f32, colors().border_strong),
    );
}

pub fn paint_corners(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let len = 12.0;
    let stroke = egui::Stroke::new(1.0_f32, colors().accent_dim);
    let p = ui.painter();
    let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    for (x, y, dx, dy) in [
        (l, t, 1.0, 1.0),
        (r, t, -1.0, 1.0),
        (l, b, 1.0, -1.0),
        (r, b, -1.0, -1.0),
    ] {
        p.line_segment(
            [
                egui::pos2(x, y + dy * len),
                egui::pos2(x, y),
            ],
            stroke,
        );
        p.line_segment(
            [
                egui::pos2(x, y),
                egui::pos2(x + dx * len, y),
            ],
            stroke,
        );
    }
}

pub fn paint_grid(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let stroke = egui::Stroke::new(1.0_f32, colors().grid_line);
    let p = ui.painter();
    let step = 36.0;
    let mut x = rect.left() - rect.left().rem_euclid(step);
    while x < rect.right() {
        p.line_segment(
            [
                egui::pos2(x, rect.top()),
                egui::pos2(x, rect.bottom()),
            ],
            stroke,
        );
        x += step;
    }
    let mut y = rect.top() - rect.top().rem_euclid(step);
    while y < rect.bottom() {
        p.line_segment(
            [
                egui::pos2(rect.left(), y),
                egui::pos2(rect.right(), y),
            ],
            stroke,
        );
        y += step;
    }
}

pub fn paint_tab_accent(ui: &egui::Ui, rect: egui::Rect) {
    let y = rect.bottom() - 1.5;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + 4.0, y),
            egui::pos2(rect.right() - 4.0, y),
        ],
        egui::Stroke::new(2.0_f32, colors().accent),
    );
}

pub fn accent_separator(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 3.0), egui::Sense::hover());
    let y = rect.center().y;
    let p = ui.painter();
    let c = colors();
    p.line_segment(
        [
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), y),
        ],
        egui::Stroke::new(1.0_f32, c.border),
    );
    p.line_segment(
        [
            egui::pos2(rect.left(), y),
            egui::pos2((rect.left() + 96.0).min(rect.right()), y),
        ],
        egui::Stroke::new(1.0_f32, c.accent),
    );
}

pub fn apply(ctx: &egui::Context, palette: Palette) {
    CURRENT.with(|c| c.set(palette));
    let c = colors();
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();

    v.panel_fill = c.bg;
    v.window_fill = c.window;
    v.extreme_bg_color = c.extreme_bg;
    v.faint_bg_color = c.faint_bg;
    v.override_text_color = Some(c.text);
    v.hyperlink_color = c.accent;
    v.selection.bg_fill = c.accent_dim;
    v.selection.stroke = egui::Stroke::new(1.0_f32, c.accent_bright);
    v.window_stroke = egui::Stroke::new(1.0_f32, c.border_strong);
    v.window_rounding = egui::Rounding::same(6.0);

    v.widgets.noninteractive.bg_fill = c.panel;
    v.widgets.noninteractive.weak_bg_fill = c.panel;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, c.border);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, c.text);
    v.widgets.inactive.bg_fill = c.widget;
    v.widgets.inactive.weak_bg_fill = c.widget;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, c.text_dim);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, c.border);
    v.widgets.hovered.bg_fill = c.widget_hover;
    v.widgets.hovered.weak_bg_fill = c.widget_hover;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.2_f32, c.text);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, c.accent);
    v.widgets.active.bg_fill = c.accent_dim;
    v.widgets.active.weak_bg_fill = c.accent_dim;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.3_f32, Color32::WHITE);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, c.accent_bright);
    v.widgets.open.bg_fill = c.widget;
    v.widgets.open.weak_bg_fill = c.widget;
    v.widgets.open.fg_stroke = egui::Stroke::new(1.2_f32, c.text);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = egui::Rounding::same(4.0);
    }

    style.spacing.item_spacing = egui::vec2(10.0, 7.0);
    style.spacing.button_padding = egui::vec2(14.0, 6.0);
    style.spacing.menu_margin = egui::Margin::same(8.0);
    #[cfg(target_os = "android")]
    {
        style.spacing.interact_size = egui::vec2(44.0, 44.0);
        style.spacing.button_padding = egui::vec2(14.0, 10.0);
    }

    ctx.set_style(style);
}
