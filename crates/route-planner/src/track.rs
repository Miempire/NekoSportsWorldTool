//! 跑道：OSM 候选提取（无歧义拼接）、轮廓校验、逆时针方向校验与
//! 「连接段一次 + 整圈 + 最后部分圈」路由。
//!
//! 候选只依赖节点共享关系：任一路段节点的度数 ≠ 2 即视为歧义或断路，
//! 该连通分量整体不提供（宁可缺候选，也不猜拼接顺序）。

use std::collections::{HashMap, HashSet};

use petgraph::graph::NodeIndex;

use crate::graph::{dist, segments_intersect, Coord, RoadGraph};
use crate::project::add_virtual_nodes;
use crate::route::shortest_path;
use crate::smooth::smooth_and_sample;
use crate::Route;
use crate::RouteOptions;

/// 一条跑道候选：闭合、逆时针、经节点共享无歧义拼接的环。
///
/// `points` 与 `ring` 等长，末元素重复首元素（闭合），可直接 `windows(2)` 遍历。
#[derive(Clone, Debug)]
pub struct TrackCandidate {
    /// 稳定标识（构成环的最小 way id），用于 GUI 跨刷新选择。
    pub key: u64,
    pub way_ids: Vec<u64>,
    pub points: Vec<Coord>,
    pub ring: Vec<NodeIndex>,
    pub length_m: f64,
    pub sport: String,
    pub surface: String,
    /// 不可用原因（自交 / 单行方向冲突 / 断路）；None 表示可确认。
    pub issue: Option<String>,
}

/// 一次跑道规划的结果。
pub struct TrackRoute {
    pub route: Route,
    /// 连接段长度（起点 → 必经点 → 入环点，只执行一次）。
    pub connector_m: f64,
    pub ring_len: f64,
    /// 完整圈数。
    pub laps: usize,
    /// 最后部分圈长度。
    pub partial_m: f64,
    /// 入环点。
    pub entry: Coord,
}

/// 有向面积（x=lon, y=lat）：正值逆时针。
pub fn signed_area(points: &[Coord]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..points.len() {
        let p = points[i];
        let q = points[(i + 1) % points.len()];
        a += p.lon * q.lat - q.lon * p.lat;
    }
    a * 0.5
}

/// 轮廓自交检测：返回第一对相交的非相邻线段下标。
pub fn outline_self_intersects(points: &[Coord]) -> Option<(usize, usize)> {
    let n = points.len();
    if n < 4 {
        return None;
    }
    for i in 0..n - 1 {
        for j in i + 2..n - 1 {
            // 闭合环：第 0 段与最后一段共享首尾点，不算自交
            if i == 0 && j == n - 2 {
                continue;
            }
            if segments_intersect(
                [points[i].lon, points[i].lat],
                [points[i + 1].lon, points[i + 1].lat],
                [points[j].lon, points[j].lat],
                [points[j + 1].lon, points[j + 1].lat],
            ) {
                return Some((i, j));
            }
        }
    }
    None
}

/// 环是否严格沿有向边可通行：缺边但有反向边 → 方向冲突；双向都无 → 断路。
pub fn check_ring_direction(g: &RoadGraph, ring: &[NodeIndex]) -> Result<(), String> {
    if ring.len() < 2 {
        return Err("跑道轮廓为空".into());
    }
    if ring.first() != ring.last() {
        return Err("跑道轮廓未闭合".into());
    }
    if ring
        .iter()
        .any(|n| g.graph.node_weight(*n).is_none())
    {
        return Err("跑道轮廓节点已不在当前路网（可能被围栏裁剪）".into());
    }
    for w in ring.windows(2) {
        if g.graph.find_edge(w[0], w[1]).is_some() {
            continue;
        }
        let (a, b) = (g.node_coord(w[0]), g.node_coord(w[1]));
        if g.graph.find_edge(w[1], w[0]).is_some() {
            return Err(format!(
                "跑道方向与单行方向冲突：({:.6},{:.6}) → ({:.6},{:.6}) 仅允许反向通行",
                a.lat, a.lon, b.lat, b.lon
            ));
        }
        return Err(format!(
            "跑道中断：({:.6},{:.6}) → ({:.6},{:.6}) 之间没有道路连接",
            a.lat, a.lon, b.lat, b.lon
        ));
    }
    Ok(())
}

/// 最近的图节点（米制容差内）。
pub fn nearest_node(g: &RoadGraph, c: Coord, tol_m: f64) -> Option<NodeIndex> {
    let p = g.to_m(c);
    let mut best: Option<(NodeIndex, f64)> = None;
    for n in g.graph.node_indices() {
        let d = dist(p, g.to_m(g.node_coord(n)));
        if d <= tol_m && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((n, d));
        }
    }
    best.map(|(n, _)| n)
}

/// 手动轮廓校验：顶点必须落在共享道路节点上，相邻顶点必须有有向边，
/// 轮廓不得自交，并统一为逆时针。返回闭合环（末元素重复首元素）。
pub fn validate_outline(
    g: &RoadGraph,
    verts: &[(f64, f64)],
    tol_m: f64,
) -> Result<Vec<NodeIndex>, String> {
    if verts.len() < 3 {
        return Err("手动轮廓至少需要 3 个顶点".into());
    }
    let mut v: Vec<Coord> = verts.iter().map(|&(lat, lon)| Coord::new(lon, lat)).collect();
    // 允许末尾重复首点
    if v.len() >= 2 {
        let (p0, pn) = (v[0], *v.last().unwrap());
        if (p0.lon - pn.lon).abs() < 1e-12 && (p0.lat - pn.lat).abs() < 1e-12 {
            v.pop();
        }
    }
    // 去掉连续重复顶点
    v.dedup_by(|a, b| {
        (a.lon - b.lon).abs() < 1e-12 && (a.lat - b.lat).abs() < 1e-12
    });
    if v.len() < 3 {
        return Err("手动轮廓去重后少于 3 个顶点".into());
    }
    let mut closed = v.clone();
    closed.push(v[0]);
    if let Some((i, j)) = outline_self_intersects(&closed) {
        return Err(format!("手动轮廓自交（第 {}/{} 段）", i + 1, j + 1));
    }
    let mut nodes: Vec<NodeIndex> = Vec::with_capacity(v.len());
    for (i, c) in v.iter().enumerate() {
        let n = nearest_node(g, *c, tol_m).ok_or_else(|| {
            format!(
                "手动轮廓第 {} 个顶点 ({:.6},{:.6}) 未落在共享道路节点上（{:.0}m 内无节点）",
                i + 1,
                c.lat,
                c.lon,
                tol_m
            )
        })?;
        if nodes.last() == Some(&n) {
            return Err(format!("手动轮廓第 {} 与第 {} 个顶点落在同一道路节点", i, i + 1));
        }
        nodes.push(n);
    }
    if nodes.first() == nodes.last() {
        nodes.pop();
    }
    if nodes.len() < 3 {
        return Err("手动轮廓有效顶点不足 3 个".into());
    }
    // 统一逆时针
    let mut coords: Vec<Coord> = nodes.iter().map(|&n| g.node_coord(n)).collect();
    coords.push(coords[0]);
    if signed_area(&coords) < 0.0 {
        nodes.reverse();
    }
    let mut ring: Vec<NodeIndex> = nodes;
    ring.push(ring[0]);
    check_ring_direction(g, &ring)?;
    Ok(ring)
}

/// 从路网中提取跑道候选（已裁剪/对齐的图）。
pub fn track_candidates(g: &RoadGraph) -> Vec<TrackCandidate> {
    struct WayInfo {
        id: u64,
        nodes: Vec<u64>,
        sport: String,
        surface: String,
    }

    let mut id_map: HashMap<u64, (Coord, NodeIndex)> = HashMap::new();
    let mut ways: Vec<WayInfo> = Vec::new();
    for w in &g.track_ways {
        if w.node_refs.len() < 2 || w.node_refs.len() != w.graph_nodes.len() {
            continue;
        }
        // 围栏裁剪后节点可能已删除：整条 way 不可用
        if w.graph_nodes.iter().any(|n| g.graph.node_weight(*n).is_none()) {
            continue;
        }
        for (id, n) in w.node_refs.iter().zip(&w.graph_nodes) {
            id_map.entry(*id).or_insert((g.node_coord(*n), *n));
        }
        ways.push(WayInfo {
            id: w.id,
            nodes: w.node_refs.clone(),
            sport: w.sport.clone(),
            surface: w.surface.clone(),
        });
    }
    if ways.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<TrackCandidate> = Vec::new();
    let mut open: Vec<usize> = Vec::new();

    // 一、自身闭合的 way
    for (i, w) in ways.iter().enumerate() {
        let first = *w.nodes.first().unwrap();
        let last = *w.nodes.last().unwrap();
        if first == last && w.nodes.len() >= 4 {
            let ids = dedup_ring_nodes(&w.nodes);
            if let Some(c) = build_candidate(
                g,
                &id_map,
                &ids,
                vec![w.id],
                &w.sport,
                &w.surface,
            ) {
                out.push(c);
            }
        } else {
            open.push(i);
        }
    }

    // 二、开口 way：按节点共享拼接；任何节点度 ≠ 2 的分量整体跳过（歧义或断路）
    let mut adj: HashMap<u64, Vec<u64>> = HashMap::new();
    for &i in &open {
        for pair in ways[i].nodes.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a == b {
                continue;
            }
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
    }
    let mut keys: Vec<u64> = adj.keys().copied().collect();
    keys.sort_unstable();
    let mut seen: HashSet<u64> = HashSet::new();
    for start in keys {
        if seen.contains(&start) {
            continue;
        }
        let mut comp: Vec<u64> = Vec::new();
        let mut stack = vec![start];
        seen.insert(start);
        let mut ambiguous = false;
        while let Some(n) = stack.pop() {
            comp.push(n);
            let nbrs = adj.get(&n).cloned().unwrap_or_default();
            if nbrs.len() != 2 {
                ambiguous = true;
            }
            for m in nbrs {
                if seen.insert(m) {
                    stack.push(m);
                }
            }
        }
        if ambiguous || comp.len() < 4 {
            continue;
        }
        let ring = walk_cycle(&adj, start, comp.len());
        if ring.len() != comp.len() {
            continue;
        }
        let comp_set: HashSet<u64> = comp.iter().copied().collect();
        let involved: Vec<&WayInfo> = ways
            .iter()
            .filter(|w| w.nodes.iter().any(|id| comp_set.contains(id)))
            .collect();
        let way_ids: Vec<u64> = involved.iter().map(|w| w.id).collect();
        let sport = involved
            .iter()
            .find(|w| !w.sport.is_empty())
            .map(|w| w.sport.clone())
            .unwrap_or_default();
        let surface = involved
            .iter()
            .find(|w| !w.surface.is_empty())
            .map(|w| w.surface.clone())
            .unwrap_or_default();
        if let Some(c) = build_candidate(g, &id_map, &ring, way_ids, &sport, &surface) {
            out.push(c);
        }
    }

    out.sort_by_key(|c| c.key);
    out
}

/// 去掉闭合环末尾重复点与连续重复点。
fn dedup_ring_nodes(nodes: &[u64]) -> Vec<u64> {
    let mut v = nodes.to_vec();
    if v.len() > 1 && v[0] == v[v.len() - 1] {
        v.pop();
    }
    let mut out: Vec<u64> = Vec::with_capacity(v.len());
    for id in v {
        if out.last() != Some(&id) {
            out.push(id);
        }
    }
    while out.len() > 1 && out[0] == out[out.len() - 1] {
        out.pop();
    }
    out
}

/// 在度全为 2 的连接分量中沿环走一圈（返回不含闭合重复点的节点序列）。
fn walk_cycle(adj: &HashMap<u64, Vec<u64>>, start: u64, want: usize) -> Vec<u64> {
    let mut out = vec![start];
    let Some(first) = adj.get(&start) else {
        return out;
    };
    let mut prev = start;
    let mut cur = first[0];
    while cur != start && out.len() <= want {
        out.push(cur);
        let Some(nbrs) = adj.get(&cur) else {
            break;
        };
        if nbrs.len() != 2 {
            break;
        }
        let next = if nbrs[0] != prev { nbrs[0] } else { nbrs[1] };
        prev = cur;
        cur = next;
    }
    if out.len() == want && cur == start {
        out
    } else {
        Vec::new()
    }
}

/// 由环上唯一节点序列构建候选（闭合、逆时针、带不可用原因）。
fn build_candidate(
    g: &RoadGraph,
    id_map: &HashMap<u64, (Coord, NodeIndex)>,
    ids: &[u64],
    way_ids: Vec<u64>,
    sport: &str,
    surface: &str,
) -> Option<TrackCandidate> {
    if ids.len() < 4 {
        return None;
    }
    let mut points: Vec<Coord> = Vec::with_capacity(ids.len() + 1);
    let mut ring: Vec<NodeIndex> = Vec::with_capacity(ids.len() + 1);
    for id in ids {
        let (c, n) = *id_map.get(id)?;
        points.push(c);
        ring.push(n);
    }
    points.push(points[0]);
    ring.push(ring[0]);
    let length_m = ring_length(g, &ring[..ring.len() - 1]);
    if length_m < 50.0 {
        return None;
    }
    // 统一逆时针
    if signed_area(&points) < 0.0 {
        points.pop();
        ring.pop();
        points.reverse();
        ring.reverse();
        points.push(points[0]);
        ring.push(ring[0]);
    }
    let mut issue = None;
    if let Some((i, j)) = outline_self_intersects(&points) {
        issue = Some(format!("跑道轮廓自交（第 {}/{} 段）", i + 1, j + 1));
    }
    if issue.is_none() {
        // 拼接只保证无向连通；有向可通行性（单行/断路）在这里判
        if let Err(e) = check_ring_direction(g, &ring) {
            issue = Some(e);
        }
    }
    Some(TrackCandidate {
        key: way_ids.iter().copied().min().unwrap_or(0),
        way_ids,
        points,
        ring,
        length_m,
        sport: sport.to_string(),
        surface: surface.to_string(),
        issue,
    })
}

fn ring_length(g: &RoadGraph, nodes: &[NodeIndex]) -> f64 {
    let n = nodes.len();
    if n < 2 {
        return 0.0;
    }
    (0..n)
        .map(|i| {
            dist(
                g.to_m(g.node_coord(nodes[i])),
                g.to_m(g.node_coord(nodes[(i + 1) % n])),
            )
        })
        .sum()
}

fn push_coord(v: &mut Vec<Coord>, c: Coord) {
    if let Some(p) = v.last() {
        if (p.lon - c.lon).abs() < 1e-12 && (p.lat - c.lat).abs() < 1e-12 {
            return;
        }
    }
    v.push(c);
}

fn push_connector(g: &RoadGraph, path: &mut Vec<NodeIndex>, seg: &[NodeIndex], len: &mut f64) {
    for &n in seg.iter().skip(1) {
        let prev = *path.last().unwrap();
        if prev == n {
            continue;
        }
        *len += dist(g.to_m(g.node_coord(prev)), g.to_m(g.node_coord(n)));
        path.push(n);
    }
}

/// 跑道模式路由：连接段（起点 → 按序必经点 → 入环点，只执行一次），
/// 随后整圈 × n，最后按剩余距离走部分圈。
pub fn plan_track_route(
    g: &RoadGraph,
    ring: &[NodeIndex],
    start: Coord,
    must: &[Coord],
    target_len_m: f64,
    opts: &RouteOptions,
) -> Result<TrackRoute, String> {
    if ring.len() < 4 {
        return Err("跑道轮廓顶点不足".into());
    }
    check_ring_direction(g, ring)?;

    let unique: Vec<NodeIndex> = ring[..ring.len() - 1].to_vec();
    // 入环点：离最后一个必经点最近（无必经点则离起点最近）的跑道节点
    let anchor = must.last().copied().unwrap_or(start);
    let entry = unique
        .iter()
        .copied()
        .min_by(|a, b| {
            dist(g.to_m(g.node_coord(*a)), g.to_m(anchor))
                .partial_cmp(&dist(g.to_m(g.node_coord(*b)), g.to_m(anchor)))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or("跑道轮廓为空")?;
    let entry_i = unique.iter().position(|n| *n == entry).unwrap();
    let loop_nodes: Vec<NodeIndex> = (0..unique.len())
        .map(|k| unique[(entry_i + k) % unique.len()])
        .collect();
    let ring_len = ring_length(g, &loop_nodes);
    if ring_len < 10.0 {
        return Err("跑道单圈长度异常".into());
    }

    // —— 连接段：起点 → 必经点（按序）→ 入环点 ——
    let mut cg = g.clone();
    let mut queries: Vec<Coord> = Vec::with_capacity(must.len() + 2);
    queries.push(start);
    queries.extend(must.iter().copied());
    queries.push(g.node_coord(loop_nodes[0]));
    // 逐点投影：避免 add_virtual_nodes 的坐标去重让「起点=必经点=入环点」时索引错位
    let mut vn: Vec<NodeIndex> = Vec::with_capacity(queries.len());
    for q in &queries {
        let single = add_virtual_nodes(&mut cg, std::slice::from_ref(q))?;
        vn.push(single[0]);
    }
    let start_n = vn[0];
    let entry_n = *vn.last().unwrap();
    let mut path = vec![start_n];
    let mut connector_m = 0.0;
    let mut cur = start_n;
    for &m in &vn[1..vn.len() - 1] {
        let seg = shortest_path(&cg, cur, m).ok_or_else(|| {
            let c = cg.node_coord(m);
            format!(
                "必经点 ({:.6},{:.6}) 不可达：可能落在围栏外或路网不连通",
                c.lat, c.lon
            )
        })?;
        push_connector(&cg, &mut path, &seg, &mut connector_m);
        cur = m;
    }
    if cur != entry_n {
        let seg = shortest_path(&cg, cur, entry_n).ok_or_else(|| {
            let c = cg.node_coord(entry_n);
            format!("跑道入口 ({:.6},{:.6}) 不可达", c.lat, c.lon)
        })?;
        push_connector(&cg, &mut path, &seg, &mut connector_m);
    }
    if connector_m > target_len_m + 0.5 {
        return Err(format!(
            "连接段 {:.0}m 已超过目标距离 {:.0}m，无法再完成整圈",
            connector_m, target_len_m
        ));
    }

    // —— 整圈 + 最后的部分圈（同一份弧长口径）——
    let mut coords: Vec<Coord> = path.iter().map(|&n| cg.node_coord(n)).collect();
    let ring_coords: Vec<Coord> = loop_nodes.iter().map(|&n| g.node_coord(n)).collect();
    let remain = (target_len_m - connector_m).max(0.0);
    let laps = (remain / ring_len).floor() as usize;
    let partial_m = remain - laps as f64 * ring_len;
    for _ in 0..laps {
        for &c in &ring_coords {
            push_coord(&mut coords, c);
        }
        // 补闭合边：整圈必须走回入环点，否则每圈都少最后一段
        push_coord(&mut coords, ring_coords[0]);
    }
    if partial_m > 0.5 {
        let mut walked = 0.0;
        for k in 0..ring_coords.len() {
            let a = ring_coords[k];
            let b = ring_coords[(k + 1) % ring_coords.len()];
            let seg = dist(g.to_m(a), g.to_m(b));
            if seg <= 1e-9 {
                continue;
            }
            if walked + seg >= partial_m {
                let t = ((partial_m - walked) / seg).clamp(0.0, 1.0);
                push_coord(
                    &mut coords,
                    Coord::new(a.lon + (b.lon - a.lon) * t, a.lat + (b.lat - a.lat) * t),
                );
                break;
            }
            push_coord(&mut coords, a);
            walked += seg;
        }
    }
    if coords.len() < 2 {
        return Err("跑道路线为空".into());
    }
    let points = smooth_and_sample(g, &coords, opts.min_radius_m, opts.turn_threshold_deg, 1.0);
    if points.is_empty() {
        return Err("跑道路线平滑后为空".into());
    }
    let length_m = points.last().unwrap().s;
    Ok(TrackRoute {
        route: Route { points, length_m },
        connector_m,
        ring_len,
        laps,
        partial_m: partial_m.max(0.0),
        entry: ring_coords[0],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeData;
    use crate::load::parse_osm;
    use crate::load::build_graph;

    /// 400m 矩形跑道（两段 way 拼接）+ 一条接入道路。
    fn fixture() -> RoadGraph {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<osm version="0.6">
  <node id="1" lat="30.0000" lon="120.0000"/>
  <node id="2" lat="30.0000" lon="120.0010"/>
  <node id="3" lat="30.0010" lon="120.0010"/>
  <node id="4" lat="30.0010" lon="120.0000"/>
  <node id="10" lat="30.0000" lon="120.0020"/>
  <way id="100">
    <nd ref="1"/><nd ref="2"/>
    <tag k="leisure" v="track"/><tag k="sport" v="running"/>
  </way>
  <way id="101">
    <nd ref="2"/><nd ref="3"/><nd ref="4"/><nd ref="1"/>
    <tag k="leisure" v="track"/><tag k="sport" v="running"/>
  </way>
  <way id="200">
    <nd ref="1"/><nd ref="10"/>
    <tag k="highway" v="footway"/>
  </way>
</osm>"#;
        let parsed = parse_osm(xml.as_bytes()).expect("parse");
        build_graph(&parsed)
    }

    #[test]
    fn stitches_two_ways_into_one_ring() {
        let g = fixture();
        let cands = track_candidates(&g);
        assert_eq!(cands.len(), 1, "两段无歧义拼接应得到一个候选");
        let c = &cands[0];
        assert!(c.issue.is_none(), "候选应可用：{:?}", c.issue);
        assert_eq!(c.ring.len(), c.points.len());
        assert_eq!(c.ring.first(), c.ring.last());
        assert!(c.length_m > 100.0);
        assert!(signed_area(&c.points) > 0.0, "应统一为逆时针");
    }

    #[test]
    fn ambiguous_junction_is_not_offered() {
        let mut g = fixture();
        // 第三条跑道 way 也接在节点 2 上：度 3，拼接歧义 → 不提供
        let n1 = g
            .graph
            .node_indices()
            .find(|n| {
                (g.node_coord(*n).lon - 120.0010).abs() < 1e-9
                    && (g.node_coord(*n).lat - 30.0000).abs() < 1e-9
            })
            .unwrap();
        let v = g.add_node(Coord::new(120.0015, 30.0005));
        g.add_edge(
            n1,
            v,
            EdgeData {
                way_id: 102,
                oneway: false,
                highway: "track".into(),
                maxspeed_kmh: None,
                length_m: 60.0,
            },
        );
        g.track_ways.push(crate::graph::TrackWay {
            id: 102,
            node_refs: vec![2, 999],
            graph_nodes: vec![n1, v],
            oneway: false,
            oneway_reversed: false,
            sport: "running".into(),
            surface: String::new(),
        });
        // 999 不在 id_map 里也无所谓（只有 2 参与度统计）
        let cands = track_candidates(&g);
        assert!(cands.is_empty(), "歧义拼接不应提供候选");
    }

    #[test]
    fn connector_once_then_laps_and_partial() {
        let g = fixture();
        let c = &track_candidates(&g)[0];
        let ring = c.ring.clone();
        let start = Coord::new(120.0020, 30.0000);
        let opts = RouteOptions::default();
        // 连接段约 96m；目标 300m → 不足一圈，走部分圈
        let tr = plan_track_route(&g, &ring, start, &[], 300.0, &opts).expect("plan");
        assert!(tr.connector_m > 50.0);
        assert_eq!(tr.laps, 0);
        assert!(tr.partial_m > 0.0);
        assert!(tr.route.length_m > 0.0);
        // 目标大于连接段 + 一整圈：应至少 1 圈
        let tr2 = plan_track_route(&g, &ring, start, &[], tr.connector_m + tr.ring_len + 30.0, &opts)
            .expect("plan2");
        assert_eq!(tr2.laps, 1);
        assert!(tr2.partial_m <= tr2.ring_len);
    }

    #[test]
    fn unreachable_must_point_returns_err() {
        let mut g = fixture();
        // 一段与跑道不连通的孤立道路，把必经点放在它上面 → 不可达
        let a = g.add_node(Coord::new(120.0030, 30.0030));
        let b = g.add_node(Coord::new(120.0040, 30.0030));
        for (from, to) in [(a, b), (b, a)] {
            g.add_edge(
                from,
                to,
                EdgeData {
                    way_id: 300,
                    oneway: false,
                    highway: "footway".into(),
                    maxspeed_kmh: None,
                    length_m: 96.0,
                },
            );
        }
        let c = &track_candidates(&g)[0];
        let opts = RouteOptions::default();
        let err = plan_track_route(
            &g,
            &c.ring,
            Coord::new(120.0020, 30.0000),
            &[Coord::new(120.0035, 30.0030)],
            300.0,
            &opts,
        );
        assert!(err.is_err(), "不可达必经点必须报错");
    }

    #[test]
    fn manual_outline_requires_shared_nodes() {
        let g = fixture();
        // 顶点偏离道路节点 20m → 拒绝
        let off = vec![
            (30.0000, 120.0000),
            (30.0000, 120.0010),
            (30.0010, 120.0010),
            (30.0010, 120.0000),
        ];
        let ok = validate_outline(&g, &off, 5.0);
        assert!(ok.is_ok(), "共享节点上的轮廓应通过：{:?}", ok.err());

        let bad = vec![
            (30.0000, 120.0002),
            (30.0000, 120.0010),
            (30.0010, 120.0010),
            (30.0010, 120.0000),
        ];
        assert!(validate_outline(&g, &bad, 5.0).is_err(), "偏离节点必须拒绝");
    }
}
