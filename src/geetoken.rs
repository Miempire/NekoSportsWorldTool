//! 极验 geeToken 本地生成（纯 Rust，按真机模板拼装 + AES-256-CBC 加密）。
//!
//! 结构来源：`neko-re-sources-v0.6.32/native/reimpl-rust/geetoken.rs`，
//! 明文模板见 [`geetoken_template.json`](geetoken_template.json)。
//!
//! 说明：
//! - 本地 token 形如 `GEE\0…`（base64 后约 3384 字符），`errorCode` 对应 `-300`；
//! - 服务端 token 需把本地 token POST 到 `riskct.geetest.com/g2/api/v1/client_report`
//!   换回 `G02\0…`（348~500 字符），对应 `errorCode=200`；本模块不自动联网。
//! - 设备档案按机型生成并缓存在 identity.json 的 `gt` 字段；设备串 `d` 首次生成后
//!   必须复用，否则 `fp` 变化会被当成"换了台设备"。

use rand::Rng;
use serde::{Deserialize, Serialize};

/// 本地 token 前缀（`GEE\0` 的 base64 开头）。
pub const TOKEN_PREFIX_LOCAL: &str = "R0VF";
/// 服务端 token 前缀（`G02\0` 的 base64 开头）。
pub const TOKEN_PREFIX_SERVER: &str = "RzAy";
/// 极验回执接口。
pub const CLIENT_REPORT_URL: &str = "https://riskct.geetest.com/g2/api/v1/client_report";
/// client_report 的 AppID 头（取自 `geet_geeguard_id`）。
pub const GT_APP_ID: &str = "mhs0n2abroy5ayzs5adtslzzsk1cya6h";
/// 本地 token 加密密钥（真机 .rodata，逐字节 XOR 0x6e）。
pub const GT_KEY: &[u8; 32] = b"WS1nbWQfMJ1merhqY5RsFZIxrJ/54JE=";

const TEMPLATE: &str = include_str!("geetoken_template.json");

/// 极验设备档案（`data.dev` 的 40 个字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GtDevProfile {
    pub app: String,
    pub ven: String,
    pub ps: String,
    pub ps2: String,
    pub prp: String,
    pub pla: String,
    #[serde(rename = "mod")]
    pub model: String,
    pub osv: String,
    pub rel: String,
    pub bui: String,
    pub bs: i64,
    pub br: i64,
    pub plu: i64,
    pub hea: i64,
    pub btv: i64,
    pub cou: String,
    pub lang: String,
    pub w: i64,
    pub h: i64,
    pub bt: i64,
    pub bri: i64,
    pub s: f64,
    pub op: String,
    pub car: String,
    pub btc: i64,
    pub rad: String,
    pub mem: String,
    pub tsz: i64,
    pub asz: i64,
    pub os: String,
    pub loc: serde_json::Value,
    pub ism: i64,
    pub ip: serde_json::Value,
    pub sim: i64,
    pub ssid: String,
    pub bssid: String,
    pub rssi: i64,
    pub gpa: i64,
    pub hdt: String,
    pub aum: i64,
}

impl Default for GtDevProfile {
    fn default() -> Self {
        Self {
            app: "%E9%98%B3%E5%85%89%E4%BD%93%E8%82%B2".into(),
            ven: "com.zjwh.android_wh_physicalfitness".into(),
            ps: "C62C38BE109D2A72528C307BDFC21E34".into(),
            ps2: "767dd585192630228dc58c782a3832e99ca0ec5b".into(),
            prp: "/data/app/com.zjwh.android_wh_physicalfitness/base.apk".into(),
            pla: "google".into(),
            model: "Nexus 5X".into(),
            osv: "6.0".into(),
            rel: crate::crypto::header::ANDROID_APP_VERSION.into(),
            bui: "408".into(),
            bs: -1,
            br: -1,
            plu: -1,
            hea: -1,
            btv: -1,
            cou: "CN".into(),
            lang: "zh".into(),
            w: 1080,
            h: 2400,
            bt: 1990654450,
            bri: 0,
            s: 2.75,
            op: "46000".into(),
            car: "China Mobile".into(),
            btc: 4500,
            rad: "WIFI".into(),
            mem: String::new(),
            tsz: 0,
            asz: 0,
            os: "android".into(),
            loc: serde_json::json!({"st": 0, "se": "1"}),
            ism: 0,
            ip: serde_json::json!({}),
            sim: 1,
            ssid: String::new(),
            bssid: String::new(),
            rssi: -1,
            gpa: 1,
            hdt: String::new(),
            aum: 0,
        }
    }
}

impl GtDevProfile {
    /// 模板占位符替换表。
    pub fn placeholders(&self) -> Vec<(String, String)> {
        vec![
            ("{{DEV_app}}".into(), self.app.clone()),
            ("{{DEV_ven}}".into(), self.ven.clone()),
            ("{{DEV_ps}}".into(), self.ps.clone()),
            ("{{DEV_ps2}}".into(), self.ps2.clone()),
            ("{{DEV_prp}}".into(), self.prp.clone()),
            ("{{DEV_pla}}".into(), self.pla.clone()),
            ("{{DEV_mod}}".into(), self.model.clone()),
            ("{{DEV_osv}}".into(), self.osv.clone()),
            ("{{DEV_rel}}".into(), self.rel.clone()),
            ("{{DEV_bui}}".into(), self.bui.clone()),
            ("{{DEV_bs}}".into(), self.bs.to_string()),
            ("{{DEV_br}}".into(), self.br.to_string()),
            ("{{DEV_plu}}".into(), self.plu.to_string()),
            ("{{DEV_hea}}".into(), self.hea.to_string()),
            ("{{DEV_btv}}".into(), self.btv.to_string()),
            ("{{DEV_cou}}".into(), self.cou.clone()),
            ("{{DEV_lang}}".into(), self.lang.clone()),
            ("{{DEV_w}}".into(), self.w.to_string()),
            ("{{DEV_h}}".into(), self.h.to_string()),
            ("{{DEV_bt}}".into(), self.bt.to_string()),
            ("{{DEV_bri}}".into(), self.bri.to_string()),
            ("{{DEV_s}}".into(), format!("{}", self.s)),
            ("{{DEV_op}}".into(), self.op.clone()),
            ("{{DEV_car}}".into(), self.car.clone()),
            ("{{DEV_btc}}".into(), self.btc.to_string()),
            ("{{DEV_rad}}".into(), self.rad.clone()),
            ("{{DEV_mem}}".into(), self.mem.clone()),
            ("{{DEV_tsz}}".into(), self.tsz.to_string()),
            ("{{DEV_asz}}".into(), self.asz.to_string()),
            ("{{DEV_os}}".into(), self.os.clone()),
            (
                "{{DEV_loc}}".into(),
                serde_json::to_string(&self.loc).unwrap_or_else(|_| "null".into()),
            ),
            ("{{DEV_ism}}".into(), self.ism.to_string()),
            (
                "{{DEV_ip}}".into(),
                serde_json::to_string(&self.ip).unwrap_or_else(|_| "{}".into()),
            ),
            ("{{DEV_sim}}".into(), self.sim.to_string()),
            ("{{DEV_ssid}}".into(), self.ssid.clone()),
            ("{{DEV_bssid}}".into(), self.bssid.clone()),
            ("{{DEV_rssi}}".into(), self.rssi.to_string()),
            ("{{DEV_gpa}}".into(), self.gpa.to_string()),
            ("{{DEV_hdt}}".into(), self.hdt.clone()),
            ("{{DEV_aum}}".into(), self.aum.to_string()),
        ]
    }
}

/// 持久化在 identity.json 里的极验状态：设备档案 + 设备串 `d`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GtState {
    pub dev: GtDevProfile,
    /// 设备串：20hex + 12hex + 13 位毫秒；生成后必须复用。
    pub d: String,
}

/// 生成/补齐身份里的极验状态。
pub fn ensure_state(identity: &crate::crypto::header::HeaderIdentity) -> GtState {
    if let Some(state) = &identity.gt {
        return state.clone();
    }
    let mut rng = seeded_rng(identity);
    let dev = generate_dev(identity, &mut rng);
    let d = fresh_device_string(&identity.device_id, identity.app_install_time.max(0) as i64);
    GtState { dev, d }
}

fn seeded_rng(identity: &crate::crypto::header::HeaderIdentity) -> rand::rngs::StdRng {
    use rand::SeedableRng;
    let digest = crate::crypto::envelope::md5_hex(
        format!("{}|{}|gt-v1", identity.device_id, identity.device_name).as_bytes(),
    );
    let seed = u64::from_str_radix(&digest[..16], 16).unwrap_or(0);
    rand::rngs::StdRng::seed_from_u64(seed)
}

/// 屏幕参数（宽、高、scaledDensity）。
fn screen_for(model: &str, platform: &str) -> (i64, i64, f64) {
    let m = model.to_lowercase();
    let table: &[(&str, i64, i64, f64)] = &[
        ("iphone 15 pro max", 1290, 2796, 3.0),
        ("iphone 15", 1179, 2556, 3.0),
        ("iphone 14", 1170, 2532, 3.0),
        ("iphone 13", 1170, 2532, 3.0),
        ("iphone se", 750, 1334, 2.0),
        ("22081212c", 1220, 2712, 2.75),
        ("k50", 1220, 2712, 2.75),
        ("k60", 1440, 3200, 3.0),
        ("x100", 1260, 2800, 3.0),
        ("pixel 7", 1080, 2400, 2.625),
        ("pixel 8", 1080, 2400, 2.625),
        ("sm-", 1080, 2340, 2.625),
        ("v2366ga", 1080, 2400, 2.75),
    ];
    for (key, w, h, s) in table {
        if m.contains(key) {
            return (*w, *h, *s);
        }
    }
    if platform == "android" {
        (1080, 2400, 2.75)
    } else {
        (1170, 2532, 3.0)
    }
}

fn generate_dev(
    identity: &crate::crypto::header::HeaderIdentity,
    rng: &mut rand::rngs::StdRng,
) -> GtDevProfile {
    let (w, h, density) = screen_for(&identity.device_name, &identity.platform);
    let (op, car) = match rng.gen_range(0..3) {
        0 => ("46000", "China Mobile"),
        1 => ("46001", "China Unicom"),
        _ => ("46011", "China Telecom"),
    };
    let rad = match rng.gen_range(0..10) {
        0..=6 => "WIFI",
        7..=8 => "4G",
        _ => "5G",
    };
    let octet = rng.gen_range(2..=254);
    let ip = serde_json::json!({ "wlan0": format!("192.168.1.{octet}") });
    let suffix: String = (0..4)
        .map(|_| {
            let c = rng.gen_range(b'A'..=b'Z');
            c as char
        })
        .collect();
    let ssid = format!("ChinaNet-{suffix}");
    let bssid = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        rng.gen_range(0x00..=0xFF),
        0x08,
        rng.gen_range(0x00..=0xFF),
        rng.gen_range(0x00..=0xFF),
        rng.gen_range(0x00..=0xFF),
        rng.gen_range(0x00..=0xFF)
    );
    let rssi = rng.gen_range(-75i64..=-40);
    let uptime = rng.gen_range(3_600_000i64..=(30 * 86_400_000));

    let mut dev = GtDevProfile {
        pla: if identity.manufacturer.is_empty() {
            "google".into()
        } else {
            identity.manufacturer.clone()
        },
        model: identity.device_name.clone(),
        osv: identity.os_version.clone(),
        w,
        h,
        s: density,
        op: op.into(),
        car: car.into(),
        rad: rad.into(),
        ip,
        ssid,
        bssid,
        rssi,
        bt: uptime,
        ..GtDevProfile::default()
    };
    if let Some(battery) = &identity.battery {
        dev.bs = battery.status as i64;
        dev.br = battery.level as i64;
        dev.plu = battery.plugged as i64;
        dev.hea = battery.health as i64;
        dev.btv = battery.voltage_mv as i64;
        dev.btc = battery.capacity_mah as i64;
    }
    if let Some(sensors) = &identity.sensors {
        dev.gpa = if sensors
            .sensors
            .iter()
            .any(|s| s.type_name.contains("gyroscope") || s.type_name == "Gyroscope")
        {
            1
        } else {
            0
        };
    }
    dev
}

/// 首次生成设备串 `d`（之后必须复用）。
pub fn fresh_device_string(seed: &str, now_ms: i64) -> String {
    let h = crate::crypto::envelope::md5_hex(seed.as_bytes());
    format!("{}{}{}", &h[..20], &h[20..32], now_ms)
}

/// 用当前身份生成设备档案（不写盘，供身份更新时重算）。
pub fn generate_dev_for(identity: &crate::crypto::header::HeaderIdentity) -> GtDevProfile {
    let mut rng = seeded_rng(identity);
    generate_dev(identity, &mut rng)
}

/// 生成 token 所需的业务参数。
#[derive(Debug, Clone)]
pub struct GtRun {
    pub uid: i64,
    pub uuid: String,
    pub scene: i64,
    pub ts_ms: i64,
    pub et: i64,
}

impl GtRun {
    pub fn new(uid: i64, uuid: &str, scene: i64) -> Self {
        let ts_ms = crate::crypto::envelope::now_ms();
        Self {
            uid,
            uuid: uuid.to_string(),
            scene,
            ts_ms,
            et: 2900,
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_to_bytes(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect()
}

fn base64_std(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn scene_payload(uid: i64, uuid: &str, scene: i64) -> String {
    if uuid.is_empty() {
        format!("{{\"attr\":\"{{\\\"uid\\\":{}}}\",\"scene\":{}}}", uid, scene)
    } else {
        format!(
            "{{\"attr\":\"{{\\\"uid\\\":{},\\\"uuid\\\":\\\"{}\\\"}}\",\"scene\":{}}}",
            uid, uuid, scene
        )
    }
}

/// 组装明文 JSON（模板 + 占位符，与真机同构）。
pub fn build_plain(run: &GtRun, state: &GtState) -> String {
    let dev = &state.dev;
    let d = &state.d;
    let ts_sec = run.ts_ms / 1000;
    let gts = (ts_sec - 2).to_string();
    let fp = sha256_hex(d.as_bytes());
    let payload = scene_payload(run.uid, &run.uuid, run.scene);
    let sign = sha256_hex(payload.as_bytes());
    let gid_digest = sha256_hex(format!("G02{fp}{gts}").as_bytes());
    let gid_sign = gid_digest[gid_digest.len() - 16..].to_string();
    let g30 = format!(
        "G02-{gts}-{fp}-{}",
        base64_std(&hex_to_bytes(&gid_digest)[26..32])
    );
    let mut plain = TEMPLATE
        .replace("{{SIGN}}", &sign)
        .replace("{{G30}}", &g30)
        .replace("{{FP}}", &fp)
        .replace("{{D}}", d)
        .replace("{{GSIGN}}", &gid_sign)
        .replace("{{GTS}}", &gts)
        .replace("{{TS}}", &run.ts_ms.to_string())
        .replace("{{ET}}", &run.et.to_string());
    for (key, value) in dev.placeholders() {
        plain = plain.replace(&key, &value);
    }
    plain
}

/// 用已还原的算法加密：`plain` 是紧凑 JSON，`random4` 是 4 字节随机数。
pub fn encrypt_token(plain: &[u8], random4: [u8; 4]) -> String {
    use cbc::cipher::{block_padding::Pkcs7, BlockModeEncrypt, KeyIvInit};
    type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

    let digest = {
        use sha2::Digest;
        sha2::Sha256::digest(plain)
    };
    let mut nonce = [0u8; 8];
    nonce[..4].copy_from_slice(&random4);
    nonce[4..].copy_from_slice(&digest[..4]);
    let nonce_hex = hex_lower(&nonce).into_bytes();
    let iv: [u8; 16] = nonce_hex[..16].try_into().expect("nonce 固定 16 字节");
    let key: [u8; 32] = *GT_KEY;
    let body = Aes256CbcEnc::new(&key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(plain);

    let mut out = Vec::with_capacity(25 + body.len());
    out.extend_from_slice(b"GEE\0");
    out.push(0x05);
    out.extend_from_slice(&iv);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    base64_std(&out)
}

/// 由 token 形态推导提交体里的 `errorCode`（回执与提交必须一致）。
pub fn code_for_token(token: &str) -> i64 {
    let t = token.trim();
    if t.is_empty() {
        return -999;
    }
    if t.starts_with(TOKEN_PREFIX_SERVER) {
        200
    } else if t.starts_with(TOKEN_PREFIX_LOCAL) {
        -300
    } else {
        -999
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 一条本地 token 及其明文派生信息。
#[derive(Debug, Clone)]
pub struct LocalToken {
    pub token: String,
    /// 明文 `data.gid["3.0"]`，client_report 的 `GeeID` 头必须是它。
    pub gee_id: String,
    pub plain: serde_json::Value,
}

/// 生成一条本地 token（GEE…，-300）。
pub fn make_token(identity: &crate::crypto::header::HeaderIdentity, uid: i64, uuid: &str, scene: i64) -> (String, i64) {
    let local = generate_local(identity, uid, uuid, scene);
    (local.token, -300)
}

/// 生成本地 token 并返回 geeID / 明文，供 client_report 换服务端 token。
pub fn generate_local(
    identity: &crate::crypto::header::HeaderIdentity,
    uid: i64,
    uuid: &str,
    scene: i64,
) -> LocalToken {
    let state = ensure_state(identity);
    let run = GtRun::new(uid, uuid, scene);
    let plain_text = build_plain(&run, &state);
    let plain: serde_json::Value = serde_json::from_str(&plain_text).unwrap_or_default();
    let gee_id = plain
        .get("data")
        .and_then(|data| data.get("gid"))
        .and_then(|gid| gid.get("3.0"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let mut random4 = [0u8; 4];
    rand::thread_rng().fill(&mut random4);
    let token = encrypt_token(plain_text.as_bytes(), random4);
    LocalToken {
        token,
        gee_id,
        plain,
    }
}

/// 用本地 token 换服务端 token（`G02…`，errorCode=200）。
///
/// 请求：`POST https://riskct.geetest.com/g2/api/v1/client_report`
/// 头：`API-Version: 1`、`AppID`、`GeeID`、`Client-Type: 1`，body 为本地 token 原文。
pub fn exchange_online(local: &LocalToken, log: &mut dyn FnMut(&str)) -> Result<String, String> {
    if local.gee_id.is_empty() {
        return Err("明文里没有 data.gid[\"3.0\"]，无法走 client_report".into());
    }
    log("[gee] 用本地 token 换取服务端 token…");
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(20))
        .build();
    let resp = agent
        .post(CLIENT_REPORT_URL)
        .set("API-Version", "1")
        .set("AppID", GT_APP_ID)
        .set("GeeID", &local.gee_id)
        .set("Client-Type", "1")
        .set("Content-Type", "text/plain; charset=utf-8")
        .send_bytes(local.token.as_bytes())
        .map_err(crate::api::client::ureq_err)?;
    let text = resp.into_string().unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("client_report 响应不是 JSON: {e} | {}", tail(&text)))?;
    let code = value.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(format!("client_report 业务失败 code={code} | {}", tail(&text)));
    }
    let token = value
        .get("data")
        .and_then(|data| data.get("gee_token"))
        .and_then(|token| token.as_str())
        .unwrap_or("");
    if token.is_empty() {
        return Err(format!("client_report 未返回 gee_token | {}", tail(&text)));
    }
    log(&format!("[gee] 服务端 token 已换取，{} 字符", token.len()));
    Ok(token.to_string())
}

fn tail(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 120 {
        trimmed.to_string()
    } else {
        trimmed.chars().skip(trimmed.chars().count() - 120).collect()
    }
}

/// 生成 token：默认本地；`online=true` 时再换服务端 token（失败则退回本地）。
pub fn make_token_with_mode(
    identity: &crate::crypto::header::HeaderIdentity,
    uid: i64,
    uuid: &str,
    scene: i64,
    online: bool,
    log: &mut dyn FnMut(&str),
) -> (String, i64) {
    let local = generate_local(identity, uid, uuid, scene);
    if !online {
        return (local.token, -300);
    }
    match exchange_online(&local, log) {
        Ok(token) => (token, 200),
        Err(error) => {
            log(&format!("⚠ [gee] 换取服务端 token 失败，退回本地 token: {error}"));
            (local.token, -300)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> crate::crypto::header::HeaderIdentity {
        let mut id = crate::crypto::header::HeaderIdentity::default();
        id.platform = "android".into();
        id.device_id = "DEV-GT-1".into();
        id.device_name = "22081212C".into();
        id.manufacturer = "Xiaomi".into();
        id.os_version = "14".into();
        id.battery = Some(crate::battery::generate_for(&id));
        id.sensors = Some(crate::sensors::generate_for(&id));
        let state = ensure_state(&id);
        id.gt = Some(state);
        id
    }

    #[test]
    fn token_roundtrip_shape() {
        let id = identity();
        let (token, code) = make_token(&id, 10001, "UUID-1", 7);
        assert!(token.starts_with(TOKEN_PREFIX_LOCAL), "应为本地 token");
        assert_eq!(code, -300);
        assert!(token.len() > 1000, "本地 token 长度异常: {}", token.len());
    }

    #[test]
    fn device_string_is_stable() {
        let id = identity();
        let a = ensure_state(&id);
        let b = ensure_state(&id);
        assert_eq!(a.d, b.d);
        assert_eq!(a.dev, b.dev);
    }

    #[test]
    fn plain_contains_scene_payload() {
        let id = identity();
        let state = ensure_state(&id);
        let run = GtRun::new(42, "U-42", 7);
        let plain = build_plain(&run, &state);
        assert!(plain.contains("\\\"uid\\\":42"));
        assert!(plain.contains("\\\"uuid\\\":\\\"U-42\\\""));
        assert!(plain.contains(&state.d));
    }

    /// 与参考实现一致的黄金向量：plain=`{"a":1}`、random4=11 22 33 44。
    #[test]
    fn encrypt_matches_golden_vector() {
        let token = encrypt_token(br#"{"a":1}"#, [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(
            token,
            "R0VFAAUxMTIyMzM0NDAxNWFiZDdmAAAAECc8nwc1mlvLNbq5fbAGXQs=",
            "AES-256-CBC 本地 token 与参考实现不一致"
        );
    }
}
