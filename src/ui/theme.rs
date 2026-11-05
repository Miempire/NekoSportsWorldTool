use eframe::egui;
use egui::Color32;

const BG: Color32 = Color32::from_rgb(8, 13, 22);
const PANEL: Color32 = Color32::from_rgb(11, 18, 31);
const HEADER_BG: Color32 = Color32::from_rgb(9, 16, 28);
const WINDOW: Color32 = Color32::from_rgb(13, 21, 35);
const WIDGET: Color32 = Color32::from_rgb(17, 28, 46);
const WIDGET_HOVER: Color32 = Color32::from_rgb(24, 40, 63);
const BORDER: Color32 = Color32::from_rgb(30, 58, 95);
const BORDER_STRONG: Color32 = Color32::from_rgb(46, 111, 163);
const ACCENT: Color32 = Color32::from_rgb(0, 196, 255);
const ACCENT_BRIGHT: Color32 = Color32::from_rgb(63, 220, 255);
const ACCENT_DIM: Color32 = Color32::from_rgb(9, 96, 128);
const TEXT: Color32 = Color32::from_rgb(223, 235, 250);
const TEXT_DIM: Color32 = Color32::from_rgb(122, 145, 175);
const TEXT_FAINT: Color32 = Color32::from_rgb(78, 100, 130);
const TAB_IDLE: Color32 = Color32::from_rgb(13, 23, 38);
const TAB_ACTIVE: Color32 = Color32::from_rgb(17, 51, 78);

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
    ACCENT
}
pub fn accent_dim() -> Color32 {
    ACCENT_DIM
}
pub fn text_dim() -> Color32 {
    TEXT_DIM
}

pub fn plain() -> Color32 {
    Color32::from_rgb(196, 214, 232)
}
pub fn text() -> Color32 {
    TEXT
}

pub fn map_bg() -> Color32 {
    Color32::from_rgb(6, 11, 18)
}
pub fn map_road() -> Color32 {
    Color32::from_rgb(58, 103, 143)
}
pub fn map_building() -> Color32 {
    Color32::from_rgb(30, 60, 92)
}
pub fn map_route() -> Color32 {
    Color32::from_rgb(38, 198, 255)
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
    Color32::from_rgb(143, 179, 217)
}
pub fn map_muted() -> Color32 {
    Color32::from_rgb(91, 117, 149)
}

pub fn primary_btn(text: &str) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(text.to_owned())
            .color(Color32::from_rgb(4, 18, 30))
            .strong(),
    )
    .fill(ACCENT_BRIGHT)
    .stroke(egui::Stroke::new(1.0_f32, ACCENT))
    .rounding(egui::Rounding::same(4.0))
}

pub fn tab_button(selected: bool, label: &str) -> egui::Button<'static> {
    let text = if selected {
        egui::RichText::new(label.to_owned())
            .color(ACCENT_BRIGHT)
            .strong()
    } else {
        egui::RichText::new(label.to_owned()).color(TEXT_DIM)
    };
    egui::Button::new(text)
        .fill(if selected { TAB_ACTIVE } else { TAB_IDLE })
        .stroke(egui::Stroke::new(
            1.0_f32,
            if selected { ACCENT } else { BORDER },
        ))
        .rounding(egui::Rounding::same(3.0))
}

pub fn top_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(HEADER_BG)
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
}

pub fn status_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(HEADER_BG)
        .inner_margin(egui::Margin::symmetric(12.0, 2.0))
}

pub fn brand(ui: &mut egui::Ui, online: bool) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("MIEMPIRE'S RUN TOOL")
                .color(ACCENT_BRIGHT)
                .strong()
                .size(16.0),
        );
        ui.label(
            egui::RichText::new("// RUN CONSOLE")
                .color(TEXT_FAINT)
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
        egui::Stroke::new(1.0_f32, BORDER_STRONG),
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
        egui::Stroke::new(1.0_f32, BORDER_STRONG),
    );
}

pub fn paint_corners(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let len = 12.0;
    let stroke = egui::Stroke::new(1.0_f32, ACCENT_DIM);
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
    let stroke = egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(64, 150, 210, 9));
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
        egui::Stroke::new(2.0_f32, ACCENT),
    );
}

pub fn accent_separator(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 3.0), egui::Sense::hover());
    let y = rect.center().y;
    let p = ui.painter();
    p.line_segment(
        [
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), y),
        ],
        egui::Stroke::new(1.0_f32, BORDER),
    );
    p.line_segment(
        [
            egui::pos2(rect.left(), y),
            egui::pos2((rect.left() + 96.0).min(rect.right()), y),
        ],
        egui::Stroke::new(1.0_f32, ACCENT),
    );
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();

    v.panel_fill = BG;
    v.window_fill = WINDOW;
    v.extreme_bg_color = Color32::from_rgb(7, 13, 23);
    v.faint_bg_color = Color32::from_rgb(14, 24, 40);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT_BRIGHT);
    v.window_stroke = egui::Stroke::new(1.0_f32, BORDER_STRONG);
    v.window_rounding = egui::Rounding::same(6.0);

    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_fill = WIDGET;
    v.widgets.inactive.weak_bg_fill = WIDGET;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT_DIM);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    v.widgets.hovered.bg_fill = WIDGET_HOVER;
    v.widgets.hovered.weak_bg_fill = WIDGET_HOVER;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.2_f32, TEXT);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.bg_fill = ACCENT_DIM;
    v.widgets.active.weak_bg_fill = ACCENT_DIM;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.3_f32, Color32::WHITE);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT_BRIGHT);
    v.widgets.open.bg_fill = WIDGET;
    v.widgets.open.weak_bg_fill = WIDGET;
    v.widgets.open.fg_stroke = egui::Stroke::new(1.2_f32, TEXT);
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
