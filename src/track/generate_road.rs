//! 真实道路路由轨迹生成器（模式 B）。
//!
//! 与经典模式（`generator::build`）的分工：
//! - 几何源为路网闭环路线（而非 Catmull-Rom 环）；
//! - 配速为「相关随机目标 + 一阶平滑响应 + 弯道预判刹车」（`profile`）；
//! - 步频同法合成，步数由步频积分得到；
//! - GPS 抖动为 AR(1) 相关漂移 + 高斯测量噪声 + 建筑 SDF 衰减（`noise::GpsJitter`）。
//!
//! 10s 窗 / 60s 段 / 圈均由同一份累计序列派生；距离不可满足时返回 `Err`。

use super::geom::{
    fmt_gain_time, ring_point_at, round_to, to_bd, wgs84_to_bd09, Rng, MET_PER_DEG_LAT,
    MET_PER_DEG_LNG,
};
use super::model::{GenPoint, Segment, TenWindow, Track};
use super::postfix::apply_post_fixes;
use route_planner::profile::{
    correlated_cadence_profile, cumulative, time_grid, windows_from_cum, CadenceSpec,
};
use route_planner::{
    fatigue_from_km, pace_profile_with_mul, plan_route_split, point_in_polygon,
    segment_intersects_polygon, Coord, GpsJitter, KinParams, RoadGraph, Route, RouteOptions, Sdf,
};

/// 路线算法模式（三选一）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RouteMode {
    /// 经典打卡点环（Catmull-Rom）。
    #[default]
    Legacy,
    /// 真实道路拓扑路由。
    Road,
    /// OSM 跑道绕圈（轮廓需人工确认）。
    Track,
}

impl RouteMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "road" => RouteMode::Road,
            "track" => RouteMode::Track,
            _ => RouteMode::Legacy,
        }
    }
}

/// 从本地文件加载 OSM 路网。
pub fn load_network_path(path: &str) -> Result<RoadGraph, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取 {path} 失败: {e}"))?;
    route_planner::load_osm(&bytes)
}

/// 路网预览计划（供地图显示）：对齐后的路网/建筑 + 生成的路线（BD 系）。
pub struct RoadPlan {
    pub route: Vec<(f64, f64)>,
    pub length_m: f64,
    pub edges: Vec<Vec<(f64, f64)>>,
    pub buildings: Vec<Vec<(f64, f64)>>,
    pub checkpoints: Vec<(f64, f64)>,
    pub fences: Vec<Vec<(f64, f64)>>,
    /// 路线中穿过建筑轮廓的段数（建筑避让后仍存在的残差，0 为佳）。
    pub building_hits: usize,
}

/// 用围栏（BD 系，与打卡点同系）裁剪路网：仅保留围栏内道路；裁剪后无路则退回原网。
pub fn apply_fences(net: &RoadGraph, fences: &[Vec<(f64, f64)>]) -> RoadGraph {
    if fences.is_empty() {
        return net.clone();
    }
    let mut g = net.clone();
    let polys: Vec<Vec<Coord>> = fences
        .iter()
        .map(|r| r.iter().map(|p| Coord::new(p.1, p.0)).collect())
        .collect();
    g.retain_inside_any(&polys);
    // 进一步裁剪：移除「穿越围栏边界」的边（端点在内但中段跑出围栏，如跨校区道路）。
    g.graph.retain_edges(|graph, e| {
        let (a, b) = graph.edge_endpoints(e).unwrap();
        edge_inside(&polys, graph[a].coord, graph[b].coord)
    });
    g.rebuild_index();
    if g.graph.edge_count() == 0 {
        return net.clone();
    }
    g
}

/// 建筑避让：移除穿过建筑轮廓的边。
///
/// 若建筑轮廓与道路重叠过多（保留边不足一半），判定为轮廓精度不足并放弃避让，
/// 避免把整个校区路网切碎导致路由失败。返回 (路网, 移除边数, 是否放弃)。
pub fn apply_obstacles(net: &RoadGraph) -> (RoadGraph, usize, bool) {
    if net.buildings.is_empty() {
        return (net.clone(), 0, false);
    }
    let before = net.graph.edge_count();
    let mut g = net.clone();
    g.retain_outside_buildings(5);
    let after = g.graph.edge_count();
    if after < 2 || after * 2 < before {
        return (net.clone(), 0, true);
    }
    (g, before - after, false)
}

/// 边是否整体落在任一围栏内：沿边等距采样，所有采样点（含端点）必须在围栏内。
fn edge_inside(polys: &[Vec<Coord>], a: Coord, b: Coord) -> bool {
    const SEGS: usize = 8;
    for k in 0..=SEGS {
        let t = k as f64 / SEGS as f64;
        let p = Coord::new(a.lon + (b.lon - a.lon) * t, a.lat + (b.lat - a.lat) * t);
        if !polys.iter().any(|poly| point_in_polygon(poly, p)) {
            return false;
        }
    }
    true
}

/// 生成路网预览（对齐 + 围栏裁剪 + 闭环路由），不产生协议字段。
///
/// 必经点（policy，按序）与打卡点**全部强制经过**，与提交时的路由口径一致：
/// 任一点不可达会直接返回 `Err`，而不是静默跳过。
pub fn plan_road_view(
    net: &RoadGraph,
    points_bd: &[(f64, f64)],
    must_bd: &[(f64, f64)],
    dist: f64,
    seed: u64,
    fences: &[Vec<(f64, f64)>],
) -> Result<RoadPlan, String> {
    let mut g = net.clone();
    align_network(&mut g);
    g = apply_fences(&g, fences);
    g = apply_obstacles(&g).0;
    let ordered = merge_route_points(points_bd, must_bd);
    let waypoints: Vec<Coord> = ordered.iter().map(|p| Coord::new(p.1, p.0)).collect();
    // 与提交一致：所有路线点（必经点 + 打卡点）都是硬约束
    let must: Vec<Coord> = ordered.iter().map(|p| Coord::new(p.1, p.0)).collect();
    let opts = RouteOptions::default();
    let route = plan_route_split(&g, &waypoints, &must, dist, seed, &opts)?;
    let route_pts: Vec<(f64, f64)> = route.points.iter().map(|p| (p.lat, p.lon)).collect();
    let building_hits = route
        .points
        .windows(2)
        .filter(|w| {
            let a = Coord::new(w[0].lon, w[0].lat);
            let b = Coord::new(w[1].lon, w[1].lat);
            g.buildings
                .iter()
                .any(|ring| segment_intersects_polygon(ring, a, b))
        })
        .count();
    let edges: Vec<Vec<(f64, f64)>> = g
        .edges_deg()
        .iter()
        .map(|e| e.iter().map(|c| (c.lat, c.lon)).collect())
        .collect();
    let buildings: Vec<Vec<(f64, f64)>> = g
        .buildings
        .iter()
        .map(|r| r.iter().map(|c| (c.lat, c.lon)).collect())
        .collect();
    Ok(RoadPlan {
        route: route_pts,
        length_m: route.length_m,
        edges,
        buildings,
        checkpoints: ordered,
        fences: fences.to_vec(),
        building_hits,
    })
}

/// 必经点覆盖：返回每个必经点到轨迹最近点的距离（米），供日志核对「必达」。
pub fn must_coverage(track: &Track, must_bd: &[(f64, f64)]) -> Vec<f64> {
    must_bd
        .iter()
        .map(|&(mlat, mlon)| {
            track
                .locations
                .iter()
                .map(|p| {
                    let dx = (p.lat - mlat) * MET_PER_DEG_LAT;
                    let dy = (p.lng - mlon) * MET_PER_DEG_LNG;
                    (dx * dx + dy * dy).sqrt()
                })
                .fold(f64::INFINITY, f64::min)
        })
        .collect()
}

/// 将 WGS84 路网逐节点转换为 BD-09（与打卡点同系）。
///
/// OSM 为 WGS84，打卡点为 BD-09。两者不是常数平移关系（境内 GCJ/BD 偏移随位置
/// 变化），故用标准 WGS84→GCJ-02→BD-09 变换逐节点转换，校园尺度下与真实道路
/// 对齐（优于旧版质心常数平移的米级~数十米残差）。
pub fn align_network(net: &mut RoadGraph) {
    if net.graph.node_count() == 0 {
        return;
    }
    for node in net.graph.node_weights_mut() {
        let (lat, lon) = wgs84_to_bd09(node.coord.lat, node.coord.lon);
        node.coord.lat = lat;
        node.coord.lon = lon;
    }
    for ring in net.buildings.iter_mut() {
        for c in ring.iter_mut() {
            let (lat, lon) = wgs84_to_bd09(c.lat, c.lon);
            c.lat = lat;
            c.lon = lon;
        }
    }
    net.compute_anchor();
    net.rebuild_index();
}

/// 按质心角度排序打卡点，形成自然环序。
pub fn radial_order(points_bd: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let n = points_bd.len();
    if n == 0 {
        return Vec::new();
    }
    let (cx, cy) = (
        points_bd.iter().map(|q| q.0).sum::<f64>() / n as f64,
        points_bd.iter().map(|q| q.1).sum::<f64>() / n as f64,
    );
    let mut ordered = points_bd.to_vec();
    ordered.sort_by(|a, b| {
        (a.0 - cx)
            .atan2(a.1 - cy)
            .partial_cmp(&(b.0 - cx).atan2(b.1 - cy))
            .unwrap()
    });
    ordered
}

/// 合并路线点：必经点（按策略顺序）在前，其余打卡点去重后按质心角排序在后。
/// 用于道路模式预览与提交共用同一份顺序，避免两处口径分叉。
pub fn merge_route_points(
    points_bd: &[(f64, f64)],
    must_bd: &[(f64, f64)],
) -> Vec<(f64, f64)> {
    let mut out = must_bd.to_vec();
    let free: Vec<(f64, f64)> = points_bd
        .iter()
        .copied()
        .filter(|p| !out.iter().any(|q| (q.0 - p.0).abs() < 1e-6 && (q.1 - p.1).abs() < 1e-6))
        .collect();
    if !free.is_empty() {
        out.extend(radial_order(&free));
    }
    out
}

/// 路线 → 平面折线 + 弧长表。
///
/// `arcs[k]` 必须与 `dense[k]` 一一对应（`ring_point_at` 按同下标取弧长），
/// 因此直接取每个采样点的累计弧长 `p.s`（`dense` 末点已闭合回起点）。
fn route_ring(route: &Route, c_lat: f64, c_lng: f64) -> (Vec<(f64, f64)>, Vec<f64>) {
    let dense: Vec<(f64, f64)> = route
        .points
        .iter()
        .map(|p| {
            (
                (p.lon - c_lng) * MET_PER_DEG_LNG,
                (p.lat - c_lat) * MET_PER_DEG_LAT,
            )
        })
        .collect();
    let arcs: Vec<f64> = route.points.iter().map(|p| p.s).collect();
    (dense, arcs)
}

/// 模式 B：真实道路拓扑路由轨迹生成。
///
/// `route_bd[0]` 为起点，`route_bd[1..]` 为软引导点（用于方向锚点，不必全经过）；
/// `must_bd` 为强制必经点（按序，可为空，最终必达）；吸附在 `route_bd` 全量上做。
pub fn build_road(
    dist: f64,
    dur: i64,
    seed: u64,
    start_ms: i64,
    route_bd: &[(f64, f64)],
    must_bd: &[(f64, f64)],
    net: &RoadGraph,
) -> Result<Track, String> {
    let avg_v = dist / dur as f64;
    let a_c_max = 2.5;
    let radius = (avg_v * avg_v / a_c_max).max(6.0);
    let opts = RouteOptions {
        a_c_max,
        min_radius_m: radius,
        ..Default::default()
    };

    let waypoints: Vec<Coord> = route_bd.iter().map(|p| Coord::new(p.1, p.0)).collect();
    let must: Vec<Coord> = must_bd.iter().map(|p| Coord::new(p.1, p.0)).collect();
    let route = plan_route_split(net, &waypoints, &must, dist, seed, &opts)?;
    build_from_route(dist, dur, seed, start_ms, route_bd, &route, net)
}

/// 模式 C：跑道绕圈。
///
/// 连接段（起点 → 按序必经点 → 入环点）只执行一次，随后整圈 × n 与最后的部分圈；
/// 轮廓为人工确认后的 BD 闭合轮廓（`(lat, lon)`），必须通过共享道路节点接入。
pub fn build_track(
    dist: f64,
    dur: i64,
    seed: u64,
    start_ms: i64,
    route_bd: &[(f64, f64)],
    must_bd: &[(f64, f64)],
    net: &RoadGraph,
    outline_bd: &[(f64, f64)],
) -> Result<Track, String> {
    let start_bd = *route_bd.first().ok_or("跑道模式缺少起点")?;
    let ring = route_planner::validate_outline(net, outline_bd, 5.0)?;
    let start = Coord::new(start_bd.1, start_bd.0);
    let must: Vec<Coord> = must_bd.iter().map(|p| Coord::new(p.1, p.0)).collect();
    let avg_v = dist / dur as f64;
    let a_c_max = 2.5;
    let radius = (avg_v * avg_v / a_c_max).max(6.0);
    let opts = RouteOptions {
        a_c_max,
        min_radius_m: radius,
        ..Default::default()
    };
    let tr = route_planner::plan_track_route(net, &ring, start, &must, dist, &opts)?;
    build_from_route(dist, dur, seed, start_ms, route_bd, &tr.route, net)
}

/// 跑道预览计划（供地图显示）。
pub struct TrackPlanView {
    pub candidates: Vec<route_planner::TrackCandidate>,
    /// 选中的候选 key（未选中/手动轮廓时为 None）。
    pub selected: Option<u64>,
    pub route: Vec<(f64, f64)>,
    pub length_m: f64,
    pub ring_len: f64,
    pub laps: usize,
    pub partial_m: f64,
    pub connector_m: f64,
    pub edges: Vec<Vec<(f64, f64)>>,
    pub buildings: Vec<Vec<(f64, f64)>>,
    pub fences: Vec<Vec<(f64, f64)>>,
    pub checkpoints: Vec<(f64, f64)>,
    /// 最终轮廓（BD, lat/lon，闭合），供 GUI 编辑与人工确认。
    pub outline: Vec<(f64, f64)>,
}

/// 生成跑道预览（对齐 + 围栏裁剪 + 候选提取 + 跑道路由）。
///
/// `outline_bd` 为手动/已确认轮廓（None 表示按 `select_key` 选候选）；
/// 预览不做建筑避让（跑道本身紧邻看台，避让会切断环）。
pub fn plan_track_view(
    net: &RoadGraph,
    points_bd: &[(f64, f64)],
    must_bd: &[(f64, f64)],
    dist: f64,
    _seed: u64,
    fences: &[Vec<(f64, f64)>],
    outline_bd: Option<&[(f64, f64)]>,
    select_key: Option<u64>,
) -> Result<TrackPlanView, String> {
    let mut g = net.clone();
    align_network(&mut g);
    g = apply_fences(&g, fences);
    let candidates = route_planner::track_candidates(&g);

    let chosen: Option<usize> = if outline_bd.is_some() {
        None
    } else {
        let usable: Vec<usize> = candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.issue.is_none())
            .map(|(i, _)| i)
            .collect();
        match select_key {
            Some(k) => usable.iter().copied().find(|i| candidates[*i].key == k),
            None => usable.first().copied(),
        }
    };

    let (outline_latlon, ring) = match outline_bd {
        Some(o) => (o.to_vec(), route_planner::validate_outline(&g, o, 5.0)?),
        None => {
            let idx = chosen.ok_or_else(|| {
                if candidates.is_empty() {
                    "未找到跑道候选（OSM 中缺少 leisure=track / sport=running|athletics / highway=track 闭合环）"
                        .to_string()
                } else {
                    "没有可用跑道候选（自交 / 单行方向冲突 / 断路）".to_string()
                }
            })?;
            let c = &candidates[idx];
            if let Some(issue) = &c.issue {
                return Err(format!("跑道候选不可用：{issue}"));
            }
            (
                c.points.iter().map(|p| (p.lat, p.lon)).collect(),
                c.ring.clone(),
            )
        }
    };

    let ordered = radial_order(points_bd);
    // 无点位缓存时退回必经点作为起点，避免「无法确认轮廓」的死局
    let start_bd = *ordered
        .first()
        .or_else(|| must_bd.first())
        .ok_or("无打卡点/必经点缓存，无法预览跑道路线")?;
    let start = Coord::new(start_bd.1, start_bd.0);
    let must: Vec<Coord> = must_bd.iter().map(|p| Coord::new(p.1, p.0)).collect();
    let opts = RouteOptions::default();
    let tr = route_planner::plan_track_route(&g, &ring, start, &must, dist, &opts)?;

    let edges: Vec<Vec<(f64, f64)>> = g
        .edges_deg()
        .iter()
        .map(|e| e.iter().map(|c| (c.lat, c.lon)).collect())
        .collect();
    let buildings: Vec<Vec<(f64, f64)>> = g
        .buildings
        .iter()
        .map(|r| r.iter().map(|c| (c.lat, c.lon)).collect())
        .collect();
    Ok(TrackPlanView {
        selected: chosen.map(|i| candidates[i].key),
        candidates,
        route: tr.route.points.iter().map(|p| (p.lat, p.lon)).collect(),
        length_m: tr.route.length_m,
        ring_len: tr.ring_len,
        laps: tr.laps,
        partial_m: tr.partial_m,
        connector_m: tr.connector_m,
        edges,
        buildings,
        fences: fences.to_vec(),
        checkpoints: ordered,
        outline: outline_latlon,
    })
}

/// 由已规划路线组装轨迹（模式 B / C 共用）。
fn build_from_route(
    dist: f64,
    dur: i64,
    seed: u64,
    start_ms: i64,
    route_bd: &[(f64, f64)],
    route: &Route,
    net: &RoadGraph,
) -> Result<Track, String> {
    let mut rng = Rng::new(seed);
    let dur_f = dur as f64;
    let avg_v = dist / dur_f;
    let a_c_max = 2.5;
    let n_pts = route_bd.len().max(1);
    let (c_lat, c_lng) = (
        route_bd.iter().map(|q| q.0).sum::<f64>() / n_pts as f64,
        route_bd.iter().map(|q| q.1).sum::<f64>() / n_pts as f64,
    );
    let (dense, arcs) = route_ring(route, c_lat, c_lng);

    // 时间轴：末点严格落在 dur，Σdt = 总时长（不补时长）
    let p_drop = rng.uniform(0.004, 0.030);
    let times = time_grid(dur_f, p_drop, seed)?;
    let n = times.len();
    let mut dts: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    dts.push(0.0);

    // 速度剖面：相关随机目标 + 一阶平滑响应 + 弯道预判刹车
    let kin = KinParams {
        a_c_max,
        ..Default::default()
    };
    let speed_seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    // 均值倍率：疲劳 × 三正弦波动（与经典模式同口径；热身/收尾由 KinParams 负责，
    // 随机部分由 profile 的相关目标 + 一阶响应给出）。
    let fat0 = rng.uniform(1.00, 1.04);
    let fat_slope = rng.uniform(0.010, 0.020);
    let fat_min = rng.uniform(0.94, 0.97);
    let phase_v = rng.uniform(0.0, std::f64::consts::TAU);
    let (wp1, wp2, wp3) = (
        rng.uniform(100.0, 140.0),
        rng.uniform(38.0, 56.0),
        rng.uniform(15.0, 23.0),
    );
    let (wa1, wa2, wa3) = (
        rng.uniform(0.002, 0.006),
        rng.uniform(0.008, 0.018),
        rng.uniform(0.003, 0.008),
    );
    // 疲劳包络：默认硬限制在 [fat_min, fat0]，只允许 1~2 个短窗（20~44s、
    // 余弦渐变）越界，越界幅度 1.5~3%，避免长时间、大范围来回摆动。
    let exc_n = if rng.random() < 0.5 { 1 } else { 2 };
    let mut exc_windows: Vec<(f64, f64, f64)> = Vec::with_capacity(exc_n);
    for _ in 0..exc_n {
        let center = rng.uniform(0.15 * dur_f, 0.85 * dur_f);
        let half = rng.uniform(10.0, 22.0);
        let eps = rng.uniform(0.015, 0.030);
        exc_windows.push((center, half, eps));
    }
    let excursion = |t: f64| -> f64 {
        let mut best = 0.0f64;
        for &(center, half, eps) in &exc_windows {
            let x = (t - center).abs();
            if x < half {
                let w = 0.5 * (1.0 + (std::f64::consts::PI * x / half).cos());
                if eps * w > best {
                    best = eps * w;
                }
            }
        }
        best
    };
    let mean_mul: Vec<f64> = times
        .iter()
        .map(|&tt| {
            let km = (tt / dur_f) * dist / 1000.0;
            let fatigue = if km <= 0.5 {
                fat0
            } else {
                (fat0 - fat_slope * (km - 0.5)).max(fat_min)
            };
            let wave = 1.0
                + wa1 * (std::f64::consts::TAU * tt / wp1 + phase_v).sin()
                + wa2 * (std::f64::consts::TAU * tt / wp2 + phase_v * 2.3).sin()
                + wa3 * (std::f64::consts::TAU * tt / wp3 + phase_v * 3.7).sin();
            let e = excursion(tt);
            let lo = fat_min * (1.0 - e);
            let hi = fat0 * (1.0 + e);
            (fatigue * wave).clamp(lo, hi)
        })
        .collect();
    // 限速边界按次抽样：角点限速不需要每次都压到同一个固定值
    let (v_lo, v_hi) = (rng.uniform(1.90, 1.98), rng.uniform(6.22, 6.28));
    let speeds = pace_profile_with_mul(
        &route,
        avg_v,
        &dts,
        dist,
        speed_seed,
        &kin,
        v_lo,
        v_hi,
        &mean_mul,
    )?;
    let cum_dist = cumulative(&times, &speeds, false);
    // 距离必须来自速度积分，且与请求值一致；否则如实报错
    let integral_dist = *cum_dist.last().unwrap_or(&0.0);
    if (integral_dist - dist).abs() > 1.0 {
        return Err(format!(
            "路网轨迹距离与目标不一致：积分 {:.1}m ≠ 请求 {:.1}m",
            integral_dist, dist
        ));
    }

    // 点位类型（同经典模式）
    let tl_w = [
        ((-1i64, 4i64), 54u32),
        ((-1, 1), 27),
        ((-1, 12), 13),
        ((-1, 5), 3),
        ((-1, 6), 2),
    ];
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
    // 步频：相关随机目标 + 一阶平滑响应；步数由步频积分得到
    let fatigue: Vec<f64> = (0..n)
        .map(|i| fatigue_from_km((times[i] / dur_f).clamp(0.0, 1.0) * dist / 1000.0))
        .collect();
    let cadence_seed = seed.wrapping_add(0xC6A4_A793_5BD1_E995);
    let cadences = correlated_cadence_profile(&CadenceSpec {
        speeds: &speeds,
        dts: &dts,
        fatigue: &fatigue,
        floor: 110.0,
        ceil: 220.0,
        theta: 0.25,
        sigma: 0.5,
        response: 0.45,
        accel: 4.0,
        seed: cadence_seed,
    })?;
    let cum_steps = cumulative(&times, &cadences, true);

    // 建筑 SDF（自适应放大漂移）
    let sdf = Sdf::from_buildings(
        &net.buildings,
        |c| {
            [
                (c.lon - c_lng) * MET_PER_DEG_LNG,
                (c.lat - c_lat) * MET_PER_DEG_LAT,
            ]
        },
        2.0,
        30.0,
    );



    // GPS 抖动：AR(1) 相关漂移 + 高斯测量噪声（SDF 逐点放大）
    let mut jitter = GpsJitter::new(0.72, 0.75, 0.3);

    let mut locs: Vec<GenPoint> = Vec::with_capacity(n);
    let mut s = 0.0f64;
    // 海拔：校区基准 + 按次抽样的缓变起伏（与经典模式一致）
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
        // 距离恒来自速度积分（异常点也占用该区间的真实里程）
        let d_step = speeds[i] * dt;
        s += d_step;
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
            // AR(1) 相关漂移 + 高斯测量噪声，SDF 自适应放大
            let sc = sdf.sigma_scale([bx, by]);
            let z = [
                rng.gauss(0.0, 1.0),
                rng.gauss(0.0, 1.0),
                rng.gauss(0.0, 1.0),
                rng.gauss(0.0, 1.0),
            ];
            let (jxd, jyd) = jitter.step(sc, z);
            px = bx + jxd;
            py = by + jyd;
            rad = round_to(
                if typ == 3 {
                    rng.uniform(1.4, 5.1)
                } else {
                    rng.uniform(1.4, 2.4)
                },
                2,
            );
            state = if typ == 0 {
                rng.weighted(&[(1, 145), (2, 45), (3, 164)])
            } else {
                rng.weighted(&[(1, 145), (2, 256), (3, 151)])
            };
        } else {
            // 异常点的报告位置偏移量（不影响累计里程）
            let mut off = 0.0f64;
            match lt {
                4 => {
                    if rng.random() >= 0.68 {
                        off = if rng.random() < 0.95 {
                            rng.uniform(2.0, 60.0)
                        } else {
                            rng.uniform(60.0, 250.0)
                        };
                    }
                }
                1 => {
                    if rng.random() >= 0.83 {
                        off = rng.uniform(0.5, 36.0);
                    }
                }
                12 => {
                    off = if rng.random() < 0.9 {
                        rng.uniform(5.0, 80.0)
                    } else {
                        rng.uniform(80.0, 220.0)
                    };
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
                py = (last.gLat - c_lat) * MET_PER_DEG_LAT;
                px = (last.gLng - c_lng) * MET_PER_DEG_LNG;
            } else {
                let (jx, jy) = jitter.state();
                px = x + jx;
                py = y + jy;
            }
            rad = if lt == 4 {
                round_to(
                    if rng.random() < 0.75 {
                        rng.uniform(30.0, 100.0)
                    } else {
                        rng.uniform(100.0, 550.0)
                    },
                    2,
                )
            } else if lt == 1 {
                round_to(
                    if rng.random() < 0.75 {
                        rng.uniform(1.6, 12.0)
                    } else {
                        rng.uniform(12.0, 95.0)
                    },
                    2,
                )
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
        let nxt = pos(s + 2.0);
        let brg =
            ((nxt.0 - x).atan2(nxt.1 - y).to_degrees() + rng.gauss(0.0, 12.0)).rem_euclid(360.0);
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
        .map(|(sec, d)| TenWindow {
            time: sec.round().max(1.0) as i64,
            value: round_to(d, 2),
        })
        .collect();
    let ten_steps: Vec<TenWindow> = windows_from_cum(&times, &cum_steps, dur_f, 10.0)
        .into_iter()
        .map(|(sec, s)| TenWindow {
            time: sec.round().max(1.0) as i64,
            value: round_to(s, 0),
        })
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

    // 点位吸附：<40m 精确落位（全量打卡点，含必经点与普通打卡点）
    for pl in route_bd {
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
