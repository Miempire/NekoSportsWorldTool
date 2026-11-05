//! 账号安全接口（新版本客户端，非跑步提交链）。
//!
//! - 发送验证码：
//!   GT4(REGISTER captchaId) → `POST /api/v70270/security/geevalidate`
//!   （businessType=1，osType=0）→ `POST /api/v75/users/check/code`
//!   body 为 `{phone, type, uuid, sign}`，`type=5` 表示注销验证码。
//! - 注销账号：
//!   `POST /api/v61300/user/userLogout`，body `{code}`。
//!   反编译源码里 `CommonServer.getDeleteUserInfo` 指向的正是这个路径。
//!
//! `sign` 算法（`SignCode.getSign`）：按 key 忽略大小写排序，拼成
//! `k=v&k=v...` 后接盐，取 MD5 小写十六进制。

use super::client::{check_business, ApiClient};
use super::gt4::solve_gt4;
use super::model::HOST;
use crate::crypto::envelope::md5_hex;
use crate::crypto::header::UA_IOS;
use serde_json::{json, Value};

pub const CHECK_CODE_PATH: &str = "/api/v75/users/check/code";
pub const DELETE_USER_PATH: &str = "/api/v61300/user/userLogout";
pub const GEEVALIDATE_PATH: &str = "/api/v70270/security/geevalidate";

/// 登录场景 captchaId（资源 gt4_captcha_login）。
///
/// ⚠ 实测：用它对 geevalidate(businessType=0) 会稳定 10003；登录链必须用
/// `login::CAPTCHA_ID_BIZ0`（`8c065103…`），此常量仅作记录，勿用于登录。
pub const LOGIN_CAPTCHA_ID: &str = "3b02ad39bd099fd3a8336d9347a189ab";
/// 发短信场景 captchaId（资源 gt4_captcha_register）。
pub const REGISTER_CAPTCHA_ID: &str = "d30b3493dd08ff6aedc850648e7e0466";

/// `SportJniUtils.getSecretKey(5)`，与 tokenSign 后缀同源。
pub const SIGN_SALT: &str = "2slhe02lsfiwowlcixisla_sls-_slaor";

/// `VerCodeDeleteUser`：注销账号用的短信验证码类型。
pub const VER_CODE_DELETE_USER: i32 = 5;

/// 按 `SignCode.getSign` 的规则计算签名。
pub fn sign_params(params: &[(&str, String)]) -> String {
    let mut pairs: Vec<(&str, String)> = params
        .iter()
        .filter(|(key, _)| !key.eq_ignore_ascii_case("sign"))
        .map(|(key, value)| (*key, value.clone()))
        .collect();
    pairs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    let joined = pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    md5_hex(format!("{joined}{SIGN_SALT}").as_bytes())
}

/// 发送短信验证码。`code_type` 默认用 [`VER_CODE_DELETE_USER`]。
///
/// 先过 GT4 二次校验（businessType=1），服务端会把校验结果绑定到 `uuid`，
/// 随后同一个 `uuid` 才能用于发码。
pub fn send_ver_code(
    client: &mut ApiClient,
    phone: &str,
    code_type: i32,
    log: &mut dyn FnMut(&str),
) -> Result<Value, String> {
    let uuid = uuid::Uuid::new_v4().to_string();

    log("[vercode] 求解 GT4（REGISTER 场景）…");
    if client.identity.platform != "android" {
        log("[vercode] 提示：当前设备身份为 iOS；发码链路按 Android 客户端参数实现，若返回 10003 请把 identity.json 的 platform 改为 android");
    }
    let creds = solve_gt4(REGISTER_CAPTCHA_ID, client, &mut *log)?;
    let validate_body = json!({
        "lotNumber": creds.get("lotNumber").cloned().unwrap_or(Value::Null),
        "captchaOutput": creds.get("captchaOutput").cloned().unwrap_or(Value::Null),
        "passToken": creds.get("passToken").cloned().unwrap_or(Value::Null),
        "genTime": creds.get("genTime").cloned().unwrap_or(Value::Null),
        "isOffline": false,
        "osType": 0,
        "businessType": 1,
        "uuid": &uuid,
        "username": Value::Null,
    })
    .to_string();
    let url = format!("{HOST}{GEEVALIDATE_PATH}");
    let out = client.envelope_request("POST", &url, &validate_body, UA_IOS, &[])?;
    let decrypted = out
        .decrypted
        .ok_or_else(|| format!("geevalidate 响应解密失败（HTTP {}）", out.http_status))?;
    let validated = check_business(&decrypted.business)?;
    log(&format!(
        "[vercode] geevalidate 通过（{}）",
        validated.get("message").and_then(|v| v.as_str()).unwrap_or("ok")
    ));

    let params = vec![
        ("phone", phone.to_string()),
        ("type", code_type.to_string()),
        ("uuid", uuid.clone()),
    ];
    let sign = sign_params(&params);
    let body = json!({
        "phone": phone,
        "type": code_type,
        "uuid": uuid,
        "sign": sign,
    })
    .to_string();
    client.call("POST", CHECK_CODE_PATH, &body, &[])
}

/// 注销账号。验证码来自 [`send_ver_code`]（type=5）。
pub fn unregister(client: &mut ApiClient, code: &str) -> Result<Value, String> {
    let body = json!({ "code": code }).to_string();
    client.call("POST", DELETE_USER_PATH, &body, &[])
}
