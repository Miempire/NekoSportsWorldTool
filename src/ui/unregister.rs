//! 强制注销页：短信验证码 + 二次确认。
//!
//! 链路与 CLI 的 `send-code` / `unregister` 一致：
//!   1) 发送验证码：GT4 校验 → /api/v75/users/check/code（type=5）
//!   2) 注销账号：/api/v61300/user/userLogout（body 只有 code）
//! 注销不可逆，页面必须走"点击一次 → 再确认"两步。

use super::{mobile, theme, App};
use eframe::egui;
use egui::RichText;

/// 后台任务完成消息：`__UNREG_DONE__{"ok":true,"kind":"code|delete","message":"…"}`
pub const UNREG_DONE: &str = "__UNREG_DONE__";

#[derive(Default)]
pub struct UnregisterPage {
    pub phone: String,
    pub code: String,
    pub busy: bool,
    /// 已点过一次“强制注销”，等待二次确认。
    pub armed: bool,
    /// 验证码是否已发送成功（仅用于提示）。
    pub sent: bool,
    /// (是否成功, 文案)
    pub message: Option<(bool, String)>,
}

pub enum UnregisterAction {
    None,
    SendCode,
    Delete,
}

impl UnregisterPage {
    pub fn draw(&mut self, ui: &mut egui::Ui, logged_in: bool) -> UnregisterAction {
        let mut action = UnregisterAction::None;
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    RichText::new("⚠ 强制注销账号（不可逆）")
                        .strong()
                        .color(theme::err()),
                );
                ui.label(
                    RichText::new(
                        "注销后该账号的跑步记录、学期数据无法恢复；验证码会发往账号绑定的手机号。",
                    )
                    .color(theme::text_dim()),
                );
                ui.separator();

                if !logged_in {
                    ui.label(
                        RichText::new("请先在「跑步」页登录要注销的账号（注销需要登录态）。")
                            .color(theme::warn()),
                    );
                }

                let width = if mobile::compact_ui(ui) {
                    ui.available_width().min(240.0)
                } else {
                    240.0
                };
                mobile::row(ui, |ui| {
                    ui.label("手机号");
                    mobile::text_edit(
                        ui,
                        "unreg_phone",
                        &mut self.phone,
                        crate::platform::InputKind::Text,
                        width,
                    );
                    if ui
                        .add_enabled(
                            !self.busy && !self.phone.trim().is_empty(),
                            theme::primary_btn("发送验证码"),
                        )
                        .clicked()
                    {
                        action = UnregisterAction::SendCode;
                    }
                });
                if self.sent {
                    ui.label(RichText::new("验证码已发送，请查收短信").color(theme::ok()));
                }

                mobile::row(ui, |ui| {
                    ui.label("验证码");
                    mobile::text_edit(
                        ui,
                        "unreg_code",
                        &mut self.code,
                        crate::platform::InputKind::Text,
                        160.0,
                    );
                });

                ui.add_space(8.0);
                let can_delete = logged_in && !self.busy && !self.code.trim().is_empty();
                if self.armed {
                    ui.label(
                        RichText::new("再次确认：注销后无法恢复，确定继续吗？")
                            .strong()
                            .color(theme::err()),
                    );
                    mobile::row(ui, |ui| {
                        if ui
                            .add_enabled(can_delete, theme::primary_btn("确认注销"))
                            .clicked()
                        {
                            self.armed = false;
                            action = UnregisterAction::Delete;
                        }
                        if ui.button("取消").clicked() {
                            self.armed = false;
                        }
                    });
                } else if ui
                    .add_enabled(can_delete, theme::primary_btn("强制注销"))
                    .clicked()
                {
                    self.armed = true;
                }

                if self.busy {
                    ui.add_space(4.0);
                    ui.label(RichText::new("处理中…").color(theme::text_dim()));
                }
                if let Some((ok, msg)) = &self.message {
                    ui.add_space(4.0);
                    let color = if *ok { theme::ok() } else { theme::err() };
                    ui.label(RichText::new(msg).color(color));
                }
            });
        action
    }
}

impl App {
    pub fn draw_unregister(&mut self, ui: &mut egui::Ui) {
        let logged_in = self.session.is_some();
        let action = self.unregister_page.draw(ui, logged_in);
        match action {
            UnregisterAction::SendCode => self.do_send_unregister_code(),
            UnregisterAction::Delete => self.do_unregister_account(),
            UnregisterAction::None => {}
        }
    }
}
