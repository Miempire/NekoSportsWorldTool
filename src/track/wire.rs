//! OBS 对象组装。
//!
//! 11 键对象，每值 gzip+base64；BD→GCJ 单次转换；27 键协议点集；
//! 10s 窗（speed/step_freq）；每 1000m 一圈；五点 fixed_point_json。
#![allow(non_snake_case)]

use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use std::io::Write;

use super::altitude::smooth_altitudes;
use super::geom::round_to;
use super::model::{GenPoint, Track};

const X_PI: f64 = std::f64::consts::PI * 3000.0 / 180.0;

/// 百度 BD-09 → 高德 GCJ-02。
pub fn bd09_to_gcj02(bd_lat: f64, bd_lng: f64) -> (f64, f64) {
    let x = bd_lng - 0.0065;
    let y = bd_lat - 0.006;
    let z = (x * x + y * y).sqrt() - 0.00002 * (y * X_PI).sin();
    let theta = y.atan2(x) - 0.000003 * (x * X_PI).cos();
    (z * theta.sin(), z * theta.cos())
}

/// gzip + base64。
pub fn gz(data: &[u8]) -> String {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).expect("gzip 写入内存失败");
    let out = enc.finish().expect("gzip 收尾失败");
    super::super::crypto::envelope::b64_encode(&out)
}

fn gz_json(v: &Value) -> String {
    gz(v.to_string().as_bytes())
}

fn gz_str(v: &str) -> String {
    gz(v.as_bytes())
}

/// 27 键协议点集（gen 点 → OBS 点；gLat/gLng 由 BD 转 GCJ）。
pub fn conv_point(p: &GenPoint, start_ms: i64) -> Value {
    let (glat, glng) = bd09_to_gcj02(p.gLat, p.gLng);
    json!({
        "avgSpeed": round_to(p.avgSpeed, 4),
        "bdA": round_to(p.bdA, 2),
        "bdD": round_to(p.bdD, 2),
        "bdG": p.bdG,
        "bdS": round_to(p.bdS, 4),
        "coorType": "gcj02",
        "count": p.count,
        "dtr": 0.0,
        "flag": start_ms,
        "gLat": round_to(glat, 7),
        "gLng": round_to(glng, 7),
        "gainTime": p.gainTime,
        "id": p.id,
        "lat": -1.0,
        "lng": -1.0,
        "locType": p.locType,
        "locationId": "",
        "queueNum": p.queueNum,
        "radius": round_to(p.radius, 2),
        "speed": round_to(p.speed, 4),
        "state": p.state,
        "stepDistance": 0.0,
        "totalDis": round_to(p.totalDis, 4),
        "totalTime": p.totalTime,
        "type": p.ptype,
        "validDis": round_to(p.validDis, 4),
        "validTime": p.validTime,
    })
}

/// 跑区元数据：随围栏接口一起返回，提交体/OBS 的 `fixed_point_json` 三件套。
/// 默认值 = 自由跑口径（无区域）。
#[derive(Clone, Debug)]
pub struct RunAreaMeta {
    pub run_area_id: i64,
    pub geo_fences: Vec<Value>,
    pub freedom_show_fence: bool,
}

impl Default for RunAreaMeta {
    fn default() -> Self {
        Self {
            run_area_id: -1,
            geo_fences: Vec::new(),
            freedom_show_fence: false,
        }
    }
}

/// 五点实体（实时点位 → 跑完态 isPass=true）。
pub fn five_point_payload(points: &[Value], start_ms: i64) -> Vec<Value> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            json!({
                "flag": start_ms,
                "glat": p["glat"].as_f64().unwrap_or(0.0),
                "glon": p["glon"].as_f64().unwrap_or(0.0),
                "id": i as i64 + 1,
                "isFixed": p["isFixed"].as_i64().unwrap_or(0),
                "isPass": true,
                "lat": p["lat"].as_f64().unwrap_or(0.0),
                "lon": p["lon"].as_f64().unwrap_or(0.0),
                "pointName": p["pointName"].as_str().unwrap_or(""),
                "position": i as i64,
                "state": 0,
            })
        })
        .collect()
}

/// 提交 body 的 fivePointJson 包装串（自由跑口径，无跑区）。
pub fn five_point_wrapper(points: &[Value], start_ms: i64) -> String {
    five_point_wrapper_with_area(points, start_ms, &RunAreaMeta::default())
}

/// 提交 body 的 fivePointJson 包装串（带真实跑区）。
pub fn five_point_wrapper_with_area(
    points: &[Value],
    start_ms: i64,
    area: &RunAreaMeta,
) -> String {
    let five = five_point_payload(points, start_ms);
    json!({
        "useZip": false,
        "fivePointJson": Value::Array(five).to_string(),
        "runAreaId": area.run_area_id,
        "geoFencesJson": Value::Array(area.geo_fences.clone()).to_string(),
        "freedomShowFence": area.freedom_show_fence,
    })
    .to_string()
}

/// 校验提交用五点轨迹仍是同一次点位请求产生的有效数据。
pub fn validate_five_point_wrapper(wrapper: &str) -> Result<(), String> {
    let outer: Value = serde_json::from_str(wrapper).map_err(|e| format!("五点轨迹 JSON 无效: {e}"))?;
    let raw = outer.get("fivePointJson").and_then(Value::as_str).ok_or("五点轨迹缺少 fivePointJson")?;
    let points: Vec<Value> = serde_json::from_str(raw).map_err(|e| format!("五点轨迹数组无效: {e}"))?;
    if points.is_empty() { return Err("五点轨迹不能为空".into()); }
    for point in points {
        let lat = point.get("lat").and_then(Value::as_f64).unwrap_or(0.0);
        let lon = point.get("lon").and_then(Value::as_f64).unwrap_or(0.0);
        let glat = point.get("glat").and_then(Value::as_f64).unwrap_or(0.0);
        let glon = point.get("glon").and_then(Value::as_f64).unwrap_or(0.0);
        if (lat == 0.0 && lon == 0.0) || (glat == 0.0 && glon == 0.0) { return Err("五点轨迹包含缺失坐标".into()); }
        crate::location::Coordinate::new(lat, lon, 0.0)?;
        crate::location::Coordinate::new(glat, glon, 0.0)?;
    }
    Ok(())
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    #[test] fn rejects_empty_or_malformed_five_point_payload() { assert!(validate_five_point_wrapper("{}").is_err()); assert!(validate_five_point_wrapper(r#"{"fivePointJson":"[]"}"#).is_err()); }

    #[test]
    fn laps_are_rebuilt_from_overridden_altitude() {
        let points = vec![(38.901678, 121.540241), (38.902564, 121.541233)];
        let mut track = crate::track::generator::build(
            1200.0,
            600,
            7,
            (38.9, 121.54),
            1_700_000_000_000,
            &points,
            None,
        )
        .expect("build");
        crate::track::altitude::override_bd_a(&mut track, 36.75).unwrap();
        let laps = build_laps(&track, track.startTime, "android");
        assert!(!laps.is_empty());
        assert!(laps.iter().all(|lap| lap["elevationGain"] == 0.0));
        assert!(laps.iter().all(|lap| lap["endAltAbs"] == 36.75));
        assert!(laps.iter().all(|lap| lap["endAltRel"] == 0.0));
    }

    /// 整圈必须精确 1000m（插值切圈，不是取首个 ≥1000m 的采样点），
    /// 且 avgPace = 圈时长(分钟)/圈距(公里)，圈距合计 = 总距。
    #[test]
    fn laps_are_exact_1000m_splits() {
        let points = vec![
            (38.901678, 121.540241),
            (38.902564, 121.541233),
            (38.900921, 121.542310),
        ];
        let track = crate::track::generator::build(
            3300.0,
            1220,
            11,
            (38.9, 121.54),
            1_700_000_000_000,
            &points,
            None,
        )
        .expect("build");
        let laps = build_laps(&track, track.startTime, "android");
        assert_eq!(laps.len(), 4, "3300m 应为 3 整圈 + 1 尾圈");
        for (i, lap) in laps.iter().enumerate() {
            let d = lap["distance"].as_f64().unwrap();
            if lap["isFullLap"].as_bool().unwrap() {
                assert!((d - 1000.0).abs() < 1e-6, "第{i}圈 distance={d} 非精确 1000m");
                let t = lap["duration"].as_i64().unwrap();
                let pace = lap["avgPace"].as_f64().unwrap();
                assert!(
                    (pace - t as f64 / 60.0).abs() < 0.01,
                    "第{i}圈 avgPace={pace} 与时长 {t}s 不符"
                );
            }
        }
        let sum: f64 = laps.iter().map(|l| l["distance"].as_f64().unwrap()).sum();
        assert!(
            (sum - track.totalDistance).abs() < 1.0,
            "圈距合计 {sum} ≠ 总距 {}",
            track.totalDistance
        );
    }
}

/// 10s 时间窗，id=(rrid%100000)*1000+窗口序秒（6 个真人样本跨 9 月记录验证一致；
/// 旧版 App 样本为全局序号，不适用当前版本）。
fn build_windows(track: &Track, rrid: i64) -> (Vec<Value>, Vec<Value>) {
    let start_ms = track.startTime;
    let total_time = track.totalTime;
    let mut sp = Vec::new();
    let mut stf = Vec::new();
    for (i, a) in track.speedPerTenSec.iter().enumerate() {
        let b = &track.stepsPerTenSec[i];
        let lo = (i * 10) as i64;
        // 尾窗 time 为实际秒数（通常 <10），距离也是该时长内的真实距离，二者必须一致
        let hi = (lo + a.time).min(total_time);
        let id = (rrid % 100000) * 1000 + hi;
        sp.push(json!({
            "beginTime": start_ms + lo * 1000,
            "distance": a.value,
            "endTime": start_ms + hi * 1000,
            "flag": start_ms,
            "id": id,
            "queueNum": 0,
            "state": 0,
        }));
        stf.push(json!({
            "avgDiff": 0.0,
            "beginTime": start_ms + lo * 1000,
            "endTime": start_ms + hi * 1000,
            "flag": start_ms,
            "id": id,
            "maxDiff": 0.0,
            "minDiff": 1000.0,
            "queueNum": 0,
            "state": 0,
            "stepsNum": b.value as i64,
        }));
    }
    (sp, stf)
}

/// 单圈对象（avgStride 单位厘米；avgPace 单位按平台：iOS=秒/公里，Android=分钟/公里）。
#[allow(clippy::too_many_arguments)]
fn lap_json(
    idx: i64,
    lap_d: f64,
    lap_t: i64,
    lap_steps: i64,
    end_alt: f64,
    alt0: f64,
    start_ms: i64,
    lap_start_t: i64,
    gain: f64,
    platform: &str,
) -> Value {
    // iOS 客户端按「秒/公里」解析 avgPace 并格式化为 分'秒''；
    // 若写成分钟/公里会被缩到 1/60（7'00" 显示成 0'07"）。安卓端为分钟/公里。
    let avg_pace = if platform == "ios" {
        round_to(lap_t as f64 / (lap_d / 1000.0), 2)
    } else {
        round_to((lap_t as f64 / 60.0) / (lap_d / 1000.0), 2)
    };
    json!({
        "avgCadence": round_to(lap_steps as f64 / (lap_t as f64 / 60.0), 2),
        "avgPace": avg_pace,
        "avgStride": round_to(lap_d / 1.max(lap_steps) as f64 * 100.0, 2),
        "cumulativeDuration": lap_start_t + lap_t,
        "distance": round_to(lap_d, 4),
        "duration": lap_t,
        "elevationGain": round_to(gain, 2),
        "endAltAbs": round_to(end_alt, 2),
        "endAltRel": round_to(end_alt - alt0, 2),
        "flag": start_ms,
        "id": idx,
        "isFullLap": lap_d >= 999.999,
        "lapIndex": idx,
        "step": lap_steps,
    })
}

/// 圈（每 1000m 一圈，末圈 isFullLap=false）。
///
/// 圈边界按相邻采样点线性插值，精确落在 1000m 整倍数上：
/// 旧写法取「首个累计距离 ≥1000m 的采样点」切圈，5s 采样网格 × 中速会让每圈
/// 拉长 1~3%（首圈尤其明显，约 1015~1020m），使首段 `duration`/`avgPace` 偏大。
/// 不足 1m 的尾部余量不构成一圈（0 距离圈会算出 16.7 的整数倍这种荒谬配速）。
fn build_laps(track: &Track, start_ms: i64, platform: &str) -> Vec<Value> {
    let locs = &track.locations;
    let mut laps = Vec::new();
    if locs.len() < 2 {
        return laps;
    }
    // 上报 bdA 含 GPS 高频抖动，直接累计会放大成假爬升；用 5 点滑动平均
    // 后的序列计算圈爬升与圈末海拔（显示仍用原始 bdA）。
    let alt = smooth_altitudes(&locs.iter().map(|p| p.bdA).collect::<Vec<_>>(), 5);
    let alt0 = alt[0];
    // 当前圈起点（累计距离/时间/步数）；圈边界为插值点，可能落在两个采样点之间
    let (mut cur_d, mut cur_t, mut cur_steps) = (0.0f64, 0i64, 0i64);
    let mut gain = 0.0f64;

    for (i, w) in locs.windows(2).enumerate() {
        let (p0, p1) = (&w[0], &w[1]);
        let (d0, d1) = (p0.totalDis, p1.totalDis);
        let span = d1 - d0;
        // 断崖可能让相邻点时间回退，插值只用非负间隔
        let dt = (p1.totalTime - p0.totalTime).max(0) as f64;
        let dalt = alt[i + 1] - alt[i];
        if dalt > 0.0 {
            gain += dalt;
        }
        while span > 0.0 && d1 - cur_d >= 1000.0 {
            let target = cur_d + 1000.0;
            let frac = ((target - d0) / span).clamp(0.0, 1.0);
            let end_t = ((p0.totalTime as f64 + frac * dt).round() as i64).max(cur_t + 1);
            let end_steps =
                (p0.steps as f64 + frac * (p1.steps - p0.steps) as f64).round() as i64;
            let end_alt = alt[i] + frac * dalt;
            let idx = laps.len() as i64 + 1;
            laps.push(lap_json(
                idx,
                1000.0,
                (end_t - cur_t).max(1),
                end_steps - cur_steps,
                end_alt,
                alt0,
                start_ms,
                cur_t,
                gain,
                platform,
            ));
            cur_d = target;
            cur_t = end_t;
            cur_steps = end_steps;
            gain = 0.0;
        }
    }

    let last = &locs[locs.len() - 1];
    let rest = last.totalDis - cur_d;
    if rest > 1.0 {
        let end_t = last.totalTime.max(cur_t + 1);
        let idx = laps.len() as i64 + 1;
        laps.push(lap_json(
            idx,
            rest,
            (end_t - cur_t).max(1),
            last.steps - cur_steps,
            alt[alt.len() - 1],
            alt0,
            start_ms,
            cur_t,
            gain,
            platform,
        ));
    }
    laps
}

/// 组装 11 键 OBS 对象（自由跑口径，无跑区）。
pub fn build_obs_object(
    track: &Track,
    rrid: i64,
    uuid: &str,
    uid: i64,
    live_points: &[Value],
    platform: &str,
) -> Value {
    build_obs_object_with_area(
        track,
        rrid,
        uuid,
        uid,
        live_points,
        platform,
        &RunAreaMeta::default(),
    )
}

/// 组装 11 键 OBS 对象（值均 gzip+base64；segment_json/runFaceCheck/extension_json 为空串 gzip）。
/// 键序对齐真机 `o0OO00O` 的 Map.put 顺序。
/// `platform` 决定 laps_json 的 avgPace 口径：iOS=秒/公里，Android=分钟/公里。
/// `area` 来自围栏接口：校园跑必须带真实 `runAreaId/geoFencesJson/freedomShowFence`。
pub fn build_obs_object_with_area(
    track: &Track,
    rrid: i64,
    uuid: &str,
    uid: i64,
    live_points: &[Value],
    platform: &str,
    area: &RunAreaMeta,
) -> Value {
    let start_ms = track.startTime;
    let pts: Vec<Value> = track.locations.iter().map(|p| conv_point(p, start_ms)).collect();
    let run_wrap = json!({ "allLocJson": Value::Array(pts).to_string(), "useZip": false });
    let (sp, stf) = build_windows(track, rrid);
    let laps = build_laps(track, start_ms, platform);
    let five = five_point_payload(live_points, start_ms);
    let fx = json!({
        "fivePointJson": Value::Array(five).to_string(),
        "freedomShowFence": area.freedom_show_fence,
        "geoFencesJson": Value::Array(area.geo_fences.clone()).to_string(),
        "runAreaId": area.run_area_id,
        "useZip": false,
    });
    json!({
        "rrid": gz_str(&rrid.to_string()),
        "uid": gz_str(&uid.to_string()),
        "uuid": gz_str(uuid),
        "run_data": gz_json(&run_wrap),
        "step_freq_json": gz_json(&Value::Array(stf)),
        "speed_json": gz_json(&Value::Array(sp)),
        "segment_json": gz_str(""),
        "fixed_point_json": gz_json(&fx),
        "runFaceCheck": gz_str(""),
        "extension_json": gz_str(""),
        "laps_json": gz_json(&Value::Array(laps)),
    })
}

/// 官方 objectKey 只有一个：`<type>/<北京时间 YYYYMMDDHH>/<uuid>.json`。
/// 真机只 PUT 这个 key（不存在 rrid 兜底 key）。
pub fn obs_keys(track: &Track, _rrid: i64, uuid: &str) -> Vec<String> {
    let t0 = super::geom::fmt_cst(track.startTime, "%Y%m%d%H");
    vec![format!("run_data/{t0}/{uuid}.json")]
}
