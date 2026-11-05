//! 极验回执上报（deviceVerify）。
//!
//! 起跑拿到五点后，客户端把 DeepKnow 生成的 `geeToken` 报到
//! `partner.iydsj.com`，服务端用它做设备/交互校验。
//! 文档：`docs/research/APP-NETWORK-MAP.md` §5.5。
//!
//! 触发点（官方）：三个起跑 ViewModel 的 `requestFivePoint` onSuccess、
//! 上传成功回调、室内起跑。本工具默认不发，需显式调用。

use super::client::ApiClient;
use crate::crypto::header::{UA_ANDROID, UA_IOS};
use serde_json::{json, Value};

pub const PARTNER_HOST: &str = "https://partner.iydsj.com";
pub const DEVICE_VERIFY_PATH: &str = "/api/v620/datacollection/geetest/deviceVerify";
/// 官方写死的 interactive 值。
pub const INTERACTIVE: i64 = 5;

/// 上报一次极验回执。`rrid == -1` 时按官方语义不写该字段；`uuid` 为空同样省略。
pub fn device_verify(
    client: &mut ApiClient,
    gee_token: &str,
    rrid: i64,
    uuid: Option<&str>,
    error_code: i64,
) -> Result<Value, String> {
    if gee_token.trim().is_empty() {
        return Err("geeToken 为空：需要先取到 DeepKnow 回执".into());
    }
    let mut body = json!({
        "geeToken": gee_token,
        "interactive": INTERACTIVE,
        "errorCode": error_code,
    });
    if rrid != -1 {
        body["rrid"] = json!(rrid);
    }
    if let Some(value) = uuid {
        if !value.is_empty() {
            body["uuid"] = json!(value);
        }
    }
    let ua = if client.identity.platform == "android" {
        UA_ANDROID
    } else {
        UA_IOS
    };
    let url = format!("{PARTNER_HOST}{DEVICE_VERIFY_PATH}");
    let out = client.envelope_request("POST", &url, &body.to_string(), ua, &[])?;
    match out.decrypted {
        Some(decrypted) => Ok(decrypted.business),
        None => Err(format!(
            "deviceVerify 响应解密失败（HTTP {}）{}",
            out.http_status, out.raw_head
        )),
    }
}
