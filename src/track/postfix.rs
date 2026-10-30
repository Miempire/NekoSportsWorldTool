//! 轨迹后处理：起终点哨兵（索引1 type=5 全零 / 末点 type=6 / 首点 type∈{0,7}）、
//! 后半段 type=1/2 散布、首点速度取首个非零值。
//!
//! 不再做「结尾断崖」的时间外扩与索引 2 速度修正：累计时长/距离/步数必须与
//! 速度、步频积分保持一致，任何末点改写都会破坏同一份累计数据源。

use super::geom::{fmt_gain_time, round_to, Rng};
use super::model::GenPoint;

/// 在相邻采样点之间按 `max_step_s` 秒插值补点，把 5s/10s/15s 的稀疏采样
/// 加密到接近 1Hz。
///
/// 官方 App 地图用上传点连线着色，采样间隔过大（尤其丢点造成的 10/15s 间隔）
/// 会画成灰色段；同时 5s 间隔的直线弦在弯道处明显偏离道路。补点位置取
/// **路网弧长插值**（落在道路上，不是两点间的直线弦），累计里程/步数由同一
/// 累计序列线性插值，因此总距离、总时长、总步数不变。
///
/// `arc_at[i]` 是第 i 个点实际落位处的路由弧长（未经 `cum_dist` 平移）；
/// 补点几何必须按它插值，否则偏移量会把中间点整体拖离道路。
/// 仅在两端的点都是正常点（`ptype != -1`）时补点：异常点的漂移/跳变保持原样。
pub fn densify_locations<F, G, H>(
    locs: Vec<GenPoint>,
    times: &[f64],
    cum_dist: &[f64],
    cum_steps: &[f64],
    arc_at: &[f64],
    max_step_s: f64,
    pos: F,
    to_plane: G,
    to_lat_lng: H,
    rng: &mut Rng,
) -> Vec<GenPoint>
where
    F: Fn(f64) -> (f64, f64),
    G: Fn(f64, f64) -> (f64, f64),
    H: Fn(f64, f64) -> (f64, f64),
{
    let n = locs.len();
    if n < 2 || times.len() < n || cum_dist.len() < n || cum_steps.len() < n || arc_at.len() < n {
        return locs;
    }
    let max_step_s = max_step_s.max(0.5);
    // 各正常点相对道路基线的平面偏移（相关漂移 + 测量噪声），补点时按时间插值，
    // 保持漂移连续而不是每点重新采样。
    let mut offset: Vec<Option<(f64, f64)>> = Vec::with_capacity(n);
    for (i, p) in locs.iter().enumerate() {
        if p.ptype == -1 {
            offset.push(None);
        } else {
            let (bx, by) = pos(arc_at[i]);
            let (px, py) = to_plane(p.gLat, p.gLng);
            offset.push(Some((px - bx, py - by)));
        }
    }

    let mut out: Vec<GenPoint> = Vec::with_capacity(n * 2);
    for i in 0..n {
        out.push(locs[i].clone());
        if i + 1 >= n {
            break;
        }
        let (a, b) = (&locs[i], &locs[i + 1]);
        let (oa, ob) = match (offset[i], offset[i + 1]) {
            (Some(x), Some(y)) if a.ptype != -1 && b.ptype != -1 => (x, y),
            _ => continue,
        };
        let dt = times[i + 1] - times[i];
        if !(dt > 0.0) {
            continue;
        }
        let k = (dt / max_step_s).ceil() as usize;
        if k < 2 {
            continue;
        }
        let d_seg = cum_dist[i + 1] - cum_dist[i];
        let st_seg = cum_steps[i + 1] - cum_steps[i];
        let v_seg = d_seg / dt;
        // 几何按实际落位弧长插值（含行进方向符号）；上报的累计里程仍按 cum_dist。
        let arc_a = arc_at[i];
        let d_arc = arc_at[i + 1] - arc_at[i];
        // 末段两个网格点落在同一弧长（里程仍在累加但位置不前进），补点只会在终点
        // 堆叠出一串零长度段；该段保持原样，不插点。
        if d_arc.abs() < 0.25 {
            continue;
        }
        let sg = if d_arc < 0.0 { -1.0 } else { 1.0 };
        for j in 1..k {
            let u = j as f64 / k as f64;
            let tt = times[i] + dt * u;
            let s_d = cum_dist[i] + d_seg * u;
            let steps = (cum_steps[i] + st_seg * u).round() as i64;
            let arc_m = arc_a + d_arc * u;
            let (bx, by) = pos(arc_m);
            let ox = oa.0 + (ob.0 - oa.0) * u;
            let oy = oa.1 + (ob.1 - oa.1) * u;
            let (px, py) = (
                bx + ox + rng.gauss(0.0, 0.15),
                by + oy + rng.gauss(0.0, 0.15),
            );
            let (lat, lng) = to_lat_lng(px, py);
            let nxt = pos(arc_m + 2.0 * sg);
            let brg = ((nxt.0 - bx).atan2(nxt.1 - by).to_degrees() + rng.gauss(0.0, 12.0))
                .rem_euclid(360.0);
            let rad = round_to(rng.uniform(1.4, 2.4), 2);
            let total_dis = round_to(s_d, 4);
            let total_time = round_to(tt, 0) as i64;
            out.push(GenPoint {
                id: 0,
                flag: a.flag,
                lat: -1.0,
                lng: -1.0,
                gLat: round_to(lat, 7),
                gLng: round_to(lng, 7),
                speed: round_to(a.speed + (b.speed - a.speed) * u, 4),
                avgSpeed: round_to(v_seg, 4),
                radius: rad,
                accuracy: round_to(
                    (rad * rng.uniform(0.7, 1.4)).min(28.0 + 2.0 * rng.random()),
                    2,
                ),
                ptype: 0,
                locType: 1,
                hasAltitude: true,
                totalTime: total_time,
                totalDis: total_dis,
                validDis: total_dis,
                validTime: total_time,
                steps,
                stepDistance: 0.0,
                gainTime: fmt_gain_time(a.flag + total_time * 1000),
                gainTimeMs: a.flag + total_time * 1000,
                queueNum: 0,
                coorType: "gcj02".into(),
                bdA: round_to(a.bdA + (b.bdA - a.bdA) * u, 2),
                bdD: round_to(brg, 2),
                bdS: round_to(a.bdS + (b.bdS - a.bdS) * u, 3),
                bdG: rng.choice(&[1, 1, 1, -1]),
                count: rng.randint(20, 88),
                dtr: 0.0,
                // 与 type=0 正常点同分布（补点本身 type=0）
                state: rng.weighted(&[(1, 145), (2, 45), (3, 164)]),
                locationId: String::new(),
            });
        }
    }
    for (idx, p) in out.iter_mut().enumerate() {
        p.id = idx as i64 + 1;
    }
    out
}

pub fn apply_post_fixes(locs: &mut [GenPoint], rng: &mut Rng, _start_ms: i64) {
    if locs.len() > 2 {
        // 起点哨兵（type=5→setStartPoint）：全零累计 + state=1
        let sp = &mut locs[1];
        sp.ptype = 5;
        sp.state = 1;
        sp.locType = 1;
        sp.radius = round_to(rng.uniform(2.0, 3.5), 2);
        sp.speed = 0.0;
        sp.avgSpeed = 0.0;
        sp.bdS = 0.0;
        sp.totalDis = 0.0;
        sp.validDis = 0.0;
        sp.totalTime = 0;
        sp.validTime = 0;
        sp.steps = 0;
        // 终点哨兵
        let last = locs.len() - 1;
        let e = &mut locs[last];
        e.ptype = 6;
        e.locType = 1;
        e.radius = round_to(rng.uniform(1.5, 3.0), 2);
        // 后半段散布 type=1/2/8（67-99% 行程区间；type=8 真人低频出现）
        let from = (locs.len() as f64 * 0.67) as usize;
        for i in from..locs.len().saturating_sub(2) {
            if rng.random() < 0.06 {
                locs[i].ptype = rng.choice(&[1, 1, 2, 8]);
                locs[i].locType = 1;
            }
        }
    }
    // 首点 = 起点：totalTime=0、type∈{0,7}、速度取首个非零值
    if !locs.is_empty() {
        locs[0].totalTime = 0;
        locs[0].validTime = 0;
        locs[0].state = 1;
        locs[0].ptype = rng.choice(&[0, 0, 0, 7]);
        locs[0].locType = 1;
        locs[0].radius = round_to(rng.uniform(1.5, 3.0), 2);
        for j in 1..locs.len() {
            if locs[j].avgSpeed > 0.0 {
                locs[0].avgSpeed = locs[j].avgSpeed;
                locs[0].bdS = locs[j].bdS;
                break;
            }
        }
        if locs[0].speed == 0.0 {
            locs[0].speed = round_to(rng.uniform(7.0, 13.0), 4);
        }
    }
}
