# Miempire's RUN TOOL API 文档

本文档按当前仓库代码整理，覆盖客户端实际访问的全部服务端接口（RUN 业务域、DISCOVERY 排行域、Geetest GT4、OBS 对象存储）。

> 说明：以下 URL、字段名来自源代码；示例中的 uid、token、经纬度、时间戳、签名等均为**示意值**，不是真实抓包。实际请求体是加密信封，示例同时给出「信封外层」与「解密后的业务 JSON」两种形态。

---

## 1. 基础信息

### 1.1 域名

| 常量 | 值 | 用途 |
|---|---|---|
| `HOST` | `https://run.gxapp.iydsj.com` | 登录、跑步策略、点位、围栏、提交、记录、学期、AI、用户信息、OBS 临时凭证 |
| `DISCOVERY` | `https://discovery.gxapp.iydsj.com` | 排行榜、历史排行、学生排行、违规名单 |
| `GEE_HOST` | `https://gcaptcha4.geetest.com` | GT4 滑块 `/load`、`/verify` |
| `STATIC_HOST` | `https://static.geetest.com/` | GT4 背景图、滑块图静态资源 |

### 1.2 公共请求头

除 GT4 与 OBS 预签名 PUT 外，业务接口统一携带：

| 请求头 | 说明 |
|---|---|
| `Content-Type` | `application/json; charset=utf-8` |
| `User-Agent` | 登录/普通接口用 `UA_IOS`；提交跑步记录用 `UA_ANDROID` |
| `appVersion` | 登录/普通接口用 `IOS_APP_VERSION`；提交记录用 `ANDROID_APP_VERSION` |
| `headerSign` | 请求头信封 JSON，键序 `k,p,d,h,t` |
| `Authorization` | 仅登录接口：`Basic base64(username:password)` |
| `runes` | 仅提交记录：`{policy 响应 timestamp}{uid}` |
| `runef` | 仅提交记录：`{跑步记录 uuid}{startTime 毫秒}` |

`UA_*`、`*_APP_VERSION` 的常量值定义在 [header.rs](file:///c:/Users/ADMIN/Downloads/NekoSportsWorldTool-main/src/crypto/header.rs)。

### 1.3 请求信封

每次业务请求会生成 **两个信封**：

1. `headerSign`：由请求头身份信息加密，外层键序为 `k,p,d,h,t`。
2. HTTP body：由业务 JSON 加密，外层键序为 `d,h,k,p,t`。

HTTP body 实际发送内容（示意）：

```json
{"d":"q1hK...9Q==","h":"3f2a...e1","k":"Rm9v...==","p":101,"t":0}
```

`headerSign` 实际发送内容（示意）：

```json
{"k":"Rm9v...==","p":101,"d":"q1hK...9Q==","h":"3f2a...e1","t":0}
```

字段含义：

| 字段 | 说明 |
|---|---|
| `d` | inner container 的 AES-128-CBC/PKCS7 密文，Base64 |
| `h` | `MD5(inner container)`，32 位小写 hex |
| `k` | 16 字节 `reqKey` 的 RSA-1024 PKCS#1 v1.5 密文，Base64（解码后 128 字节） |
| `p` | 固定 `101` |
| `t` | 固定 `0` |

`d` 解密后得到 inner container（键序固定）：

```json
{"data":"<业务 JSON 的 Base64>","timeStamp":1793711249000,"platform":1,"keyDataOne":"nhang.school","keyDataTwo":"5K0E8400-E29","keyDataThree":"597DEA1AFB49","keyDataFour":"86123.456787"}
```

- `data`：业务 JSON 明文的 Base64。
- `timeStamp`：毫秒时间戳；body 的时间戳 = header 时间戳 + 1。
- `platform`：固定 `1`。
- `keyDataOne..Four`：响应解密所需的会话密钥材料；同一次客户端会话复用同一个 `keyDataFour`。
- 加密参数：AES-128-CBC、PKCS7、IV 全 0；`h = MD5(container)`。

### 1.4 响应信封

服务端返回加密 JSON（示意）：

```json
{"r":"<Base64 密文>","s":"<Base64 签名/校验值>","v":101}
```

客户端用请求信封中的 `keyDataOne..Four` 派生密钥，先解外层，再解业务 JSON。解密后的业务结构：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "uid": 123456,
    "token": "mock-token"
  }
}
```

判定规则：`error == 10000` 为成功，其余按失败处理；失败消息读取 `message`，缺失时读取 `msg`。业务字段查找顺序为「先顶层，再 `data` 子对象」。

### 1.5 已知业务错误码

| error | 含义 | 出现位置 |
|---|---|---|
| `10000` | 成功 | 全部接口 |
| `10003` | GT4 验证失败 | `/api/v70270/security/geevalidate`；登录会重破滑块，最多 3 轮 |
| `10121` | 设备风控（设备标识漂移等） | 登录/业务通用；代码注释明确提示 device_id 必须固定复用 |

### 1.6 会话与鉴权

- 登录成功后保存 `uid`、`token`、`unid`、`name`、`weight`、`username`、`device_id`、`profile` 到 `session.json`。
- 后续请求把 `uid`/`token` 放进请求头明文（再由信封封装），不额外传 `Authorization`。
- `uid` 缺省为 `-1`，`token` 缺省为空字符串。
- 设备身份来自 `identity.json`，`device_id` 首次生成后全生命周期复用。

---

## 2. 登录与验证

登录顺序：

```
checkGeeUse ──true──> 直接 login
     │
     └─false─> GT4 /load → /verify → geevalidate → login
```

### 2.1 查询是否可跳过验证码

- **方法/路径**：`POST /api/v65/security/checkGeeUse`
- **域名**：`HOST`
- **鉴权**：未登录态（`uid=-1`、`token=""`）

发送（解密前业务 JSON）：

```json
{
  "username": "demo_user",
  "uuid": "0f8fad5b-d9cb-469f-a165-70867728950e",
  "unid": 0,
  "type": 2
}
```

返回（解密后）：

```json
{
  "error": 10000,
  "message": "success",
  "data": true
}
```

`data=true` 表示本次可跳过 GT4 与 geevalidate；请求异常一律按 `false` 处理，继续走验证码流程。

---

### 2.2 GT4 获取验证参数

- **方法/路径**：`GET /load`
- **域名**：`GEE_HOST`
- **Query**：`client_type=web`、`risk_type=slide`、`lang=zh`、`captcha_id=<CAPTCHA_ID_BIZ0>`、`callback=<JSONP 回调名>`
- **说明**：返回 JSONP，不是加密信封。

发送示例：

```http
GET /load?client_type=web&risk_type=slide&lang=zh&captcha_id=8c065103d81f5fd3efec8ad3e3a84c30&callback=geetest_1793711249000 HTTP/1.1
Host: gcaptcha4.geetest.com
User-Agent: Mozilla/5.0 ... Chrome/140.0.0.0 Safari/537.36
```

返回（JSONP 内 JSON，示意）：

```json
{
  "status": "success",
  "data": {
    "captcha_id": "8c065103d81f5fd3efec8ad3e3a84c30",
    "lot_number": "a1b2c3d4e5f6",
    "bg": "/captcha_v4/.../bg.png",
    "slice": "/captcha_v4/.../slice.png",
    "payload": "payload-token",
    "process_token": "process-token",
    "pow_detail": {
      "version": "1",
      "bits": 5,
      "hashfunc": "sha256",
      "datetime": "2026-11-03T12:00:00.000Z"
    }
  }
}
```

随后客户端从 `STATIC_HOST` 下载 `bg`、`slice` 两图，做缺口识别，并按 `pow_detail.bits` 计算 PoW nonce。

---

### 2.3 GT4 提交验证

- **方法/路径**：`GET /verify`
- **域名**：`GEE_HOST`
- **Query**：`payload_protocol=1`、`pt=1`、`w=<计算值>`、`callback=<JSONP 回调名>`
- **说明**：返回 JSONP。

发送示例：

```http
GET /verify?payload_protocol=1&pt=1&w=0a1b2c...&callback=geetest_1793711249000 HTTP/1.1
Host: gcaptcha4.geetest.com
User-Agent: Mozilla/5.0 ... Chrome/140.0.0.0 Safari/537.36
```

返回（示意）：

```json
{
  "status": "success",
  "data": {
    "seccode": "mock-seccode",
    "lot_number": "a1b2c3d4e5f6",
    "captcha_output": "mock-captcha-output",
    "pass_token": "mock-pass-token",
    "gen_time": "1793711249"
  }
}
```

客户端把 `lot_number`、`captcha_output`、`pass_token`、`gen_time` 作为「四凭证」提交给 `geevalidate`。

---

### 2.4 提交 GT4 验证结果

- **方法/路径**：`POST /api/v70270/security/geevalidate`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{
  "lotNumber": "a1b2c3d4e5f6",
  "captchaOutput": "mock-captcha-output",
  "passToken": "mock-pass-token",
  "genTime": "1793711249",
  "isOffline": false,
  "osType": 1,
  "businessType": 0,
  "uuid": "0f8fad5b-d9cb-469f-a165-70867728950e",
  "username": "demo_user"
}
```

返回（解密后）：

```json
{
  "error": 10000,
  "message": "验证通过",
  "data": true
}
```

失败返回 `error=10003` 时重新执行 GT4，最多 3 轮；其他错误码不阻塞登录。

---

### 2.5 登录

- **方法/路径**：`POST /api/v70100/login`
- **域名**：`HOST`
- **额外请求头**：`Authorization: Basic base64(username:password)`

发送头示例：

```http
POST /api/v70100/login HTTP/1.1
Host: run.gxapp.iydsj.com
Content-Type: application/json; charset=utf-8
User-Agent: <UA_IOS>
appVersion: <IOS_APP_VERSION>
Authorization: Basic ZGVtb191c2VyOmRlbW9fcGFzcw==
headerSign: {"k":"...","p":101,"d":"...","h":"...","t":0}
```

发送（解密前业务 JSON）：

```json
{
  "device_model": "iPhone 15",
  "os_version": "18.0",
  "mac_address": "AA:BB:CC:DD:EE:FF",
  "imei": "",
  "loginType": 0,
  "username": "demo_user",
  "password": "demo_pass",
  "uuid": "0f8fad5b-d9cb-469f-a165-70867728950e",
  "osType": "0"
}
```

返回（解密后）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "uid": 123456,
    "token": "mock-token-123456",
    "unid": "1001",
    "name": "演示用户",
    "weight": 68.0,
    "profile": {
      "schoolName": "演示学校",
      "className": "高一(3)班"
    }
  }
}
```

客户端至少校验 `uid >= 1` 且 `token` 非空；`weight` 缺省 `68.0`，`unid` 缺省 `"0"`。

---

### 2.6 登出

- **方法/路径**：`POST /api/v6/user/logout`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{}
```

返回（解密后）：

```json
{"error":10000,"message":"success"}
```

随后客户端删除本地 `session.json`。

---

## 3. 跑步业务

标准顺序：策略 → 点位 → （本地合成轨迹）→ 围栏/路线 → 提交 → OBS → 回读详情。

### 3.1 获取跑步策略

- **方法/路径**：`POST /api/v70103/runModePolicy`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{
  "runMode": 1,
  "ruleUpdateTime": 0,
  "geoFenceUpdateTime": 0,
  "selectUnid": 1001,
  "operateType": 0
}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "timestamp": 1793711249,
    "policy": 1,
    "runRuleModel": {
      "minDistance": 1000,
      "validTime": 3600
    },
    "pointList": [
      {"lat": 39.9042, "lon": 116.4074},
      {"glat": 39.9051, "glon": 116.4082}
    ]
  }
}
```

字段用途：

| 字段 | 用途 |
|---|---|
| `timestamp` | 提交记录时拼接 `runes`，并作为策略版本 |
| `policy` | 提交 body 的 `policy` |
| `runRuleModel.minDistance` | 提交 body 的 `selDistance`，缺省 `1000` |
| `runRuleModel.validTime` | 有效时长，缺省 `0` |
| 必经点数组 | 依次尝试 `pointList`、`runPointList`、`mustPointList`、`passPointList`、`mustPoints`、`passPoints`、`runPoints`、`points`、`nodeList`、`checkPointList` |

必经点优先读 `lat/lon`（BD 坐标），缺失时读 `glat/glon` 并做 GCJ→BD 转换。

---

### 3.2 获取跑步点位

- **方法/路径**：`POST /api/v560/get/1/distance/1`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{
  "sportType": 4,
  "longitude": 116.4074,
  "latitude": 39.9042,
  "sign": "<MD5(URL + 盐)>",
  "uuid": "0f8fad5b-d9cb-469f-a165-70867728950e",
  "selectedUnid": 1001,
  "runec": "<跑动上下文>",
  "runAreaId": 0
}
```

说明：

- `sign` 由「请求 URL + 内置盐」做 MD5 得到，具体拼接在代码内完成。
- `runAreaId` 为可选字段，仅在指定区域时携带。
- 点位结果本地缓存 300 秒。

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "pointsResModels": [
      {
        "id": 1,
        "name": "一号点位",
        "latitude": 39.9042,
        "longitude": 116.4074,
        "type": 1
      },
      {
        "id": 2,
        "name": "二号点位",
        "latitude": 39.9050,
        "longitude": 116.4080,
        "type": 2
      }
    ]
  }
}
```

---

### 3.3 获取跑步电子围栏

- **方法/路径**：`POST /api/v1/getGeoFenceForRun`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{"geoFenceUpdateTime":0}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "geoFences": [
      {
        "id": 1,
        "name": "操场围栏",
        "points": [
          {"lat": 39.9030, "lon": 116.4060},
          {"lat": 39.9060, "lon": 116.4060},
          {"lat": 39.9060, "lon": 116.4090},
          {"lat": 39.9030, "lon": 116.4090}
        ]
      }
    ]
  }
}
```

点位解析规则：优先 `lat/lon`；缺失时回退 `glat/glon` 并做 GCJ→BD；`(0,0)` 会被过滤。

---

### 3.4 提交跑步记录

- **方法/路径**：`POST /api/v70260/runnings/save/record`
- **域名**：`HOST`
- **请求头**：Android UA + `ANDROID_APP_VERSION` + `runes` + `runef`

发送头示例：

```http
POST /api/v70260/runnings/save/record HTTP/1.1
Host: run.gxapp.iydsj.com
Content-Type: application/json; charset=utf-8
User-Agent: <UA_ANDROID>
appVersion: <ANDROID_APP_VERSION>
headerSign: {"k":"...","p":101,"d":"...","h":"...","t":0}
runes: 1793711249123456
runef: 0F8FAD5BD9CB469FA16570867728950E1793711249000
```

发送（解密前业务 JSON，字段按实际生成顺序）：

```json
{
  "allLocJson": "",
  "sportType": 1,
  "policy": 1,
  "totalTime": 1200,
  "startTime": 1793711249000,
  "stopTime": 1793712449000,
  "getPrize": false,
  "status": 0,
  "uuid": "0F8FAD5BD9CB469FA16570867728950E",
  "uid": 123456,
  "selectedUnid": 1001,
  "selRunTime": 1200,
  "selDistance": 1000,
  "totalDis": 3000,
  "speed": 2720,
  "validDis": 3000,
  "validTime": 1200,
  "complete": true,
  "unCompleteReason": 0,
  "calorie": 180,
  "totalSteps": 3600,
  "avgStepFreq": 180,
  "useMobilityTools": 0,
  "faceCheck": 0,
  "totalAscent": 12,
  "avgPower": 150,
  "speedPerTenSec": [
    {
      "beginTime": 1793711259000,
      "distance": 24.5,
      "endTime": 1793711269000,
      "flag": 1793711249000,
      "id": 60000,
      "queueNum": 0,
      "state": 0
    }
  ],
  "stepsPerTenSec": [
    {
      "avgDiff": 0.0,
      "beginTime": 1793711259000,
      "endTime": 1793711269000,
      "flag": 1793711249000,
      "id": 60000,
      "maxDiff": 0.0,
      "minDiff": 1000.0,
      "queueNum": 0,
      "state": 0,
      "stepsNum": 30
    }
  ],
  "isUpload": false,
  "more": false,
  "latitude": 39.9042,
  "longitude": 116.4074,
  "maxRunTime": 0,
  "minSteps": 0,
  "fivePointJson": "<五点轨迹 JSON 字符串>",
  "errorCode": 0,
  "geeToken": "",
  "unauthorized": 0,
  "themeId": 0,
  "goalId": null,
  "address": "演示地址",
  "signature": "<body 签名>",
  "originalSign": "<原始签名>"
}
```

说明：

| 字段 | 说明 |
|---|---|
| `totalTime` | 总时长，秒 |
| `startTime` / `stopTime` | 毫秒时间戳 |
| `totalDis` / `validDis` | 总距离/有效距离，取整 |
| `speed` | 按 `totalTime / 距离 * 50 / 3 * 1024` 取整 |
| `avgStepFreq` | 每分钟步频，最小 1 |
| `calorie` / `avgPower` / `totalAscent` | 由轨迹、体重、海拔计算 |
| `speedPerTenSec` / `stepsPerTenSec` | 每 10 秒窗口统计，`id` 从 60000 起 |
| `fivePointJson` | 实时五点轨迹，不能为空，且会做包装结构校验 |
| `signature` / `originalSign` | 由 `crypto/sign.rs` 生成 |

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "rrid": 987654,
    "complete": true
  }
}
```

`rrid <= 0` 视为提交失败。

---

## 4. 记录、详情与学期

### 4.1 获取跑步记录列表

- **方法/路径**：`POST /api/v70230/runnings/records`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "records": [
      {
        "rrid": 987654,
        "totalDis": 3000,
        "totalTime": 1200,
        "startTime": 1793711249000,
        "complete": true,
        "calorie": 180
      }
    ]
  }
}
```

客户端按记录行读取展示字段；具体行字段以代码读取为准。

---

### 4.2 获取单条记录详情

- **方法/路径**：`POST /api/v70260/runnings/get_one_record`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{
  "rrid": 987654,
  "uuid": null,
  "calculateBadge": false
}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "rrid": 987654,
    "uuid": "0F8FAD5BD9CB469FA16570867728950E",
    "totalDis": 3000,
    "validDis": 3000,
    "totalTime": 1200,
    "totalSteps": 3600,
    "avgStepFreq": 180,
    "calorie": 180,
    "avgPower": 150,
    "totalAscent": 12,
    "startTime": 1793711249000,
    "stopTime": 1793712449000,
    "complete": true,
    "locations": [],
    "fivePointJson": "<五点轨迹>"
  }
}
```

提交成功后客户端会调用本接口回读，用于校验服务端落库结果。

---

### 4.3 学期汇总

- **方法/路径**：`POST /api/v55/runnings/recordssummary/semester`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "sname": "2026-2027-1",
    "semesterDis": 12000,
    "semesterValidDis": 12000,
    "semesterCount": 4,
    "semesterValidCount": 4
  }
}
```

---

### 4.4 获取个人学期信息

- **方法/路径**：`POST /api/v41/running/getPersonalSemesterInfo`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{"runMode":1}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "sname": "2026-2027-1",
    "needDistance": 3000,
    "completedDistance": 12000,
    "completedCount": 4,
    "isCompleted": true
  }
}
```

---

### 4.5 标记学期任务完成

- **方法/路径**：`POST /api/v44/runnings/personalSemesterCompleted`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{"rrid":987654}
```

返回（解密后，示意）：

```json
{"error":10000,"message":"success","data":true}
```

---

## 5. 用户信息

### 5.1 首页用户信息

- **方法/路径**：`GET /api/v41/user/HomePageInfo`
- **域名**：`HOST`
- **说明**：GET 请求同样携带加密信封 body。

发送示例：

```http
GET /api/v41/user/HomePageInfo HTTP/1.1
Host: run.gxapp.iydsj.com
Content-Type: application/json; charset=utf-8
User-Agent: <UA_IOS>
appVersion: <IOS_APP_VERSION>
headerSign: {"k":"...","p":101,"d":"...","h":"...","t":0}

{"d":"...","h":"...","k":"...","p":101,"t":0}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "uid": 123456,
    "name": "演示用户",
    "weight": 68.0,
    "schoolName": "演示学校",
    "className": "高一(3)班",
    "totalDistance": 12000,
    "totalCount": 4
  }
}
```

---

## 6. 排行与违规名单

以下接口使用 `DISCOVERY` 域名，但信封与鉴权方式与 `HOST` 完全相同。

### 6.1 排行榜

- **方法/路径**：`POST /api/v41/rank`
- **域名**：`DISCOVERY`

发送（解密前业务 JSON）：

```json
{
  "unid": "1001",
  "type": 1,
  "sortType": 1,
  "date": "2026-11-03",
  "gender": 0
}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "rankList": [
      {"rank": 1, "name": "张三", "distance": 5000, "duration": 1800},
      {"rank": 2, "name": "李四", "distance": 4200, "duration": 1600}
    ],
    "selfRank": {"rank": 18, "distance": 3000, "duration": 1200}
  }
}
```

---

### 6.2 历史排行

- **方法/路径**：`POST /api/v41/historyRank`
- **域名**：`DISCOVERY`

发送（解密前业务 JSON）：

```json
{
  "unid": "1001",
  "sortType": 1,
  "gender": 0,
  "pageNo": 1,
  "pageSize": 20
}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "list": [
      {"rank": 1, "name": "张三", "distance": 5000, "date": "2026-10-30"},
      {"rank": 2, "name": "李四", "distance": 4200, "date": "2026-10-29"}
    ],
    "total": 120
  }
}
```

---

### 6.3 学生室内排行

- **方法/路径**：`POST /api/v43/runnings/indoor/studentRank`
- **域名**：`DISCOVERY`

发送（解密前业务 JSON）：

```json
{
  "pageNum": 1,
  "pageSize": 20,
  "gender": 0,
  "dateRange": "2026-09-01,2026-11-03"
}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "list": [
      {"rank": 1, "name": "王五", "distance": 6000, "duration": 2000}
    ],
    "total": 35
  }
}
```

---

### 6.4 违规名单

- **方法/路径**：`POST /api/v78/cheat/cheatlist`
- **域名**：`DISCOVERY`

发送（解密前业务 JSON）：

```json
{
  "pageNum": 1,
  "pageSize": 20,
  "unid": "1001"
}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "list": [
      {
        "uid": 200001,
        "name": "违规用户",
        "reason": "轨迹异常",
        "date": "2026-10-20"
      }
    ],
    "self": {
      "uid": 123456,
      "isCheat": false
    }
  }
}
```

---

## 7. AI 运动

### 7.1 获取 AI 项目列表

- **方法/路径**：`GET /api/v1/sport/ai/list`
- **域名**：`HOST`
- **说明**：GET 同样携带信封 body；成功结果会永久缓存到 `ai_sports.json`。

发送示例：

```http
GET /api/v1/sport/ai/list HTTP/1.1
Host: run.gxapp.iydsj.com
Content-Type: application/json; charset=utf-8
User-Agent: <UA_IOS>
appVersion: <IOS_APP_VERSION>
headerSign: {"k":"...","p":101,"d":"...","h":"...","t":0}

{"d":"...","h":"...","k":"...","p":101,"t":0}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": [
    {"sportId": 1, "name": "开合跳", "unit": "次", "icon": "https://..."},
    {"sportId": 2, "name": "深蹲", "unit": "次", "icon": "https://..."}
  ]
}
```

---

### 7.2 上传 AI 运动记录

- **方法/路径**：`POST /api/v65/sport/ai/record/upload`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{
  "sportId": 1,
  "type": 1,
  "score": 95,
  "timeConsume": 60,
  "speed": 1.58,
  "consume": 12.5,
  "scoreDate": "2026-11-03",
  "uuid": "0F8FAD5BD9CB469FA16570867728950E",
  "taskId": 0
}
```

返回（解密后，示意）：

```json
{"error":10000,"message":"success","data":{"id":10001}}
```

---

### 7.3 获取 AI 记录列表

- **方法/路径**：`GET /api/v66/sport/ai/record/infos?sportId=&pageSize=&pageNum=1`
- **域名**：`HOST`

发送示例：

```http
GET /api/v66/sport/ai/record/infos?sportId=1&pageSize=20&pageNum=1 HTTP/1.1
Host: run.gxapp.iydsj.com
Content-Type: application/json; charset=utf-8
User-Agent: <UA_IOS>
appVersion: <IOS_APP_VERSION>
headerSign: {"k":"...","p":101,"d":"...","h":"...","t":0}

{"d":"...","h":"...","k":"...","p":101,"t":0}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "list": [
      {"id": 10001, "sportId": 1, "score": 95, "timeConsume": 60, "scoreDate": "2026-11-03"}
    ],
    "total": 1
  }
}
```

---

### 7.4 获取单条 AI 记录

- **方法/路径**：`GET /api/v66/sport/ai/record/info?id=<记录 id>`
- **域名**：`HOST`

发送示例：

```http
GET /api/v66/sport/ai/record/info?id=10001 HTTP/1.1
Host: run.gxapp.iydsj.com
Content-Type: application/json; charset=utf-8
User-Agent: <UA_IOS>
appVersion: <IOS_APP_VERSION>
headerSign: {"k":"...","p":101,"d":"...","h":"...","t":0}

{"d":"...","h":"...","k":"...","p":101,"t":0}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "id": 10001,
    "sportId": 1,
    "sportName": "开合跳",
    "score": 95,
    "timeConsume": 60,
    "speed": 1.58,
    "consume": 12.5,
    "scoreDate": "2026-11-03"
  }
}
```

---

### 7.5 获取 AI 项目详情

- **方法/路径**：`GET /api/v1/sport/ai/info?sportId=<项目 id>`
- **域名**：`HOST`

发送示例：

```http
GET /api/v1/sport/ai/info?sportId=1 HTTP/1.1
Host: run.gxapp.iydsj.com
Content-Type: application/json; charset=utf-8
User-Agent: <UA_IOS>
appVersion: <IOS_APP_VERSION>
headerSign: {"k":"...","p":101,"d":"...","h":"...","t":0}

{"d":"...","h":"...","k":"...","p":101,"t":0}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "sportId": 1,
    "name": "开合跳",
    "unit": "次",
    "description": "AI 计数项目",
    "icon": "https://..."
  }
}
```

---

## 8. OBS 对象存储

提交记录后，轨迹或扩展数据会先申请 OBS 预签名 URL，再直接 PUT 上传，最后 GET 回读校验。

### 8.1 申请临时上传地址

- **方法/路径**：`POST /api/obs/temporary/url`
- **域名**：`HOST`

发送（解密前业务 JSON）：

```json
{
  "bucketName": "iydsj-hbase-hot",
  "objectKey": "run/2026/11/03/0F8FAD5BD9CB469FA16570867728950E.json",
  "method": "Put",
  "contentType": "application/json"
}
```

返回（解密后，示意）：

```json
{
  "error": 10000,
  "message": "success",
  "data": {
    "signedUrl": "https://iydsj-hbase-hot.obs.cn-north-4.myhuaweicloud.com/run/...?AWSAccessKeyId=...&Expires=...&Signature=..."
  }
}
```

### 8.2 上传对象

- **方法**：`PUT <data.signedUrl>`
- **请求头**：`Content-Type: application/json`
- **Body**：待上传 JSON 字节

```http
PUT <signedUrl> HTTP/1.1
Content-Type: application/json

{"rrid":987654,"points":[...]}
```

返回：

```http
HTTP/1.1 200 OK
Content-Length: 0
```

### 8.3 回读对象

- **方法**：`GET <signedUrl>`（或对应的读取地址）
- **返回**：上传的 JSON 原文。

---

## 9. 接口总览

| # | 方法 | 路径 | 域名 | 用途 |
|---|---|---|---|---|
| 1 | POST | `/api/v65/security/checkGeeUse` | HOST | 探测是否可跳过验证码 |
| 2 | GET | `/load` | GEE_HOST | GT4 获取验证参数与图片 |
| 3 | GET | `/verify` | GEE_HOST | GT4 提交滑块并取四凭证 |
| 4 | POST | `/api/v70270/security/geevalidate` | HOST | 提交 GT4 四凭证 |
| 5 | POST | `/api/v70100/login` | HOST | 账号密码登录 |
| 6 | POST | `/api/v6/user/logout` | HOST | 登出 |
| 7 | POST | `/api/v70103/runModePolicy` | HOST | 跑步策略与必经点 |
| 8 | POST | `/api/v560/get/1/distance/1` | HOST | 跑步点位 |
| 9 | POST | `/api/v1/getGeoFenceForRun` | HOST | 电子围栏 |
| 10 | POST | `/api/v70260/runnings/save/record` | HOST | 提交跑步记录 |
| 11 | POST | `/api/v70230/runnings/records` | HOST | 记录列表 |
| 12 | POST | `/api/v70260/runnings/get_one_record` | HOST | 记录详情 |
| 13 | POST | `/api/v55/runnings/recordssummary/semester` | HOST | 学期汇总 |
| 14 | POST | `/api/v41/running/getPersonalSemesterInfo` | HOST | 个人学期信息 |
| 15 | POST | `/api/v44/runnings/personalSemesterCompleted` | HOST | 学期任务完成 |
| 16 | GET | `/api/v41/user/HomePageInfo` | HOST | 首页用户信息 |
| 17 | GET | `/api/v1/sport/ai/list` | HOST | AI 项目列表 |
| 18 | POST | `/api/v65/sport/ai/record/upload` | HOST | 上传 AI 记录 |
| 19 | GET | `/api/v66/sport/ai/record/infos` | HOST | AI 记录列表 |
| 20 | GET | `/api/v66/sport/ai/record/info` | HOST | AI 记录详情 |
| 21 | GET | `/api/v1/sport/ai/info` | HOST | AI 项目详情 |
| 22 | POST | `/api/v41/rank` | DISCOVERY | 排行榜 |
| 23 | POST | `/api/v41/historyRank` | DISCOVERY | 历史排行 |
| 24 | POST | `/api/v43/runnings/indoor/studentRank` | DISCOVERY | 学生室内排行 |
| 25 | POST | `/api/v78/cheat/cheatlist` | DISCOVERY | 违规名单 |
| 26 | POST | `/api/obs/temporary/url` | HOST | OBS 临时上传地址 |
| 27 | PUT | `<signedUrl>` | OBS | 上传对象 |
| 28 | GET | `<signedUrl>` | OBS | 回读对象 |

---

## 10. 备注

1. 所有 `HOST` / `DISCOVERY` 业务接口都走同一套信封链；`DISCOVERY` 只是域名不同，鉴权与加密一致。
2. GET 接口并非无 body：客户端会照常发送加密信封 body。
3. 请求头信封与 body 信封的时间戳不同，body 固定比 header 大 1 毫秒。
4. 响应解密依赖请求信封的 `keyDataOne..Four`，因此不能用另一会话的密钥解当前响应。
5. 提交记录使用 Android 设备形态（`os_version=14`、`device_name=22081212C`）与 Android UA/appVersion，其余接口使用 iOS 形态。
6. `device_id` 必须跨请求固定复用；每次随机会被服务端判定为设备漂移并触发 `10121` 风控。
7. 本文档字段以代码读取与构造顺序为准；服务端可能返回更多未使用字段，客户端会忽略。
