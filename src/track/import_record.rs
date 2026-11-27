//! 导入已生成的跑步记录（目前支持 sw-control 的 `run_*.json`）。
//!
//! 用途
//! ----
//! 纯生成 + 纯提交是两条独立的链路。这里让「别处生成好的记录」直接进提交链：
//! 读文件 → 转成内部 [`Track`] → 跳过生成直接提交。
//!
//! 支持的格式（自动识别）
//! ----------------------
//! **sw-control 记录**（`data/generator/output/run_*.json`）：
//! ```json
//! {"metrics": {"distance_m":3658.74,"duration_s":1591.53,...},
//!  "points": [{"i":0,"ts":1795754397000,"lat":43.92,"lon":126.56,
//!              "ele":200.36,"speed":2.2311,"cadence":153.1,
//!              "stride_cm":87.4,"hr":116,"dist":0.0,"seg_m":0.0}, ...]}
//! ```
//!
//! 坐标约定
//! --------
//! sw-control 的 `lat/lon` 是 **WGS-84**，而内部 `GenPoint.gLat/gLng` 是 **BD-09**
//! （提交时 `wire::conv_point` 会做 BD-09 → GCJ-02）。所以导入时要做一次
//! WGS-84 → BD-09。
//!
//! 单位转换
//! --------
//! | 字段 | sw-control | 内部 |
//! |---|---|---|
//! | `speed` | m/s | km/h（×3.6） |
//! | `cadence` | 步/分 | `count` 也是步/分（原样） |
//! | `stride_cm` | 厘米 | `stepDistance` 米（÷100） |
//! | `dist` | 米 | `totalDis` 米（原样） |
//! | `ts` | 绝对毫秒 | `gainTimeMs`（原样） |

use serde_json::Value;

use super::geom::{fmt_cst, round_to, wgs84_to_bd09};
use super::model::{GenPoint, Track};

/// 导入结果：转好的轨迹 + 来源摘要。
pub struct ImportedRecord {
    pub track: Track,
    /// 人类可读的来源说明（文件名 / 点数 / 距离时长）
    pub summary: String,
}

/// 从文件导入一条记录。
pub fn load(path: impl AsRef<std::path::Path>) -> Result<ImportedRecord, String> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} 不是合法 JSON: {e}", path.display()))?;

    let track = if value.get("points").and_then(|p| p.as_array()).is_some() {
        from_swcontrol(&value)?
    } else {
        return Err(
            "无法识别的记录格式：需要含 `points` 数组（sw-control 的 run_*.json）".into(),
        );
    };

    let summary = format!(
        "{} · {} 点 · {:.2} km · {} · 起点 {:.6},{:.6}",
        path.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        track.locations.len(),
        track.totalDistance / 1000.0,
        fmt_duration(track.totalTime),
        track.startLatitude,
        track.startLongitude,
    );
    Ok(ImportedRecord { track, summary })
}

fn fmt_duration(sec: i64) -> String {
    if sec >= 3600 {
        format!("{}:{:02}:{:02}", sec / 3600, sec % 3600 / 60, sec % 60)
    } else {
        format!("{}:{:02}", sec / 60, sec % 60)
    }
}

/// sw-control 记录 → [`Track`]。
fn from_swcontrol(value: &Value) -> Result<Track, String> {
    let raw_points = value
        .get("points")
        .and_then(|p| p.as_array())
        .ok_or("缺少 points 数组")?;
    if raw_points.len() < 2 {
        return Err(format!("点数不足（{} 个），至少要 2 个", raw_points.len()));
    }

    let metrics = value.get("metrics");
    let metric_f = |key: &str| -> Option<f64> {
        metrics
            .and_then(|m| m.get(key))
            .and_then(|v| v.as_f64())
    };

    // 起点时间：第一个点的 ts
    let start_ms = raw_points[0]
        .get("ts")
        .and_then(|v| v.as_i64())
        .ok_or("第一个点缺 ts")?;

    let mut locs: Vec<GenPoint> = Vec::with_capacity(raw_points.len());
    let mut cum_steps: f64 = 0.0;
    let mut prev_ts = start_ms;

    for (idx, p) in raw_points.iter().enumerate() {
        let lat = p.get("lat").and_then(|v| v.as_f64()).ok_or("点缺 lat")?;
        let lon = p.get("lon").and_then(|v| v.as_f64()).ok_or("点缺 lon")?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err(format!("第 {idx} 点坐标越界: {lat},{lon}"));
        }
        let ts = p
            .get("ts")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| format!("第 {idx} 点缺 ts"))?;
        let dist = p.get("dist").and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0);
        let ele = p.get("ele").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let speed_ms = p.get("speed").and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0);
        let cadence = p.get("cadence").and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0);
        let stride_cm = p.get("stride_cm").and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0);
        let seg_m = p.get("seg_m").and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0);

        // WGS-84 → BD-09（内部 gLat/gLng 用 BD-09）
        let (bd_lat, bd_lng) = wgs84_to_bd09(lat, lon);

        // 步数累计：区间秒数 × 步频 / 60
        let dt_s = ((ts - prev_ts).max(0)) as f64 / 1000.0;
        if idx > 0 {
            cum_steps += cadence * dt_s / 60.0;
        }
        prev_ts = ts;
        let total_time_s = ((ts - start_ms).max(0)) / 1000;

        locs.push(GenPoint {
            id: idx as i64 + 1,
            flag: start_ms,
            lat: -1.0,
            lng: -1.0,
            gLat: round_to(bd_lat, 7),
            gLng: round_to(bd_lng, 7),
            // 内部 speed 是 km/h
            speed: round_to(speed_ms * 3.6, 4),
            avgSpeed: if total_time_s > 0 {
                round_to(dist / total_time_s as f64, 3)
            } else {
                0.0
            },
            // sw-control 不带精度半径，给真机常见范围
            radius: 4.0,
            accuracy: 6.0,
            ptype: 0,
            locType: 1,
            hasAltitude: true,
            totalTime: total_time_s,
            totalDis: round_to(dist, 4),
            validDis: round_to(dist, 4),
            validTime: total_time_s,
            steps: cum_steps.round() as i64,
            stepDistance: round_to(stride_cm / 100.0, 2),
            gainTime: fmt_cst(ts, "%Y-%m-%d %H:%M:%S"),
            gainTimeMs: ts,
            queueNum: 0,
            coorType: "gcj02".into(),
            bdA: round_to(ele, 2),
            // sw-control 不提供方位角 / 坡度
            bdD: 0.0,
            bdS: round_to(speed_ms, 3),
            bdG: 1,
            // count = 步频（步/分），与真机 OBS 点一致
            count: round_to(cadence, 0) as i64,
            dtr: 0.0,
            state: 1,
            locationId: String::new(),
        });
        // seg_m 只用于校验连续性，不落进 GenPoint
        let _ = seg_m;
    }

    let last = locs.last().ok_or("没有点")?;
    let total_time = last.totalTime;
    let total_dis = metric_f("distance_m").unwrap_or(last.totalDis);
    let total_steps = metric_f("total_steps")
        .map(|v| v.round() as i64)
        .unwrap_or_else(|| locs.last().map(|p| p.steps).unwrap_or(0));

    if total_time <= 0 {
        return Err("总时长为 0：检查点的 ts 是否递增".into());
    }

    let first = &locs[0];
    Ok(Track {
        totalTime: total_time,
        totalDistance: total_dis,
        validDistance: total_dis,
        validTime: total_time,
        startTime: start_ms,
        startLatitude: first.gLat,
        startLongitude: first.gLng,
        totalSteps: total_steps,
        locations: locs,
        // 下面三项由提交层按需重算，这里留空
        speedPerTenSec: Vec::new(),
        stepsPerTenSec: Vec::new(),
        segments: Vec::new(),
    })
}
