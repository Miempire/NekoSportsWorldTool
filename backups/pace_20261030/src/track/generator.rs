//! 自然轨迹生成器。
//!
//! 画像驱动：打卡点拟合闭合环（Catmull-Rom，每段 18 采样）→ 弧长表；
//! 采样间隔主 5s（80%）；速度 = 相关随机目标 × 热身/疲劳/波动/凹陷 + 一阶响应 + 弯道限速；
//! 距离由速度积分、步数由步频积分；10s 窗与 60s 段由同一累计序列派生；
//! 哨兵点（首 type∈{0,7}、索引1 type=5、末 type=6）；点位吸附。
//! 距离与时长无法同时满足时返回 `Err`。
#![allow(non_snake_case)]

use super::geom::{fmt_gain_time, make_point_ring, ring_point_at, round_to, to_bd, Rng, MET_PER_DEG_LAT, MET_PER_DEG_LNG};
use super::model::{GenPoint, Segment, TenWindow, Track};
use super::postfix::apply_post_fixes;
use route_planner::profile::{
    correlated_cadence_profile, correlated_speed_profile, cumulative, time_grid, windows_from_cum,
    CadenceSpec, SpeedSpec,
};

/// 有效配速窗口（判定规则 2'21"-10'00"/km ≈ 1.667-7.092 m/s），硬边界留余量。
pub const SPEED_FLOOR: f64 = 1.90;
pub const SPEED_CEIL: f64 = 6.30;

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

/// 轨迹生成主入口。points_bd 为 BD 系打卡点；不可行参数返回 `Err`。
pub fn build(
    dist: f64,
    dur: i64,
    seed: u64,
    _center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
) -> Result<Track, String> {
    let mut rng = Rng::new(seed);
    let dur_f = dur as f64;
    let (dense, arcs, pc) = make_point_ring(points_bd);
    let (c_lat, c_lng) = pc;
    let direction: f64 = rng.choice(&[1.0, -1.0]);
    let s0 = rng.uniform(0.0, *arcs.last().unwrap_or(&400.0));
    let phase_v = rng.uniform(0.0, std::f64::consts::TAU);

    // 采样节拍：冷启动前两次 1s 快速定位，之后 5s 定时；偶发丢点形成 10/15s 间隔
    let p_drop = rng.uniform(0.004, 0.030);
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
    // 每次运行抽样一组波动谱参数，避免跨轨迹出现相同的功率谱峰
    let (wp1, wp2, wp3) = (
        rng.uniform(100.0, 140.0),
        rng.uniform(38.0, 56.0),
        rng.uniform(15.0, 23.0),
    );
    // 幅度收敛在 ±2% 量级：保留长/中/短三档起伏的形状，不再叠加出过大的配速方差
    let (wa1, wa2, wa3) = (
        rng.uniform(0.002, 0.006),
        rng.uniform(0.008, 0.018),
        rng.uniform(0.003, 0.008),
    );
    let fat0 = rng.uniform(1.00, 1.06);
    let fat_slope = rng.uniform(0.04, 0.08);
    let fat_min = rng.uniform(0.76, 0.86);
    let base = dist / dur_f;
    // 速度均值倍率：热身 ramp × 疲劳 × 三正弦波动 × 余弦凹陷（不含白噪声，
    // 随机部分统一由 profile 的相关目标 + 一阶响应给出）
    let mut mean_mul = Vec::with_capacity(n);
    for &tt in &times {
        let ramp = if tt < 8.0 {
            (0.85 + 0.15 * (tt / 1.0f64.max(8.0f64.min(dur_f / 20.0)))).min(1.0)
        } else {
            1.0
        };
        let km_done = (tt / dur_f) * dist / 1000.0;
        let fatigue = if km_done <= 0.5 {
            fat0
        } else {
            (fat0 - fat_slope * (km_done - 0.5)).max(fat_min)
        };
        let wave = (1.0 + wa1 * (std::f64::consts::TAU * tt / wp1 + phase_v).sin()
            + wa2 * (std::f64::consts::TAU * tt / wp2 + phase_v * 2.3).sin()
            + wa3 * (std::f64::consts::TAU * tt / wp3 + phase_v * 3.7).sin())
            * dip_factor(tt);
        mean_mul.push(ramp * fatigue * wave);
    }
    // 逐点速度约束在有效配速窗口内侧；边界按次抽样，避免固定钳制值形成平顶
    let (v_lo, v_hi) = (rng.uniform(1.92, 1.98), rng.uniform(6.22, 6.28));
    let speed_seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let build_spec = |caps: Vec<f64>| SpeedSpec {
        dts: dts.clone(),
        target_dist: dist,
        base_v: base,
        floor: v_lo,
        ceil: v_hi,
        caps,
        mean_mul: mean_mul.clone(),
        theta: 0.15,
        sigma: 0.010,
        accel: 0.55,
        decel: 0.45,
        response: 0.45,
        warmup_s: 0.0,
        warmup_frac: 1.0,
        tail_s: 0.0,
        tail_frac: 1.0,
        seed: speed_seed,
    };
    let mut speeds = correlated_speed_profile(&build_spec(vec![v_hi; n]))?;
    // 弯道限速：限速会改变弧长，迭代两轮收敛
    for _ in 0..2 {
        let mut s_of = vec![0.0f64; n];
        let mut acc = s0;
        for i in 0..n {
            s_of[i] = acc;
            acc += direction * speeds[i] * dts[i];
        }
        let caps = ring_speed_caps(&dense, &arcs, &s_of, 2.5, 1.5, 25.0, v_hi);
        speeds = correlated_speed_profile(&build_spec(caps))?;
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

    let tl_w = [((-1i64, 4i64), 54u32), ((-1, 1), 27), ((-1, 12), 13), ((-1, 5), 3), ((-1, 6), 2)];
    let p_type3 = rng.uniform(0.30, 0.45);
    let p_odd = rng.uniform(0.018, 0.035);
    let p_drift = p_odd + rng.uniform(0.012, 0.030);
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
        if kinds[i].0 != -1 && prev_drift && rng.random() < 0.08 {
            kinds[i] = (if rng.random() < 0.75 { 7 } else { 8 }, 1);
        }
    }
    for i in 1..n {
        if kinds[i].0 == -1 && kinds[i - 1].0 == -1 {
            kinds[i] = (rng.choice(&[3, 0]), 1);
        }
    }
    let mut locs: Vec<GenPoint> = Vec::with_capacity(n);
    let mut s = s0;
    let mut jx = 0.0f64;
    let mut jy = 0.0f64;
    // 海拔：校区基准 + 按次抽样的缓变起伏，避免每次都以同一曲线回归同一常数
    let alt_base = 82.0 + rng.uniform(-3.0, 3.0);
    let alt_amp = rng.uniform(0.4, 4.0);
    let alt_period = rng.uniform(240.0, 900.0);
    let alt_phase = rng.uniform(0.0, std::f64::consts::TAU);
    let alt_theta = rng.uniform(0.02, 0.09);
    let alt_mean = |tt: f64| {
        alt_base + alt_amp * (std::f64::consts::TAU * tt / alt_period + alt_phase).sin()
    };
    let alt_sigma = rng.uniform(0.15, 0.35);
    let mut alt = alt_mean(0.0) + rng.uniform(-0.4, 0.4);

    for i in 0..n {
        let dt = dts[i];
        let (typ, lt) = kinds[i];
        // 距离恒来自速度积分（异常点不改变累计里程）
        let d_step = speeds[i] * dt;
        s += direction * d_step;
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
        alt += alt_theta * (alt_mean(times[i]) - alt) + rng.gauss(0.0, alt_sigma);
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
            steps: cum_steps[i].round() as i64,
            stepDistance: 0.0,
            gainTime: fmt_gain_time(start_ms + (times[i] * 1000.0) as i64),
            gainTimeMs: start_ms + (times[i] * 1000.0) as i64,
            queueNum: 0,
            coorType: "gcj02".into(),
            bdA: round_to(alt, 2),
            bdD: round_to(brg, 2),
            bdS: round_to((avg_sp * rng.uniform(0.6, 0.95)).max(0.0), 3),
            bdG: rng.choice(&[1, 1, 1, -1]),
            count: rng.randint(20, 88),
            dtr: 0.0,
            state,
            locationId: String::new(),
        });
    }
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

    apply_post_fixes(&mut locs, &mut rng, start_ms);

    // 点位吸附：<40m 精确落位
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
