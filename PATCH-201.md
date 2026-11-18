# PATCH-201 — 数据异常 201 的四点修复

> 目标项目：`根据v7.4.0做的软件(也会风控)\NekoSportsWorldTool-main`
> 依据：`ERR-201-ANALYSIS.md`（六条成因）+ 源码实测
> 本机**没有 Rust 工具链**（`cargo`/`rustc` 都不在 PATH，`target/` 是历史编译产物），所以**未做 `cargo check`**；下面是逐处改动，语法已人工核对。

---

## 改动 1（核心）：errorCode 与 geeToken 强制配对

**文件**：`src/api/submit.rs:197-213`
**改动前**：`body.insert("errorCode", p.error_code)` —— 原样写入，不校验。
**改动后**：用 `geetoken::code_for_token()` 以 token 形态强制推导，并告警。

```rust
let paired_code = crate::geetoken::code_for_token(&p.gee_token);
if paired_code != p.error_code {
    log(&format!(
        "⚠ [submit] errorCode 与 geeToken 不配对：传入 {} 但按 token 应为 {}（已强制改为 {}）token前缀={}",
        p.error_code, paired_code, paired_code,
        p.gee_token.chars().take(4).collect::<String>()
    ));
}
body.insert("errorCode".into(), Value::from(paired_code));
```

`code_for_token`（`geetoken.rs:456-468`）的既有规则：

| geeToken | errorCode |
|---|---|
| `""` | **-999** |
| `RzAy…`（G02，服务端 token） | **200** |
| `R0VF…`（GEE，本地 token） | **-300** |
| 其它 | -999 |

→ 彻底消灭「本地 token 配 0/200」「服务端 token 配 -300」这类错配。

---

## 改动 2：不走极验时 errorCode 改为 -999

**文件**：`src/api/flow.rs:418-422`
**改动前**：`(String::new(), 0)` —— 空 token 配 0，与 `code_for_token("")` 的 -999 冲突。
**改动后**：

```rust
} else {
    // [201 修复] 不走极验时，提交体里的 errorCode 必须是 -999
    (String::new(), -999)
};
```

---

## 改动 3：device_id 为空不再随机，直接拒绝

**文件**：`src/api/submit.rs:272-282`
**改动前**：`device_id` 为空时 `uuid::Uuid::new_v4()` 随机 → 每次提交设备漂移。
**改动后**：

```rust
let device_id = if client.identity.device_id.is_empty() {
    return Err("device_id 为空：提交前必须设置固定设备 ID（否则每次随机会导致设备漂移 → 10121/数据异常）".into());
} else {
    client.identity.device_id.clone()
};
```

---

## 改动 4：提交体口径硬自查（告警不阻断）

**文件**：`src/api/submit.rs:219-263`
按源码注释里的官方口径逐项自查，日志里直接打印异常项：

- `totalDis` / `validDis` 必须是**整数**（截断不四舍五入）
- `latitude` / `longitude` 必须是 **0.0**
- `faceCheck` 必须是 **0**
- `speedPerTenSec` / `stepsPerTenSec` 必须是**空数组**
- `validTime ≤ totalTime`、`selRunTime ≤ totalTime`

通过时打印：`[submit] 口径自查通过（…）`

---

## 未改动但已核对（本来是正确顺序）

`submit.rs:265-270` 的顺序是对的：**body 全部字段写完 → 算 `signature` / `originalSign` → 再插进 body**。签名覆盖 31 字段，这两个字段自身不参与；改动 1、3、4 都发生在**算签名之前**，不会破坏签名。

---

## 验证步骤（到有 Rust 的机器上）

```powershell
cd <NekoSportsWorldTool-main>
rustup show              # 确认工具链
cargo check              # 语法/类型检查
cargo build --release    # 产出 target\release\
```

编译通过后，跑一次提交，检查日志：

1. 有没有 `⚠ [submit] errorCode 与 geeToken 不配对` —— 有就说明调用方传错了，现在已被强制改对
2. 有没有 `[submit] 口径自查通过` —— 没有就说明口径还有一项不达标
3. `[gee] scene=6 打卡回执 i/N` 的 N 是否等于跑步点数；最后一条是否为 `scene=7 提交回执`

## 还要盯的三个（本次未改，因为要业务决策）

| 项 | 现状 | 建议 |
|---|---|---|
| 极验回执只有本地 token（-300） | 服务端可能更认 `G02`（200） | 用 `client_report` 把本地 token 换服务端 token（`geetoken.rs:521` 已有函数） |
| `totalDis` 截断 vs 真实距离 | 截断会造成「距离比轨迹短」 | 若 201 仍出现，试 `round` 或与 OBS 轨迹严格一致 |
| OBS 对象上传 | 提交体里两个数组恒空，真数据在 OBS | 确认 OBS 上传成功且 `bucketName/objectKey` 与轨迹一致 |
---

# 追加：把逆向得到的 Android 真相灌进代码（第二波改动）

> 第一波只修了「提交体口径 + errorCode 配对」；这一波把**实测的 Android 链口径**真正写进信封层。

## 改动 5：新增 Android RSA 公钥（实测提取）

**文件**：`src/crypto/envelope.rs`
**来源**：`sw_app.apk` → `lib/arm64-v8a/libnetseckit-4.2.4.so`，文件偏移 **`0x171990`** 的 SPKI DER（162 B），模数 `ccb3a6c585be65ad…`、指数 65537。

```rust
pub const RSA_PUB_PEM_ANDROID: &str = "-----BEGIN PUBLIC KEY-----\n\
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDMs6bFhb5lrf/xqwtNcKVHW09c…\n\
-----END PUBLIC KEY-----";

pub fn rsa_public_key_android() -> Result<rsa::RsaPublicKey, String> { … }
```

**与 iOS 的 `RSA_PUB_PEM`（模数 `b9a6a953…`）不是同一把**。这把同时被用作响应 `s` 的验签公钥。

## 改动 6：新增 Android 信封构造（6 处口径差异全对齐）

**文件**：`src/crypto/envelope.rs`，新增 `build_envelope_android()` / `serialize_container_android()` / `serialize_envelope_fields_android()`

| 项 | iOS（原实现） | Android（新函数） |
|---|---|---|
| `platform` | 1 | **2** |
| `timeStamp` | 毫秒 | **秒** |
| `h` | MD5(container) | **MD5(container + "\n")** |
| `p` / `t` | 数字 | **字符串 "101"/"0"** |
| RSA 公钥 | `b9a6a953…` | **`ccb3a6c5…`** |
| 容器键序 | 插入序 | **字典序**（serde_json Map 默认即字典序） |

签名：

```rust
pub fn build_envelope_android(
    req_key: &str,
    key_data: [String; 4],
    plain: &str,
    out_order: OuterOrder,
    ts_sec: i64,
) -> BuiltEnvelope
```

## 改动 7：`EnvelopeSession` 支持注入 Android 运行时材料

**文件**：`src/crypto/envelope.rs`

```rust
pub struct EnvelopeSession {
    four: Option<String>,
    android_req_key: Option<String>,          // 每进程随机 16B，外部注入
    android_key_data: Option<[String; 4]>,    // 会话随机 4 段，外部注入
}

impl EnvelopeSession {
    pub fn set_android_material(&mut self, req_key: &str, key_data: [String; 4]) -> Result<(), String>;
    pub fn android_ready(&self) -> bool;
    pub fn android_req_key(&self) -> Option<&str>;
}
```

校验规则：`reqKey` 必须 **16B**；`keyData` 每段必须 **8..12B**（与 `derive_paes_key` 的 assert 一致）。

## 改动 8：`client.rs` 按平台分流（两处请求入口都改）

**文件**：`src/api/client.rs`

```rust
let (header_env, body_env) = if self.identity.platform == "android" && self.session.android_ready() {
    let rk = self.session.android_req_key().unwrap_or_default().to_string();
    let kd = self.session.key_data();
    let ts_sec = chrono::Utc::now().timestamp();
    let henv = build_envelope_android(&rk, kd.clone(), &header_plain, OuterOrder::Observed, ts_sec);
    let benv = build_envelope_android(&rk, kd, body_plain, OuterOrder::Insert, ts_sec + 1);
    (henv, benv)
} else {
    let henv = build_envelope(&mut self.session, &header_plain, OuterOrder::Observed);
    let benv = build_envelope_ts(&mut self.session, body_plain, OuterOrder::Insert, henv.ts_ms + 1);
    (henv, benv)
};
```

`raw_request()` 里的 headerSign 也加了同样的 Android 分支。

## 改动 9：`appVersion` 跟平台走（不再是硬编码 iOS）

**文件**：`src/api/client.rs`（`envelope_request` 与 `raw_request` 两处）

```rust
let app_version = if self.identity.platform == "android" { ANDROID_APP_VERSION } else { IOS_APP_VERSION };
req = req.set("appVersion", app_version);
```

> 原来 Android 身份也发 `IOS_APP_VERSION`，服务端一致性校验会直接判异常。

## 还差什么（必须外部提供，代码无法自造）

`build_envelope_android` 的两个入参必须在运行时拿到：

| 参数 | 为什么不能自造 | 从哪拿 |
|---|---|---|
| `req_key` | Android 实测**每进程随机 16B**，随机源不是 `/dev/urandom`（lab 12k 候选暴破未命中） | 设备内存（root 读 `/proc/pid/mem`）或抓包 `[AES key?]` 行 |
| `key_data[4]` | Android 实测是**会话随机短串**（8–12B，字符集含 `^ ' ,`），不是 iOS 那四个常量 | 同上，container 明文里就有 |

拿到后调用：

```rust
client.session.set_android_material("756KKPKRT78jhgJ", ["852dbd1acca","24b33b4420","852b0d9b","688'^z,T"])?;
```

之后所有请求自动走 Android 口径。

## 编译验证（到有 Rust 的机器）

```powershell
cargo check
cargo build --release
```

关键检查点：
- `rsa_public_key_android()` 能解析 PEM（`DecodePublicKey`）
- `build_envelope_android` 返回的 `json` 里 `"p":"101"`、`"t":"0"` 是字符串
- `serialize_container_android` 输出里 `"platform":2`、`"timeStamp"` 是 10 位秒

---

# 追加二：把逆向口径贯通到「提交主链路 + 响应验签 + 身份持久化」

第一波只动了 client.rs 的通用请求；但 **201 出现在「跑步记录提交」这条主线**，那是 `submit.rs` 独立拼装信封的地方，也必须改。

## 改动 10：`submit.rs` 提交主链路切到 Android 信封

**文件**：`src/api/submit.rs:289-306`
提交（`/runnings/save/record`）原来硬走 iOS 信封；现在：

```rust
let (header_env, body_env) = if client.session.android_ready() {
    let rk = client.session.android_req_key().unwrap_or_default().to_string();
    let kd = client.session.key_data();
    let ts_sec = crate::crypto::envelope::now_ms() / 1000;
    let henv = build_envelope_android(&rk, kd.clone(), &hp, OuterOrder::Observed, ts_sec);
    let benv = build_envelope_android(&rk, kd, &body_plain, OuterOrder::Insert, ts_sec + 1);
    (henv, benv)
} else {
    log("[submit] 未注入 Android 信封材料，退回 iOS 口径（服务端大概率判数据异常）");
    // …原 iOS build_envelope / build_envelope_ts…
};
```

## 改动 11：响应验签公钥换 Android 那把

**文件**：`src/api/submit.rs:324-334`、`src/api/client.rs:132-143 / 196-207`
`decrypt_response` 的第三参是「响应 `s` 的 RSA 验签公钥」。原来一律用 iOS 的 `rsa_public_key()`（`b9a6a953…`）；现在按平台：

```rust
let verify_key = if self.identity.platform == "android" {
    rsa_public_key_android()   // ccb3a6c5…
} else {
    rsa_public_key()           // b9a6a953…
};
```

（lab 实测：Android 的请求加密公钥与响应验签公钥是**同一把**，都在 `libnetseckit` 0x171990。）

## 改动 12：身份持久化新增两个运行时字段

**文件**：`src/crypto/header.rs`（`HeaderIdentity` + `Default`）

```rust
#[serde(default)]
pub android_req_key: Option<String>,          // 16B
#[serde(default)]
pub android_key_data: Option<[String; 4]>,    // 每段 8–12B
```

这两个量写进 `identity.json` 后，`ApiClient::new()` 会在构造时自动把它们灌进 `EnvelopeSession`（`src/api/client.rs:38-64`），此后**所有**请求（通用 + 提交）自动走 Android 口径，无需再逐处 set。

## identity.json 注入示例

```json
{
  "platform": "android",
  "device_id": "<32位hex小写>",
  "android_req_key": "756KKPKRT78jhgJ",
  "android_key_data": ["852dbd1acca", "24b33b4420", "852b0d9b", "688'^z,T"]
}
```

> 注意：`android_req_key` 必须是 16 字节；`android_key_data` 每段 8–12 字节（`set_android_material` 会校验，不合法则告警并退回 iOS 口径）。
> 上面用的是 iOS 那组常量做演示，**Android 真机这两个量是每进程随机的**，必须从设备内存/抓包拿到后再填，不能照抄示例。

## 完整改动文件清单

| 文件 | 改动 |
|---|---|
| `src/crypto/envelope.rs` | `RSA_PUB_PEM_ANDROID`、`rsa_public_key_android()`、`build_envelope_android()`、`serialize_container_android()`、`serialize_envelope_fields_android()`、`EnvelopeSession` 注入器 |
| `src/crypto/header.rs` | `HeaderIdentity` 加 `android_req_key`/`android_key_data` |
| `src/api/client.rs` | 通用请求/raw 请求按平台分流信封 + 响应验签换公钥 + `ApiClient::new` 自动注入 |
| `src/api/submit.rs` | 提交主链路切 Android 信封 + 响应验签换公钥 |
| `src/api/flow.rs` | 不走极验时 errorCode 改为 -999 |

## 编译 / 运行

```powershell
cargo check            # 语法/类型
cargo build --release  # 出包
```

跑一次提交后看日志：应出现 `[client] Android 信封口径已启用（reqKey 前 8 字符 = …）`；
若出现 `[submit] 未注入 Android 信封材料，退回 iOS 口径`，说明 identity.json 还没填那两个量。