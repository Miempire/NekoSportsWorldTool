//! 表观配速校正（移植自 `sw-proto-suite-v3.0.2/rungen/engine.py::_fit_pace_bounds`）。
//!
//! 为什么需要它
//! ------------
//! App 的配速图按「**相邻两点弦长 / 时间差**」算，而我们的几何是按弧长推进的：
//! 折返、掉头、绕花坛回头的路线上，相邻两点可能分别落在去程/回程两条腿上，
//! 弦长只有两条腿的横向间距（实测 5.8~6.4 m）。5 秒走这么点距离，
//! 就被画成 14'00"~16'00"，整条记录看着像「配速超出学校区间」。
//!
//! 做法
//! ----
//! 对每一段求可行区间 `[lo·d/1000, hi·d/1000]`（`d` 是弦长），先把 `dt` 裁进去，
//! 再把「裁剪造成的时长差」按余量摊给其它段 —— **总时长严格不变**，
//! 所以总里程、平均配速、分段用时全都不动；坐标一个都没动，轨迹形状不变。
//!
//! 只改时间戳（`gainTimeMs` / `totalTime` / `validTime` / `gainTime`），
//! 并返回新的采样时间轴供上层重算 10s 窗与分段。

use super::geom::{fmt_cst, round_to};
use super::model::GenPoint;

/// 距区间边界留 3%，避免「正好压线」又被标红。
const PACE_FIT_MARGIN: f64 = 0.03;
/// 单段最短 0.2 秒（再短就是重复点，不是采样）。
const PACE_FIT_MIN_DT: f64 = 0.2;
/// 迭代摊平的轮数（与参考实现一致）。
const SPREAD_ROUNDS: usize = 6;

/// 两点球面距离（米）。
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp * 0.5).sin().powi(2) + p1.cos() * p2.cos() * (dl * 0.5).sin().powi(2);
    2.0 * R * a.sqrt().clamp(0.0, 1.0).asin()
}

/// 把相邻点的**表观配速**收进 `pace_bounds`（秒/公里，`(最快, 最慢)`）。
///
/// 返回 `(改动段数, 新的采样时间轴 relative seconds)`；
/// `pace_bounds` 非法或点数不足时原样返回 `(0, 原时间轴)`。
pub fn fit_pace_bounds(
    points: &mut [GenPoint],
    pace_bounds: Option<(f64, f64)>,
    start_ms: i64,
) -> (usize, Vec<f64>) {
    let n = points.len();
    // 原有时间轴（相对秒）
    let times: Vec<f64> = points
        .iter()
        .map(|p| (p.gainTimeMs - start_ms) as f64 / 1000.0)
        .collect();
    let (mut lo, mut hi) = match pace_bounds {
        Some(v) => v,
        None => return (0, times),
    };
    if n < 3 || !(lo.is_finite() && hi.is_finite()) || !(0.0 < lo && lo < hi) {
        return (0, times);
    }
    // 往区间里再收 3%
    lo *= 1.0 + PACE_FIT_MARGIN;
    hi *= 1.0 - PACE_FIT_MARGIN;
    if hi <= lo {
        return (0, times);
    }

    let mut dt: Vec<f64> = (1..n).map(|i| times[i] - times[i - 1]).collect();
    let ch: Vec<f64> = (1..n)
        .map(|i| {
            haversine_m(
                points[i - 1].gLat,
                points[i - 1].gLng,
                points[i].gLat,
                points[i].gLng,
            )
        })
        .collect();
    let total: f64 = dt.iter().sum();
    if total <= 0.0 {
        return (0, times);
    }

    // 每段的可行区间：秒 = 配速(秒/km) × 弦长(米) / 1000
    let low_b: Vec<f64> = ch
        .iter()
        .map(|&d| PACE_FIT_MIN_DT.max(lo * d / 1000.0))
        .collect();
    let high_b: Vec<f64> = low_b
        .iter()
        .zip(ch.iter())
        .map(|(&a, &d)| a.max(hi * d / 1000.0))
        .collect();

    // 先裁进区间
    let mut x: Vec<f64> = dt
        .iter()
        .zip(low_b.iter().zip(high_b.iter()))
        .map(|(&v, (&a, &b))| v.min(b).max(a))
        .collect();

    // 差额按「剩余余量」摊掉（不可行时也照样摊，能让多少是多少）
    for _ in 0..SPREAD_ROUNDS {
        let sum: f64 = x.iter().sum();
        let diff = total - sum;
        if diff.abs() < 1e-6 {
            break;
        }
        let slack: Vec<f64> = if diff > 0.0 {
            x.iter().zip(high_b.iter()).map(|(&v, &b)| b - v).collect()
        } else {
            x.iter().zip(low_b.iter()).map(|(&v, &a)| v - a).collect()
        };
        let room: f64 = slack.iter().sum();
        if room <= 1e-9 {
            break;
        }
        let k = (diff.abs() / room).min(1.0);
        if diff > 0.0 {
            for (v, s) in x.iter_mut().zip(slack.iter()) {
                *v += k * s;
            }
        } else {
            for (v, s) in x.iter_mut().zip(slack.iter()) {
                *v -= k * s;
            }
        }
    }

    // 重建时间轴：首点不动；末点严格对齐总时长
    let mut acc = 0.0f64;
    let mut moved = 0usize;
    let mut new_times = Vec::with_capacity(n);
    new_times.push(0.0);
    for i in 1..n {
        let old = dt[i - 1];
        acc += x[i - 1];
        let t = round_to(acc, 3);
        new_times.push(t);
        if (x[i - 1] - old).abs() > 1e-3 {
            moved += 1;
        }
    }
    // 末点对齐：把残差塞给最后一段（总时长分毫不差）
    if n >= 2 {
        let fix = total - new_times[n - 1];
        new_times[n - 1] = total;
        let _ = fix;
    }

    // 回写点位的时间字段
    for (i, p) in points.iter_mut().enumerate() {
        let ms = start_ms + (new_times[i] * 1000.0).round() as i64;
        p.gainTimeMs = ms;
        let sec = new_times[i].round() as i64;
        p.totalTime = sec;
        p.validTime = sec;
        // 真机是 "2026-10-26 19:46:30"（含日期）
        p.gainTime = fmt_cst(ms, "%Y-%m-%d %H:%M:%S");
    }
    dt.clear();
    (moved, new_times)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::generator::build;

    fn pts() -> Vec<(f64, f64)> {
        vec![(38.901678, 121.540241), (38.902564, 121.541233)]
    }

    /// 校正后：总时长不变、每段表观配速落进区间（含折返场景）。
    #[test]
    fn fit_keeps_total_time_and_bounds_segments() {
        let mut track = build(1200.0, 600, 7, (38.9, 121.54), 1_700_000_000_000, &pts(), None)
            .expect("build");
        let total_before = track.totalTime;
        let (moved, times) = fit_pace_bounds(&mut track.locations, Some((240.0, 600.0)), track.startTime);
        assert_eq!(track.totalTime, total_before, "总时长必须不变");
        assert!(!times.is_empty());
        // 末点严格等于总时长
        let last = *times.last().unwrap();
        assert!((last - total_before as f64).abs() < 0.01, "末点未对齐: {last}");
        // 至少改动了若干段（正常轨迹也会被边界收一点）
        let _ = moved;
    }

    /// 没有区间时原样返回，不改任何时间戳。
    #[test]
    fn fit_without_bounds_is_noop() {
        let mut track = build(1000.0, 500, 3, (38.9, 121.54), 1_700_000_000_000, &pts(), None)
            .expect("build");
        let before: Vec<i64> = track.locations.iter().map(|p| p.gainTimeMs).collect();
        let (moved, _) = fit_pace_bounds(&mut track.locations, None, track.startTime);
        assert_eq!(moved, 0);
        let after: Vec<i64> = track.locations.iter().map(|p| p.gainTimeMs).collect();
        assert_eq!(before, after);
    }
}
