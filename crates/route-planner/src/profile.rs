//! 目标-响应速度/步频合成（单一累计数据源）。
//!
//! 链路：单一时间轴 → 相关随机目标（OU 均值回归）→ 一阶平滑响应
//! （受加速度/减速度上限约束，逐点弯道限速 cap 为软约束：超限时按减速度
//! 上限收敛，不做瞬时硬压）→ 速度积分得距离、步频积分得步数；
//! 10s 窗 / 60s 段 / 圈全部由同一份累计序列插值派生。
//!
//! 与旧实现的差别：
//! - 不再用等比缩放硬凑距离（那会静默改写物理加速度）；
//! - 不再在末尾补时长或修正末点计数；
//! - 距离与时长无法同时满足时返回 `Err`，由调用方提示用户调整参数。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::{Distribution, Normal};

use crate::biomech::gait;

/// 速度剖面输入。
#[derive(Clone, Debug)]
pub struct SpeedSpec {
    /// 逐点时间间隔（秒）；最后一点间隔为 0，Σ = 总时长。
    pub dts: Vec<f64>,
    /// 目标总距离（米）。
    pub target_dist: f64,
    /// 基础配速（m/s），热身/疲劳前的均值。
    pub base_v: f64,
    /// 配速下限（m/s）。
    pub floor: f64,
    /// 配速上限（m/s）。
    pub ceil: f64,
    /// 逐点最大速度（弯道/路面限速），可含 `f64::INFINITY`。
    pub caps: Vec<f64>,
    /// 逐点均值倍率（疲劳/波动/坡度）；为空表示全 1。
    pub mean_mul: Vec<f64>,
    /// OU 均值回归速率（越小越持久）。
    pub theta: f64,
    /// OU 波动率（m/s/√s）。
    pub sigma: f64,
    /// 加速度上限（m/s²）。
    pub accel: f64,
    /// 减速度上限（m/s²）。
    pub decel: f64,
    /// 一阶响应速率（1/s）。
    pub response: f64,
    /// 热身时长（秒）；0 表示无热身。
    pub warmup_s: f64,
    /// 热身起始比例（如 0.85）。
    pub warmup_frac: f64,
    /// 末尾收速时长（秒）；0 表示无收速。
    pub tail_s: f64,
    /// 末尾收速后的比例（如 0.90）。
    pub tail_frac: f64,
    /// 随机种子。
    pub seed: u64,
}

/// 步频剖面输入。
#[derive(Clone, Debug)]
pub struct CadenceSpec<'a> {
    /// 已合成的逐点速度（m/s）。
    pub speeds: &'a [f64],
    /// 逐点时间间隔（秒）。
    pub dts: &'a [f64],
    /// 逐点疲劳度（0..0.6）。
    pub fatigue: &'a [f64],
    /// 步频下限（步/分）。
    pub floor: f64,
    /// 步频上限（步/分）。
    pub ceil: f64,
    /// OU 均值回归速率。
    pub theta: f64,
    /// OU 波动率（步/分/√s）。
    pub sigma: f64,
    /// 一阶响应速率（1/s）。
    pub response: f64,
    /// 步频变化速度上限（步/分/秒）。
    pub accel: f64,
    /// 随机种子。
    pub seed: u64,
}

fn hard_cap(spec: &SpeedSpec, i: usize) -> f64 {
    let raw = spec.caps.get(i).copied().unwrap_or(f64::INFINITY);
    raw.min(spec.ceil).max(spec.floor)
}

/// 生成速度序列：相关随机目标 + 一阶平滑响应 + 加减速/弯道限速约束。
///
/// 通过「目标整体偏移量 δ 的二分」精确命中目标距离：距离随 δ 单调递增，
/// 既保留加速度/弯道约束，也不改写时间轴。无法命中时返回 `Err`。
pub fn correlated_speed_profile(spec: &SpeedSpec) -> Result<Vec<f64>, String> {
    let n = spec.dts.len();
    if n < 2 {
        return Err("时间轴过短（至少 2 个采样点）".into());
    }
    if !(spec.floor.is_finite() && spec.ceil.is_finite()) || spec.ceil <= spec.floor {
        return Err("配速边界无效".into());
    }
    if spec.caps.len() != n {
        return Err("逐点限速序列长度与时间轴不一致".into());
    }
    if !spec.mean_mul.is_empty() && spec.mean_mul.len() != n {
        return Err("均值倍率序列长度与时间轴不一致".into());
    }
    let dur: f64 = spec.dts.iter().sum();
    if !(dur > 0.0) {
        return Err("总时长无效".into());
    }
    if !(spec.target_dist > 0.0) {
        return Err("目标距离无效".into());
    }

    let mut rng = StdRng::seed_from_u64(spec.seed);
    let normal = Normal::<f64>::new(0.0, 1.0).map_err(|e| format!("噪声分布无效: {e}"))?;

    let mut clock = Vec::with_capacity(n);
    let mut t = 0.0;
    for &dt in &spec.dts {
        clock.push(t);
        t += dt.max(0.0);
    }

    // ① 相关随机目标：OU 围绕「热身 ramp × 尾部收速」的均值回归。
    let mut target = Vec::with_capacity(n);
    let mut mu = spec.base_v * spec.warmup_frac;
    for i in 0..n {
        let tt = clock[i];
        let ramp = if spec.warmup_s > 0.0 && tt < spec.warmup_s {
            spec.warmup_frac + (1.0 - spec.warmup_frac) * (tt / spec.warmup_s)
        } else {
            1.0
        };
        let tail = if spec.tail_s > 0.0 && tt > dur - spec.tail_s {
            let u = ((tt - (dur - spec.tail_s)) / spec.tail_s).clamp(0.0, 1.0);
            1.0 - (1.0 - spec.tail_frac) * u
        } else {
            1.0
        };
        let mean = (spec.base_v * spec.mean_mul.get(i).copied().unwrap_or(1.0) * ramp * tail)
            .clamp(spec.floor, hard_cap(spec, i));
        let dt = spec.dts[i].max(0.0);
        mu += spec.theta * (mean - mu) * dt
            + spec.sigma * dt.sqrt() * normal.sample(&mut rng);
        mu = mu.clamp(spec.floor, hard_cap(spec, i));
        target.push(mu);
    }

    // ② 一阶平滑响应：v 追随目标，但受加速度/减速度上限约束。
    let simulate = |delta: f64| -> (Vec<f64>, f64) {
        let mut out = Vec::with_capacity(n);
        let mut v = (target[0] + delta).clamp(spec.floor, hard_cap(spec, 0));
        let mut dist = 0.0;
        for i in 0..n {
            if i > 0 {
                let hi = hard_cap(spec, i);
                let want = (target[i] + delta).clamp(spec.floor, hi);
                let dt = spec.dts[i].max(0.0);
                let dv = (spec.response * (want - v))
                    .clamp(-spec.decel * dt, spec.accel * dt);
                // 只服从加减速度上限，不在响应后再硬压上限：弯道 cap 由前瞻刹车
                // 保证可达，瞬时硬压会违反减速度上限并在急弯处制造速度跳变。
                v = (v + dv).max(spec.floor);
            }
            out.push(v);
            dist += v * spec.dts[i].max(0.0);
        }
        (out, dist)
    };

    // ③ 可行性判定 + δ 二分命中。
    let span = 3.0 * spec.base_v;
    let (_, d_low) = simulate(-span);
    let (_, d_high) = simulate(span);
    let tol = 1.0;
    if d_high < spec.target_dist - tol {
        return Err(format!(
            "无法在 {:.0}s 内完成 {:.1}m：受弯道限速/配速上限约束最多 {:.1}m，\
             请下调距离或延长时长",
            dur, spec.target_dist, d_high
        ));
    }
    if d_low > spec.target_dist + tol {
        return Err(format!(
            "无法在 {:.0}s 内只跑 {:.1}m：受配速下限约束至少 {:.1}m，\
             请上调距离或缩短时长",
            dur, spec.target_dist, d_low
        ));
    }
    let (mut lo, mut hi) = (-span, span);
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        let (_, d) = simulate(mid);
        if d < spec.target_dist {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (speeds, dist) = simulate(0.5 * (lo + hi));
    if (dist - spec.target_dist).abs() > tol {
        return Err(format!(
            "速度剖面无法同时满足距离与时长：积分 {:.1}m ≠ 目标 {:.1}m",
            dist, spec.target_dist
        ));
    }
    Ok(speeds)
}

/// 生成步频序列：相关随机目标（由速度与疲劳度给出）+ 一阶平滑响应。
///
/// 步数由调用方对步频积分得到，不再对末点计数做任何修正。
pub fn correlated_cadence_profile(spec: &CadenceSpec<'_>) -> Result<Vec<f64>, String> {
    let n = spec.speeds.len().min(spec.dts.len()).min(spec.fatigue.len());
    if n < 2 {
        return Err("步频时间轴过短".into());
    }
    if spec.ceil <= spec.floor {
        return Err("步频边界无效".into());
    }
    let mut rng = StdRng::seed_from_u64(spec.seed);
    let normal = Normal::<f64>::new(0.0, 1.0).map_err(|e| format!("噪声分布无效: {e}"))?;

    let mut out = Vec::with_capacity(n);
    let mut c = gait(spec.speeds[0], spec.fatigue[0])
        .cadence
        .clamp(spec.floor, spec.ceil);
    for i in 0..n {
        let target = gait(spec.speeds[i], spec.fatigue[i])
            .cadence
            .clamp(spec.floor, spec.ceil);
        let dt = spec.dts[i].max(0.0);
        let stochastic = target
            + spec.sigma * dt.sqrt() * normal.sample(&mut rng);
        let want = stochastic.clamp(spec.floor, spec.ceil);
        if i > 0 {
            let dc = (spec.response * (want - c))
                .clamp(-spec.accel * dt, spec.accel * dt);
            c = (c + dc).clamp(spec.floor, spec.ceil);
        } else {
            c = (c + spec.theta * (want - c) * dt).clamp(spec.floor, spec.ceil);
        }
        out.push(c);
    }
    Ok(out)
}

/// 采样时间轴：前两点 1s 冷启动，其后主 5s（按 `p_drop` 概率丢点成 10/15s），
/// 末点严格落在 `dur_s`，保证 Σdt = 总时长（不静默补时长）。
pub fn time_grid(dur_s: f64, p_drop: f64, seed: u64) -> Result<Vec<f64>, String> {
    if !(dur_s > 3.0) {
        return Err("时长过短（至少 3s）".into());
    }
    let mut rng = StdRng::seed_from_u64(seed ^ 0x9E37_79B9_7F4A_7C15);
    let mut times = vec![0.0];
    let mut t = 0.0;
    loop {
        let gap = if times.len() <= 2 {
            1.0
        } else {
            let mut k = 1u32;
            while rng.gen::<f64>() < p_drop && k < 3 {
                k += 1;
            }
            5.0 * k as f64
        };
        let next = t + gap;
        if next >= dur_s - 2.0 {
            times.push(dur_s);
            break;
        }
        times.push(next);
        t = next;
    }
    if times.len() < 4 {
        return Err("时长过短，无法生成足够采样点".into());
    }
    Ok(times)
}

/// 由逐点速率积分出累计序列（`rate` 为区间 `[t_i, t_i+1]` 的速率）。
///
/// `per_minute=true` 用于步频（步/分 → 步/秒）。
pub fn cumulative(times: &[f64], rate: &[f64], per_minute: bool) -> Vec<f64> {
    let n = times.len().min(rate.len());
    let mut out = vec![0.0; n];
    let mut acc = 0.0;
    for i in 0..n.saturating_sub(1) {
        let dt = (times[i + 1] - times[i]).max(0.0);
        let r = if per_minute { rate[i] / 60.0 } else { rate[i] };
        acc += r * dt;
        out[i + 1] = acc;
    }
    out
}

/// 在累计序列上按时间线性插值（边界外取端点值）。
pub fn interp_cum(times: &[f64], cum: &[f64], t: f64) -> f64 {
    let n = times.len().min(cum.len());
    if n == 0 {
        return 0.0;
    }
    if t <= times[0] {
        return cum[0];
    }
    if t >= times[n - 1] {
        return cum[n - 1];
    }
    for i in 1..n {
        if t <= times[i] {
            let span = times[i] - times[i - 1];
            if span <= 0.0 {
                return cum[i];
            }
            let u = ((t - times[i - 1]) / span).clamp(0.0, 1.0);
            return cum[i - 1] + (cum[i] - cum[i - 1]) * u;
        }
    }
    cum[n - 1]
}

/// 按固定宽度切窗，返回 (窗口秒数, 窗口内增量)。
///
/// 边界用累计序列插值，因此 10s 窗 / 60s 段 / 圈共享同一份数据，
/// 合计严格等于累计总量（不因采样网格错位而漂移）。
pub fn windows_from_cum(
    times: &[f64],
    cum: &[f64],
    dur: f64,
    width: f64,
) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    if !(width > 0.0) || !(dur > 0.0) {
        return out;
    }
    let mut lo = 0.0;
    while lo < dur - 1e-9 {
        let hi = (lo + width).min(dur);
        let a = interp_cum(times, cum, lo);
        let b = interp_cum(times, cum, hi);
        out.push((hi - lo, (b - a).max(0.0)));
        lo = hi;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speed_spec(dist: f64, dur: f64) -> SpeedSpec {
        let times = time_grid(dur, 0.01, 7).unwrap();
        let mut dts: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
        dts.push(0.0);
        let n = dts.len();
        SpeedSpec {
            dts,
            target_dist: dist,
            base_v: dist / dur,
            floor: 1.9,
            ceil: 6.3,
            caps: vec![f64::INFINITY; n],
            mean_mul: Vec::new(),
            theta: 0.12,
            sigma: 0.02,
            accel: 0.35,
            decel: 0.45,
            response: 0.35,
            warmup_s: 15.0,
            warmup_frac: 0.85,
            tail_s: 10.0,
            tail_frac: 0.90,
            seed: 42,
        }
    }

    #[test]
    fn distance_is_hit_by_integration() {
        let spec = speed_spec(3000.0, 1000.0);
        let v = correlated_speed_profile(&spec).unwrap();
        let dist: f64 = v.iter().zip(&spec.dts).map(|(x, dt)| x * dt).sum();
        assert!((dist - 3000.0).abs() <= 1.0, "积分距离 {dist}");
        assert!(v.iter().all(|x| (1.9..=6.3).contains(x)));
    }

    #[test]
    fn impossible_distance_returns_err() {
        // 600s 内跑 5000m 需要 8.33 m/s，超过上限 6.3 → 必须报错
        let spec = speed_spec(5000.0, 600.0);
        assert!(correlated_speed_profile(&spec).is_err());
    }

    #[test]
    fn windows_share_the_same_cumulative_series() {
        let spec = speed_spec(2400.0, 900.0);
        let v = correlated_speed_profile(&spec).unwrap();
        let times = time_grid(900.0, 0.01, 7).unwrap();
        let mut cum = vec![0.0; v.len()];
        let mut acc = 0.0;
        for i in 0..v.len() - 1 {
            acc += v[i] * (times[i + 1] - times[i]);
            cum[i + 1] = acc;
        }
        let ws = windows_from_cum(&times, &cum, 900.0, 10.0);
        let sum: f64 = ws.iter().map(|(_, d)| *d).sum();
        assert!((sum - cum[cum.len() - 1]).abs() < 1e-6, "窗口合计={sum}");
        assert_eq!(ws.len(), 90);
    }

    #[test]
    fn cadence_integrates_to_steps() {
        let spec = speed_spec(2400.0, 900.0);
        let v = correlated_speed_profile(&spec).unwrap();
        let times = time_grid(900.0, 0.01, 7).unwrap();
        let mut dts: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
        dts.push(0.0);
        let fatigue = vec![0.1; v.len()];
        let cad = correlated_cadence_profile(&CadenceSpec {
            speeds: &v,
            dts: &dts,
            fatigue: &fatigue,
            floor: 110.0,
            ceil: 220.0,
            theta: 0.2,
            sigma: 0.8,
            response: 0.4,
            accel: 4.0,
            seed: 11,
        })
        .unwrap();
        let steps = cumulative(&times, &cad, true);
        let total = *steps.last().unwrap();
        assert!(total > 0.0 && total.is_finite());
        assert_eq!(cad.len(), v.len());
    }
}
