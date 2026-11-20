//! 轨迹海拔覆盖。
//!
//! 海拔是轨迹点的一部分，而不是提交时临时拼出来的字段。统一在生成器
//! 完成后覆盖 `bdA`，这样提交体的 totalAscent、OBS 的 run_data 和每圈
//! elevationGain 都会读取同一份数据。

#![allow(non_snake_case)]

use super::geom::{round_to, Rng};
use super::model::Track;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AltitudeRange {
    pub min_m: f64,
    pub max_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AltitudeSpec {
    Single(f64),
    Range(AltitudeRange),
}

pub fn parse_spec(text: &str) -> Result<Option<AltitudeSpec>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if let Some((left, right)) = text.split_once('-') {
        let min_m = left.trim().parse::<f64>().map_err(|_| "手动海拔区间格式应为 min-max，例如 11.6-22.8".to_string())?;
        let max_m = right.trim().parse::<f64>().map_err(|_| "手动海拔区间格式应为 min-max，例如 11.6-22.8".to_string())?;
        return Ok(Some(AltitudeSpec::Range(validate_range(min_m, max_m)?)));
    }
    let altitude_m = text.parse::<f64>().map_err(|_| "手动海拔应为数字或 min-max 区间，例如 17.2 或 11.6-22.8".to_string())?;
    validate_altitude(altitude_m)?;
    Ok(Some(AltitudeSpec::Single(altitude_m)))
}

fn validate_altitude(altitude_m: f64) -> Result<(), String> {
    if !altitude_m.is_finite() || !(-500.0..=9000.0).contains(&altitude_m) {
        return Err("手动海拔必须是 -500 到 9000 米之间的数字".into());
    }
    Ok(())
}

fn validate_range(min_m: f64, max_m: f64) -> Result<AltitudeRange, String> {
    validate_altitude(min_m)?;
    validate_altitude(max_m)?;
    if min_m > max_m {
        return Err("手动海拔区间的最小值不能大于最大值".into());
    }
    Ok(AltitudeRange { min_m, max_m })
}

/// 将轨迹所有点的百度海拔覆盖为用户输入的绝对海拔（米）。
///
/// 返回 `Err` 而不是静默接受 NaN/无穷值，避免生成不可序列化或被服务端
/// 拒绝的提交。海拔范围采用常见地表范围，既能覆盖地下场地也能覆盖高原。
pub fn override_bd_a(track: &mut Track, altitude_m: f64) -> Result<(), String> {
    validate_altitude(altitude_m)?;
    for point in &mut track.locations {
        point.bdA = round_to(altitude_m, 2);
        point.hasAltitude = true;
    }
    Ok(())
}

/// 对海拔序列做对称滑动平均（窗口 `window`，自动取奇数），用于抑制上报
/// bdA 中混入的 GPS 高频抖动后再计算爬升/圈末海拔。
///
/// 上报点的 `bdA` 含逐点测量噪声（±1~3m），若直接对原始值求正差分累计，
/// 700 个 1s 点会把噪声累计成数百米的假爬升。先做约 5s 窗口的滑动平均，
/// 既压住高频抖动，又保留低频地形趋势（长坡起伏）。
pub fn smooth_altitudes(a: &[f64], window: usize) -> Vec<f64> {
    if a.is_empty() {
        return Vec::new();
    }
    let w = window.max(1) | 1; // 向上取奇
    let half = w / 2;
    let n = a.len();
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(n);
            a[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
        })
        .collect()
}

/// 将生成器的海拔曲线平滑映射到用户指定的绝对海拔区间。
pub fn override_bd_a_range(track: &mut Track, range: AltitudeRange) -> Result<(), String> {
    let range = validate_range(range.min_m, range.max_m)?;
    let (mut current_min, mut current_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for point in &track.locations {
        current_min = current_min.min(point.bdA);
        current_max = current_max.max(point.bdA);
    }
    let span = current_max - current_min;
    let target_span = range.max_m - range.min_m;
    for point in &mut track.locations {
        let mapped = if span.abs() < f64::EPSILON {
            (range.min_m + range.max_m) / 2.0
        } else {
            range.min_m + ((point.bdA - current_min) / span) * target_span
        };
        point.bdA = round_to(mapped.clamp(range.min_m, range.max_m), 2);
        point.hasAltitude = true;
    }
    Ok(())
}

/// 碗形海拔剖面：h(u) = base − depth·sin²(πu) + climb·sin²(π·min(u/0.25,1))·(1−u)
///
/// `u` = 点序号 / (点数−1) ∈ [0,1]（近似距离进度）：
/// - `sin²(πu)` 在 u=0 与 u=1 处值与一阶导数都为 0 → **起终点水平**
/// - 中段 u=0.5 处下凹 `depth`（4~12 m）
/// - `climb_enabled` 时在起始 25% 叠加 2~7 m 爬升，随后按 (1−u) 衰减，收尾仍水平
///
/// 供 Road / Track 模式使用（经典模式由 `generator::build_with_mode` 内部完成）。
/// 直接覆盖 `bdA`，下游 totalAscent、OBS run_data、每圈 elevationGain 读的都是这一份。
pub fn apply_bowl_profile(track: &mut Track, seed: u64, climb_enabled: bool) {
    let n = track.locations.len();
    if n == 0 {
        return;
    }
    // 基准取现有海拔均值，保持与该路线原本所处高度一致
    let base = track.locations.iter().map(|p| p.bdA).sum::<f64>() / n as f64;
    let mut rng = Rng::new(seed ^ 0x9E37_79B9_7F4A_7C15);
    let depth = rng.uniform(4.0, 12.0);
    let climb_h = if climb_enabled {
        rng.uniform(2.0, 7.0)
    } else {
        0.0
    };
    let noise = rng.uniform(1.6, 2.8);
    let denom = (n - 1).max(1) as f64;
    for (i, p) in track.locations.iter_mut().enumerate() {
        let u = i as f64 / denom;
        let bowl = -(std::f64::consts::PI * u).sin().powi(2) * depth;
        let climb = if climb_h > 0.0 {
            let x = (u / 0.25).clamp(0.0, 1.0);
            (std::f64::consts::PI * x).sin().powi(2) * climb_h * (1.0 - u)
        } else {
            0.0
        };
        p.bdA = round_to(base + bowl + climb + rng.gauss(0.0, noise), 2);
        p.hasAltitude = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::submit::total_ascent;
    use crate::track::generator::build;

    fn points() -> Vec<(f64, f64)> {
        vec![(38.901678, 121.540241), (38.902564, 121.541233)]
    }

    #[test]
    fn override_replaces_every_bd_a_and_clears_ascent() {
        let mut track = build(1200.0, 600, 7, (38.9, 121.54), 1_700_000_000_000, &points(), None)
            .expect("build");
        override_bd_a(&mut track, 36.75).unwrap();
        assert!(track.locations.iter().all(|p| p.bdA == 36.75));
        assert_eq!(total_ascent(&track.locations), 0.0);
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_values() {
        let mut track = build(1000.0, 500, 1, (38.9, 121.54), 1_700_000_000_000, &points(), None)
            .expect("build");
        assert!(override_bd_a(&mut track, f64::NAN).is_err());
        assert!(override_bd_a(&mut track, 9001.0).is_err());
    }

    #[test]
    fn parses_single_value_and_range() {
        assert_eq!(parse_spec("17.2").unwrap(), Some(AltitudeSpec::Single(17.2)));
        assert_eq!(parse_spec("11.6-22.8").unwrap(), Some(AltitudeSpec::Range(AltitudeRange { min_m: 11.6, max_m: 22.8 })));
        assert!(parse_spec("22.8-11.6").is_err());
    }

    #[test]
    fn range_mapping_stays_inside_requested_bounds() {
        let mut track = build(1200.0, 600, 7, (38.9, 121.54), 1_700_000_000_000, &points(), None)
            .expect("build");
        override_bd_a_range(&mut track, AltitudeRange { min_m: 11.6, max_m: 22.8 }).unwrap();
        assert!(track.locations.iter().all(|p| (11.6..=22.8).contains(&p.bdA)));
    }

    /// 碗形剖面：两端水平、中段下凹（供路网/跑道模式复用）。
    #[test]
    fn bowl_profile_has_flat_ends_and_middle_dip() {
        let mut track = build(1200.0, 600, 7, (38.9, 121.54), 1_700_000_000_000, &points(), None)
            .expect("build");
        apply_bowl_profile(&mut track, 7, false);
        let a: Vec<f64> = track.locations.iter().map(|p| p.bdA).collect();
        let n = a.len();
        assert!(n >= 25, "点数太少: {n}");
        let seg = (n / 5).max(1);
        let mean = |r: &[f64]| r.iter().sum::<f64>() / r.len() as f64;
        let head = mean(&a[..seg]);
        let center = mean(&a[n / 2 - seg / 2..n / 2 + seg / 2]);
        let tail = mean(&a[n - seg..]);
        assert!((head - tail).abs() < 3.0, "两端不水平: 头 {head:.2} 尾 {tail:.2}");
        assert!(center < head - 3.0, "中段没有下凹: 中 {center:.2} 头 {head:.2}");
    }
}
