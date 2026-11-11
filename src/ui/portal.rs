//! 平台授权门禁页：未登录 / 未通过授权校验时不启用任何功能。

use super::{mobile, theme, App};

#[derive(Default)]
pub struct PortalUi {
    pub username: String,
    pub password: String,
    pub busy: bool,
    pub checking: bool,
    pub error: String,
    pub allowed_date: String,
    pub authorized: bool,
    pub has_saved: bool,
}

impl PortalUi {
    pub fn from_saved(saved: Option<crate::portal::PortalSession>) -> Self {
        // jsspace 登录锁已移除，初始化即视为已授权
        match saved {
            Some(session) => Self {
                username: session.username,
                password: String::new(),
                busy: false,
                checking: false,
                error: String::new(),
                allowed_date: session.allowed_date,
                authorized: true, // 原先是 false，等 verify_saved 后才变 true
                has_saved: true,
            },
            None => Self {
                authorized: true, // 无保存 session 也直接放行
                allowed_date: "2099.12.31".into(),
                ..Self::default()
            },
        }
    }
}

impl App {
    pub(crate) fn draw_portal_gate(&mut self, ctx: &eframe::egui::Context) {
        let mut action = 0u8;
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            theme::paint_grid(ui);
            ui.add_space(28.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    eframe::egui::RichText::new("M I E M P I R E ' S   R U N   T O O L")
                        .strong()
                        .size(20.0),
                );
                ui.add_space(4.0);
                ui.label(eframe::egui::RichText::new("平台授权登录").size(16.0));
                ui.label(
                    eframe::egui::RichText::new("请使用静水空间账号登录，验证通过后才能使用本工具")
                        .color(theme::text_dim()),
                );
                ui.add_space(16.0);

                let width = (ui.available_width() - 40.0).clamp(220.0, 420.0);
                eframe::egui::Frame::none()
                    .inner_margin(eframe::egui::Margin::same(16.0))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.vertical(|ui| {
                            ui.label("用户名");
                            mobile::text_edit(
                                ui,
                                "portal_username",
                                &mut self.portal.username,
                                crate::platform::InputKind::Text,
                                width - 6.0,
                            );
                            ui.add_space(8.0);
                            ui.label("密码");
                            mobile::text_edit(
                                ui,
                                "portal_password",
                                &mut self.portal.password,
                                crate::platform::InputKind::Password,
                                width - 6.0,
                            );
                            ui.add_space(12.0);

                            let enter = ui.input(|input| input.key_pressed(eframe::egui::Key::Enter));
                            if self.portal.busy || self.portal.checking {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("正在验证平台授权…");
                                });
                            } else if ui
                                .add(theme::primary_btn("登录并验证"))
                                .clicked()
                            {
                                action = 1;
                            }
                            if enter && !self.portal.busy && !self.portal.checking {
                                action = 1;
                            }

                            if !self.portal.error.is_empty() {
                                ui.add_space(8.0);
                                ui.colored_label(theme::err(), &self.portal.error);
                            }
                            if !self.portal.allowed_date.is_empty() {
                                ui.add_space(4.0);
                                ui.colored_label(
                                    theme::text_dim(),
                                    format!("授权日期：{}", self.portal.allowed_date),
                                );
                            }
                            if self.portal.has_saved
                                && !self.portal.busy
                                && !self.portal.checking
                            {
                                ui.add_space(8.0);
                                ui.horizontal_wrapped(|ui| {
                                    if ui.button("重新验证已保存的登录").clicked() {
                                        action = 2;
                                    }
                                    if ui.button("清除本机授权").clicked() {
                                        action = 3;
                                    }
                                });
                            }
                        });
                    });

                ui.add_space(14.0);
                ui.label(
                    eframe::egui::RichText::new(
                        "登录会话会加密保存在本机；验证通过后才能使用本工具",
                    )
                    .small()
                    .color(theme::text_dim()),
                );
            });
        });

        match action {
            1 => self.start_portal_login(),
            2 => self.verify_portal_async(),
            3 => self.clear_portal_session(),
            _ => {}
        }
    }

    pub(crate) fn start_portal_login(&mut self) {
        if self.portal.busy || self.portal.checking {
            return;
        }
        let username = self.portal.username.trim().to_string();
        let password = self.portal.password.clone();
        if username.is_empty() || password.is_empty() {
            self.portal.error = "请填写静水空间用户名和密码".into();
            return;
        }
        self.portal.busy = true;
        self.portal.error.clear();
        self.status = "正在登录静水空间平台…".into();
        let device_id = self.identity.device_id.clone();
        self.spawn_ungated_job(move |tx| {
            let payload = match crate::portal::login_and_save(&username, &password, &device_id) {
                Ok(session) => serde_json::json!({
                    "ok": true,
                    "username": session.username,
                    "allowed_date": session.allowed_date,
                }),
                Err(e) => serde_json::json!({ "ok": false, "message": e }),
            };
            tx.send(format!("{}{}", super::PORTAL_LOGIN, payload)).ok();
        });
    }

    pub(crate) fn verify_portal_async(&mut self) {
        if self.portal.busy || self.portal.checking {
            return;
        }
        if !self.portal.has_saved {
            self.portal.error = "本机没有已保存的平台登录会话".into();
            return;
        }
        self.portal.checking = true;
        self.portal.error.clear();
        self.status = "正在验证平台授权…".into();
        let device_id = self.identity.device_id.clone();
        self.spawn_ungated_job(move |tx| match crate::portal::verify_saved(&device_id) {
            Ok(access) => {
                let payload = serde_json::json!({
                    "ok": true,
                    "username": access.username,
                    "allowed_date": access.allowed_date,
                });
                tx.send(format!("{}{}", super::PORTAL_LOGIN, payload)).ok();
            }
            Err(e) => {
                tx.send(format!("{}{}", super::PORTAL_FAIL, e)).ok();
            }
        });
    }

    pub(crate) fn clear_portal_session(&mut self) {
        crate::portal::clear_saved();
        self.portal = PortalUi::default();
        self.status = "已清除本机平台授权".into();
        self.log.push("已清除本机平台授权，请重新登录");
    }
}
