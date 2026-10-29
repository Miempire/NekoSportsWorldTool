//! 运动学速度剖面：OU 配速 + 弯道预判刹车（有限减速度 lookahead）。
//!
//! 与逐点硬限速的区别：真人不会在入弯瞬间才减速，而是在进入弯道前一段距离
//! 就以有限制动减速度（约 1.5 m/s²）开始收速。这里对每个弧长 s 向后看
//! `lookahead_m` 范围内的曲率限速，并施加 `v_now² ≤ v_corner² + 2·b·d` 的
//! 可刹停约束，得到平滑且带预判的速度剖面。

use crate::smooth::turn_speed_limit;
use crate::Route;

#[derive(Clone, Copy, Debug)]
pub struct KinParams {
    /// 最大向心加速度 m/s²（弯道限速上限）。
    pub a_c_max: f64,
    /// 最大制动减速度 m/s²（人体可接受的收速斜率）。
    pub brake_max: f64,
    /// 弯道预判前瞻距离（米）。
    pub lookahead_m: f64,
    /// 热身时长（秒），起点从 0.85× 匀速 ramp 到 1.0×。
    pub warmup_s: f64,
    /// OU 均值回归速率 θ（越小越持久）。
    pub theta: f64,
    /// OU 波动率 σ（m/s / √s）。
    pub sigma: f64,
}

impl Default for KinParams {
    fn default() -> Self {
        KinParams {
            a_c_max: 2.5,
            brake_max: 1.5,
            lookahead_m: 40.0,
            warmup_s: 20.0,
            theta: 0.12,
            sigma: 0.06,
        }
    }
}

/// 从弧长 s 出发，在 lookahead 范围内取最小的「可刹停速度」。
///
/// 对每个前方曲率限速 v_corner(q)（距离 d=q.s-s），当前速度最多允许
/// `sqrt(v_corner² + 2·b·d)`，取所有候选的最小值。
pub fn speed_limit_ahead(route: &Route, s: f64, p: &KinParams) -> f64 {
    let mut v_max = f64::INFINITY;
    for pt in &route.points {
        let d = pt.s - s;
        if d <= 0.0 || d > p.lookahead_m {
            continue;
        }
        let vc = turn_speed_limit(p.a_c_max, pt.curvature.abs());
        if vc.is_finite() {
            let ok = (vc * vc + 2.0 * p.brake_max * d).sqrt();
            if ok < v_max {
                v_max = ok;
            }
        }
    }
    v_max
}

/// 生成速度序列：相关随机目标 + 一阶平滑响应 + 弯道预判限速 + 精确命中目标距离。
///
/// 弯道限速由「当前弧长估计 → 可刹停上限」迭代 3 轮修正（限速改变会移动弧长）。
/// 距离与时长无法同时满足时返回 `Err`，不再等比缩放或改写时间轴。
pub fn pace_profile(
    route: &Route,
    v0: f64,
    dts: &[f64],
    target_dist: f64,
    seed: u64,
    p: &KinParams,
    floor: f64,
    ceil: f64,
) -> Result<Vec<f64>, String> {
    use crate::profile::{correlated_speed_profile, SpeedSpec};

    let n = dts.len();
    if n < 2 {
        return Err("速度剖面的时间轴过短".into());
    }

    // 初估弧长：以基础配速匀速推进，用于第一轮弯道限速
    let mut s_of = vec![0.0f64; n];
    let mut s = 0.0;
    let mut v_est = v0;
    for i in 0..n {
        s_of[i] = s;
        s += v_est * dts[i];
        v_est = (v_est * 1.02).min(ceil);
    }

    let mut speeds = vec![v0; n];
    for round in 0..3 {
        let caps: Vec<f64> = (0..n)
            .map(|i| {
                let raw = speed_limit_ahead(route, s_of[i], p);
                if raw.is_finite() {
                    raw.clamp(floor, ceil)
                } else {
                    ceil
                }
            })
            .collect();
        let spec = SpeedSpec {
            dts: dts.to_vec(),
            target_dist,
            base_v: v0,
            floor,
            ceil,
            caps,
            mean_mul: Vec::new(),
            theta: p.theta,
            sigma: p.sigma,
            accel: 0.55,
            decel: p.brake_max,
            response: 0.45,
            warmup_s: p.warmup_s,
            warmup_frac: 0.85,
            tail_s: 12.0,
            tail_frac: 0.90,
            seed: seed ^ (round as u64) * 0x9E37_79B9,
        };
        speeds = correlated_speed_profile(&spec)?;
        let mut acc = 0.0;
        for i in 0..n {
            s_of[i] = acc;
            acc += speeds[i] * dts[i];
        }
    }
    Ok(speeds)
}
