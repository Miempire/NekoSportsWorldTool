//! NetSecKit 信封加密（iOS 请求链）。
//!
//! - keyDataOne = "com.wanhang.school" 末 12 字符
//! - keyDataTwo = "5K0E8400-E29B-11D4-A716-4G6RW65G544F" 前 12 字符
//! - keyDataThree = "DE6AED50-2B3A-5327-ACED-597DEA1AFB49" 末 12 字符
//! - keyDataFour = format!("{:.6}", now_ms_f64) 的末 12 字符（native %f 语义，
//!   singleton 首次生成后缓存——同一会话内所有请求复用同一个 Four）
//! - inner container 固定键序 data,timeStamp,platform,keyDataOne..Four
//! - d=AES-128-CBC/PKCS7/IV=0(reqKey)；h=MD5(container)；k=RSA-1024 PKCS1v15(reqKey)
//! - outer 键序：observed=[k,p,d,h,t]（headerSign 用）；insert=[d,h,k,p,t]（body 用）

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use cbc::cipher::{block_padding::Pkcs7, BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use md5::{Digest, Md5};
use rand::Rng;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

pub const KEYDATA_ONE: &str = "nhang.school";
pub const KEYDATA_TWO: &str = "5K0E8400-E29";
pub const KEYDATA_THREE: &str = "597DEA1AFB49";

/// native reqKey 白名单字符池。
pub const SALT_ALPHABET: &str = "+kot8A*B45jF6CD@a!UVWubcdKLZ{efgMpNOxyz01PQ}Rn)Tvw23XYh(iG7rsEqJHI9+Slm/";

/// 客户端内置 RSA-1024 公钥（SPKI PEM）。
pub const RSA_PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----\n\
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQC5pqlTzsGNZk1RxhH4O4x3JNTD\n\
V7FbVH66mPfW5v1tnIy4ty7xv8DGMG4Zn/TvstwlJWeYOADHdi8uF21lJaBvzvPt\n\
VEhifHXZq825fI9hGYtDoaVQmCN/Nfs2dKmt89XDrhtl3SZxO6TumOCTQt+5oqjF\n\
2Jo3o1YtkAyGzjaJnwIDAQAB\n\
-----END PUBLIC KEY-----";

/// 【逆向实测】Android 链（易盾 NetSecKit 4.2.4）请求加密用的 RSA-1024 公钥。
///
/// 来源：`sw_app.apk` → `lib/arm64-v8a/libnetseckit-4.2.4.so`，文件偏移 `0x171990`
/// 的 SPKI DER（`30 81 9f 30 0d 06 09 2a 86 48 86 f7 0d 01 01 01 05 00 03 81 8d 00 …`，
/// 162 B），模数 `ccb3a6c585be65ad…`、指数 65537。
///
/// 注意：这与 iOS 链的 `RSA_PUB_PEM`（模数 `b9a6a953…`）**不是同一把**——
/// lab 用 unidbg 实测 Android 的 `k` 就是用这把公钥加密的。
/// 另外这把也同时被用作响应 `s` 的验签公钥（同一 SPKI 两用）。
pub const RSA_PUB_PEM_ANDROID: &str = "-----BEGIN PUBLIC KEY-----\n\
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDMs6bFhb5lrf/xqwtNcKVHW09c\n\
oMbtLAmr9Rti7vnSnVroFuLHJEzxMqPQeItByr1Fu4RAQMR/aq/oknn53kCOuSaV\n\
sd9mnDCusrs7GmExDxlKe2pNAKTEZnT1FITQYT2SJkeCZ8iBKujqN61C3SXQ4Xa8\n\
QRr2uMTicec6XC08WQIDAQAB\n\
-----END PUBLIC KEY-----";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OuterOrder {
    /// [k,p,d,h,t] —— 真实 headerSign 样本顺序
    Observed,
    /// [d,h,k,p,t] —— body 用插入序
    Insert,
}

/// 缓存 RSA 公钥（进程内只解析一次）。
pub fn rsa_public_key() -> Result<rsa::RsaPublicKey, String> {
    use rsa::pkcs8::DecodePublicKey;
    rsa::RsaPublicKey::from_public_key_pem(RSA_PUB_PEM)
        .map_err(|e| format!("内置 RSA 公钥解析失败: {e}"))
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn md5_hex(data: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(data);
    let out = h.finalize();
    let mut s = String::with_capacity(32);
    for b in out {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

pub fn b64_encode(data: &[u8]) -> String {
    B64.encode(data)
}

pub fn b64_decode(data: &str) -> Result<Vec<u8>, String> {
    B64.decode(data).map_err(|e| format!("base64 解码失败: {e}"))
}

/// AES-128-CBC / PKCS7 / IV=0 加密（请求 d 与响应两层解密同参数）。
pub fn aes128_cbc_encrypt(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let key: &[u8; 16] = key
        .try_into()
        .map_err(|_| format!("AES key 必须是 16B，得到 {}", key.len()))?;
    let iv = [0u8; 16];
    Ok(Aes128CbcEnc::new(key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(plaintext))
}

/// AES-128-CBC / PKCS7 / IV=0 解密。
pub fn aes128_cbc_decrypt(key: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let key: &[u8; 16] = key
        .try_into()
        .map_err(|_| format!("AES key 必须是 16B，得到 {}", key.len()))?;
    aes128_cbc_decrypt_iv(key, &[0u8; 16], ciphertext)
}

/// AES-128-CBC / PKCS7 / 自定义 IV 解密（本地加密会话等使用）。
pub fn aes128_cbc_decrypt_iv(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let key: &[u8; 16] = key
        .try_into()
        .map_err(|_| format!("AES key 必须是 16B，得到 {}", key.len()))?;
    let iv: &[u8; 16] = iv
        .try_into()
        .map_err(|_| format!("AES IV 必须是 16B，得到 {}", iv.len()))?;
    Aes128CbcDec::new(key.into(), iv.into())
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|e| format!("AES 解密/PKCS7 失败: {e}"))
}

/// AES-128-CBC / PKCS7 / 自定义 IV 加密（GT4 get_w 用 ASCII "0000"x4 IV）。
pub fn aes128_cbc_encrypt_iv(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let key: &[u8; 16] = key
        .try_into()
        .map_err(|_| format!("AES key 必须是 16B，得到 {}", key.len()))?;
    let iv: &[u8; 16] = iv
        .try_into()
        .map_err(|_| format!("AES IV 必须是 16B，得到 {}", iv.len()))?;
    Ok(Aes128CbcEnc::new(key.into(), iv.into()).encrypt_padded_vec::<Pkcs7>(plaintext))
}

/// 从 SALT_ALPHABET 独立抽取 n 个字符（native reqKey 语义）。
pub fn random_fragment(n: usize) -> String {
    let alphabet = SALT_ALPHABET.as_bytes();
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..n)
        .map(|_| alphabet[rng.gen_range(0..alphabet.len())])
        .collect();
    String::from_utf8(bytes).unwrap_or_default()
}

pub fn req_key() -> String {
    random_fragment(16)
}

/// Android keyData 一段：随机 8–12 字节可打印 ASCII（0x20..0x7e）。
///
/// 【静态实证】libnetseckit-4.2.4.so 里存在一条完整的可打印字符池常量
/// 从 0x20 空格到 0x7e 波浪号共 95 个字符，与 lab unidbg 观测到的 keyData
/// 含 ^ 单引号 逗号 等字符一致。之前取 0x21..0x7e（少了空格），已按实证修正。
fn random_keydata() -> String {
    let mut rng = rand::thread_rng();
    let len = rng.gen_range(8..=12);
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        s.push(rng.gen_range(0x20..=0x7e) as u8 as char);
    }
    s
}

/// normalize_raw_key_data：≤7B 补随机字母数字到 12；≥13B 取 UTF-8 末 12；8-12 原样。
pub fn normalize_raw_key_data(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() <= 7 {
        let missing = 12 - bytes.len();
        let mut out = value.to_string();
        out.push_str(&random_fragment(missing));
        out
    } else if bytes.len() >= 13 {
        String::from_utf8_lossy(&bytes[bytes.len() - 12..]).into_owned()
    } else {
        value.to_string()
    }
}

/// keyDataFour：native `stringWithFormat:"%f"`（毫秒 double → 6 位小数）取末 12 字符。
pub fn key_data_four_from_ms(now_ms_f64: f64) -> String {
    let formatted = format!("{:.6}", now_ms_f64);
    normalize_raw_key_data(&formatted)
}

/// NTESSecurityKit singleton 会话：Four 首次生成后整个会话复用。
pub struct EnvelopeSession {
    four: Option<String>,
    /// 【Android 链】每进程随机 16B reqKey（实测不可由本进程推导，必须外部注入）。
    android_req_key: Option<String>,
    /// 【Android 链】会话随机 4 段 keyData（长度 8–12，字符集含 `^ ' ,`，必须外部注入）。
    android_key_data: Option<[String; 4]>,
}

impl Default for EnvelopeSession {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvelopeSession {
    pub fn new() -> Self {
        Self { four: None, android_req_key: None, android_key_data: None }
    }

    /// 【Android 链】显式覆盖 reqKey + 4 段 keyData（可选）。
    ///
    /// 正常情况下**不必调用**：`ensure_android_material()` 会自动随机生成。仅当要复现
    /// 某一特定会话（把抓包/内存抓到的值原样复刻）时才用它覆盖。
    /// 返回 Err 表示值不合法（reqKey 必须 16B；keyData 每段 8–12B）。
    pub fn set_android_material(
        &mut self,
        req_key: &str,
        key_data: [String; 4],
    ) -> Result<(), String> {
        if req_key.as_bytes().len() != 16 {
            return Err(format!(
                "Android reqKey 必须是 16B，得到 {}B",
                req_key.as_bytes().len()
            ));
        }
        for (i, v) in key_data.iter().enumerate() {
            let l = v.as_bytes().len();
            if !(8..=12).contains(&l) {
                return Err(format!(
                    "Android keyData[{}] 长度必须是 8..12B，得到 {}B: {:?}",
                    i, l, v
                ));
            }
        }
        self.android_req_key = Some(req_key.to_string());
        self.android_key_data = Some(key_data);
        Ok(())
    }

    /// Android 链是否已具备可用的运行时材料。
    pub fn android_ready(&self) -> bool {
        self.android_req_key.is_some() && self.android_key_data.is_some()
    }

    /// 取已注入的 Android reqKey（未注入返回 None）。
    pub fn android_req_key(&self) -> Option<&str> {
        self.android_req_key.as_deref()
    }

    /// 【关键更正】Android 的 reqKey + keyData 是**客户端每会话随机生成**的，不是服务端下发的秘密：
    /// 服务端用 RSA 私钥从 `k` 解出 reqKey，再用它解 `d` 得到 container 及 keyData 明文，
    /// 并据此派生响应密钥 —— 所以重实现客户端**自己随机生成即可，无需从真机窃取**。
    ///
    /// 本方法在未注入时自动随机填充，使 Android 平台下 `android_ready()` 恒为 true。
    /// 只在「要复现某一特定会话」时才需要用 `set_android_material` 覆盖为抓到的值。
    pub fn ensure_android_material(&mut self) -> String {
        if self.android_req_key.is_none() {
            self.android_req_key = Some(random_fragment(16));
        }
        if self.android_key_data.is_none() {
            self.android_key_data = Some([
                random_keydata(),
                random_keydata(),
                random_keydata(),
                random_keydata(),
            ]);
        }
        self.android_req_key.clone().unwrap()
    }

        pub fn key_data(&mut self) -> [String; 4] {
        if let Some(kd) = &self.android_key_data {
            return kd.clone();
        }
        if self.four.is_none() {
            self.four = Some(key_data_four_from_ms(now_ms() as f64));
        }
        [
            KEYDATA_ONE.to_string(),
            KEYDATA_TWO.to_string(),
            KEYDATA_THREE.to_string(),
            self.four.clone().unwrap(),
        ]
    }

    /// 测试注入固定 Four。
    #[allow(dead_code)]
    pub fn with_four(four: &str) -> Self {
        Self { four: Some(four.to_string()), android_req_key: None, android_key_data: None }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BuiltEnvelope {
    /// 外层信封 JSON（按指定键序）。
    pub json: String,
    /// inner container 原文字节（h 的输入）。
    pub container: Vec<u8>,
    pub req_key: String,
    pub ts_ms: i64,
    pub key_data: [String; 4],
}

pub fn build_envelope(session: &mut EnvelopeSession, plain: &str, order: OuterOrder) -> BuiltEnvelope {
    build_envelope_ts(session, plain, order, now_ms())
}

pub fn build_envelope_ts(
    session: &mut EnvelopeSession,
    plain: &str,
    order: OuterOrder,
    ts_ms: i64,
) -> BuiltEnvelope {
    let key_data = session.key_data();
    let container_data = B64.encode(plain.as_bytes());
    let container = serialize_container(&container_data, ts_ms, &key_data);
    let rk = req_key();
    let d = B64.encode(aes128_cbc_encrypt(rk.as_bytes(), &container).unwrap_or_default());
    let h = md5_hex(&container);
    let k = {
        let pk = rsa_public_key().expect("内置 RSA 公钥解析失败");
        let mut rng = rand::thread_rng();
        let ct = pk
            .encrypt(&mut rng, rsa::Pkcs1v15Encrypt, rk.as_bytes())
            .unwrap_or_default();
        B64.encode(ct)
    };
    let json = serialize_envelope_fields(&d, &h, &k, order);
    BuiltEnvelope { json, container, req_key: rk, ts_ms, key_data }
}

/// 【逆向实测】Android 链信封（易盾 NetSecKit 4.2.4 口径）。
///
/// 与 iOS 的 `build_envelope_ts` 有 **6 处硬差异**（依据：
/// `已有分析\native\envelope\envelope-review\docs\envelope-full-report.md` 的 unidbg 实测 +
/// `信封格式对比-iOS-vs-Android.md`）：
///
/// | 项 | iOS | Android（本函数） |
/// |---|---|---|
/// | `platform` | 1 | **2** |
/// | `timeStamp` | 毫秒 | **秒**（10 位） |
/// | `h` | MD5(container) | **MD5(container + "\n")** |
/// | `p` / `t` | 数字 101/0 | **字符串 "101"/"0"** |
/// | RSA 公钥 | `b9a6a953…` | **`ccb3a6c5…`**（`RSA_PUB_PEM_ANDROID`） |
/// | 容器键序 | 插入序 | **字典序**（serde_json Map 默认即字典序，天然满足） |
///
/// 仍需外部注入的运行时量（**本函数无法生成**）：
/// - `reqKey`：Android 实测是**每进程随机 16B**，随机源未知（不是 /dev/urandom）；
/// - `keyDataOne..Four`：Android 实测是**会话随机短串**（长度 8–12，字符集含 `^ ' ,`），
///   不是 iOS 那四个常量。
pub fn build_envelope_android(
    req_key: &str,
    key_data: [String; 4],
    plain: &str,
    out_order: OuterOrder,
    ts_sec: i64,
) -> BuiltEnvelope {
    let container_data = B64.encode(plain.as_bytes());
    let container = serialize_container_android(&container_data, ts_sec, &key_data);
    let d = B64.encode(aes128_cbc_encrypt(req_key.as_bytes(), &container).unwrap_or_default());
    // Android：h 覆盖 container + 一个换行符（lab 结论）。
    // 若服务端始终返回 HTTP200+空体，可用 MRT_H_NO_LF=1 试「不加换行」这一变体。
    let h = if env_flag("MRT_H_NO_LF") {
        md5_hex(&container)
    } else {
        let mut h_input = container.clone();
        h_input.push(b'\n');
        md5_hex(&h_input)
    };
    let k = {
        let pk = match rsa_public_key_android() {
            Ok(v) => v,
            Err(_) => return BuiltEnvelope {
                json: String::new(),
                container,
                req_key: req_key.to_string(),
                ts_ms: ts_sec,
                key_data,
            },
        };
        let mut rng = rand::thread_rng();
        let ct = pk
            .encrypt(&mut rng, rsa::Pkcs1v15Encrypt, req_key.as_bytes())
            .unwrap_or_default();
        B64.encode(ct)
    };
    let json = serialize_envelope_fields_android(&d, &h, &k, out_order);
    BuiltEnvelope { json, container, req_key: req_key.to_string(), ts_ms: ts_sec, key_data }
}

/// Android 内层容器：字典序 + `platform=2` + `timeStamp` 用**秒**。
pub fn serialize_container_android(data_b64: &str, ts_sec: i64, key_data: &[String; 4]) -> Vec<u8> {
    let mut m = serde_json::Map::new();
    m.insert("data".into(), serde_json::Value::String(data_b64.to_string()));
    m.insert("keyDataFour".into(), serde_json::Value::String(key_data[3].clone()));
    m.insert("keyDataOne".into(), serde_json::Value::String(key_data[0].clone()));
    m.insert("keyDataThree".into(), serde_json::Value::String(key_data[2].clone()));
    m.insert("keyDataTwo".into(), serde_json::Value::String(key_data[1].clone()));
    m.insert("platform".into(), serde_json::Value::from(2));
    m.insert("timeStamp".into(), serde_json::Value::from(ts_sec));
    serde_json::to_vec(&serde_json::Value::Object(m)).unwrap_or_default()
}

/// Android 外层信封：`d,h,k,p,t`，`p`/`t` 默认是**字符串**（lab 结论）。
///
/// 若服务端始终返回 HTTP200+空体，可用 `MRT_PT_NUM=1` 试「数字」变体。
pub fn serialize_envelope_fields_android(d: &str, h: &str, k: &str, order: OuterOrder) -> String {
    let (p, t) = if env_flag("MRT_PT_NUM") {
        ("101".to_string(), "0".to_string())
    } else {
        ("\"101\"".to_string(), "\"0\"".to_string())
    };
    match order {
        OuterOrder::Observed => {
            format!("{{\"k\":\"{k}\",\"p\":{p},\"d\":\"{d}\",\"h\":\"{h}\",\"t\":{t}}}")
        }
        OuterOrder::Insert => {
            format!("{{\"d\":\"{d}\",\"h\":\"{h}\",\"k\":\"{k}\",\"p\":{p},\"t\":{t}}}")
        }
    }
}

/// 读环境变量开关（"1" 为真）。
fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().as_deref() == Some("1")
}

/// Android 链 RSA 公钥（模数 `ccb3a6c5…`）。
pub fn rsa_public_key_android() -> Result<rsa::RsaPublicKey, String> {
    use rsa::pkcs8::DecodePublicKey;
    rsa::RsaPublicKey::from_public_key_pem(RSA_PUB_PEM_ANDROID)
        .map_err(|e| format!("Android RSA 公钥解析失败: {e}"))
}

/// inner container：固定键序紧凑 JSON（UTF-8 字节）。
pub fn serialize_container(data_b64: &str, ts_ms: i64, key_data: &[String; 4]) -> Vec<u8> {
    let mut m = serde_json::Map::new();
    m.insert("data".into(), serde_json::Value::String(data_b64.to_string()));
    m.insert("timeStamp".into(), serde_json::Value::from(ts_ms));
    m.insert("platform".into(), serde_json::Value::from(1));
    m.insert("keyDataOne".into(), serde_json::Value::String(key_data[0].clone()));
    m.insert("keyDataTwo".into(), serde_json::Value::String(key_data[1].clone()));
    m.insert("keyDataThree".into(), serde_json::Value::String(key_data[2].clone()));
    m.insert("keyDataFour".into(), serde_json::Value::String(key_data[3].clone()));
    serde_json::to_vec(&serde_json::Value::Object(m)).unwrap_or_default()
}

/// 外层信封序列化（手工拼接保证键序，字符串值均为无需转义的 base64/hex）。
pub fn serialize_envelope_fields(d: &str, h: &str, k: &str, order: OuterOrder) -> String {
    match order {
        // observed: k,p,d,h,t（真实 headerSign 样本序）
        OuterOrder::Observed => format!("{{\"k\":\"{k}\",\"p\":101,\"d\":\"{d}\",\"h\":\"{h}\",\"t\":0}}"),
        // insert: d,h,k,p,t（body 插入序）
        OuterOrder::Insert => format!("{{\"d\":\"{d}\",\"h\":\"{h}\",\"k\":\"{k}\",\"p\":101,\"t\":0}}"),
    }
}

/// 本地验证一个信封（不需要 RSA 私钥；生产未用，测试/诊断用）：
/// AES 回解 == container、h == MD5(container)、k 长度 128B。
#[allow(dead_code)]
pub fn validate_local_envelope(env: &BuiltEnvelope) -> Result<(), String> {
    if env.req_key.len() != 16 {
        return Err("reqKey UTF-8 长度不是 16B".into());
    }
    let v: serde_json::Value =
        serde_json::from_str(&env.json).map_err(|e| format!("信封 JSON 解析失败: {e}"))?;
    let d = v["d"].as_str().ok_or("信封缺 d")?;
    let h = v["h"].as_str().ok_or("信封缺 h")?;
    let k = v["k"].as_str().ok_or("信封缺 k")?;
    let plain = aes128_cbc_decrypt(env.req_key.as_bytes(), &b64_decode(d)?)?;
    if plain != env.container {
        return Err("d AES 解密结果与 inner container 不一致".into());
    }
    if h != md5_hex(&env.container) {
        return Err("h != MD5(inner container)".into());
    }
    if b64_decode(k)?.len() != 128 {
        return Err("k 解码长度 != 128B (RSA-1024)".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 实测向量。
    #[test]
    fn test_md5_vector() {
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    /// 实测向量（AES-128-CBC/IV=0/PKCS7）。
    #[test]
    fn test_aes_cbc_vector() {
        let key = b"0123456789abcdef";
        let pt = b"hello world, this is a test!!";
        let ct = aes128_cbc_encrypt(key, pt).unwrap();
        assert_eq!(
            ct.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "c4cf3b785dd429f0d80254ad853d92e1295bc6969a5f029dd83b939da9566c78"
        );
        let back = aes128_cbc_decrypt(key, &ct).unwrap();
        assert_eq!(back, pt);
    }

    /// keyDataFour：native %f 语义（毫秒 double 6 位小数取末 12 字符）。
    #[test]
    fn test_key_data_four() {
        assert_eq!(key_data_four_from_ms(1_788_958_186_123.456_8), "86123.456787");
        // 长度规则
        let four = key_data_four_from_ms(1_788_958_186_123.0);
        assert_eq!(four.len(), 12);
        assert!(four.ends_with(".000000"));
    }

    /// normalize_raw_key_data 长度规则。
    #[test]
    fn test_normalize_raw_key_data() {
        assert_eq!(normalize_raw_key_data("nhang.school"), "nhang.school");
        assert_eq!(normalize_raw_key_data("5K0E8400-E29B-11D4"), "00-E29B-11D4");
        let short = normalize_raw_key_data("abc");
        assert_eq!(short.len(), 12);
        assert!(short.starts_with("abc"));
    }

    /// 完整信封构造→反解 roundtrip（本地验证，无需网络/私钥）。
    #[test]
    fn test_envelope_roundtrip() {
        let mut session = EnvelopeSession::with_four("86123.456789");
        let plain = r#"{"runMode":1,"ruleUpdateTime":0}"#;
        let env = build_envelope(&mut session, plain, OuterOrder::Insert);
        validate_local_envelope(&env).expect("本地信封校验失败");
        assert_eq!(env.key_data[0], KEYDATA_ONE);
        assert_eq!(env.key_data[1], KEYDATA_TWO);
        assert_eq!(env.key_data[2], KEYDATA_THREE);
        // observed 键序
        let env2 = build_envelope(&mut session, plain, OuterOrder::Observed);
        assert!(env2.json.starts_with("{\"k\":\""));
        assert!(env2.json.ends_with(",\"p\":101") || env2.json.contains("\",\"p\":101,\"d\":\""));
        // insert 键序
        assert!(env.json.starts_with("{\"d\":\""));
        // 同一会话 Four 复用
        assert_eq!(env.key_data[3], env2.key_data[3]);
    }

    /// reqKey 白名单与长度。
    #[test]
    fn test_req_key() {
        for _ in 0..20 {
            let rk = req_key();
            assert_eq!(rk.len(), 16);
            assert!(rk.chars().all(|c| SALT_ALPHABET.contains(c)));
        }
    }

    /// RSA 公钥解析 + 加密长度 = 128B（RSA-1024 块）。
    #[test]
    fn test_rsa_encrypt_len() {
        let pk = rsa_public_key().unwrap();
        let mut rng = rand::thread_rng();
        let ct = pk.encrypt(&mut rng, rsa::Pkcs1v15Encrypt, b"0123456789abcdef").unwrap();
        assert_eq!(ct.len(), 128);
    }
}
