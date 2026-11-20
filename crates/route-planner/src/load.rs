//! OSM `.osm`（XML）解析：节点 / 道路 way / 建筑 way+multipolygon relation。
//!
//! 输入为 openstreetmap.org 导出的标准 OSM XML（`<node>`/`<way>`/`<relation>`）。
//! 只抽取：`highway` 路径（剔除机动车道）、`building` 轮廓。

use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::graph::{Coord, EdgeData, RoadGraph, TrackWay};

/// 一条跑道 way（OSM 原始标签 + 节点序列），供候选拼接使用。
pub struct OsmTrackWay {
    pub id: u64,
    pub node_refs: Vec<u64>,
    pub oneway: bool,
    pub oneway_reversed: bool,
    pub sport: String,
    pub surface: String,
}

/// 一条道路 way。
pub struct OsmRoad {
    pub id: u64,
    pub node_refs: Vec<u64>,
    pub oneway: bool,
    pub oneway_reversed: bool,
    pub highway: String,
    pub maxspeed_kmh: Option<f64>,
}

/// 一个建筑：外环为节点 id 序列。
pub struct OsmBuilding {
    pub outer: Vec<Vec<u64>>,
}

pub struct ParsedOsm {
    pub nodes: HashMap<u64, Coord>,
    pub roads: Vec<OsmRoad>,
    pub buildings: Vec<OsmBuilding>,
    /// 跑道 way（同时并入 `roads`，使路由可直接沿跑道行进）。
    pub tracks: Vec<OsmTrackWay>,
}

/// OSM 元素 ID 解析。
///
/// JOSM 未上传的导出里所有元素 ID 都是负数（如 `-26132`），直接
/// `parse::<u64>()` 会整批失败，导致节点与 way 引用全被丢弃（地图空白）。
/// 这里先按 `i64` 解析，再无损映射到 `u64`（负数走二进制补码）；定义端与
/// 引用端共用同一映射，查找依然一致。
fn parse_osm_id(s: &str) -> Option<u64> {
    s.trim().parse::<i64>().ok().map(|v| v as u64)
}

/// 不应被当作可跑步道路的 highway 值。
fn is_motorized(h: &str) -> bool {
    matches!(
        h,
        "motorway"
            | "motorway_link"
            | "trunk"
            | "trunk_link"
            | "primary"
            | "primary_link"
            | "secondary"
            | "secondary_link"
            | "construction"
            | "proposed"
    )
}

fn parse_maxspeed(v: &str) -> Option<f64> {
    let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let n: f64 = digits.parse().ok()?;
    if v.to_ascii_lowercase().contains("mph") {
        Some(n * 1.60934)
    } else {
        Some(n)
    }
}

struct Attrs {
    map: HashMap<String, String>,
}

impl Attrs {
    fn new() -> Self {
        Attrs {
            map: HashMap::new(),
        }
    }
    fn insert(&mut self, k: &[u8], v: &[u8]) {
        self.map.insert(
            String::from_utf8_lossy(k).into_owned(),
            String::from_utf8_lossy(v).into_owned(),
        );
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(|s| s.as_str())
    }
    fn id(&self) -> Option<u64> {
        self.get("id").and_then(parse_osm_id)
    }
}

/// 解析 OSM XML 字节流。
pub fn parse_osm(bytes: &[u8]) -> Result<ParsedOsm, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    let mut nodes: HashMap<u64, Coord> = HashMap::new();
    let mut roads: Vec<OsmRoad> = Vec::new();
    let mut buildings: Vec<OsmBuilding> = Vec::new();
    let mut tracks: Vec<OsmTrackWay> = Vec::new();

    // 所有 way 的节点序列（供 relation 建筑外环回填）。
    let mut way_nodes: HashMap<u64, Vec<u64>> = HashMap::new();
    // relation 建筑：收集 outer way id。
    let mut rel_buildings: Vec<Vec<u64>> = Vec::new();

    let mut cur_way: Option<(u64, Vec<u64>, Vec<(String, String)>)> = None;
    let mut cur_rel: Option<Vec<(String, String, String)>> = None;
    let mut cur_rel_tags: Vec<(String, String)> = Vec::new();

    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let mut attrs = Attrs::new();
                for a in e.attributes().with_checks(false) {
                    let a = a.map_err(|e| format!("属性解析失败: {e}"))?;
                    attrs.insert(a.key.as_ref(), a.value.as_ref());
                }
                match local.as_str() {
                    "node" => {
                        if let (Some(id), Some(lat), Some(lon)) = (
                            attrs.id(),
                            attrs.get("lat").and_then(|v| v.parse().ok()),
                            attrs.get("lon").and_then(|v| v.parse().ok()),
                        ) {
                            nodes.insert(id, Coord::new(lon, lat));
                        }
                    }
                    "way" => {
                        cur_way = Some((attrs.id().unwrap_or(0), Vec::new(), Vec::new()));
                    }
                    "relation" => {
                        cur_rel = Some(Vec::new());
                        cur_rel_tags = Vec::new();
                    }
                    "nd" => {
                        if let Some((_, refs, _)) = cur_way.as_mut() {
                            if let Some(r) = attrs.get("ref").and_then(parse_osm_id) {
                                refs.push(r);
                            }
                        }
                    }
                    "tag" => {
                        let k = attrs.get("k").unwrap_or("").to_string();
                        let v = attrs.get("v").unwrap_or("").to_string();
                        if let Some((_, _, tags)) = cur_way.as_mut() {
                            tags.push((k.clone(), v.clone()));
                        }
                        if cur_rel.is_some() {
                            cur_rel_tags.push((k, v));
                        }
                    }
                    "member" => {
                        if let Some(members) = cur_rel.as_mut() {
                            let mtype = attrs.get("type").unwrap_or("").to_string();
                            let mref = attrs.get("ref").unwrap_or("").to_string();
                            let role = attrs.get("role").unwrap_or("").to_string();
                            members.push((mtype, mref, role));
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let local = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match local.as_str() {
                    "way" => {
                        if let Some((id, refs, tags)) = cur_way.take() {
                            way_nodes.insert(id, refs.clone());
                            let is_building =
                                tags.iter().any(|(k, v)| k == "building" && v != "no");
                            if is_building && refs.len() >= 3 {
                                buildings.push(OsmBuilding { outer: vec![refs] });
                            } else if refs.len() >= 2 {
                                let highway = tags
                                    .iter()
                                    .find(|(k, _)| k == "highway")
                                    .map(|(_, v)| v.clone());
                                let sport = tags
                                    .iter()
                                    .find(|(k, _)| k == "sport")
                                    .map(|(_, v)| v.clone())
                                    .unwrap_or_default();
                                let surface = tags
                                    .iter()
                                    .find(|(k, _)| k == "surface")
                                    .map(|(_, v)| v.clone())
                                    .unwrap_or_default();
                                let leisure_track = tags
                                    .iter()
                                    .any(|(k, v)| k == "leisure" && v == "track");
                                // 田径/跑步跑道：leisure=track 且 sport 含 running|athletics（未标 sport 也接受）。
                                let running_track = leisure_track
                                    && (sport.is_empty()
                                        || sport.contains("running")
                                        || sport.contains("athletics"));
                                let is_track =
                                    running_track || highway.as_deref() == Some("track");
                                let oneway = tags.iter().any(|(k, v)| {
                                    k == "oneway" && matches!(v.as_str(), "yes" | "true" | "1")
                                });
                                let oneway_reversed =
                                    tags.iter().any(|(k, v)| k == "oneway" && v == "-1");
                                if is_track {
                                    tracks.push(OsmTrackWay {
                                        id,
                                        node_refs: refs.clone(),
                                        oneway,
                                        oneway_reversed,
                                        sport: sport.clone(),
                                        surface: surface.clone(),
                                    });
                                }
                                // 跑道本身也要进入路网，否则连接段无法落到跑道上。
                                // 无 highway 标签的 leisure=track 以 "track" 作为虚拟道路类型。
                                let runnable = match &highway {
                                    Some(h) => !is_motorized(h),
                                    None => running_track,
                                };
                                if runnable {
                                    let maxspeed = tags
                                        .iter()
                                        .find(|(k, _)| k == "maxspeed")
                                        .and_then(|(_, v)| parse_maxspeed(v));
                                    roads.push(OsmRoad {
                                        id,
                                        node_refs: refs,
                                        oneway,
                                        oneway_reversed,
                                        highway: highway.unwrap_or_else(|| "track".into()),
                                        maxspeed_kmh: maxspeed,
                                    });
                                }
                            }
                        }
                    }
                    "relation" => {
                        if let Some(members) = cur_rel.take() {
                            let is_building = cur_rel_tags
                                .iter()
                                .any(|(k, v)| k == "building" && v != "no");
                            if is_building {
                                let outer: Vec<u64> = members
                                    .iter()
                                    .filter(|(mtype, _r, role)| mtype == "way" && role == "outer")
                                    .filter_map(|(_, r, _)| parse_osm_id(r))
                                    .collect();
                                if !outer.is_empty() {
                                    rel_buildings.push(outer);
                                }
                            }
                        }
                        cur_rel_tags = Vec::new();
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("OSM 解析失败: {e}")),
            _ => {}
        }
    }

    // 回填 relation 建筑外环（outer way 的节点序列）。
    for outer_ways in rel_buildings {
        let mut rings: Vec<Vec<u64>> = Vec::new();
        for wid in outer_ways {
            if let Some(refs) = way_nodes.get(&wid) {
                if refs.len() >= 3 {
                    rings.push(refs.clone());
                }
            }
        }
        if !rings.is_empty() {
            buildings.push(OsmBuilding { outer: rings });
        }
    }

    Ok(ParsedOsm {
        nodes,
        roads,
        buildings,
        tracks,
    })
}

/// 用道路 way 构建路网图。
pub fn build_graph(parsed: &ParsedOsm) -> RoadGraph {
    let mut g = RoadGraph::new();
    let mut node_map: HashMap<u64, petgraph::graph::NodeIndex> = HashMap::new();

    for r in &parsed.roads {
        // 先解析节点 id → NodeIndex（避免闭包跨迭代借用）。
        let idxs: Vec<petgraph::graph::NodeIndex> = r
            .node_refs
            .iter()
            .filter_map(|id| {
                let c = *parsed.nodes.get(id)?;
                if let Some(&n) = node_map.get(id) {
                    Some(n)
                } else {
                    let n = g.add_node(c);
                    node_map.insert(*id, n);
                    Some(n)
                }
            })
            .collect();
        for w in idxs.windows(2) {
            let (p, next) = (w[0], w[1]);
            let a = g.node_coord(p);
            let b = g.node_coord(next);
            let len = geo_haversine(a, b);
            if !r.oneway_reversed {
                g.add_edge(
                    p,
                    next,
                    EdgeData {
                        way_id: r.id,
                        oneway: r.oneway,
                        highway: r.highway.clone(),
                        maxspeed_kmh: r.maxspeed_kmh,
                        length_m: len,
                    },
                );
            }
            if !r.oneway {
                g.add_edge(
                    next,
                    p,
                    EdgeData {
                        way_id: r.id,
                        oneway: r.oneway,
                        highway: r.highway.clone(),
                        maxspeed_kmh: r.maxspeed_kmh,
                        length_m: len,
                    },
                );
            }
        }
    }

    let mut bld: Vec<Vec<Coord>> = Vec::new();
    for b in &parsed.buildings {
        for ring in &b.outer {
            let coords: Vec<Coord> = ring
                .iter()
                .filter_map(|id| parsed.nodes.get(id).copied())
                .collect();
            if coords.len() >= 3 {
                bld.push(coords);
            }
        }
    }
    g.buildings = bld;

    // 跑道 way 的节点在 roads 中已建好，直接回填图节点；缺节点的 way 丢弃（无法拼接）。
    let mut track_ways: Vec<TrackWay> = Vec::new();
    for t in &parsed.tracks {
        let mut refs = Vec::with_capacity(t.node_refs.len());
        let mut nodes = Vec::with_capacity(t.node_refs.len());
        let mut ok = true;
        for id in &t.node_refs {
            match node_map.get(id) {
                Some(&n) => {
                    refs.push(*id);
                    nodes.push(n);
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && nodes.len() >= 2 {
            track_ways.push(TrackWay {
                id: t.id,
                node_refs: refs,
                graph_nodes: nodes,
                oneway: t.oneway,
                oneway_reversed: t.oneway_reversed,
                sport: t.sport.clone(),
                surface: t.surface.clone(),
            });
        }
    }
    g.track_ways = track_ways;

    g.compute_anchor();
    g.rebuild_index();
    g
}

/// 完整流程：解析 OSM → 构建路网。
pub fn load_osm(bytes: &[u8]) -> Result<RoadGraph, String> {
    let parsed = parse_osm(bytes)?;
    if parsed.roads.is_empty() {
        // 带上解析统计：节点数为 0 说明坐标/ID 没解析出来；节点正常但没有道路
        // 说明 way 缺 highway 标签或全被 is_motorized 过滤。
        return Err(format!(
            "OSM 中未找到可用的 highway 道路（解析到 {} 个节点、{} 个建筑、{} 条跑道）",
            parsed.nodes.len(),
            parsed.buildings.len(),
            parsed.tracks.len()
        ));
    }
    Ok(build_graph(&parsed))
}

/// 球面近似距离（米）。
fn geo_haversine(a: Coord, b: Coord) -> f64 {
    use geo::HaversineDistance;
    let pa = geo::Point::new(a.lon, a.lat);
    let pb = geo::Point::new(b.lon, b.lat);
    pa.haversine_distance(&pb)
}

// ===================== JSON 路网（GeoJSON / Overpass / 通用 nodes+ways） =====================

/// 坐标 → 节点 id 的量化去重（1e-7 度 ≈ 1.1 cm）。GeoJSON 没有节点 id，靠它生成稳定 id。
fn node_for(
    coord_id: &mut HashMap<(i64, i64), u64>,
    nodes: &mut HashMap<u64, Coord>,
    next_id: &mut u64,
    lon: f64,
    lat: f64,
) -> u64 {
    let key = (
        (lon * 10_000_000.0).round() as i64,
        (lat * 10_000_000.0).round() as i64,
    );
    if let Some(&id) = coord_id.get(&key) {
        return id;
    }
    let id = *next_id;
    *next_id += 1;
    coord_id.insert(key, id);
    nodes.insert(id, Coord::new(lon, lat));
    id
}

/// 读一个 JSON 布尔标记：兼容 true / "yes" / 1。
fn json_flag(v: &serde_json::Value, key: &str) -> bool {
    match v.get(key) {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => {
            let t = s.trim().to_ascii_lowercase();
            t == "yes" || t == "true" || t == "1"
        }
        Some(serde_json::Value::Number(n)) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
        _ => false,
    }
}

fn json_f64(v: &serde_json::Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
    })
}

#[allow(clippy::too_many_arguments)]
fn push_geojson_line(
    line: &serde_json::Value,
    coord_id: &mut HashMap<(i64, i64), u64>,
    nodes: &mut HashMap<u64, Coord>,
    next_id: &mut u64,
    roads: &mut Vec<OsmRoad>,
    way_id: &mut u64,
    highway: &str,
    oneway: bool,
    maxspeed: Option<f64>,
) {
    let Some(arr) = line.as_array() else { return };
    let mut refs = Vec::with_capacity(arr.len());
    for pt in arr {
        if let Some(c) = pt.as_array() {
            let lon = c.first().and_then(|x| x.as_f64());
            let lat = c.get(1).and_then(|x| x.as_f64());
            if let (Some(lon), Some(lat)) = (lon, lat) {
                refs.push(node_for(coord_id, nodes, next_id, lon, lat));
            }
        }
    }
    if refs.len() >= 2 {
        roads.push(OsmRoad {
            id: *way_id,
            node_refs: refs,
            oneway,
            oneway_reversed: false,
            highway: highway.to_string(),
            maxspeed_kmh: maxspeed,
        });
        *way_id += 1;
    }
}

/// 把一条 GeoJSON 坐标环（`[[lon,lat],...]`）转成节点 id 序列。
fn push_geojson_ring(
    ring: &serde_json::Value,
    coord_id: &mut HashMap<(i64, i64), u64>,
    nodes: &mut HashMap<u64, Coord>,
    next_id: &mut u64,
) -> Vec<u64> {
    let Some(arr) = ring.as_array() else {
        return Vec::new();
    };
    let mut refs = Vec::with_capacity(arr.len());
    for pt in arr {
        if let Some(c) = pt.as_array() {
            let lon = c.first().and_then(|x| x.as_f64());
            let lat = c.get(1).and_then(|x| x.as_f64());
            if let (Some(lon), Some(lat)) = (lon, lat) {
                refs.push(node_for(coord_id, nodes, next_id, lon, lat));
            }
        }
    }
    refs
}

/// GeoJSON：FeatureCollection 里的 LineString / MultiLineString / Polygon / MultiPolygon。
fn parse_geojson(v: &serde_json::Value) -> Result<ParsedOsm, String> {
    let mut nodes = HashMap::new();
    let mut coord_id: HashMap<(i64, i64), u64> = HashMap::new();
    let mut next_id: u64 = 1;
    let mut roads = Vec::new();
    let mut way_id: u64 = 1;
    let mut buildings: Vec<OsmBuilding> = Vec::new();
    let features = v["features"].as_array().cloned().unwrap_or_default();
    for feat in features {
        let geom = &feat["geometry"];
        let props = &feat["properties"];
        let highway = props
            .get("highway")
            .and_then(|x| x.as_str())
            .unwrap_or("footway");
        let is_building_feature = props
            .get("building")
            .map(|x| {
                x.as_str().map(|s| !s.eq_ignore_ascii_case("no")).unwrap_or(true)
            })
            .unwrap_or(false);
        let oneway = json_flag(props, "oneway");
        let maxspeed = json_f64(props, "maxspeed");
        match geom["type"].as_str() {
            Some("LineString") => push_geojson_line(
                &geom["coordinates"],
                &mut coord_id,
                &mut nodes,
                &mut next_id,
                &mut roads,
                &mut way_id,
                highway,
                oneway,
                maxspeed,
            ),
            Some("MultiLineString") => {
                if let Some(parts) = geom["coordinates"].as_array() {
                    for part in parts {
                        push_geojson_line(
                            part,
                            &mut coord_id,
                            &mut nodes,
                            &mut next_id,
                            &mut roads,
                            &mut way_id,
                            highway,
                            oneway,
                            maxspeed,
                        );
                    }
                }
            }
            Some("Polygon") => {
                // coordinates[0] 是外环，其余是洞；建筑只用外环
                if let Some(rings) = geom["coordinates"].as_array() {
                    if let Some(outer) = rings.first() {
                        let r = push_geojson_ring(
                            outer,
                            &mut coord_id,
                            &mut nodes,
                            &mut next_id,
                        );
                        if r.len() >= 3 {
                            buildings.push(OsmBuilding { outer: vec![r] });
                        }
                    }
                }
                let _ = is_building_feature;
            }
            Some("MultiPolygon") => {
                if let Some(polys) = geom["coordinates"].as_array() {
                    for poly in polys {
                        if let Some(rings) = poly.as_array() {
                            if let Some(outer) = rings.first() {
                                let r = push_geojson_ring(
                                    outer,
                                    &mut coord_id,
                                    &mut nodes,
                                    &mut next_id,
                                );
                                if r.len() >= 3 {
                                    buildings.push(OsmBuilding { outer: vec![r] });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if nodes.is_empty() || (roads.is_empty() && buildings.is_empty()) {
        return Err("GeoJSON 里没有可用的 LineString / Polygon 要素".into());
    }
    Ok(ParsedOsm {
        nodes,
        roads,
        buildings,
        tracks: Vec::new(),
    })
}

/// Overpass API 的 JSON（`{"elements":[{"type":"node",...},{"type":"way",...}]}`）。
fn parse_overpass(v: &serde_json::Value) -> Result<ParsedOsm, String> {
    let mut nodes = HashMap::new();
    let mut roads = Vec::new();
    let mut buildings: Vec<OsmBuilding> = Vec::new();
    let elements = v["elements"].as_array().cloned().unwrap_or_default();
    // 先收节点
    for el in &elements {
        if el["type"].as_str() != Some("node") {
            continue;
        }
        let id = el["id"].as_u64();
        let lat = el["lat"].as_f64();
        let lon = el["lon"].as_f64();
        if let (Some(id), Some(lat), Some(lon)) = (id, lat, lon) {
            nodes.insert(id, Coord::new(lon, lat));
        }
    }
    // 再收 way（区分建筑与道路）
    for el in &elements {
        if el["type"].as_str() != Some("way") {
            continue;
        }
        let id = el["id"].as_u64().unwrap_or(0);
        let refs: Vec<u64> = el["nodes"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_u64()).collect())
            .unwrap_or_default();
        if refs.len() < 2 {
            continue;
        }
        let tags = &el["tags"];
        // building=* 且不是 no → 当建筑轮廓（>=3 点），不再作道路
        let is_building = tags
            .get("building")
            .map(|x| {
                x.as_str()
                    .map(|s| !s.eq_ignore_ascii_case("no"))
                    .unwrap_or(true)
            })
            .unwrap_or(false);
        if is_building && refs.len() >= 3 {
            buildings.push(OsmBuilding { outer: vec![refs] });
            continue;
        }
        let highway = tags.get("highway").and_then(|x| x.as_str()).unwrap_or("footway");
        roads.push(OsmRoad {
            id,
            node_refs: refs,
            oneway: json_flag(tags, "oneway"),
            oneway_reversed: false,
            highway: highway.to_string(),
            maxspeed_kmh: json_f64(tags, "maxspeed"),
        });
    }
    if nodes.is_empty() || (roads.is_empty() && buildings.is_empty()) {
        return Err("Overpass JSON 里没有可用的 way 元素".into());
    }
    Ok(ParsedOsm {
        nodes,
        roads,
        buildings,
        tracks: Vec::new(),
    })
}

/// 通用路网 JSON：`{"nodes":[{"id","lat","lon"}], "ways":[{"id","nodes":[id...],"highway","oneway"}]}`
fn parse_generic(v: &serde_json::Value) -> Result<ParsedOsm, String> {
    let mut nodes = HashMap::new();
    let mut roads = Vec::new();
    for nd in v["nodes"].as_array().cloned().unwrap_or_default() {
        let id = nd["id"]
            .as_u64()
            .or_else(|| nd["id"].as_str().and_then(|s| s.parse().ok()));
        let lat = nd["lat"]
            .as_f64()
            .or_else(|| nd["latitude"].as_f64());
        let lon = nd["lon"]
            .as_f64()
            .or_else(|| nd["lng"].as_f64())
            .or_else(|| nd["longitude"].as_f64());
        if let (Some(id), Some(lat), Some(lon)) = (id, lat, lon) {
            nodes.insert(id, Coord::new(lon, lat));
        }
    }
    for wy in v["ways"].as_array().cloned().unwrap_or_default() {
        let id = wy["id"].as_u64().unwrap_or(0);
        let refs: Vec<u64> = wy["nodes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        x.as_u64()
                            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if refs.len() < 2 {
            continue;
        }
        let highway = wy["highway"].as_str().unwrap_or("footway");
        roads.push(OsmRoad {
            id,
            node_refs: refs,
            oneway: json_flag(&wy, "oneway"),
            oneway_reversed: false,
            highway: highway.to_string(),
            maxspeed_kmh: json_f64(&wy, "maxspeed"),
        });
    }
    let buildings = parse_generic_buildings(v);
    if nodes.is_empty() || (roads.is_empty() && buildings.is_empty()) {
        return Err("JSON 里 nodes/ways 为空或字段缺失".into());
    }
    Ok(ParsedOsm {
        nodes,
        roads,
        buildings,
        tracks: Vec::new(),
    })
}

/// 通用 JSON 里的建筑：支持两种写法
/// - `"buildings": [[id1,id2,id3,...], ...]`（节点 id 环数组）
/// - `"buildings": [{"nodes": [id...]}, ...]`
fn parse_generic_buildings(v: &serde_json::Value) -> Vec<OsmBuilding> {
    let mut out = Vec::new();
    for b in v["buildings"].as_array().cloned().unwrap_or_default() {
        let ring: Vec<u64> = if let Some(arr) = b.as_array() {
            arr.iter()
                .filter_map(|x| {
                    x.as_u64()
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                })
                .collect()
        } else {
            b["nodes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            x.as_u64()
                                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        if ring.len() >= 3 {
            out.push(OsmBuilding { outer: vec![ring] });
        }
    }
    out
}

/// 解析 JSON 路网（自动识别 GeoJSON / Overpass / 通用 nodes+ways）。
pub fn parse_json(bytes: &[u8]) -> Result<ParsedOsm, String> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("JSON 解析失败: {e}"))?;
    if v.get("type").and_then(|t| t.as_str()) == Some("FeatureCollection") {
        return parse_geojson(&v);
    }
    if v.get("elements").is_some() {
        return parse_overpass(&v);
    }
    if v.get("nodes").is_some() && v.get("ways").is_some() {
        return parse_generic(&v);
    }
    Err("无法识别的 JSON 路网格式（支持 GeoJSON / Overpass / {nodes,ways}）".into())
}

/// 按扩展名 + 内容嗅探自动选择解析器：`.json` 或首字符为 `{`/`[` 走 JSON，否则按 OSM XML。
pub fn load_auto(bytes: &[u8], path: &str) -> Result<RoadGraph, String> {
    let lower = path.to_ascii_lowercase();
    let looks_json = lower.ends_with(".json")
        || lower.ends_with(".geojson")
        || bytes
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .map(|b| *b == b'{' || *b == b'[')
            .unwrap_or(false);
    let parsed = if looks_json {
        parse_json(bytes)?
    } else {
        parse_osm(bytes)?
    };
    if parsed.roads.is_empty() {
        return Err(format!(
            "路网中没有可用道路：解析到 {} 个节点、{} 个建筑、{} 条跑道；请检查 way 的节点引用是否都能对上节点 id",
            parsed.nodes.len(),
            parsed.buildings.len(),
            parsed.tracks.len()
        ));
    }
    Ok(build_graph(&parsed))
}
