//! 跑步提交：POST /api/v70260/runnings/save/record。
//! Android 身份 + runes/runef 头；body 31+ 字段 + signature/originalSign。

use super::client::{check_business, ureq_err, ApiClient};
use super::model::HOST;
use crate::crypto::decrypt::{decrypt_response, derive_paes_key};
use crate::crypto::envelope::{
    build_envelope, build_envelope_ts, rsa_public_key, OuterOrder,
};
use crate::crypto::header::{build_android_header, HeaderIdentity, ANDROID_APP_VERSION, UA_ANDROID};
use crate::crypto::sign::{original_sign, signature};
use crate::track::calorie::{avg_power, official_kcal};
use crate::track::geom::round_to;
use crate::track::model::{GenPoint, Track};
use crate::track::wire::validate_five_point_wrapper;
use serde_json::{Map, Value};

pub const RECORD_PATH: &str = "/api/v70260/runnings/save/record";

/// 官方 `UploadFormatEntity` 口径（7.3.93 抓包 + 反编译交叉确认）：
/// - latitude/longitude：户外跑路径从不 set，保持 Java double 默认 0.0；
/// - more：字段声明就是 `private boolean more = true`；
/// - goalId：`SportRecordManager.getGoalId()` 把 null 归一化为 0（0 = 无目标）。
const BODY_LAT: f64 = 0.0;
const BODY_LON: f64 = 0.0;
const BODY_MORE: bool = true;
const BODY_GOAL_ID: i64 = 0;

/// 官方样本 stopTime − startTime 比 totalTime 多 1~5 s（运动中暂停）。
/// 用 uuid 派生稳定抖动，避免出现"零暂停"的整秒值。
fn pause_extra_ms(uuid: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in uuid.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let count = 1 + (h % 3) as i64; // 1~3 次暂停
    let mut extra = 0i64;
    for i in 0..count {
        extra += 1 + ((h >> (8 * i)) % 5) as i64; // 每次 1~5 秒
    }
    extra * 1000
}

/// 真机 `UploadFormatEntity` 字段声明为字母序，Gson 按声明序输出；
/// 在序列化层按 key 排序，保持提交体 45 键顺序与真机一致。
fn sorted_body_json(body: &Map<String, Value>) -> String {
    let mut items: Vec<(&String, &Value)> = body.iter().collect();
    items.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = String::with_capacity(1024);
    out.push('{');
    for (i, (k, v)) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(k).unwrap_or_default());
        out.push(':');
        out.push_str(&serde_json::to_string(v).unwrap_or_default());
    }
    out.push('}');
    out
}

/// bdA 正差分累计。
///
/// 上报 bdA 含 GPS 高频抖动，直接对原始值累计会把噪声放大成假爬升；
/// 先用 5 点滑动平均过滤高频分量，再对平滑后的序列取正差分。
pub fn total_ascent(locs: &[GenPoint]) -> f64 {
    let alts: Vec<f64> = locs.iter().map(|p| p.bdA).collect();
    let smoothed = crate::track::altitude::smooth_altitudes(&alts, 5);
    let mut ascent = 0.0;
    for i in 1..smoothed.len() {
        let d = smoothed[i] - smoothed[i - 1];
        if d > 0.0 {
            ascent += d;
        }
    }
    ascent
}

pub struct SubmitParams {
    pub track: Track,
    pub uid: i64,
    pub selected_unid: i64,
    pub policy: i64,
    pub policy_ts: i64,
    pub min_distance: i64,
    pub weight: f64,
    pub face_check: i64,
    pub five_point_json: String,
    pub address: String,
    /// 本次提交的记录 uuid（由调用方生成，便于提前生成 geeToken）。
    pub uuid: String,
    /// 极验 geeToken；空串表示不携带。
    pub gee_token: String,
    /// 与 geeToken 配套的 errorCode（-999/ -300 / 200）。
    pub error_code: i64,
}

#[allow(dead_code)]
pub struct SubmitResult {
    pub rrid: i64,
    pub uuid: String,
    pub start_ms: i64,
    pub complete: Option<bool>,
    pub total_dis: f64,
    pub total_time: i64,
    pub total_steps: i64,
    pub avg_step_freq: i64,
    pub calorie: i64,
    pub avg_power: i64,
    pub sel_distance: i64,
}

/// 提交跑步记录（sportType=1 自由跑 + 实时五点）。
pub fn submit_record(client: &mut ApiClient, p: &SubmitParams, log: &mut dyn FnMut(&str)) -> Result<SubmitResult, String> {
    let track = &p.track;
    track.validate_consistency()?;
    if p.five_point_json.is_empty() { return Err("五点轨迹不能为空".into()); }
    validate_five_point_wrapper(&p.five_point_json)?;
    let total_time = track.totalTime;
    let total_dis = track.totalDistance;
    let total_steps = track.totalSteps;
    let start_ms = track.startTime;
    let ascent = total_ascent(&track.locations);
    let power = avg_power(p.weight, total_dis, total_time);
    let kcal = official_kcal(p.weight, total_time, total_dis);

    let run_uuid = if p.uuid.is_empty() {
        uuid::Uuid::new_v4().to_string().to_uppercase()
    } else {
        p.uuid.clone()
    };
    let unid = p.selected_unid;
    // 官方样本 stopTime − startTime 比 totalTime 多 1~5 s（运动中暂停）。
    let stop_ms = start_ms + total_time * 1000 + pause_extra_ms(&run_uuid);

    let dis_ceil = (total_dis * 100.0).ceil() / 100.0;
    // 官方口径：speed = 配速(分/公里) × 1000（千分位分钟/公里），Android/iOS 一致。
    let pace_min = total_time as f64 / 60.0 / (dis_ceil / 1000.0).max(0.001);
    let speed = round_to(pace_min * 1000.0, 0) as i64;
    let avg_step_freq = 1i64.max(round_to(total_steps as f64 / total_time as f64 * 60.0, 0) as i64);

    let mut body = Map::new();
    body.insert("allLocJson".into(), Value::String(String::new()));
    // 官方 45 键提交体必带：自由跑时为空串，真实分段在 OBS 对象里。
    body.insert("segmentJson".into(), Value::String(String::new()));
    body.insert("sportType".into(), Value::from(1));
    // 官方 45 键提交体必带：非房间跑固定 0。
    // 注意：签名只在 sportType=3 时把 roomId 纳入 31 字段，这里保持 has_room_id=false。
    body.insert("roomId".into(), Value::from(0));
    body.insert("policy".into(), Value::from(p.policy));
    body.insert("totalTime".into(), Value::from(total_time));
    body.insert("startTime".into(), Value::from(start_ms));
    body.insert("stopTime".into(), Value::from(stop_ms));
    body.insert("getPrize".into(), Value::Bool(false));
    body.insert("status".into(), Value::from(0));
    body.insert("uuid".into(), Value::String(run_uuid.clone()));
    body.insert("uid".into(), Value::from(p.uid));
    body.insert("selectedUnid".into(), Value::from(unid));
    body.insert("selRunTime".into(), Value::from(total_time));
    body.insert("selDistance".into(), Value::from(p.min_distance));
    // 官方样本 93.90 → 93：提交层按截断，不四舍五入（内部计算仍保留精度）。
    body.insert("totalDis".into(), Value::from(total_dis as i64));
    body.insert("speed".into(), Value::from(speed));
    body.insert("validDis".into(), Value::from(total_dis as i64));
    body.insert("validTime".into(), Value::from(total_time));
    body.insert("complete".into(), Value::Bool(true));
    body.insert("unCompleteReason".into(), Value::from(0));
    body.insert("calorie".into(), Value::from(kcal));
    body.insert("totalSteps".into(), Value::from(total_steps));
    body.insert("avgStepFreq".into(), Value::from(avg_step_freq));
    body.insert("useMobilityTools".into(), Value::from(0));
    // 官方语义：faceCheck 与 OBS runFaceCheck 必须同源一致。
    // 本工具不产抽检记录（runFaceCheck 恒空串），因此强制 0；
    // 若显式传 --face 1，只提示不写入，避免制造官方不存在的矛盾组合。
    if p.face_check > 0 {
        log("⚠ [submit] 未实现抽检记录，faceCheck 强制为 0（避免与 runFaceCheck 空串矛盾）");
    }
    body.insert("faceCheck".into(), Value::from(0));
    body.insert("totalAscent".into(), Value::from(round_to(ascent, 0) as i64));
    body.insert("avgPower".into(), Value::from(power));
    // 官方口径：提交体里这两个数组为空，真实数据在 OBS 对象里。
    body.insert("speedPerTenSec".into(), Value::Array(Vec::new()));
    body.insert("stepsPerTenSec".into(), Value::Array(Vec::new()));
    body.insert("isUpload".into(), Value::Bool(false));
    body.insert("more".into(), Value::Bool(BODY_MORE));
    // 官方口径：提交体里的 latitude/longitude 固定 0.0，不是起点坐标。
    body.insert("latitude".into(), Value::from(BODY_LAT));
    body.insert("longitude".into(), Value::from(BODY_LON));
    body.insert("maxRunTime".into(), Value::from(0));
    body.insert("minSteps".into(), Value::from(0));
    if !p.five_point_json.is_empty() {
        body.insert("fivePointJson".into(), Value::String(p.five_point_json.clone()));
    }
    // Android 必现扩展字段
    body.insert("errorCode".into(), Value::from(p.error_code));
    body.insert("geeToken".into(), Value::String(p.gee_token.clone()));
    body.insert("unauthorized".into(), Value::from(0));
    body.insert("themeId".into(), Value::from(-1));
    body.insert("goalId".into(), Value::from(BODY_GOAL_ID));
    body.insert("address".into(), Value::String(p.address.trim().to_string()));

    let body_val = Value::Object(body.clone());
    let sig = signature(&body_val, false);
    let orig = original_sign(&body_val, false);
    body.insert("signature".into(), Value::String(sig));
    body.insert("originalSign".into(), Value::String(orig));
    let body_plain = sorted_body_json(&body);

    let device_id = if client.identity.device_id.is_empty() {
        uuid::Uuid::new_v4().to_string().to_uppercase()
    } else {
        client.identity.device_id.clone()
    };
    let android_identity = HeaderIdentity {
        platform: "android".into(),
        device_id: device_id.clone(),
        os_version: "14".into(),
        device_name: "22081212C".into(),
        ..client.identity.clone()
    };
    let (hp, hp_extra) = build_android_header(&android_identity, p.uid, &client_token(client), None);
    let header_env = build_envelope(&mut client.session, &hp, OuterOrder::Observed);
    let now = crate::crypto::envelope::now_ms();
    let body_env = build_envelope_ts(&mut client.session, &body_plain, OuterOrder::Insert, now + 1);

    let runes = format!("{}{}", p.policy_ts, p.uid);
    let runef = format!("{}{}", run_uuid, start_ms);
    let mut req = client.agent.post(&format!("{HOST}{RECORD_PATH}"));
    req = req
        .set("Content-Type", "application/json; charset=utf-8")
        .set("User-Agent", UA_ANDROID)
        .set("appVersion", ANDROID_APP_VERSION)
        .set("headerSign", &header_env.json)
        .set("runes", &runes)
        .set("runef", &runef);
    for (k, v) in &hp_extra {
        req = req.set(k, v);
    }
    let resp = req.send_string(&body_env.json).map_err(ureq_err)?;
    let status = resp.status();
    let raw = resp.into_string().unwrap_or_default();
    log(&format!("[record] sportType=1 HTTP {status} len={}", raw.len()));

    let key = derive_paes_key(
        &body_env.key_data[0], &body_env.key_data[1], &body_env.key_data[2], &body_env.key_data[3],
    );
    let dec = decrypt_response(raw.as_bytes(), &key, &rsa_public_key())
        .map_err(|e| format!("提交响应解密失败: {e}"))?;
    let biz = check_business(&dec.business)?;
    let rrid = super::client::get_field(&biz, "rrid").and_then(|v| v.as_i64()).unwrap_or(0);
    if rrid <= 0 {
        return Err(format!("提交未返回 rrid: {}", truncate_json(&biz)));
    }
    log(&format!("√ 提交成功 rrid={rrid} uuid={run_uuid}"));
    Ok(SubmitResult {
        rrid,
        uuid: run_uuid,
        start_ms,
        complete: super::client::get_field(&biz, "complete").and_then(|v| v.as_bool()),
        total_dis,
        total_time,
        total_steps,
        avg_step_freq,
        calorie: kcal,
        avg_power: power,
        sel_distance: p.min_distance,
    })
}

fn client_token(client: &ApiClient) -> String {
    client.login.as_ref().map(|s| s.token.clone()).unwrap_or_default()
}

fn truncate_json(v: &Value) -> String {
    let s = v.to_string();
    s.chars().take(240).collect()
}
