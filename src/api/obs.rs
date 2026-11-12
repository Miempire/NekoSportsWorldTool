//! OBS 上传：POST /api/obs/temporary/url 换签名 URL → PUT JSON。

use super::client::{get_field, parse_data_field, ApiClient};
use serde_json::{json, Value};

pub const OBS_SIGN_PATH: &str = "/api/obs/temporary/url";

/// 换取签名 URL。
/// 注意：`contentType` 会进 OBS 签名——官方 Put 用 `text/plain`，Get 用 `application/json`。
pub fn sign_url(client: &mut ApiClient, method: &str, key: &str) -> Result<String, String> {
    let content_type = if method.eq_ignore_ascii_case("put") {
        "text/plain"
    } else {
        "application/json"
    };
    let body = json!({
        "bucketName": "iydsj-hbase-hot",
        "objectKey": key,
        "method": method,
        "contentType": content_type,
    })
    .to_string();
    let biz = client.call("POST", OBS_SIGN_PATH, &body, &[])?;
    // business.data 可能是 JSON 字符串 {"signedUrl": ...}
    let data = parse_data_field(&biz);
    let signed = data
        .get("signedUrl")
        .and_then(|v| v.as_str())
        .or_else(|| get_field(&biz, "signedUrl").and_then(|v| v.as_str()))
        .ok_or("OBS 签名响应缺 signedUrl")?;
    Ok(signed.to_string())
}

/// PUT 上传 OBS 对象（Content-Type 必须与签名时一致：text/plain）。
pub fn put_object(signed_url: &str, payload: &[u8], log: &mut dyn FnMut(&str)) -> Result<(), String> {
    let agent = super::client::make_agent();
    let resp = agent
        .put(signed_url)
        .set("Content-Type", "text/plain")
        .send_bytes(payload)
        .map_err(super::client::ureq_err)?;
    log(&format!("[obs] PUT {} -> {}", shorten(signed_url), resp.status()));
    Ok(())
}

/// 上传到两个 objectKey（详情页 + 兜底路径）。
pub fn upload_both_keys(
    client: &mut ApiClient,
    keys: &[String],
    payload: &[u8],
    log: &mut dyn FnMut(&str),
) -> usize {
    let mut ok = 0;
    for key in keys {
        match sign_url(client, "Put", key).and_then(|url| put_object(&url, payload, log)) {
            Ok(()) => ok += 1,
            Err(e) => log(&format!("[obs] PUT {key} 失败: {e}")),
        }
    }
    ok
}

fn shorten(url: &str) -> &str {
    // 只显示 objectKey 部分，避免日志过长
    let start = url.find("run_data").unwrap_or(0);
    let end = (start + 60).min(url.len());
    &url[start..end]
}

/// 回读验证（GET 签名）。
#[allow(dead_code)]
pub fn fetch_object(client: &mut ApiClient, key: &str, log: &mut dyn FnMut(&str)) -> Result<Value, String> {
    let signed = sign_url(client, "Get", key)?;
    let agent = super::client::make_agent();
    let resp = agent.get(&signed).call().map_err(super::client::ureq_err)?;
    let text = resp.into_string().unwrap_or_default();
    log(&format!("[obs] GET 回读 {} 字节", text.len()));
    serde_json::from_str(&text).map_err(|e| format!("OBS 对象解析失败: {e}"))
}

