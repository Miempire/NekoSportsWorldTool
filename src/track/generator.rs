//! 自然轨迹生成器。
//!
//! 画像驱动：打卡点拟合闭合环（Catmull-Rom，每段 18 采样）→ 弧长表；
//! 采样间隔主 5s（80%，内部网格），输出点再按 1s 加密（`postfix::densify_locations`）；
//! 速度 = 相关随机目标 × 热身/疲劳/波动/凹陷 + 一阶响应 + 弯道限速；
//! 距离由速度积分、步数由步频积分；10s 窗与 60s 段由同一累计序列派生；
//! 哨兵点（首 type∈{0,7}、索引1 type=5、末 type=6）；点位吸附。
//! 距离与时长无法同时满足时返回 `Err`。
#![allow(non_snake_case)]

use super::geom::{fmt_gain_time, make_point_ring, ring_point_at, round_to, to_bd, Rng, MET_PER_DEG_LAT, MET_PER_DEG_LNG};
use super::model::{GenPoint, Segment, TenWindow, Track};
use super::postfix::{apply_post_fixes, densify_locations};
use route_planner::profile::{
    correlated_cadence_profile, correlated_speed_profile, cumulative, time_grid, windows_from_cum,
    CadenceSpec, SpeedSpec,
};

/// 有效配速窗口（判定规则 2'21"-10'00"/km ≈ 1.667-7.092 m/s），硬边界留余量。
pub const SPEED_FLOOR: f64 = 1.90;
pub const SPEED_CEIL: f64 = 6.30;

/// 由用户配置的配速范围（秒/km）换算速度窗口 `(最慢, 最快)`（m/s）。
///
/// 生成器原先固定用 1.90~6.28 m/s 的宽窗口，瞬时速度可以明显偏离用户设定的
/// 配速范围（例如设 6'00"~6'10"/km，波动峰段却跑到 5'30"/km）。这里把窗口
/// 收窄到用户范围本身，并夹在平台硬边界 `[SPEED_FLOOR, SPEED_CEIL]` 内。
/// 上限留 0.3% 余量：`dur = round(dist × pace)` 取整后，若窗口上限恰好等于
/// 目标均值，δ 二分可能因浮点/取整误差凑不满目标距离。
pub fn speed_bounds_from_pace(pace_a_s_per_km: f64, pace_b_s_per_km: f64) -> Option<(f64, f64)> {
    let (fast, slow) = if pace_a_s_per_km <= pace_b_s_per_km {
        (pace_a_s_per_km, pace_b_s_per_km)
    } else {
        (pace_b_s_per_km, pace_a_s_per_km)
    };
    if !(fast.is_finite() && slow.is_finite()) || fast <= 0.0 {
        return None;
    }
    let v_min = (1000.0 / slow).max(SPEED_FLOOR);
    let v_max = ((1000.0 / fast) * 1.003).min(SPEED_CEIL);
    if !(v_min.is_finite() && v_max.is_finite()) || v_max <= v_min {
        return None;
    }
    Some((v_min, v_max))
}

/// 多尺度值噪声（移植自 sw-proto `engine._noise_curve`）。
///
/// `cells` 个随机格点 → smoothstep 插值 → 去均值/归一化 → 乘 `amp`。
/// 与固定周期正弦的区别：**没有固定周期**，克点数每次随机，
/// 所以多条记录的功率谱不会出现同一组峰（固定正弦是明显的指纹）。
fn value_noise(n: usize, cells: usize, amp: f64, rng: &mut Rng) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let cells = cells.max(2);
    let grid: Vec<f64> = (0..=cells).map(|_| rng.gauss(0.0, 1.0)).collect();
    let mut out = Vec::with_capacity(n);
    let denom = (n.max(2) - 1) as f64;
    for i in 0..n {
        let x = i as f64 / denom * cells as f64;
        let i0 = (x.floor() as usize).min(cells);
        let i1 = (i0 + 1).min(cells);
        let f = x - x.floor();
        let s = f * f * (3.0 - 2.0 * f); // smoothstep
        out.push(grid[i0] * (1.0 - s) + grid[i1] * s);
    }
    let mean = out.iter().sum::<f64>() / n as f64;
    let var = out.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let sd = var.sqrt().max(1e-9);
    out.iter().map(|v| (v - mean) / sd * amp).collect()
}

/// 沿闭环逐点曲率限速（向心加速度上限），供速度剖面的弯道 cap 使用。
fn ring_speed_caps(
    dense: &[(f64, f64)],
    arcs: &[f64],
    s_of: &[f64],
    a_c_max: f64,
    brake: f64,
    lookahead: f64,
    ceil: f64,
) -> Vec<f64> {
    let m = dense.len().min(arcs.len());
    if m < 3 {
        return vec![ceil; s_of.len()];
    }
    let total = *arcs.last().unwrap_or(&1.0);
    if total <= 0.0 {
        return vec![ceil; s_of.len()];
    }
    // 每个稠密采样点的曲率 κ = 转角 / 段长
    let mut vc = vec![ceil; m];
    for i in 0..m {
        let p0 = dense[(i + m - 1) % m];
        let p1 = dense[i];
        let p2 = dense[(i + 1) % m];
        let (v1x, v1y) = (p1.0 - p0.0, p1.1 - p0.1);
        let (v2x, v2y) = (p2.0 - p1.0, p2.1 - p1.1);
        let len1 = (v1x * v1x + v1y * v1y).sqrt();
        let len2 = (v2x * v2x + v2y * v2y).sqrt();
        if len1 < 1e-6 || len2 < 1e-6 {
            continue;
        }
        let mut da = v2y.atan2(v2x) - v1y.atan2(v1x);
        while da > std::f64::consts::PI {
            da -= std::f64::consts::TAU;
        }
        while da < -std::f64::consts::PI {
            da += std::f64::consts::TAU;
        }
        let kappa = da.abs() / (0.5 * (len1 + len2)).max(1e-3);
        vc[i] = (a_c_max / kappa.max(1e-6)).sqrt().min(ceil);
    }
    // 弧长 → 稠密索引
    let idx_at = |s: f64| -> usize {
        let ss = s.rem_euclid(total);
        match arcs[..m].binary_search_by(|v| v.partial_cmp(&ss).unwrap()) {
            Ok(i) => i,
            Err(i) => i.min(m - 1),
        }
    };
    s_of.iter()
        .map(|&s| {
            let mut j = idx_at(s);
            let mut cap = ceil;
            let mut d = 0.0;
            while d <= lookahead {
                let corner = vc[j];
                let allow = (corner * corner + 2.0 * brake * d).sqrt();
                if allow < cap {
                    cap = allow;
                }
                let next = if j + 1 < m { arcs[j + 1] } else { total };
                d += (next - arcs[j]).max(1.0);
                j = (j + 1) % m;
            }
            cap.min(ceil)
        })
        .collect()
}

/// 轨迹生成模式。三种模式**相互独立**，但输出格式（Track / 27 键点集 / OBS 11 键）
/// 完全一致，下游无需区分。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackGenMode {
    /// 现有：沿闭环弧长的三谐波正弦海拔（整数周期，严格闭合）
    #[default]
    Current,
    /// 碗形海拔 + 碗形配速：**两端接近水平/基准、中段下凹（海拔）或加快（配速）**，
    /// 可选叠加**起始段爬升**。
    ///
    /// 海拔剖面 h(u) = base − depth·sin²(πu) + climb·sin²(π·min(u/0.25,1))·(1−u)
    /// 配速趋势 m(u) = 1 + depth_v·sin²(πu)（两端 1.0、中段 1+depth_v）
    /// 其中 u = 弧长（海拔）/ 时间（配速）进度 ∈ [0,1]：
    /// - `sin²(πu)` 在 u=0 与 u=1 处值与一阶导数都为 0 → 起终点是平的
    /// - 海拔中段 u=0.5 处下凹 depth 米；配速中段比两端快 depth_v（10~22%）
    /// - 起始爬升只在 u∈[0,0.25] 抬起，之后随 (1−u) 衰减，保证仍以水平收尾
    ///
    /// **总距离/平均配速不变**：碗形只改趋势形状，`correlated_speed_profile`
    /// 仍会二分命中目标距离，末端 `∫v dt = Σ v_i·Δt_i` 的微积分核验照旧守住。
    Bowl,
    /// Niko kinematics 口径：速度剖面的 OU 参数与限速参数取 Niko 默认值
    /// （theta 0.12 / sigma 0.06，a_c 2.2 / brake 1.2 / lookahead 30，迭代 3 轮），
    /// 波动比 Current 更明显、弯道收速更保守。
    Niko,
}

/// 轨迹生成主入口（= `build_with_mode(..., TrackGenMode::Current, false)`）。
///
/// `v_bounds` 为速度窗口 `(最慢, 最快)`（m/s），由用户配速范围换算而来；
/// `None` 时沿用历史宽窗口（1.92~6.28 m/s）。
pub fn build(
    dist: f64,
    dur: i64,
    seed: u64,
    center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
    v_bounds: Option<(f64, f64)>,
) -> Result<Track, String> {
    build_with_mode(
        dist,
        dur,
        seed,
        center,
        start_ms,
        points_bd,
        v_bounds,
        TrackGenMode::Current,
        false,
        false,
    )
}

/// 带生成模式的入口（`Bowl` 的「起始段爬升」由 `bowl_climb` 控制）。
pub fn build_with_mode(
    dist: f64,
    dur: i64,
    seed: u64,
    _center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
    v_bounds: Option<(f64, f64)>,
    mode: TrackGenMode,
    bowl_climb: bool,
    // 海拔是否使用碗形剖面（与配速独立控制；mode 只决定配速形态）
    bowl_altitude: bool,
) -> Result<Track, String> {
    let mut rng = Rng::new(seed);
    let dur_f = dur as f64;
    let (dense, arcs, pc) = make_point_ring(points_bd);
    let (c_lat, c_lng) = pc;
    let direction: f64 = rng.choice(&[1.0, -1.0]);
    // 【Bowl 模式】碗形剖面按弧长进度 u = s/总弧长 参数化，
    // 必须从环起点（u=0）起跑，"开始水平"才成立；其它模式保持随机起跑点。
    let s0 = if bowl_altitude {
        0.0
    } else {
        rng.uniform(0.0, *arcs.last().unwrap_or(&400.0))
    };
    // 保留抽样以维持 rng 序列（噪声模型已不需要相位）
    let _phase_v = rng.uniform(0.0, std::f64::consts::TAU);

    // 采样节拍：冷启动前两次 1s 快速定位，之后 5s 定时；丢点概率下调
    // （丢点造成 10/15s 间隔是地图灰色段的主要来源）
    let p_drop = rng.uniform(0.002, 0.012);
    let times = time_grid(dur_f, p_drop, seed)?;
    let n = times.len();
    let mut dts: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    dts.push(0.0);

    let n_dips = rng.choice(&[1, 1, 2]);
    let mut dips = Vec::new();
    for _ in 0..n_dips {
        dips.push((
            rng.uniform(0.15, 0.75) * dur_f,
            rng.uniform(25.0, 55.0),
            rng.uniform(0.05, 0.11),
        ));
    }
    let dip_factor = |t: f64| -> f64 {
        let mut f = 1.0;
        for &(c, hw, d) in &dips {
            if (t - c).abs() < hw {
                f *= 1.0 - d * 0.5 * (1.0 + std::f64::consts::PI * (t - c) / hw).cos();
            }
        }
        f
    };
    // ★ 多尺度值噪声（移植自 sw-proto engine._speed_profile）
    //
    // 无固定周期的随机过程：粗/慢/中/快四档平滑值噪声，每档格点数随机，
    // 替代原来的四条固定周期正弦（100~140s / 38~56s / 15~23s / 180~360s）。
    // 固定正弦的功率谱上有 4 个稳定的峰，多条记录只是相位不同 —— 那是可聚类的指纹。
    let noise_block = value_noise(n, rng.randint(2, 4) as usize, 0.008, &mut rng);
    let noise_slow = value_noise(n, rng.randint(4, 7) as usize, 0.012, &mut rng);
    let noise_medium = value_noise(n, rng.randint(10, 16) as usize, 0.014, &mut rng);
    let noise_fast = value_noise(n, rng.randint(26, 44) as usize, 0.0120, &mut rng);
    // 疲劳降速：5km 内相对起点约 -4~-9%，长距离收敛到 fat_min（相对 fat0 下降 3~10%）。
    // 同时叠加「回拨」：慢周期波谷/凹陷（相对趋势跑慢）时，疲劳负债按 τ≈50~85s
    // 缓释（回补 55~85% 的降幅），慢段结束后再以 2.5τ 缓慢消退，形成
    // 「慢下来还能缓过来」，而不是一路单向下坡。
    let fat0 = rng.uniform(1.00, 1.04);
    let fat_slope = rng.uniform(0.010, 0.020);
    let fat_min = rng.uniform(0.94, 0.97);
    let rec_tau = rng.uniform(50.0, 85.0);
    let rec_max = rng.uniform(0.55, 0.85);
    let mut rec = 0.0f64;
    let mut rel_ema = 1.0f64;
    let base = dist / dur_f;
    // 【Bowl 配速】碗形速度曲线：两端接近基准、中段加快（与碗形海拔同形）
    // u=0/1 处 sin²(πu)=0 → 1.0；u=0.5 处 → 1+depth
    let bowl_speed_depth = rng.uniform(0.10, 0.22);
    // 速度均值倍率：
    // - Current/Niko：热身 ramp × 疲劳（含回拨）× 四正弦波动 × 余弦凹陷
    // - Bowl        ：热身 ramp × **碗形趋势** × 四正弦波动 × 余弦凹陷
    // （不含白噪声，随机部分统一由 profile 的相关目标 + 一阶响应给出）
    let mut mean_mul = Vec::with_capacity(n);
    for (i, &tt) in times.iter().enumerate() {
        let dt = dts[i].max(0.0);
        let ramp = if tt < 8.0 {
            (0.85 + 0.15 * (tt / 1.0f64.max(8.0f64.min(dur_f / 20.0)))).min(1.0)
        } else {
            1.0
        };
        let wave = (1.0
            + noise_block[i]
            + noise_slow[i]
            + noise_medium[i]
            + noise_fast[i])
            * dip_factor(tt);
        let trend = if mode == TrackGenMode::Bowl {
            // 碗形：中段比两端快 bowl_speed_depth（两端保持 1.0）
            let u = (tt / dur_f).clamp(0.0, 1.0);
            1.0 + bowl_speed_depth * (std::f64::consts::PI * u).sin().powi(2)
        } else {
            let km_done = (tt / dur_f) * dist / 1000.0;
            let fatigue = if km_done <= 0.5 {
                fat0
            } else {
                (fat0 - fat_slope * (km_done - 0.5)).max(fat_min)
            };
            // 回拨状态：相对配速经 ~12s EMA 平滑后比趋势慢 1% 起缓释，慢 4% 达到上限
            // （瞬时波形会被短周期噪声打散，平滑后才能识别「持续跑慢」这一段）。
            let rel_alpha = 1.0 - (-dt / 12.0).exp();
            rel_ema += (wave - rel_ema) * rel_alpha;
            let slow = (1.0 - rel_ema).max(0.0);
            let rec_tgt = rec_max * ((slow - 0.01) / 0.03).clamp(0.0, 1.0);
            let k = if rec_tgt > rec {
                1.0 / rec_tau
            } else {
                1.0 / (2.5 * rec_tau)
            };
            rec += (rec_tgt - rec) * (1.0 - (-k * dt).exp());
            // 回拨量按「疲劳负债」折算回起点状态 fat0（而非折算到 1.0）：
            // 起点乘数 fat0 > 1，若折向 1.0 会出现「恢复后比刚开跑还快」的反向偏差。
            fat0 - (fat0 - fatigue) * (1.0 - rec)
        };
        mean_mul.push(ramp * trend * wave);
    }
    // 逐点速度约束在有效配速窗口内侧；优先用用户配速范围换算的窗口，
    // 未提供时按次抽样，避免固定钳制值形成平顶。
    let (v_lo, v_hi) = match v_bounds {
        Some((lo, hi)) if hi > lo => (lo, hi),
        _ => (rng.uniform(1.92, 1.98), rng.uniform(6.22, 6.28)),
    };
    let speed_seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    // 【三模式】速度剖面参数与弯道限速参数按模式取：
    // - Current：历史上验证过的温和口径（疲劳降速 + 波动）
    // - Niko   ：OU 波动更大（theta 0.12 / sigma 0.06），弯道更保守、迭代 3 轮
    // - Bowl   ：OU 参数同 Current，但趋势项换成**碗形**（两端慢、中间快）
    let (ou_theta, ou_sigma, sp_accel, sp_decel, sp_response) = match mode {
        TrackGenMode::Niko => (0.12, 0.060, 0.60, 0.50, 0.50),
        _ => (0.15, 0.010, 0.55, 0.45, 0.45),
    };
    let (ac_max, brake, lookahead, cap_rounds) = match mode {
        TrackGenMode::Niko => (2.2, 1.2, 30.0, 3usize),
        _ => (2.5, 1.5, 25.0, 2usize),
    };
    let build_spec = |caps: Vec<f64>| SpeedSpec {
        dts: dts.clone(),
        target_dist: dist,
        base_v: base,
        floor: v_lo,
        ceil: v_hi,
        caps,
        mean_mul: mean_mul.clone(),
        theta: ou_theta,
        sigma: ou_sigma,
        accel: sp_accel,
        decel: sp_decel,
        response: sp_response,
        warmup_s: 0.0,
        warmup_frac: 1.0,
        tail_s: 0.0,
        tail_frac: 1.0,
        seed: speed_seed,
    };
    let mut speeds = correlated_speed_profile(&build_spec(vec![v_hi; n]))?;
    // 弯道限速：限速会改变弧长，迭代收敛
    for _ in 0..cap_rounds {
        let mut s_of = vec![0.0f64; n];
        let mut acc = s0;
        for i in 0..n {
            s_of[i] = acc;
            acc += direction * speeds[i] * dts[i];
        }
        let caps = ring_speed_caps(&dense, &arcs, &s_of, ac_max, brake, lookahead, v_hi);
        speeds = correlated_speed_profile(&build_spec(caps))?;
    }
    // ★ 配速带等比收缩 + 瞬时上限（移植自 sw-proto engine._speed_profile）
    //
    // 两个真机实测结论（见 sw-proto-suite 的注释）：
    // 1) App 的「最快配速」= min(瞬时速度)，而真机 9'00" 记录的最快配速约 8'38"，
    //    即**瞬时速度上限只比均值快 ~4.4%**；旧上限 1.08 会造出 8'20" 的极值。
    // 2) 逐点硬 clamp 只约束单点，不约束「10 秒窗均值」——边界附近 10 秒窗必然越界。
    //    改成**围绕均值等比收缩** `v' = base + (v − base)·k`：均值不变，
    //    所以平均配速/总距离分毫不动，只是波动幅度压进允许区间。
    let base_v = dist / dur_f;
    // 两端各留 1.5% 余量：App 按整数秒显示，贴边界会四舍五入越界
    let lo_v = v_lo * 1.015;
    let hi_v = (base_v * 1.044).min(v_hi) * 0.985;
    if hi_v > lo_v {
        let mut k_max = 1.0f64;
        for &v in &speeds {
            let d = v - base_v;
            if d > 1e-9 {
                k_max = k_max.min((hi_v - base_v) / d);
            } else if d < -1e-9 {
                k_max = k_max.min((lo_v - base_v) / d);
            }
        }
        let k = k_max.clamp(0.0, 1.0);
        if k < 1.0 {
            for v in &mut speeds {
                *v = base_v + (*v - base_v) * k;
            }
        }
    }

    let cum_dist = cumulative(&times, &speeds, false);
    let integral_dist = *cum_dist.last().unwrap_or(&0.0);
    if (integral_dist - dist).abs() > 1.0 {
        return Err(format!(
            "轨迹距离与目标不一致：速度积分 {:.1}m ≠ 请求 {:.1}m",
            integral_dist, dist
        ));
    }
    // 步频：相关随机目标 + 一阶响应；步数由积分得到
    let fatigue_deg: Vec<f64> = (0..n)
        .map(|i| ((times[i] / dur_f).clamp(0.0, 1.0) * dist / 1000.0 * 1.5 - 0.5).clamp(0.0, 0.6))
        .collect();
    let cadences = correlated_cadence_profile(&CadenceSpec {
        speeds: &speeds,
        dts: &dts,
        fatigue: &fatigue_deg,
        floor: 110.0,
        ceil: 220.0,
        theta: 0.25,
        sigma: 0.6,
        response: 0.45,
        accel: 4.0,
        seed: seed.wrapping_add(0xC6A4_A793_5BD1_E995),
    })?;
    let cum_steps = cumulative(&times, &cadences, true);

    // 真机正常跑步几乎看不到灰线：type=3 与暂停/漂移点都压到"整场 0~3 处"。
    let tl_w = [((-1i64, 4i64), 54u32), ((-1, 1), 27), ((-1, 12), 13), ((-1, 5), 3), ((-1, 6), 2)];
    let p_type3 = rng.uniform(0.001, 0.004);
    let p_odd = rng.uniform(0.001, 0.003);
    let p_drift = p_odd + rng.uniform(0.0005, 0.002);
    let mut kinds: Vec<(i64, i64)> = Vec::with_capacity(n);
    for _ in 0..n {
        let u = rng.random();
        if u < p_type3 {
            kinds.push((3, 1));
        } else if u < 1.0 - p_drift {
            kinds.push((0, 1));
        } else if u < 1.0 - p_odd {
            kinds.push(rng.weighted(&tl_w));
        } else {
            kinds.push((rng.choice(&[1, 1, 1, 1, 2, 2]), 1));
        }
    }
    for i in 1..kinds.len() {
        let prev_drift = (-1 == kinds[i - 1].0) || kinds[i - 1].0 == 5 || kinds[i - 1].0 == 6;
        // 暂停后状态切换概率降低（原 0.08），减少漂移-正常-漂移的快速抖动
        if kinds[i].0 != -1 && prev_drift && rng.random() < 0.03 {
            kinds[i] = (if rng.random() < 0.75 { 7 } else { 8 }, 1);
        }
    }
    for i in 1..n {
        if kinds[i].0 == -1 && kinds[i - 1].0 == -1 {
            kinds[i] = (rng.choice(&[3, 0]), 1);
        }
    }
    let mut locs: Vec<GenPoint> = Vec::with_capacity(n);
    let mut s_at: Vec<f64> = Vec::with_capacity(n);
    let mut s = s0;
    let mut jx = 0.0f64;
    let mut jy = 0.0f64;
    // 海拔：校区基准 + 沿闭环弧长的多谐波起伏（整数周期，严格闭合）。
    // 整数周期保证「同一地点海拔恒定 + 起终点闭合」；叠加长/中/短三档谐波
    // 后形状接近真实地形（长坡 + 小起伏），而不是一眼可辨的单条正弦。
    let alt_base = 82.0 + rng.uniform(-3.0, 3.0);
    let alt_sigma = rng.uniform(1.6, 2.8); // GPS 海拔逐点测量噪声标准差（m）
    let alt_len = (*arcs.last().unwrap_or(&1.0)).max(1.0);
    let mut alt_harm: Vec<(f64, f64, f64)> = Vec::with_capacity(3);
    alt_harm.push((
        rng.choice(&[1, 1, 2, 2]) as f64,
        rng.uniform(1.0, 3.0),
        rng.uniform(0.0, std::f64::consts::TAU),
    ));
    alt_harm.push((
        rng.choice(&[2, 3, 3, 4]) as f64,
        rng.uniform(0.4, 1.2),
        rng.uniform(0.0, std::f64::consts::TAU),
    ));
    if rng.random() < 0.7 {
        alt_harm.push((
            rng.choice(&[5, 6, 7, 8]) as f64,
            rng.uniform(0.15, 0.45),
            rng.uniform(0.0, std::f64::consts::TAU),
        ));
    }
    // 碗形模式的海拔参数（只有 Bowl 模式会用到，先抽样不影响其它模式）
    let bowl_depth = rng.uniform(4.0, 12.0);
    let bowl_climb_h = if bowl_climb { rng.uniform(2.0, 7.0) } else { 0.0 };
    let alt_at = |ss: f64| {
        if bowl_altitude {
            // 归一化弧长进度 u ∈ [0,1]
            let u = (ss / alt_len).clamp(0.0, 1.0);
            // 主碗：sin²(πu) 在 u=0/1 处值与一阶导数都为 0 → 起终点水平
            let bowl = -(std::f64::consts::PI * u).sin().powi(2) * bowl_depth;
            // 起始爬升：只在 u∈[0,0.25] 抬起，之后按 (1-u) 衰减，收尾仍水平
            let climb = if bowl_climb_h > 0.0 {
                let x = (u / 0.25).clamp(0.0, 1.0);
                (std::f64::consts::PI * x).sin().powi(2) * bowl_climb_h * (1.0 - u)
            } else {
                0.0
            };
            alt_base + bowl + climb
        } else {
            let mut v = alt_base;
            for &(n, amp, ph) in &alt_harm {
                v += amp * (std::f64::consts::TAU * n * ss / alt_len + ph).sin();
            }
            v
        }
    };

    for i in 0..n {
        let dt = dts[i];
        let (typ, lt) = kinds[i];
        // 距离恒来自速度积分（异常点不改变累计里程）
        let d_step = speeds[i] * dt;
        // 先落位、后累加：点 i 必须落在 cum_dist[i]（即 times[i] 时刻）处。
        // 若先累加，第 i→i+1 点的几何位移会取到「下一段」的时间间隔：5s→10s/15s
        // 跳变处几何速度变成 2×/0.5×/0.33×，首段 1s 更是 5×。App 若按坐标推速度/
        // 配速着色，这些段就会显示成异常条块。
        s_at.push(s);
        let pos = |ss: f64| ring_point_at(&dense, &arcs, ss);
        let px;
        let py;
        let x;
        let y;
        let rad;
        let state;
        if typ != -1 {
            let (bx, by) = pos(s);
            x = bx;
            y = by;
            jx = 0.72 * jx + rng.gauss(0.0, 0.75);
            jy = 0.72 * jy + rng.gauss(0.0, 0.75);
            px = bx + jx;
            py = by + jy;
            rad = round_to(
                if typ == 3 { rng.uniform(1.4, 5.1) } else { rng.uniform(1.4, 2.4) },
                2,
            );
            state = if typ == 0 {
                rng.weighted(&[(1, 145), (2, 45), (3, 164)])
            } else {
                rng.weighted(&[(1, 145), (2, 256), (3, 151)])
            };
        } else {
            // 异常点的报告位置偏移（不影响累计里程）
            let mut off = 0.0f64;
            match lt {
                4 => {
                    if rng.random() >= 0.68 {
                        off = if rng.random() < 0.95 { rng.uniform(2.0, 60.0) } else { rng.uniform(60.0, 250.0) };
                    }
                }
                1 => {
                    if rng.random() >= 0.83 {
                        off = rng.uniform(0.5, 36.0);
                    }
                }
                12 => {
                    off = if rng.random() < 0.9 { rng.uniform(5.0, 80.0) } else { rng.uniform(80.0, 220.0) };
                }
                5 => off = rng.uniform(5.0, 60.0),
                _ => off = rng.uniform(100.0, 300.0),
            }
            let (bx, by) = pos(s);
            x = bx;
            y = by;
            if off > 0.0 {
                let ang = rng.uniform(0.0, std::f64::consts::TAU);
                px = x + off * ang.sin();
                py = y + off * ang.cos();
            } else if let Some(last) = locs.last() {
                // 零位移：精确复制上一点坐标（BD→平面）
                py = (last.gLat - c_lat) * MET_PER_DEG_LAT;
                px = (last.gLng - c_lng) * MET_PER_DEG_LNG;
            } else {
                px = x + jx;
                py = y + jy;
            }
            rad = if lt == 4 {
                round_to(if rng.random() < 0.75 { rng.uniform(30.0, 100.0) } else { rng.uniform(100.0, 550.0) }, 2)
            } else if lt == 1 {
                round_to(if rng.random() < 0.75 { rng.uniform(1.6, 12.0) } else { rng.uniform(12.0, 95.0) }, 2)
            } else if lt == 12 {
                round_to(rng.uniform(30.0, 125.0), 2)
            } else if lt == 5 {
                round_to(rng.uniform(25.0, 300.0), 2)
            } else {
                550.0
            };
            state = rng.weighted(&[(1, 102), (2, 124), (3, 136)]);
        }
        let (lat, lng) = to_bd(px, py, c_lat, c_lng);
        let alt = alt_at(s) + rng.gauss(0.0, alt_sigma);
        let nxt = pos(s + direction * 2.0);
        let brg = ((nxt.0 - x).atan2(nxt.1 - y).to_degrees() + rng.gauss(0.0, 12.0)).rem_euclid(360.0);
        // 异常点（-1）：avgSpeed 为累计均值（真人与此一致，不为 0）；
        // GPS 瞬时速度多为低速，偶发 15-46 km/h 漂移尖峰
        let (avg_sp, gps_speed) = if typ == -1 {
            let avg = round_to(cum_dist[i] / times[i].max(1.0), 4);
            let gps = if rng.random() < 0.12 {
                rng.uniform(15.0, 46.0)
            } else {
                rng.uniform(0.5, 6.0)
            };
            (avg, round_to(gps, 4))
        } else {
            let avg = if dt > 0.0 {
                round_to(d_step / dt, 4)
            } else {
                round_to(speeds[i], 4)
            };
            let kmh = avg * 3.6;
            let sigma = (kmh * 0.08).max(0.05);
            let gps = if rng.random() < 0.04 {
                round_to(kmh * rng.uniform(0.15, 0.55), 4)
            } else {
                round_to((kmh + rng.gauss(0.0, sigma)).max(0.0), 4)
            };
            (avg, gps)
        };

        // 每点步数 = 与上一点的增量（不是随机数），步幅由该段距离/步数反推，
        // 保证 sum(count)==totalSteps 且 stepDistance 与配速自洽。
        let steps_now = cum_steps[i].round() as i64;
        let steps_prev = if i == 0 { 0 } else { cum_steps[i - 1].round() as i64 };
        let interval_steps = (steps_now - steps_prev).max(0);
        let interval_dist = if i == 0 { cum_dist[i] } else { (cum_dist[i] - cum_dist[i - 1]).max(0.0) };
        let step_distance = if interval_steps > 0 {
            (interval_dist / interval_steps as f64).clamp(0.5, 1.8)
        } else {
            0.0
        };

        locs.push(GenPoint {
            id: i as i64 + 1,
            flag: start_ms,
            lat: -1.0,
            lng: -1.0,
            gLat: round_to(lat, 7),
            gLng: round_to(lng, 7),
            speed: round_to(gps_speed, 4),
            avgSpeed: avg_sp,
            radius: rad,
            accuracy: round_to(
                (rad * rng.uniform(0.7, 1.4)).min(28.0 + 2.0 * rng.random()),
                2,
            ),
            ptype: typ,
            locType: lt,
            hasAltitude: true,
            totalTime: round_to(times[i], 0) as i64,
            totalDis: round_to(cum_dist[i], 4),
            validDis: round_to(cum_dist[i], 4),
            validTime: round_to(times[i], 0) as i64,
            steps: steps_now,
            stepDistance: round_to(step_distance, 2),
            gainTime: fmt_gain_time(start_ms + (times[i] * 1000.0) as i64),
            gainTimeMs: start_ms + (times[i] * 1000.0) as i64,
            queueNum: 0,
            coorType: "gcj02".into(),
            bdA: round_to(alt, 2),
            bdD: round_to(brg, 2),
            bdS: round_to((avg_sp * rng.uniform(0.6, 0.95)).max(0.0), 3),
            bdG: rng.choice(&[1, 1, 1, -1]),
            count: interval_steps,
            dtr: 0.0,
            state,
            locationId: String::new(),
        });
        s += direction * d_step;
    }
    // ★ 表观配速校正（移植自 sw-proto engine._fit_pace_bounds）
    //
    // App 的配速图按「相邻两点弦长 / 时间差」算，而我们是按弧长推进的：
    // 折返/掉头处相邻两点落在去程/回程两条腿上，弦长只有横向间距（5~6 m），
    // 5 秒走这么点就被画成 14'00"~16'00" → 整条记录像「配速超出学校区间」。
    //
    // 这里按弦长重排时间戳：只动时间、不动坐标，**总时长严格不变**。
    // 区间由 v_bounds 反推（m/s → 秒/公里）。
    let pace_bounds_s = v_bounds.map(|(vmin, vmax)| (1000.0 / vmax, 1000.0 / vmin));
    let (_fit_moved, fitted_times) =
        crate::track::pace_fit::fit_pace_bounds(&mut locs, pace_bounds_s, start_ms);
    // 遮蔽原时间轴：下面的 10s 窗 / 60s 段都用校正后的时间
    let times = fitted_times;

    // 10s 窗 / 60s 段与圈同源：全部由累计序列按时间边界插值派生
    let ten_speed: Vec<TenWindow> = windows_from_cum(&times, &cum_dist, dur_f, 10.0)
        .into_iter()
        .map(|(sec, d)| TenWindow { time: sec.round().max(1.0) as i64, value: round_to(d, 2) })
        .collect();
    let ten_steps: Vec<TenWindow> = windows_from_cum(&times, &cum_steps, dur_f, 10.0)
        .into_iter()
        .map(|(sec, s)| TenWindow { time: sec.round().max(1.0) as i64, value: round_to(s, 0) })
        .collect();

    let mut segments: Vec<Segment> = Vec::new();
    let mut cursor = 0.0f64;
    for ((sec, d), (_, st)) in windows_from_cum(&times, &cum_dist, dur_f, 60.0)
        .into_iter()
        .zip(windows_from_cum(&times, &cum_steps, dur_f, 60.0))
    {
        let start = cursor;
        cursor += sec;
        segments.push(Segment {
            totalTime: sec.round().max(1.0) as i64,
            distance: round_to(d, 0) as i64,
            startTime: round_to(start * 1000.0, 0) as i64,
            endTime: round_to(cursor * 1000.0, 0) as i64,
            avgSpeed: round_to(d / sec.max(1.0), 3),
            avgStep: round_to(st / sec.max(1.0) * 60.0, 0) as i64,
            state: 0,
        });
    }

    // 点位吸附：<40m 精确落位。
    // 必须在加密前完成：加密按段插值偏移量，会把这次修正分摊到相邻 1s 点上；
    // 若在加密后吸附，几米的修正全压在一个 1s 点上，几何速度会出现尖峰。
    for pl in points_bd {
        let mut best_i = None;
        let mut best_d = 1e18f64;
        for (i, q) in locs.iter().enumerate() {
            let dd = ((q.gLat - pl.0) * MET_PER_DEG_LAT).powi(2)
                + ((q.gLng - pl.1) * MET_PER_DEG_LNG).powi(2);
            if dd < best_d {
                best_d = dd;
                best_i = Some(i);
            }
        }
        if let Some(i) = best_i {
            if best_d < 40.0 * 40.0 {
                locs[i].gLat = round_to(pl.0, 7);
                locs[i].gLng = round_to(pl.1, 7);
            }
        }
    }

    // 采样加密：5s 主网格（含丢点 10/15s）按 1s 补点，位置沿环线弧长插值。
    // 官方 App 用上传点连线着色，稀疏点会在图上显示为灰色段。
    let mut locs = densify_locations(
        locs,
        &times,
        &cum_dist,
        &cum_steps,
        &s_at,
        1.0,
        |ss| ring_point_at(&dense, &arcs, ss),
        |la, lo| {
            (
                (lo - c_lng) * MET_PER_DEG_LNG,
                (la - c_lat) * MET_PER_DEG_LAT,
            )
        },
        |x, y| to_bd(x, y, c_lat, c_lng),
        &mut rng,
    );
    apply_post_fixes(&mut locs, &mut rng, start_ms);
    // 瞬间配速（App 用 `speed`，单位 km/h）同样要落在用户配速范围内：
    // 原先 4% 的「GPS 低速」与异常点 15~46km/h 的漂移尖峰，会让个别点显示成
    // 1'30"~10'+/km 的极端配速。这里统一夹到同一速度窗口（km/h = m/s × 3.6）。
    let kmh_lo = round_to(v_lo * 3.6, 4);
    let kmh_hi = round_to(v_hi * 3.6, 4);
    for p in locs.iter_mut() {
        if p.speed > 0.0 {
            p.speed = round_to(p.speed.clamp(kmh_lo, kmh_hi), 4);
        }
    }

    let total_dis = round_to(integral_dist, 3);
    let total_time = round_to(dur_f, 0) as i64;
    let total_steps = cum_steps.last().copied().unwrap_or(0.0).round() as i64;
    Ok(Track {
        totalTime: total_time,
        totalDistance: total_dis,
        validDistance: total_dis,
        validTime: total_time,
        startTime: start_ms,
        startLatitude: locs[0].gLat,
        startLongitude: locs[0].gLng,
        totalSteps: total_steps,
        locations: locs,
        speedPerTenSec: ten_speed,
        stepsPerTenSec: ten_steps,
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 约 400m 边长的三角环（BD 坐标，单位为度）
    fn ring_pts() -> Vec<(f64, f64)> {
        let (lat, lng) = (38.9000_f64, 121.5400_f64);
        let dlat = 400.0 / 111_320.0;
        let dlng = 400.0 / (111_320.0 * lat.to_radians().cos());
        vec![(lat, lng), (lat + dlat, lng), (lat + dlat / 2.0, lng + dlng)]
    }

    /// 三种模式都能生成，且**微积分核验**成立：Σ v_i·Δt_i = 总距离，
    /// 平均配速 = 总时长 / 总距离，落在合理区间。
    #[test]
    fn three_modes_keep_requested_distance_and_pace() {
        let pts = ring_pts();
        let (dist, dur) = (1000.0_f64, 360_i64);
        for mode in [
            TrackGenMode::Current,
            TrackGenMode::Bowl,
            TrackGenMode::Niko,
        ] {
            let t = build_with_mode(
                dist,
                dur,
                42,
                (38.9, 121.54),
                1_788_958_186_123,
                &pts,
                None,
                mode,
                true,
                mode == TrackGenMode::Bowl,
            )
            .unwrap_or_else(|e| panic!("{mode:?} 生成失败: {e}"));
            // 速度积分的末端累计必须命中目标距离（容忍 2m 采样误差）
            assert!(
                (t.totalDistance - dist).abs() < 2.0,
                "{mode:?} 距离不符: 积分 {} vs 请求 {dist}",
                t.totalDistance
            );
            assert_eq!(t.totalTime, dur, "{mode:?} 时长不符");
            let pace = t.totalTime as f64 / 60.0 / (t.totalDistance / 1000.0);
            assert!(
                (3.0..15.0).contains(&pace),
                "{mode:?} 平均配速异常: {pace:.2} 分/公里"
            );
        }
    }

    /// Bowl 模式：配速也是碗形 —— 中段平均速度高于两端。
    #[test]
    fn bowl_speed_is_faster_in_the_middle() {
        let pts = ring_pts();
        let t = build_with_mode(
            1200.0,
            420,
            11,
            (38.9, 121.54),
            1_788_958_186_123,
            &pts,
            None,
            TrackGenMode::Bowl,
            false,
            false,
        )
        .expect("build");
        let v: Vec<f64> = t.locations.iter().map(|p| p.speed).collect();
        let n = v.len();
        assert!(n >= 25, "点数太少: {n}");
        let seg = (n / 5).max(1);
        let mean = |r: &[f64]| r.iter().sum::<f64>() / r.len() as f64;
        let head = mean(&v[..seg]);
        let center = mean(&v[n / 2 - seg / 2..n / 2 + seg / 2]);
        let tail = mean(&v[n - seg..]);
        // 中段应明显快于两端（碗形深度 10~22%，留足噪声余量）
        assert!(
            center > head * 1.03,
            "中段没有加快: 中 {center:.2} 头 {head:.2}"
        );
        assert!(
            center > tail * 1.03,
            "中段没有加快: 中 {center:.2} 尾 {tail:.2}"
        );
    }

    /// Bowl 模式：两端海拔接近且中段明显下凹（碗形）。
    #[test]
    fn bowl_altitude_has_flat_ends_and_middle_dip() {
        let pts = ring_pts();
        let t = build_with_mode(
            1000.0,
            360,
            42,
            (38.9, 121.54),
            1_788_958_186_123,
            &pts,
            None,
            TrackGenMode::Bowl,
            false,
            true,
        )
        .expect("build");
        let a: Vec<f64> = t.locations.iter().map(|p| p.bdA).collect();
        let n = a.len();
        assert!(n >= 25, "点数太少: {n}");
        // 用分段均值抵抗 GPS 高程噪声（σ 1.6~2.8m）
        let seg = (n / 5).max(1);
        let mean = |r: &[f64]| r.iter().sum::<f64>() / r.len() as f64;
        let head = mean(&a[..seg]);
        let center = mean(&a[n / 2 - seg / 2..n / 2 + seg / 2]);
        let tail = mean(&a[n - seg..]);
        assert!(
            (head - tail).abs() < 3.0,
            "两端不水平: 头 {head:.2} 尾 {tail:.2}"
        );
        assert!(
            center < head - 3.0,
            "中段没有下落: 中 {center:.2} 头 {head:.2}"
        );
    }
}