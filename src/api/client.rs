//! HTTP 客户端封装。
//!
//! 通用请求封装：body = 信封(明文, insert)，headerSign = 信封(header, observed)，
//! 响应两层解密到业务 JSON；同一实例复用同一 keyDataFour。

use crate::api::model::Session;
use crate::crypto::decrypt::{decrypt_response, derive_paes_key, Decrypted};
use crate::crypto::envelope::{
    build_envelope, build_envelope_android, build_envelope_ts, rsa_public_key, EnvelopeSession,
    OuterOrder,
};
use crate::crypto::header::{
    build_header_for, HeaderIdentity, ANDROID_APP_VERSION, IOS_APP_VERSION, UA_IOS,
};

pub fn make_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

pub struct ApiClient {
    pub agent: ureq::Agent,
    pub session: EnvelopeSession,
    pub identity: HeaderIdentity,
    pub login: Option<Session>,
}

pub struct RequestOutcome {
    pub http_status: u16,
    pub decrypted: Option<Decrypted>,
    pub raw_len: usize,
    /// 解密失败时的服务端原始返回摘要（诊断用）。
    pub raw_head: String,
    /// 本次发出的请求信封前 160 字符（诊断用：对比真机信封形状）。
    pub sent_head: String,
    /// 本次实际使用的 User-Agent（诊断用：确认平台口径没串）。
    pub sent_ua: String,
}

impl ApiClient {
    pub fn new(identity: HeaderIdentity, login: Option<Session>) -> Self {
        let mut session = EnvelopeSession::new();
        // [Android 链] 把 identity.json 里注入的运行时材料灌进会话。
        // 缺失时保持 android_ready()==false，请求自动退回 iOS 口径并可在日志里显式诊断。
        if identity.platform == "android" {
            match (&identity.android_req_key, &identity.android_key_data) {
                (Some(rk), Some(kd)) => {
                    match session.set_android_material(rk, kd.clone()) {
                        Ok(()) => eprintln!(
                            "[client] Android 信封口径已启用（reqKey 前 8 字符 = {}）",
                            rk.chars().take(8).collect::<String>()
                        ),
                        Err(e) => eprintln!("[client] Android 材料注入失败（将退回 iOS 口径）: {e}"),
                    }
                }
                _ => {
                    eprintln!(
                        "[client] Android 身份缺运行时材料（android_req_key/android_key_data），
请先抓取后写入 identity.json，否则服务端会判数据异常"
                    );
                }
            }
        }
        Self {
            agent: make_agent(),
            session,
            identity,
            login,
        }
    }

    fn uid(&self) -> i64 {
        self.login.as_ref().map(|s| s.uid).unwrap_or(-1)
    }

    fn token(&self) -> String {
        self.login.as_ref().map(|s| s.token.clone()).unwrap_or_default()
    }

    /// 发送信封请求（header/headerSign 用 observed 序，body 用 insert 序）。
    /// method 为 "POST" 或 "GET"（GET 同样携带信封 body）。
    pub fn envelope_request(
        &mut self,
        method: &str,
        url: &str,
        body_plain: &str,
        ua: &str,
        extra_headers: &[(String, String)],
    ) -> Result<RequestOutcome, String> {
        let uid = self.uid();
        let token = self.token();
        let (header_plain, header_extra) = build_header_for(&self.identity, uid, &token, None);
        let (header_env, body_env) = if self.identity.platform == "android" {
            let rk = self.session.ensure_android_material();
            let kd = self.session.key_data();
            let ts_sec = chrono::Utc::now().timestamp();
            let henv = build_envelope_android(&rk, kd.clone(), &header_plain, OuterOrder::Observed, ts_sec);
            let benv = build_envelope_android(&rk, kd, body_plain, OuterOrder::Insert, ts_sec + 1);
            (henv, benv)
        } else {
            let henv = build_envelope(&mut self.session, &header_plain, OuterOrder::Observed);
            // body 时间戳 = header ts + 1（原生两次独立读取毫秒）
            let benv = build_envelope_ts(&mut self.session, body_plain, OuterOrder::Insert, henv.ts_ms + 1);
            (henv, benv)
        };

        let mut req = self.agent.request(method, url);
        // [真机口径] API 请求的 Content-Type 是 `application/json`（无 charset 参数）。
        // 之前发 `application/json; charset=utf-8` 是 iOS 链的写法。
        req = req.set("Content-Type", "application/json");
        // [真机口径] UA 必须跟平台走：Android API 是 okhttp/4.12.0（实测 514 请求），
        // 不是 WebView 串、更不是 iOS 的 SWCampus 串。之前这里直接用调用方传的 ua，
        // 而多数调用点硬编码 UA_IOS → Android 身份发 iOS UA → 服务端返回 200+空 body。
        let ua_effective = if self.identity.platform == "android" {
            crate::crypto::header::UA_ANDROID
        } else {
            ua
        };
        req = req.set("User-Agent", ua_effective);
        // [Android 链] 头里的 appVersion 必须跟身份平台走；原来硬编码 IOS_APP_VERSION 会让
        // Android 身份发出 iOS 版本号，服务端一致性校验直接判异常。
        let app_version = if self.identity.platform == "android" {
            ANDROID_APP_VERSION
        } else {
            IOS_APP_VERSION
        };
        req = req.set("appVersion", app_version);
        req = req.set("headerSign", &header_env.json);
        // [真机口径·关键] Android 每个业务请求都**额外**带 nonce / timeStamp / tokenSign
        // 三个独立 HTTP 头（实测抓包字段：Content-Type,User-Agent,appVersion,headerSign,
        // nonce,timeStamp,tokenSign）。之前这里把 build_header_for 的第二个返回值丢弃，
        // 这三个头一个都没发 → 服务端校验不过 → HTTP 200 但 body 为空。
        // submit.rs 一直有补这三个头，通用请求却漏了。
        if self.identity.platform == "android" {
            for (k, v) in &header_extra {
                req = req.set(k, v);
            }
        }
        for (k, v) in extra_headers {
            req = req.set(k, v);
        }
        let resp = req.send_string(&body_env.json).map_err(ureq_err)?;
        let status = resp.status();
        let raw = resp.into_string().unwrap_or_default();
        let raw_len = raw.len();
        let key = derive_paes_key(
            &body_env.key_data[0],
            &body_env.key_data[1],
            &body_env.key_data[2],
            &body_env.key_data[3],
        );
        // 【实测修正】响应 `s` 的验签公钥两平台都是 b9a6a953…（RSA_PUB_PEM）。
        // 服务端实测：用它 raw 运算恢复出的摘要与 MD5(内层信封) 完全一致。
        let verify_key = rsa_public_key();
        let (decrypted, raw_head) = match verify_key {
            Ok(pk) => match decrypt_response(raw.as_bytes(), &key, &pk) {
                Ok(d) => (Some(d), String::new()),
                Err(e) => (
                    None,
                    format!("{e} | 原文: {}", crate::textlog::truncate(&raw, 160)),
                ),
            },
            Err(e) => (None, format!("{e}")),
        };
        Ok(RequestOutcome {
            http_status: status,
            decrypted,
            raw_len,
            raw_head,
            sent_head: crate::textlog::truncate(&body_env.json, 160),
            sent_ua: ua_effective.to_string(),
        })
    }

    /// 发送原始字节体（Multipart 等）。headerSign 仍按信封链生成，
    /// body 本身不做信封加密（抽检照片上传就是这种形态）。
    pub fn raw_request(
        &mut self,
        method: &str,
        url: &str,
        body: Vec<u8>,
        content_type: &str,
        ua: &str,
        extra_headers: &[(String, String)],
    ) -> Result<RequestOutcome, String> {
        let uid = self.uid();
        let token = self.token();
        let (header_plain, header_extra) = build_header_for(&self.identity, uid, &token, None);
        // [Android 链] raw 请求的 headerSign 同样要走 Android 口径
        let header_env = if self.identity.platform == "android" && self.session.android_ready() {
            let rk = self.session.android_req_key().unwrap_or_default().to_string();
            let kd = self.session.key_data();
            let ts_sec = chrono::Utc::now().timestamp();
            build_envelope_android(&rk, kd, &header_plain, OuterOrder::Observed, ts_sec)
        } else {
            build_envelope(&mut self.session, &header_plain, OuterOrder::Observed)
        };

        let mut req = self.agent.request(method, url);
        req = req.set("Content-Type", content_type);
        // [真机口径] 同 envelope_request：Android 的 UA 固定 okhttp/4.12.0
        let ua_effective = if self.identity.platform == "android" {
            crate::crypto::header::UA_ANDROID
        } else {
            ua
        };
        req = req.set("User-Agent", ua_effective);
        // [Android 链] 同 envelope_request：appVersion 跟平台走
        let app_version = if self.identity.platform == "android" {
            ANDROID_APP_VERSION
        } else {
            IOS_APP_VERSION
        };
        req = req.set("appVersion", app_version);
        req = req.set("headerSign", &header_env.json);
        // [真机口径·关键] 同 envelope_request：补 nonce / timeStamp / tokenSign 三个头
        if self.identity.platform == "android" {
            for (k, v) in &header_extra {
                req = req.set(k, v);
            }
        }
        for (k, v) in extra_headers {
            req = req.set(k, v);
        }
        let resp = req.send_bytes(&body).map_err(ureq_err)?;
        let status = resp.status();
        let raw = resp.into_string().unwrap_or_default();
        let raw_len = raw.len();
        let key = derive_paes_key(
            &header_env.key_data[0],
            &header_env.key_data[1],
            &header_env.key_data[2],
            &header_env.key_data[3],
        );
        // 【实测修正】响应 `s` 验签公钥两平台统一 b9a6a953…（实测恢复摘要与 MD5(inner) 一致）
        let verify_key = rsa_public_key();
        let (decrypted, raw_head) = match verify_key {
            Ok(pk) => match decrypt_response(raw.as_bytes(), &key, &pk) {
                Ok(d) => (Some(d), String::new()),
                Err(e) => (
                    None,
                    format!("{e} | 原文: {}", crate::textlog::truncate(&raw, 160)),
                ),
            },
            Err(e) => (None, format!("{e}")),
        };
        Ok(RequestOutcome {
            http_status: status,
            decrypted,
            raw_len,
            raw_head,
            sent_head: format!("<raw {}B>", body.len()),
            sent_ua: ua_effective.to_string(),
        })
    }

    /// 标准业务请求（RUN 域名）：返回业务 JSON；error != 10000 时 Err。
    pub fn call(
        &mut self,
        method: &str,
        path: &str,
        body_plain: &str,
        extra_headers: &[(String, String)],
    ) -> Result<serde_json::Value, String> {
        self.call_host(crate::api::model::HOST, method, path, body_plain, extra_headers)
    }

    /// 指定域名业务请求（排行榜/违规名单走 DISCOVERY，信封链相同）。
    /// 每次调用打完整解密响应日志。
    pub fn call_host(
        &mut self,
        host: &str,
        method: &str,
        path: &str,
        body_plain: &str,
        extra_headers: &[(String, String)],
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}{}", host, path);
        let out = self.envelope_request(method, &url, body_plain, UA_IOS, extra_headers)?;
        let dec = out.decrypted.ok_or_else(|| {
            // 诊断：把「实际发出的信封形状」和「实际 UA」也带出来，便于和真机抓包对比。
            // 典型失败形态是 HTTP 200 + len=0（服务端静默拒绝，不回错误体），
            // 常见原因依次是：UA 与平台不一致、Content-Type 带了 charset、信封口径串了。
            format!(
                "HTTP {} 响应解密失败（len={}）UA={} 发信封={} | 原文: {}",
                out.http_status, out.raw_len, out.sent_ua, out.sent_head, out.raw_head
            )
        })?;
        let business = match check_business(&dec.business) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[resp] {} -> Err: {}", path, e);
                return Err(e);
            }
        };
        let biz_str = serde_json::to_string(&business).unwrap_or_default();
        eprintln!("[resp] {} -> {} ({}B) {}", path, out.http_status, out.raw_len, biz_str);
        Ok(business)
    }
}

/// 业务结果检查：error == 10000 → Ok(business)；否则 Err(描述)。
pub fn check_business(v: &serde_json::Value) -> Result<serde_json::Value, String> {
    let err = v.get("error").and_then(|e| e.as_i64()).unwrap_or(0);
    if err == 10000 {
        Ok(v.clone())
    } else {
        let msg = v
            .get("message")
            .or_else(|| v.get("msg"))
            .and_then(|m| m.as_str())
            .unwrap_or("(无消息)");
        Err(format!("业务错误 {err}: {msg}"))
    }
}

/// 业务字段查找：先顶层，再 data 子对象。
pub fn get_field<'a>(biz: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    if let Some(v) = biz.get(name) {
        if !v.is_null() {
            return Some(v);
        }
    }
    biz.get("data").and_then(|d| d.get(name))
}

/// data 字段如果是 JSON 字符串则解析（Obs 一层包裹语义）。
pub fn parse_data_field(biz: &serde_json::Value) -> serde_json::Value {
    match biz.get("data") {
        Some(serde_json::Value::String(s)) => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
        Some(v) => v.clone(),
        None => serde_json::Value::Null,
    }
}

pub fn ureq_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            format!("HTTP {code}: {}", &body[..body.len().min(300)])
        }
        ureq::Error::Transport(t) => format!("网络错误: {t}"),
    }
}
