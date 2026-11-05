//! 设备电池档案：按机型生成稳定、可信的电池参数。
//!
//! 设计目标：同一设备每次运行得到同一组值（seed 由 DeviceId + 机型决定），
//! 不同机型容量基线不同，电量/电压/充电状态/健康度/温度相互自洽。
//!
//! 电压用锂电池放电曲线由电量推导（3.30V 空 → 4.35V 满，带非线性），
//! 再叠加 ±15mV 抖动；健康度以 GOOD 为主，高温时才可能 OVERHEAT。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// 电池档案。字段语义对齐 Android `BatteryManager`：
/// `status`: 2=CHARGING 3=DISCHARGING 5=FULL
/// `plugged`: 0=未插电 1=AC 2=USB
/// `health`: 2=GOOD 3=OVERHEAT 4=DEAD
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BatteryProfile {
    /// 标称容量（mAh）。
    pub capacity_mah: u32,
    /// 当前电压（mV）。
    pub voltage_mv: u32,
    /// 电量百分比 0-100。
    pub level: u8,
    pub status: i32,
    pub plugged: i32,
    pub health: i32,
    /// 电池温度（摄氏度）。
    pub temperature_c: f32,
    /// 电芯化学体系。
    pub technology: String,
}

impl Default for BatteryProfile {
    fn default() -> Self {
        Self {
            capacity_mah: 4500,
            voltage_mv: 3920,
            level: 76,
            status: 3,
            plugged: 0,
            health: 2,
            temperature_c: 30.0,
            technology: "Li-ion".into(),
        }
    }
}

/// 常见机型的标称容量（mAh）。取最长匹配，避免 "iPhone 15" 命中 "iPhone 1"。
const MODELS: &[(&str, u32)] = &[
    ("iPhone 15 Pro Max", 4441),
    ("iPhone 15 Pro", 3274),
    ("iPhone 15", 3349),
    ("iPhone 14 Pro Max", 4323),
    ("iPhone 14 Pro", 3200),
    ("iPhone 14", 3279),
    ("iPhone 13 Pro Max", 4352),
    ("iPhone 13", 3227),
    ("iPhone 12", 2815),
    ("iPhone SE", 2018),
    ("iPhone", 3200),
    ("22081212C", 5000), // Redmi K50 至尊版
    ("Redmi", 5000),
    ("Xiaomi", 5000),
    ("MI ", 5000),
    ("vivo", 4700),
    ("V2", 4700),
    ("OPPO", 5000),
    ("PJZ", 5000),
    ("OnePlus", 5000),
    ("HUAWEI", 4700),
    ("HONOR", 4700),
    ("Samsung", 4500),
    ("SM-", 4500),
    ("Pixel", 4355),
    ("realme", 5000),
    ("RMX", 5000),
    ("Meizu", 4500),
];

fn base_capacity(model: &str, manufacturer: &str) -> u32 {
    let haystack = format!("{model} {manufacturer}");
    let mut best: Option<(usize, u32)> = None;
    for (key, capacity) in MODELS {
        if haystack.to_lowercase().contains(&key.to_lowercase()) {
            let len = key.len();
            if best.map(|(l, _)| len > l).unwrap_or(true) {
                best = Some((len, *capacity));
            }
        }
    }
    best.map(|(_, c)| c).unwrap_or(4500)
}

/// 电量 → 电压（mV）。锂电池放电曲线：低电量段下降快、中段平缓。
fn voltage_for_level(level: u8) -> f64 {
    let f = (level as f64 / 100.0).clamp(0.0, 1.0);
    // 3.30V @ 0% → 4.35V @ 100%，sqrt 让中段更平缓。
    3300.0 + 1050.0 * f.sqrt()
}

/// 按设备身份生成（同一 DeviceId + 机型结果稳定）。
pub fn generate(model: &str, manufacturer: &str, device_id: &str, platform: &str) -> BatteryProfile {
    let seed_text = format!("{device_id}|{model}|battery-v1");
    let digest = crate::crypto::envelope::md5_hex(seed_text.as_bytes());
    let seed = u64::from_str_radix(&digest[..16], 16).unwrap_or(0);
    let mut rng = StdRng::seed_from_u64(seed);

    let base = base_capacity(model, manufacturer);
    // 同型号不同个体容量有 ±3% 差异，取整到 10mAh。
    let jitter = rng.gen_range(0..=(base / 30).max(1));
    let capacity_mah = ((base + jitter) / 10 * 10).max(1500);

    let level: u8 = rng.gen_range(25..=92);
    let charging = rng.gen_bool(0.18);
    let (status, plugged) = if charging {
        if level >= 95 {
            (5, if rng.gen_bool(0.8) { 1 } else { 2 })
        } else {
            (2, if rng.gen_bool(0.8) { 1 } else { 2 })
        }
    } else {
        (3, 0)
    };

    let jitter_mv = rng.gen_range(-15i32..=15);
    let voltage_mv = (voltage_for_level(level) + jitter_mv as f64).round().clamp(3300.0, 4400.0) as u32;

    let mut temperature_c = 26.0 + rng.gen_range(0.0..7.0);
    if charging {
        temperature_c += 1.5;
    }
    temperature_c = (temperature_c * 10.0).round() / 10.0;

    let health = if temperature_c >= 39.0 {
        3 // OVERHEAT
    } else if rng.gen_bool(0.01) {
        4 // DEAD（极低概率，避免每个设备都"坏电池"）
    } else {
        2 // GOOD
    };

    let technology = if platform == "android" {
        if rng.gen_bool(0.7) { "Li-poly" } else { "Li-ion" }
    } else {
        "Li-ion"
    };

    BatteryProfile {
        capacity_mah,
        voltage_mv,
        level,
        status,
        plugged,
        health,
        temperature_c,
        technology: technology.into(),
    }
}

/// 用当前身份生成电池档案。
pub fn generate_for(identity: &crate::crypto::header::HeaderIdentity) -> BatteryProfile {
    generate(
        &identity.device_name,
        &identity.manufacturer,
        &identity.device_id,
        &identity.platform,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_device() {
        let a = generate("iPhone 14", "Apple", "DEVICE-1", "ios");
        let b = generate("iPhone 14", "Apple", "DEVICE-1", "ios");
        assert_eq!(a, b, "同一设备两次生成必须一致");
        let c = generate("iPhone 14", "Apple", "DEVICE-2", "ios");
        assert_ne!(a, c, "不同设备应有差异");
    }

    #[test]
    fn values_are_plausible() {
        for model in ["iPhone 14", "22081212C", "SM-G991B", "Pixel 7", "V2366GA"] {
            let b = generate(model, "", "ID-X", "android");
            assert!((1500..=6000).contains(&b.capacity_mah), "{model} 容量 {}", b.capacity_mah);
            assert!((3300..=4400).contains(&b.voltage_mv), "{model} 电压 {}", b.voltage_mv);
            assert!((20..=100).contains(&b.level));
            assert!([2, 3, 5].contains(&b.status));
            assert!(b.temperature_c >= 24.0 && b.temperature_c <= 42.0);
        }
    }

    #[test]
    fn capacity_follows_model() {
        assert!(generate("iPhone 14", "Apple", "x", "ios").capacity_mah > 3000);
        assert!(generate("22081212C", "Xiaomi", "x", "android").capacity_mah >= 4900);
        assert!(generate("SM-G991B", "Samsung", "x", "android").capacity_mah >= 4400);
    }
}