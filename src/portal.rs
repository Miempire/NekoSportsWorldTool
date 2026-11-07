//! 静水空间平台授权：复用网站账号登录，验证会话 Cookie 与授权页。

use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{Local, NaiveDate};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, Oaep, RsaPublicKey};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;

pub const BASE: &str = "https://jsspace.xyz:8443";
pub const PASS_TOKEN: &str = "79882369153325-SSUF";
pub const PASS_PATH: &str = "/p/pass";

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const SESSION_FILE: &str = "portal_session.dat";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PortalSession {
    pub username: String,
    pub cookies: String,
    pub allowed_date: String,
    pub verified_at: i64,
}

#[derive(Debug, Clone)]
pub struct PortalAccess {
    pub username: String,
    pub allowed_date: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredEnvelope {
    v: u8,
    iv: String,
    ct: String,
    mac: String,
}

pub fn session_path() -> PathBuf {
    crate::platform::data_dir().join(SESSION_FILE)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(25))
        .redirects(0)
        .build()
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn cookie_header(jar: &[(String, String)]) -> String {
    jar.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn absorb_cookies(jar: &mut Vec<(String, String)>, values: &[String]) {
    for raw in values {
        let first = raw.split(';').next().unwrap_or("").trim();
        let Some((name, value)) = first.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        let expired = value.trim().is_empty() || lower.contains("max-age=0");
        if expired {
            jar.retain(|(key, _)| key != name);
            continue;
        }
        if let Some(slot) = jar.iter_mut().find(|(key, _)| key == name) {
            slot.1 = value.trim().to_string();
        } else {
            jar.push((name.to_string(), value.trim().to_string()));
        }
    }
}

fn set_cookie_values(resp: &ureq::Response) -> Vec<String> {
    resp.all("set-cookie")
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn hidden_value(html: &str, name: &str) -> Option<String> {
    let patterns = [
        format!("name=\"{name}\""),
        format!("name='{name}'"),
        format!("name={name}"),
    ];
    for pattern in patterns {
        let Some(pos) = html.find(&pattern) else {
            continue;
        };
        let tail = &html[pos..];
        let end = tail.find('>').unwrap_or_else(|| tail.len().min(4096));
        let tag = &tail[..end];
        for marker in ["value=\"", "value='"] {
            if let Some(value_pos) = tag.find(marker) {
                let rest = &tag[value_pos + marker.len()..];
                let quote = marker.chars().last().unwrap_or('"');
                if let Some(end_pos) = rest.find(quote) {
                    return Some(rest[..end_pos].to_string());
                }
            }
        }
    }
    None
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("RSA 公钥参数不是十六进制".into()),
    }
}

fn hex_bytes(value: &str) -> Result<Vec<u8>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("RSA 公钥参数为空".into());
    }
    let mut text = value.to_string();
    if text.len() % 2 == 1 {
        text.insert(0, '0');
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for pair in bytes.chunks(2) {
        out.push((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?);
    }
    Ok(out)
}

fn encrypt_password(
    n_hex: &str,
    e_hex: &str,
    nonce: &str,
    password: &str,
) -> Result<String, String> {
    let n = BigUint::from_bytes_be(&hex_bytes(n_hex)?);
    let e = BigUint::from_bytes_be(&hex_bytes(e_hex)?);
    let key = RsaPublicKey::new(n, e).map_err(|e| format!("RSA 公钥无效：{e}"))?;
    let payload = serde_json::json!({ "n": nonce, "p": password }).to_string();
    let max = key.size() - 64 - 2;
    let mut parts = Vec::new();
    for chunk in payload.as_bytes().chunks(max) {
        let mut rng = rand::thread_rng();
        let cipher = key
            .encrypt(&mut rng, Oaep::new::<Sha256>(), chunk)
            .map_err(|e| format!("密码加密失败：{e}"))?;
        parts.push(URL_SAFE_NO_PAD.encode(cipher));
    }
    Ok(format!("enc1:{}", parts.join(".")))
}

pub fn today() -> String {
    Local::now().format("%Y.%m.%d").to_string()
}

pub fn date_allowed(date: &str) -> bool {
    let Ok(value) = NaiveDate::parse_from_str(date.trim(), "%Y.%m.%d") else {
        return false;
    };
    value <= Local::now().date_naive()
}

/// 读取 1..=max 位连续数字（遇到分隔符即停）。
fn take_digits(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    min: usize,
    max: usize,
) -> Option<String> {
    let mut out = String::new();
    while out.len() < max {
        let Some(&ch) = chars.peek() else { break };
        if !ch.is_ascii_digit() {
            break;
        }
        chars.next();
        out.push(ch);
    }
    (out.len() >= min).then_some(out)
}

fn parse_date_prefix(text: &str) -> Option<String> {
    let mut chars = text.trim_start().chars().peekable();
    let year = take_digits(&mut chars, 4, 4)?;
    let sep1 = chars.next()?;
    if !matches!(sep1, '.' | '-' | '/' | '年') {
        return None;
    }
    let month = take_digits(&mut chars, 1, 2)?;
    let sep2 = chars.next()?;
    let day = if sep1 == '年' {
        if sep2 != '月' {
            return None;
        }
        take_digits(&mut chars, 1, 2)?
    } else {
        if sep2 != sep1 {
            return None;
        }
        take_digits(&mut chars, 1, 2)?
    };
    let y = year.parse::<i32>().ok()?;
    let m = month.parse::<u32>().ok()?;
    let d = day.parse::<u32>().ok()?;
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    Some(date.format("%Y.%m.%d").to_string())
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

fn find_user_date(html: &str, username: &str) -> Option<String> {
    let text = strip_html(html);
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(username) {
        let start = offset + pos;
        let after = &text[start + username.len()..];
        let trimmed = after.trim_start();
        let rest = trimmed
            .strip_prefix(':')
            .or_else(|| trimmed.strip_prefix('：'));
        if let Some(rest) = rest {
            if let Some(date) = parse_date_prefix(rest) {
                return Some(date);
            }
        }
        offset = start + username.len();
        if offset >= text.len() {
            break;
        }
    }

    if text.len() == text.to_lowercase().len() {
        let lowered = text.to_lowercase();
        let needle = username.to_lowercase();
        let mut offset = 0usize;
        while let Some(pos) = lowered[offset..].find(&needle) {
            let start = offset + pos;
            let after = &text[start + needle.len()..];
            let trimmed = after.trim_start();
            let rest = trimmed
                .strip_prefix(':')
                .or_else(|| trimmed.strip_prefix('：'));
            if let Some(rest) = rest {
                if let Some(date) = parse_date_prefix(rest) {
                    return Some(date);
                }
            }
            offset = start + needle.len();
            if offset >= lowered.len() {
                break;
            }
        }
    }
    None
}

fn verify_html(username: &str, html: &str) -> Result<PortalAccess, String> {
    if !html.contains(PASS_TOKEN) {
        return Err("授权页校验失败：缺少必需的验证标记".into());
    }
    let Some(allowed_date) = find_user_date(html, username) else {
        return Err("授权页校验失败：未找到当前账号的授权信息".into());
    };
    if !date_allowed(&allowed_date) {
        return Err("授权不可用：授权日期晚于当前日期".into());
    }
    Ok(PortalAccess {
        username: username.to_string(),
        allowed_date,
    })
}

pub fn verify_cookies(username: &str, cookies: &str) -> Result<PortalAccess, String> {
    let resp = agent()
        .get(&format!("{BASE}{PASS_PATH}"))
        .set("User-Agent", UA)
        .set("Cookie", cookies)
        .call()
        .map_err(|e| format!("访问授权页失败：{e}"))?;
    let status = resp.status();
    if status != 200 {
        return Err(format!("授权页返回 HTTP {status}，登录会话无效"));
    }
    let html = resp
        .into_string()
        .map_err(|e| format!("读取授权页失败：{e}"))?;
    verify_html(username, &html)
}

pub fn login_and_save(
    username: &str,
    password: &str,
    device_id: &str,
) -> Result<PortalSession, String> {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return Err("请填写静水空间用户名和密码".into());
    }
    let client = agent();
    let mut jar: Vec<(String, String)> = Vec::new();

    let resp = client
        .get(&format!("{BASE}/login?returnUrl=%2Fp%2Fpass"))
        .set("User-Agent", UA)
        .call()
        .map_err(|e| format!("访问登录页失败：{e}"))?;
    let status = resp.status();
    let cookies = set_cookie_values(&resp);
    let html = resp
        .into_string()
        .map_err(|e| format!("读取登录页失败：{e}"))?;
    if status != 200 {
        return Err(format!("登录页返回 HTTP {status}"));
    }
    absorb_cookies(&mut jar, &cookies);
    let token = hidden_value(&html, "__RequestVerificationToken")
        .ok_or("未能获取登录页防伪令牌，请稍后重试")?;

    let challenge_resp = client
        .get(&format!(
            "{BASE}/api/login/challenge?userName={}",
            url_encode(username)
        ))
        .set("User-Agent", UA)
        .set("Cookie", &cookie_header(&jar))
        .call()
        .map_err(|e| format!("获取登录挑战失败：{e}"))?;
    let challenge_cookies = set_cookie_values(&challenge_resp);
    let challenge_text = challenge_resp
        .into_string()
        .map_err(|e| format!("读取登录挑战失败：{e}"))?;
    absorb_cookies(&mut jar, &challenge_cookies);
    let challenge: serde_json::Value = serde_json::from_str(&challenge_text)
        .map_err(|e| format!("登录挑战格式无效：{e}"))?;
    let n = challenge["n"].as_str().ok_or("登录挑战缺少 RSA 模数")?;
    let e = challenge["e"].as_str().ok_or("登录挑战缺少 RSA 指数")?;
    let nonce = challenge["nonce"].as_str().ok_or("登录挑战缺少 nonce")?;
    let encrypted = encrypt_password(n, e, nonce, password)?;

    let body = format!(
        "returnUrl=%2Fp%2Fpass&UserName={}&Enc={}&RememberMe=true&__RequestVerificationToken={}",
        url_encode(username),
        url_encode(&encrypted),
        url_encode(&token)
    );
    let login_resp = client
        .post(&format!("{BASE}/login"))
        .set("User-Agent", UA)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Referer", &format!("{BASE}/login"))
        .set("Cookie", &cookie_header(&jar))
        .send_string(&body)
        .map_err(|e| format!("提交登录失败：{e}"))?;
    let login_status = login_resp.status();
    let login_cookies = set_cookie_values(&login_resp);
    let login_text = login_resp.into_string().unwrap_or_default();
    absorb_cookies(&mut jar, &login_cookies);

    let cookies = cookie_header(&jar);
    if cookies.is_empty() {
        return Err(format!(
            "登录失败（HTTP {login_status}）：服务器未返回会话 Cookie"
        ));
    }
    if login_status == 200 {
        let hint = if login_text.contains("不正确") || login_text.contains("停用") {
            "用户名或密码不正确，或账号已被停用"
        } else if login_text.contains("频繁") {
            "登录尝试过于频繁，请稍后再试"
        } else {
            "登录未通过，请检查账号信息"
        };
        return Err(hint.to_string());
    }

    let access = verify_cookies(username, &cookies)?;
    let session = PortalSession {
        username: access.username,
        cookies,
        allowed_date: access.allowed_date,
        verified_at: chrono::Utc::now().timestamp_millis(),
    };
    save_session(&session, device_id)?;
    Ok(session)
}

pub fn verify_saved(device_id: &str) -> Result<PortalAccess, String> {
    let session = load_saved(device_id).ok_or("尚未登录静水空间平台账号")?;
    verify_cookies(&session.username, &session.cookies)
}

pub fn load_saved(device_id: &str) -> Option<PortalSession> {
    let raw = std::fs::read_to_string(session_path()).ok()?;
    let stored: StoredEnvelope = serde_json::from_str(&raw).ok()?;
    if stored.v != 1 {
        return None;
    }
    let (aes_key, mac_key) = storage_keys(device_id);
    let iv = B64.decode(stored.iv.as_bytes()).ok()?;
    let cipher = B64.decode(stored.ct.as_bytes()).ok()?;
    let expected = hex_encode(&hmac_sha256(&mac_key, &iv, &cipher));
    if !constant_time_eq(&expected, &stored.mac) {
        return None;
    }
    let plain = crate::crypto::envelope::aes128_cbc_decrypt_iv(&aes_key, &iv, &cipher).ok()?;
    serde_json::from_slice(&plain).ok()
}

pub fn save_session(session: &PortalSession, device_id: &str) -> Result<(), String> {
    let plain = serde_json::to_vec(session).map_err(|e| format!("序列化登录会话失败：{e}"))?;
    let (aes_key, mac_key) = storage_keys(device_id);
    let iv: [u8; 16] = rand::random();
    let cipher = crate::crypto::envelope::aes128_cbc_encrypt_iv(&aes_key, &iv, &plain)?;
    let mac = hex_encode(&hmac_sha256(&mac_key, &iv, &cipher));
    let stored = StoredEnvelope {
        v: 1,
        iv: B64.encode(iv),
        ct: B64.encode(cipher),
        mac,
    };
    let json = serde_json::to_string(&stored).map_err(|e| format!("序列化加密会话失败：{e}"))?;
    std::fs::write(session_path(), json).map_err(|e| format!("写入登录会话失败：{e}"))
}

pub fn clear_saved() {
    let _ = std::fs::remove_file(session_path());
}

fn storage_keys(device_id: &str) -> ([u8; 16], [u8; 32]) {
    let mut hasher = Sha256::new();
    hasher.update(b"NekoSportsWorldTool/portal/v1/");
    hasher.update(device_id.as_bytes());
    let digest = hasher.finalize();
    let mut aes_key = [0u8; 16];
    aes_key.copy_from_slice(&digest[..16]);

    let mut mac_hasher = Sha256::new();
    mac_hasher.update(b"NekoSportsWorldTool/portal/hmac/v1/");
    mac_hasher.update(digest);
    let mac_digest = mac_hasher.finalize();
    let mut mac_key = [0u8; 32];
    mac_key.copy_from_slice(&mac_digest);
    (aes_key, mac_key)
}

fn hmac_sha256(key: &[u8], iv: &[u8], cipher: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        let digest = Sha256::digest(key);
        block[..32].copy_from_slice(&digest);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for i in 0..64 {
        inner_pad[i] ^= block[i];
        outer_pad[i] ^= block[i];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(iv);
    inner.update(cipher);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    let outer_digest = outer.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer_digest);
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}
