//! 真实 Build 指纹（Android `android.os.Build` / `getprop` 读取）。
//!
//! 为什么需要它
//! ------------
//! `geetoken_template.json` 里的 `test.brl` / `test.build` 段原本是**从三台不相干设备
//! 抄来的硬编码**（Nexus 6P 的 2015 年构建、Redmi K30 的 Dalvik UA、LG 的制造商），
//! 和同一份 token 里按机型生成的 `pla/mod/osv` 直接矛盾 —— 风控只要做一次交叉校验
//! 就能看出是拼出来的。
//!
//! 这里把「系统当前自称的值」整套读上来（**读 prop，不是读真硬件**），
//! 让 token 内所有设备字段来自同一套数据源，天然自洽。
//!
//! 为什么要「清洗」
//! --------------
//! 模拟器/魔改 prop 会留下几处硬伤，它们本身不等于模拟器，但与真机自相矛盾：
//!
//! | 痕迹 | 真机 | 处理 |
//! |---|---|---|
//! | `ro.product.cpu.abilist` 首项 `x86_64` | `arm64-v8a` | 重排为 arm 优先 |
//! | `ro.board.platform` = 品牌名（`OPPO`） | `mt6895` 这类 SoC 代号 | 按机型表补 |
//! | `ro.build.host` = 容器 ID（`6b29a8384f29`） | 厂商构建机名 | 换成等价形态 |
//! | `ro.build.date` 在未来 | 过去 | 收敛到现在之前 |
//!
//! 清洗只做「消除自相矛盾」，不去猜没有依据的值。

use serde::{Deserialize, Serialize};

/// 机型兜底表：`(soc_model, board, hardware)`。
///
/// 只在 prop 明显是假值（等于品牌名/为空）时启用 —— 有真值就不覆盖。
fn soc_for_model(model: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let m = model.to_uppercase();
    let table: &[(&str, &str, &str, &str)] = &[
        // (机型前缀, soc, board, hardware)
        ("PJJ110", "MT6895", "mt6895", "mt6895"), // OPPO K10 Pro（天玑8100-MAX）
        ("22081212C", "MT6895", "mt6895", "mt6895"), // Redmi K50（同为天玑8100）
        ("K50", "MT6895", "mt6895", "mt6895"),
        ("K60", "MT6983", "mt6983", "mt6983"),
        ("X100", "MT6989", "mt6989", "mt6989"),
        ("PIXEL 7", "GS101", "gs101", "gs101"),
        ("PIXEL 8", "GS301", "gs301", "gs301"),
        ("SM-", "EXYNOS2200", "exynos2200", "exynos2200"),
    ];
    for (key, soc, board, hw) in table {
        if m.contains(key) {
            return Some((soc, board, hw));
        }
    }
    None
}

/// 判断一个 prop 值是不是「假冒的占位值」：为空，或恰好等于品牌/制造商。
fn looks_fake(value: &str, brand: &str, manufacturer: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return true;
    }
    v.eq_ignore_ascii_case(brand) || v.eq_ignore_ascii_case(manufacturer)
}

/// 把 abilist 重排成「arm 优先」：真机不会有 `x86_64` 作为首项。
fn arm_first_abilist(raw: &str) -> (String, String) {
    let mut abis: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    // 去掉纯 x86 架构项（真机 OPPO 没有）
    abis.retain(|a| !a.starts_with("x86"));
    if abis.is_empty() {
        abis = vec!["arm64-v8a".into(), "armeabi-v7a".into(), "armeabi".into()];
    }
    // arm64-v8a 必须排第一
    if let Some(pos) = abis.iter().position(|a| a == "arm64-v8a") {
        if pos > 0 {
            let v = abis.remove(pos);
            abis.insert(0, v);
        }
    }
    let ai = abis.first().cloned().unwrap_or_else(|| "arm64-v8a".into());
    (abis.join(","), ai)
}

/// 构建机名清洗：纯 hex 且不含点的，视为容器/虚拟机 hostname，换成厂商风格。
fn sanitize_host(host: &str, brand: &str) -> String {
    let h = host.trim();
    let hex_only = !h.is_empty()
        && h.len() >= 8
        && h.chars().all(|c| c.is_ascii_hexdigit());
    if hex_only {
        // 厂商构建机常见形态：<型号小写>-<两段短 hex>
        let seed = crate::crypto::envelope::md5_hex(h.as_bytes());
        format!("{}-{}", brand.to_lowercase(), &seed[..8])
    } else {
        h.to_string()
    }
}

/// `ro.build.date` 未来值收敛：未来时间对着「已发布机型」不合理。
fn sanitize_build_time(utc: i64, now_ms: i64) -> i64 {
    let now_s = now_ms / 1000;
    if utc <= 0 {
        // 没有值：给一个 Android 12 机型常见的构建时间（2022 上半年）
        return 1_650_000_000;
    }
    if utc > now_s {
        // 未来 → 回退到「now - 2 年」
        return now_s - 2 * 365 * 86_400;
    }
    utc
}

/// 秒级时间戳的 `Wed Sep 30 19:53:14 UTC 2015` 形态（真机 `Build.TIME` 的格式）。
fn format_build_date(utc: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(utc, 0)
        .single()
        .map(|t| t.format("%a %b %e %H:%M:%S UTC %Y").to_string())
        .unwrap_or_else(|| "Thu Jan  1 00:00:00 UTC 1970".into())
}

/// 由 `Build.HOST` 派生的 16 位 hex（模板 `test.build.nh` = `android-<16hex>`）。
fn host_digest(host: &str) -> String {
    let h = crate::crypto::envelope::md5_hex(host.as_bytes());
    h[..16].to_string()
}

/// Android `Build` 指纹全集（首次读取后持久化，跨请求不变）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildFingerprint {
    /// `Build.FINGERPRINT`，形如 `OPPO/daoxiang/daoxiang:12/V417IR/2721:user/release-keys`
    pub fingerprint: String,
    /// `Build.VERSION.SDK_INT`（32 = Android 12L）
    pub sdk_int: i64,
    /// `Build.BRAND`（OPPO）
    pub brand: String,
    /// `Build.MANUFACTURER`（OPPO）
    pub manufacturer: String,
    /// `Build.MODEL`（PJJ110）
    pub model: String,
    /// `Build.DEVICE`（daoxiang）
    pub device: String,
    /// `Build.PRODUCT`（daoxiang）
    pub product: String,
    /// `Build.BOARD`
    pub board: String,
    /// `Build.HARDWARE`
    pub hardware: String,
    /// SoC 型号（`Build.SOC_MODEL` 31+ / 机型表兜底）
    pub soc_model: String,
    /// `Build.HOST`
    pub host: String,
    /// `Build.ID`
    pub build_id: String,
    /// `Build.TIME`（毫秒）
    pub build_time_ms: i64,
    /// `Build.TAGS`
    pub tags: String,
    /// `Build.TYPE`
    pub build_type: String,
    /// `Build.SUPPORTED_ABIS` 逗号拼接
    pub abilist: String,
    /// `Build.SUPPORTED_ABIS[0]`
    pub abi: String,
    /// `Build.getRadioVersion()`
    pub radio: String,
    /// `System.getProperty("http.agent")`
    pub mua: String,
    /// `ActivityManager.MemoryInfo.totalMem`（MB）
    pub mem_total_mb: i64,
    /// `StatFs(/data).totalBytes`（MB）
    pub storage_total_mb: i64,
    /// `StatFs(/data).availableBytes`（MB）
    pub storage_avail_mb: i64,
    /// 本应用 `versionCode`
    pub version_code: i64,
}

impl Default for BuildFingerprint {
    fn default() -> Self {
        // 全部字段都从「真机 OPPO PJJ110 / Android 12」的合理值起手，
        // 仅在完全读不到 prop 时使用（正常路径一定被 `from_props` 覆盖）。
        Self {
            fingerprint: "OPPO/PJJ110/daoxiang:12/V417IR/2721:user/release-keys".into(),
            sdk_int: 32,
            brand: "OPPO".into(),
            manufacturer: "OPPO".into(),
            model: "PJJ110".into(),
            device: "daoxiang".into(),
            product: "daoxiang".into(),
            board: "mt6895".into(),
            hardware: "mt6895".into(),
            soc_model: "MT6895".into(),
            host: "oppo-build-01".into(),
            build_id: "V417IR".into(),
            build_time_ms: 1_650_000_000_000,
            tags: "release-keys".into(),
            build_type: "user".into(),
            abilist: "arm64-v8a,armeabi-v7a,armeabi".into(),
            abi: "arm64-v8a".into(),
            radio: "1.0.0.0".into(),
            mua: String::new(),
            mem_total_mb: 7982,
            storage_total_mb: 117_000,
            storage_avail_mb: 60_000,
            version_code: 740,
        }
    }
}

impl BuildFingerprint {
    /// 从原始 prop 值构造，并清洗明显自相矛盾的模拟器痕迹。
    ///
    /// 参数顺序刻意与 `MainActivity` 的 JSON 键一一对应，避免错位。
    #[allow(clippy::too_many_arguments)]
    pub fn from_props(
        fingerprint: &str,
        sdk_int: i64,
        brand: &str,
        manufacturer: &str,
        model: &str,
        device: &str,
        product: &str,
        board: &str,
        hardware: &str,
        soc_model: &str,
        host: &str,
        build_id: &str,
        build_time_s: i64,
        tags: &str,
        build_type: &str,
        abilist: &str,
        radio: &str,
        mua: &str,
        mem_total_mb: i64,
        storage_total_mb: i64,
        storage_avail_mb: i64,
        version_code: i64,
    ) -> Self {
        let brand = brand.trim().to_string();
        let manufacturer = manufacturer.trim().to_string();
        let model = model.trim().to_string();

        // SoC / board / hardware：prop 是假值（等于品牌名或空）就按机型表补
        let fallback = soc_for_model(&model);
        let soc_model = if soc_model.trim().is_empty() {
            fallback.map(|t| t.0).unwrap_or("MT6895").to_string()
        } else {
            soc_model.trim().to_string()
        };
        let board = if looks_fake(board, &brand, &manufacturer) {
            fallback.map(|t| t.1).unwrap_or("mt6895").to_string()
        } else {
            board.trim().to_string()
        };
        let hardware = if looks_fake(hardware, &brand, &manufacturer) {
            fallback.map(|t| t.2).unwrap_or("mt6895").to_string()
        } else {
            hardware.trim().to_string()
        };

        // abilist：arm 优先、剔掉纯 x86
        let (abilist, abi) = arm_first_abilist(abilist);

        // 构建机名
        let host = sanitize_host(host, &brand);

        // 构建时间：未来值收敛（毫秒）
        let now_ms = crate::crypto::envelope::now_ms();
        let build_time_ms = sanitize_build_time(build_time_s, now_ms) * 1000;

        // fingerprint：真机格式 `{brand}/{product}/{device}:{release}/{id}/{incremental}:{type}/{tags}`
        //
        // 读到的值通常有效，直接沿用；只有明显是模拟器（generic/vbox）或为空时，
        // 用同一批 prop 重拼 —— 保证 fp 里的每一段都和 brl 其它字段同源。
        let fp = fingerprint.trim();
        let fingerprint = if fp.is_empty() || fp.contains("generic") || fp.contains("vbox") {
            let prod = if product.trim().is_empty() {
                device.trim()
            } else {
                product.trim()
            };
            let dev_name = if device.trim().is_empty() {
                prod
            } else {
                device.trim()
            };
            let id = if build_id.trim().is_empty() {
                "V417IR"
            } else {
                build_id.trim()
            };
            let bt = if build_type.trim().is_empty() {
                "user"
            } else {
                build_type.trim()
            };
            let tg = if tags.trim().is_empty() {
                "release-keys"
            } else {
                tags.trim()
            };
            // incremental 用 build_id 代替（模板里两者本就同值，如 V417IR/2721）
            format!(
                "{}/{}/{}:{}/{}/{}:{}/{}",
                brand,
                prod,
                dev_name,
                sdk_release(sdk_int),
                id,
                id,
                bt,
                tg
            )
        } else {
            fp.to_string()
        };

        Self {
            fingerprint,
            sdk_int: if sdk_int > 0 { sdk_int } else { 32 },
            brand,
            manufacturer,
            model,
            device: device.trim().to_string(),
            product: product.trim().to_string(),
            board,
            hardware,
            soc_model,
            host,
            build_id: build_id.trim().to_string(),
            build_time_ms,
            tags: tags.trim().to_string(),
            build_type: build_type.trim().to_string(),
            abilist,
            abi,
            radio: radio.trim().to_string(),
            mua: mua.trim().to_string(),
            mem_total_mb,
            storage_total_mb,
            storage_avail_mb,
            version_code,
        }
    }

    /// `Build.VERSION.RELEASE`（SDK → 版本号）。
    pub fn release(&self) -> String {
        sdk_release(self.sdk_int)
    }

    /// `Wed Sep 30 19:53:14 UTC 2015` 形态。
    pub fn build_date(&self) -> String {
        format_build_date(self.build_time_ms / 1000)
    }

    /// `test.build.nh` = `android-<16hex>`。
    pub fn host_digest(&self) -> String {
        format!("android-{}", host_digest(&self.host))
    }

    /// `test.build.rcgc`：真机 OPPO 是 `android-oppo`。
    pub fn release_channel(&self) -> String {
        format!("android-{}", self.brand.to_lowercase())
    }

    /// 缺 `http.agent` 时按 `mod`+`osv` 拼 Dalvik UA（真机就是这个格式）。
    pub fn dalvik_ua(&self) -> String {
        if !self.mua.is_empty() {
            return self.mua.clone();
        }
        format!(
            "Dalvik/2.1.0 (Linux; U; Android {}; {} Build/{})",
            self.release(),
            self.model,
            self.build_id
        )
    }
}

/// SDK → Android 版本号。
fn sdk_release(sdk: i64) -> String {
    match sdk {
        21 => "5.0",
        22 => "5.1",
        23 => "6.0",
        24 => "7.0",
        25 => "7.1",
        26 => "8.0",
        27 => "8.1",
        28 => "9",
        29 => "10",
        30 => "11",
        // Android 12 的 SDK 是 31 和 32；Build.VERSION.RELEASE 两者都返回 "12"
        // （12L 只是营销名，不会出现在 release 字段里）。
        31 => "12",
        32 => "12",
        33 => "13",
        34 => "14",
        35 => "15",
        _ => "12",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BuildFingerprint {
        BuildFingerprint::from_props(
            "OPPO/daoxiang/daoxiang:12/V417IR/2721:user/release-keys",
            32,
            "OPPO",
            "OPPO",
            "PJJ110",
            "daoxiang",
            "daoxiang",
            "OPPO",   // 假值 → 应被换成 mt6895
            "OPPO",   // 假值 → 应被换成 mt6895
            "",       // 空 → 机型表 MT6895
            "6b29a8384f29", // 容器 ID → 应被清洗
            "V417IR",
            1792671447, // 2026 未来 → 应被回退
            "release-keys",
            "user",
            "x86_64,arm64-v8a,x86,armeabi-v7a,armeabi",
            "1.0.0.0",
            "",
            4034432,
            131062788,
            129696720,
            740,
        )
    }

    #[test]
    fn soc_fallback_replaces_fake_board_and_hardware() {
        let b = sample();
        assert_eq!(b.board, "mt6895", "board 应被机型表覆盖");
        assert_eq!(b.hardware, "mt6895", "hardware 应被机型表覆盖");
        assert_eq!(b.soc_model, "MT6895");
    }

    #[test]
    fn abilist_is_arm_first_and_drops_x86() {
        let b = sample();
        assert!(
            b.abilist.starts_with("arm64-v8a"),
            "abilist 首项必须是 arm64-v8a: {}",
            b.abilist
        );
        assert!(!b.abilist.contains("x86"), "不应残留 x86: {}", b.abilist);
        assert_eq!(b.abi, "arm64-v8a");
    }

    #[test]
    fn container_hostname_is_sanitized() {
        let b = sample();
        assert!(
            !b.host.chars().all(|c| c.is_ascii_hexdigit()),
            "纯 hex 的容器 host 应被清洗: {}",
            b.host
        );
        assert!(b.host.starts_with("oppo-"), "应带品牌前缀: {}", b.host);
    }

    #[test]
    fn future_build_time_is_clamped_to_past() {
        let b = sample();
        let now = crate::crypto::envelope::now_ms();
        assert!(b.build_time_ms < now, "构建时间不应在未来");
    }

    #[test]
    fn dalvik_ua_follows_model_and_release() {
        let b = sample();
        let ua = b.dalvik_ua();
        assert!(ua.contains("PJJ110"), "UA 应含机型: {ua}");
        assert!(ua.contains("Android 12"), "UA 应含系统版本: {ua}");
        assert!(!ua.contains("Redmi"), "不应残留 Redmi: {ua}");
    }

    #[test]
    fn release_and_channels() {
        let b = sample();
        assert_eq!(b.release(), "12");
        assert_eq!(b.release_channel(), "android-oppo");
        assert!(b.host_digest().starts_with("android-"));
        assert_eq!(b.host_digest().len(), "android-".len() + 16);
    }
}