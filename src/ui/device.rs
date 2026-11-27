//! 设备信息页：iOS/Android 单选 + 6 字段 + 整套随机 + 持久化。
//! ⚠ 设备 ID 固定复用（防 10121 风控）；随机后必须手动「保存」才生效。

use super::{mobile, theme, App};
use crate::crypto::header::HeaderIdentity;
use eframe::egui;

const IOS_OS_POOL: [&str; 5] = ["17.5.1", "18.0.1", "18.1", "18.2", "26.5.2"];
const ANDROID_MODEL_POOL: [&str; 8] = [
    "22081212C", "OPPO PGBM10", "Redmi K60", "HUAWEI Mate 40", "vivo V2309A",
    "Pixel 8", "SM-S9210", "OnePlus ACE3",
];
const ANDROID_OS_POOL: [&str; 4] = ["12", "13", "14", "15"];

#[derive(Default)]
pub struct DevicePage {
    saved_flash: f32,
    // ── 极验 geeToken 生成器 ──
    /// 场景：4=起跑 / 6=打卡 / 7=提交
    gt_scene: i64,
    /// 是否连极验换服务端 token（G02…，errorCode=200）
    gt_online: bool,
    /// 当前 token 文本
    gt_token: String,
    /// 与 token 配对的 errorCode（-300 本地 / 200 服务端）
    gt_code: i64,
    /// 后台换取中
    gt_busy: bool,
    /// 状态提示
    gt_status: String,
    /// 后台换取结果槽（token, errorCode）或错误
    gt_result: std::sync::Arc<std::sync::Mutex<Option<Result<(String, i64), String>>>>,
}

impl App {
    #[cfg(target_os = "android")]
    pub(super) fn poll_device_info(&mut self) {
        let Some(reply) = crate::android::take_device_info() else { return };
        let info = match reply.result {
            Ok(Some(info)) => info,
            Ok(None) => {
                self.status = "未读取本机信息，可在设备信息页手动填写或再次读取".into();
                return;
            }
            Err(error) => { self.status = format!("× {error}"); return; }
        };
        let identity = if reply.initial { &self.identity } else { &self.device_buf };
        match crate::api::model::identity_with_device_info(identity, &info) {
            Ok(updated) => {
                self.device_buf = updated;
                if reply.initial {
                    match crate::api::model::save_identity(&self.device_buf) {
                        Ok(()) => {
                            self.identity = self.device_buf.clone();
                            crate::android::complete_device_info();
                            self.status = format!("已读取 {} {} / Android {}；UUID 保持不变",
                                info.manufacturer, info.model, info.os_version);
                        }
                        Err(error) => {
                            self.tab = 5;
                            self.status = format!("× 保存失败：{error}，请在设备页重新保存");
                        }
                    }
                } else {
                    self.status = format!("已读取 {} {}；点击保存后生效，UUID 保持不变", info.manufacturer, info.model);
                }
            }
            Err(error) => self.status = format!("× {error}"),
        }
    }

    pub fn draw_device(&mut self, ui: &mut egui::Ui) {
        let mut page = std::mem::take(&mut self.device_page);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| self.draw_device_inner(ui, &mut page));
        self.device_page = page;
    }

    fn draw_device_inner(&mut self, ui: &mut egui::Ui, page: &mut DevicePage) {
        ui.add_space(6.0);

        mobile::row(ui, |ui| {
            ui.radio_value(&mut self.device_buf.platform, "ios".to_string(), "iOS");
            ui.radio_value(&mut self.device_buf.platform, "android".to_string(), "Android");
            ui.separator();
            let platform = self.device_buf.platform.clone();
            if ui.button("随机生成").clicked() {
                randomize(&mut self.device_buf, &platform);
                self.status = "已生成随机设备（未保存——手动保存后才生效）".into();
            }
        });

        #[cfg(target_os = "android")]
        if ui.button("读取本机信息").clicked() {
            crate::android::request_device_info(false);
        }

        ui.add_space(6.0);
        let is_ios = self.device_buf.platform != "android";
        let id_label = if is_ios { "DeviceId（UUID 大写）" } else { "DeviceId（Android）" };
        let idfa_label = if is_ios { "IDFA（可空）" } else { "IMEI（可空）" };
        let name_label = if is_ios { "设备名" } else { "机型" };
        if mobile::compact_ui(ui) {
            ui.vertical(|ui| {
                ui.label(id_label);
                mobile::text_edit(ui, "device_id", &mut self.device_buf.device_id, crate::platform::InputKind::Text, ui.available_width());
                ui.label(idfa_label);
                mobile::text_edit(ui, "device_idfa", &mut self.device_buf.idfa, crate::platform::InputKind::Text, ui.available_width());
                ui.label("系统版本：");
                mobile::text_edit(ui, "device_os", &mut self.device_buf.os_version, crate::platform::InputKind::Text, ui.available_width());
                ui.label(name_label);
                mobile::text_edit(ui, "device_name", &mut self.device_buf.device_name, crate::platform::InputKind::Text, ui.available_width());
                if !is_ios {
                    ui.label("品牌（本地展示，可空）：");
                    mobile::text_edit(ui, "device_manufacturer", &mut self.device_buf.manufacturer, crate::platform::InputKind::Text, ui.available_width());
                }
                ui.label("城市：");
                mobile::text_edit(ui, "device_city", &mut self.device_buf.city, crate::platform::InputKind::Text, ui.available_width());
                ui.label("定位锚点纬度：");
                mobile::drag_f64(ui, "device_lat", &mut self.device_buf.anchor_lat, -90.0..=90.0, 0.00001, 6);
                ui.label("定位锚点经度：");
                mobile::drag_f64(ui, "device_lon", &mut self.device_buf.anchor_lon, -180.0..=180.0, 0.00001, 6);
            });
        } else {
            egui::Grid::new("device_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(id_label);
                    mobile::text_edit(ui, "device_id", &mut self.device_buf.device_id, crate::platform::InputKind::Text, 340.0);
                    ui.end_row();
                    ui.label(idfa_label);
                    mobile::text_edit(ui, "device_idfa", &mut self.device_buf.idfa, crate::platform::InputKind::Text, 340.0);
                    ui.end_row();
                    ui.label("系统版本：");
                    mobile::text_edit(ui, "device_os", &mut self.device_buf.os_version, crate::platform::InputKind::Text, 120.0);
                    ui.end_row();
                    ui.label(name_label);
                    mobile::text_edit(ui, "device_name", &mut self.device_buf.device_name, crate::platform::InputKind::Text, 200.0);
                    ui.end_row();
                    if !is_ios {
                        ui.label("品牌（本地展示，可空）：");
                        mobile::text_edit(ui, "device_manufacturer", &mut self.device_buf.manufacturer, crate::platform::InputKind::Text, 200.0);
                        ui.end_row();
                    }
                    ui.label("城市：");
                    mobile::text_edit(ui, "device_city", &mut self.device_buf.city, crate::platform::InputKind::Text, 120.0);
                    ui.end_row();
                    ui.label("定位锚点纬度：");
                    mobile::drag_f64(ui, "device_lat", &mut self.device_buf.anchor_lat, -90.0..=90.0, 0.00001, 6);
                    ui.end_row();
                    ui.label("定位锚点经度：");
                    mobile::drag_f64(ui, "device_lon", &mut self.device_buf.anchor_lon, -180.0..=180.0, 0.00001, 6);
                    ui.end_row();
                });
        }

        if let Some(battery) = &self.device_buf.battery {
            ui.add_space(4.0);
            let charge = match battery.status {
                2 => "充电中",
                5 => "已充满",
                _ => "放电中",
            };
            let plugged = match battery.plugged {
                1 => "AC",
                2 => "USB",
                _ => "未插电",
            };
            let health = match battery.health {
                2 => "良好",
                3 => "过热",
                4 => "损坏",
                _ => "未知",
            };
            ui.colored_label(
                theme::text_dim(),
                format!(
                    "电池：{} mAh / {:.2} V / {}% / {}（{}）/ 健康：{} / {:.1}℃ / {}",
                    battery.capacity_mah,
                    battery.voltage_mv as f32 / 1000.0,
                    battery.level,
                    charge,
                    plugged,
                    health,
                    battery.temperature_c,
                    battery.technology
                ),
            );
        }

        if let Some(profile) = &self.device_buf.sensors {
            let sample = profile
                .sensors
                .iter()
                .take(4)
                .map(|s| s.type_name.rsplit('.').next().unwrap_or(&s.type_name))
                .collect::<Vec<_>>()
                .join(" / ");
            ui.colored_label(
                theme::text_dim(),
                format!("传感器：{} 个（{}…）", profile.sensors.len(), sample),
            );
        }

        ui.add_space(4.0);
        ui.colored_label(
            theme::text_dim(),
            "定位锚点填写顺序：先填纬度，再填经度；地图常见的“经度,纬度”格式需要调换后填写。",
        );
        ui.colored_label(
            theme::text_dim(),
            "修改城市或定位锚点后，请点击“保存”；跑步流程只使用已保存的身份信息。",
        );
        if location_fields_changed(&self.identity, &self.device_buf) {
            ui.colored_label(
                theme::warn(),
                "检测到城市或定位锚点有未保存修改，当前跑步仍会使用上次保存的值。",
            );
        }

        ui.add_space(8.0);
        mobile::row(ui, |ui| {
            if ui.add(theme::primary_btn("保存")).clicked() {
                // 机型/系统版本改过时，电池、传感器、极验档案跟着重算（设备串 d 保留）。
                crate::api::model::refresh_derived(&mut self.device_buf);
                match crate::api::model::save_identity(&self.device_buf) {
                    Ok(()) => {
                        self.identity = self.device_buf.clone();
                        self.status = "√ 设备身份已保存并生效".into();
                        self.log.push("√  已更新（新设备 ID 从下个请求开始生效）");
                        page.saved_flash = 2.0;
                    }
                    Err(e) => self.status = format!("× 保存失败：{e}"),
                }
            }
            if ui.button("撤销修改").clicked() {
                self.device_buf = self.identity.clone();
            }
            if page.saved_flash > 0.0 {
                ui.colored_label(theme::ok(), "已保存");
            }
        });

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        mobile::row(ui, |ui| {
            // —— 导出 / 导入配置包 ——
            #[cfg(not(target_os = "android"))]
            {
                if ui.button("导出全部参数…").clicked() {
                    let path = rfd::FileDialog::new()
                        .add_filter("配置包", &["json"])
                        .set_file_name("miempires-run-tool-config.json")
                        .set_title("导出设备身份 + 跑步参数（不含登录会话）")
                        .save_file();
                    if let Some(p) = path {
                        match crate::api::model::export_bundle(&p, true) {
                            Ok(()) => self.status = format!("√ 已导出到 {}", p.display()),
                            Err(e) => self.status = format!("× 导出失败：{e}"),
                        }
                    }
                }
                if ui.button("导入参数…").clicked() {
                    let path = rfd::FileDialog::new()
                        .add_filter("配置包", &["json"])
                        .set_title("导入配置包（将覆盖当前 identity/config）")
                        .pick_file();
                    if let Some(p) = path {
                        match crate::api::model::import_bundle(&p, false) {
                            Ok(applied) => {
                                self.status = format!("√ 已导入并生效：{}", applied);
                                // identity 已被 model 层落盘；把 UI 缓冲区同步刷新
                                self.identity = crate::api::model::load_identity();
                                self.device_buf = self.identity.clone();
                            }
                            Err(e) => self.status = format!("× 导入失败：{e}"),
                        }
                    }
                }
            }
            #[cfg(target_os = "android")]
            {
                ui.colored_label(theme::text_dim(), "Android 请用 CLI：export-bundle / import-bundle");
            }
        });
        ui.add_space(2.0);
        ui.colored_label(
            theme::text_dim(),
            "导出 JSON 包含设备身份 + 跑步参数（登录会话默认不导出）。导入会立即覆盖当前配置。",
        );

        ui.add_space(12.0);
        ui.separator();
        ui.colored_label(
            theme::warn(),
            "风控提示：设备 ID 参与服务端会话校验，固定复用可显著降低 10121（设备风险）概率；",
        );
        ui.colored_label(
            theme::warn(),
            "不要每次启动都换设备；「随机生成」只改界面缓冲区，手动「保存」后才写入  生效。",
        );
        ui.add_space(4.0);
        ui.label(format!(
            "当前生效身份：{} / {} / {} / 城市 {} / 锚点({:.6},{:.6})",
            if self.identity.platform == "android" { "Android" } else { "iOS" },
            self.identity.device_name,
            self.identity.os_version,
            self.identity.city,
            self.identity.anchor_lat,
            self.identity.anchor_lon,
        ));
        if self.identity.has_unconfigured_default_location() {
            ui.colored_label(
                theme::warn(),
                "运行前请填写实际城市和定位锚点；默认大连配置不会用于提交",
            );
        }
        if page.saved_flash > 0.0 {
            page.saved_flash -= ui.ctx().input(|i| i.stable_dt);
        }
        if self.identity.platform == "android" && !self.identity.manufacturer.is_empty() {
            ui.label(format!("当前保存品牌：{}", self.identity.manufacturer));
        }

        // ── 极验 geeToken ─────────────────────────────────────────
        ui.add_space(12.0);
        ui.separator();
        ui.label(egui::RichText::new("极验 geeToken").strong());
        ui.colored_label(
            theme::text_dim(),
            "本地生成 GEE…（errorCode=-300）；勾选「换服务端」会 POST 到 riskct.geetest.com \
             换成 G02…（errorCode=200，有效期有限）。同一个 token 只能配对应的 errorCode。",
        );

        mobile::row(ui, |ui| {
            ui.label("场景：");
            ui.selectable_value(&mut page.gt_scene, 4, "起跑(4)");
            ui.selectable_value(&mut page.gt_scene, 6, "打卡(6)");
            ui.selectable_value(&mut page.gt_scene, 7, "提交(7)");
        });
        mobile::row(ui, |ui| {
            ui.checkbox(&mut page.gt_online, "换服务端 token（联网）");
        });

        // 后台结果回填
        if let Ok(mut slot) = page.gt_result.try_lock() {
            if let Some(done) = slot.take() {
                page.gt_busy = false;
                match done {
                    Ok((token, code)) => {
                        page.gt_status = format!(
                            "√ 已生成 {} 字符，errorCode={code}",
                            token.chars().count()
                        );
                        page.gt_token = token;
                        page.gt_code = code;
                    }
                    Err(error) => {
                        page.gt_status = format!("× 生成失败：{error}");
                    }
                }
            }
        }

        mobile::row(ui, |ui| {
            let gen_btn = egui::Button::new(if page.gt_busy { "生成中…" } else { "生成" });
            if ui.add_enabled(!page.gt_busy, gen_btn).clicked() {
                self.start_geetoken(page);
            }
            let can_copy = !page.gt_token.is_empty();
            if ui.add_enabled(can_copy, egui::Button::new("复制")).clicked() {
                ui.ctx().copy_text(page.gt_token.clone());
                page.gt_status = format!(
                    "√ 已复制 {} 字符到剪贴板",
                    page.gt_token.chars().count()
                );
            }
            if can_copy && ui.button("清空").clicked() {
                page.gt_token.clear();
                page.gt_code = 0;
                page.gt_status.clear();
            }
        });

        if !page.gt_status.is_empty() {
            ui.colored_label(theme::plain(), &page.gt_status);
        }
        if !page.gt_token.is_empty() {
            ui.label(format!(
                "长度 {} · errorCode {}",
                page.gt_token.chars().count(),
                page.gt_code
            ));
            // 只读多行：手机上可长按选中，配合「复制」按钮
            let mut shown = page.gt_token.clone();
            ui.add(
                egui::TextEdit::multiline(&mut shown)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .interactive(false),
            );
        }
    }

    /// 生成 geeToken：本地同步；换服务端走后台线程（联网，最长 20s）。
    fn start_geetoken(&mut self, page: &mut DevicePage) {
        let uid = self.session.as_ref().map(|s| s.uid).unwrap_or(0);
        let uuid = uuid::Uuid::new_v4().to_string(); // 真机是小写
        let scene = page.gt_scene;
        let identity = self.identity.clone();

        if !page.gt_online {
            // 本地 token：纯 CPU，毫秒级，直接同步
            let (token, code) = crate::geetoken::make_token(&identity, uid, &uuid, scene);
            page.gt_status = format!(
                "√ 本地 token {} 字符，errorCode={code}",
                token.chars().count()
            );
            page.gt_token = token;
            page.gt_code = code;
            return;
        }

        // 换服务端：后台执行，结果写回共享槽
        page.gt_busy = true;
        page.gt_status = "正在向 riskct.geetest.com 换取服务端 token…".into();
        let slot = page.gt_result.clone();
        self.spawn_job(move |tx| {
            let mut log = App::logger(tx);
            let local = crate::geetoken::generate_local(&identity, uid, &uuid, scene);
            // 服务端 token 前缀必须是 G02（对应 errorCode=200）
            let outcome = crate::geetoken::exchange_online(&local, &mut log).map(|token| {
                let code = crate::geetoken::code_for_token(&token);
                (token, code)
            });
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(outcome);
            }
        });
    }
}

fn location_fields_changed(saved: &HeaderIdentity, pending: &HeaderIdentity) -> bool {
    saved.city.trim() != pending.city.trim()
        || (saved.anchor_lat - pending.anchor_lat).abs() > 1e-9
        || (saved.anchor_lon - pending.anchor_lon).abs() > 1e-9
}

/// 整套随机：uuid v4 设备 ID（大写）；机型/系统按平台池抽取。
fn randomize(buf: &mut HeaderIdentity, platform: &str) {
    buf.manufacturer.clear();
    buf.device_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    buf.app_install_time = HeaderIdentity::fresh_install_time(platform);
    if platform == "android" {
        let i = (rand::random::<f64>() * ANDROID_MODEL_POOL.len() as f64) as usize
            % ANDROID_MODEL_POOL.len();
        let j =
            (rand::random::<f64>() * ANDROID_OS_POOL.len() as f64) as usize % ANDROID_OS_POOL.len();
        buf.device_name = ANDROID_MODEL_POOL[i].into();
        buf.os_version = ANDROID_OS_POOL[j].into();
        buf.idfa = String::new();
        buf.platform = "android".into();
    } else {
        let k = (rand::random::<f64>() * IOS_OS_POOL.len() as f64) as usize % IOS_OS_POOL.len();
        buf.device_name = "iPhone".into();
        buf.os_version = IOS_OS_POOL[k].into();
        buf.idfa = uuid::Uuid::new_v4().to_string().to_uppercase();
        buf.platform = "ios".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_changes_are_detected_until_saved() {
        let saved = HeaderIdentity::default();
        let mut pending = saved.clone();
        assert!(!location_fields_changed(&saved, &pending));

        pending.city = "北京市".into();
        assert!(location_fields_changed(&saved, &pending));

        pending = saved.clone();
        pending.anchor_lat += 0.000001;
        assert!(location_fields_changed(&saved, &pending));
    }

    #[test]
    fn random_device_does_not_keep_a_previously_imported_brand() {
        for platform in ["android", "ios"] {
            let mut identity = HeaderIdentity {
                manufacturer: "Previously imported brand".into(),
                ..Default::default()
            };
            randomize(&mut identity, platform);
            assert!(identity.manufacturer.is_empty(), "random {platform} identity must not retain a different device's brand");
        }
    }
}
