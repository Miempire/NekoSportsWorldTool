# 运动世界（SWCampus）逆向总结与过程记录

> 文件名：pojie-sum-1.md
> 目标 App：运动世界校园版（`com.zjwh.android_wh_physicalfitness`，协议代号 NekoSportsWorld / SWCampus）
> 记录时间：2026-11
> 性质：自有账号 / 自有设备的协议兼容性研究与工具实现（Rust 重写）
> 本文只做**过程与结论归纳**，具体字段口径以 `session.txt` 与 `neko-re-sources` 为准。

---

## 一、总目标与产物

目标：把「运动世界校园版」的跑步记录提交协议逆向清楚，用 Rust 重写一套跨平台工具
（Windows / macOS / Linux / Android），能在**自有账号**上复现登录 → 策略 → 点位 →
轨迹生成 → 提交 → OBS 上传 → 详情校验的完整链路。

最终产物：

| 仓库 | remote | 定位 |
|---|---|---|
| `E:\下载\NekoSportsWorldTool-main` | `Miempire/NekoSportsWorldTool` | GUI + CLI 主仓库（v1.4.3） |
| `D:\client` | `Miempire/MiempiresRunTool` | 客户端分支（带配速/海拔曲线，Android 发布） |
| `E:\下载\neko-re-sources-v0.6.32` | 本地存档 | 反编译源码 + 协议文档 + 参考实现（只读） |

---

## 二、逆向方法论（过程经验）

### 2.1 材料来源与分层

逆向的核心难点是：Java 能反编译的部分只到"业务实体 + HTTP 头拼装"，真正的加解密与
信封构造被 **ollvm / native `.so`** 吃掉。所以材料分三层、逐层对齐：

| 层 | 来源 | 能拿到什么 |
|---|---|---|
| Java 业务层 | jadx `com/zjwh/android_wh_physicalfitness` | 实体字段声明序、HTTP 头 key、房间/分段/五点数据库字段、openstreetmap 路网处理 |
| native 层 | 反汇编 + 字符串交叉引用 | 信封算法、RSA/AES 常量、符号 |
| 抓包层 | Xposed EnvCap hook `Cipher` / 拦截器 / OBS 构造器 | 明文请求/响应、OBS 11 键包体、预签名 URL 四元组 |

关键经验：

1. **jadx 会把混淆方法名变成 `OooO00o` 之类**，要结合 smali 逐指令核对 jadx 还原误差，
   不能只看 jadx 伪 Java。
2. **字符串常量能反，算法要重实现核对**：拿到 `H4sIAAAAAAAA` 说明是 gzip 标准头（mtime=0）。
3. **抓包优先于猜测**：任何字段取值/单位，没抓包样本就标"未验证"，绝不脑补。
4. **真机 sample 只代表"某一次"**：例如 `isRoot` 是字符串 "0"、`totalDis` 是截断而非四舍五入，
   都来自具体样本，要区分"这是通用规则"还是"这一次的值"。

### 2.2 分层验证顺序（避免来回返工）

```
1) 信封能加密 + 响应能解密      （否则后面全白搭）
2) 登录拿到 uid/token/unid      （会话凭证）
3) 策略 policy 拿到距离/时长/配速窗口/必经点
4) 点位 points 拿到五点坐标
5) 提交 submit 返回 rrid/uuid
6) OBS 上传轨迹明细
7) 详情校验 get_one_record 核对
```

每一层都是依赖下一层的"卡点"，先打通链路再优化字段口径。

---

## 三、请求分层结构（核心结论）

```
HTTP 头                 UA / appVersion / headerSign（+ Android 23 键明文头）
  └ body 信封            加密容器 + 校验字段（d/h/k/p/t 键族）
      └ 容器内明文        业务 data + 会话密钥材料（data,timeStamp,platform,keyData1..4）
          └ 业务 JSON     login / policy / points / submit / ...
  └ OBS                  POST 换预签名 URL → PUT gzip 对象（轨迹明细）
响应                    {r,s,v} 加密壳 → 解密 → 业务 data
```

---

## 四、加解密与信封（逆向结论）

来源：`src/crypto/envelope.rs` 头部注释 + 反汇编。

**信封（请求）**

| 项 | 结论 |
|---|---|
| inner container 固定键序 | `data, timeStamp, platform, keyDataOne..Four` |
| `keyDataOne` | `com.wanhang.school` 末 12 字符 |
| `keyDataTwo` | `5K0E8400-E29B-11D4-A716-4G6RW65G544F` 前 12 字符 |
| `keyDataThree` | `DE6AED50-2B3A-5327-ACED-597DEA1AFB49` 末 12 字符 |
| `keyDataFour` | `format!("{:.6}", now_ms_f64)` 末 12 字符（仿 native `%f`） |
| `d` | AES-128-CBC / PKCS7 / IV=0（reqKey 加密） |
| `h` | MD5(container) |
| `k` | RSA-1024 PKCS1v15 加密 16B reqKey |

**响应**

- 加密壳，外层 `{r,s,v}`，解密与验签在 `src/crypto/decrypt.rs`。
- 解不出时优先怀疑**请求侧字段**（版本号/头/信封），而不是算法本身。

**平台身份（重要坑）**

- 同一会话内 platform（iOS/Android）不能变，否则 UA / 头部字段自相矛盾。
- Android 时间戳是毫秒（`HttpUtil.java:83` `System.currentTimeMillis()`），
  之前"Android 秒级"的假设被推翻。

**外发请求（平台差异）**

| 端点 | 请求方式 |
|---|---|
| login | `Authorization: Basic` + 信封 body |
| geevalidate | 信封 body + headerSign |
| 一般业务 | 信封 body |

---

## 五、请求头（Android 真机 23 键）

来源：`src/crypto/header.rs` + 反编译 `HttpUtil.getHeaderMap`。

结论要点（全部是字符串值）：

- `osType="0"`；`appVersion` 必须与 UA、headerSign 内三者一致。
- `DeviceId` = 32 位 hex 小写无连字符；非 hex 持久值用 MD5 派生，跨请求稳定。
  （注意：Java 层 `CacheDataUtils` 里 DeviceId 实际取的是 **OAID** `deviceOAID`，不是 UUID。）
- `logicPixel = physicPixel / 3`（真机 400x805 / 1200x2416）。
- `isRoot` 是字符串 "0"（不是 bool）。
- `IMEI / androidId / blMac / wifiMac / CustomDeviceId` 全空串（真机权限受限）。
- `nonce` = 32 位小写 UUID（Java `UUID.randomUUID().toString()` 原生就是小写）。
- `appUpdateTime = appInstallTime + 26.8 天`（常量 `ANDROID_UPDATE_DELAY_MS`）。
- `tokenSign` = MD5(排序后的 token 参数 + 固定后缀)。

**iOS 头**：没有真机样本，保持 iOS 形态未验证（详见 session.txt U1）。

---

## 六、GT4 滑块（geetest v4）与验证码

来源：`src/api/gt4.rs`、`src/api/account.rs`、`src/api/login.rs`。

### 6.1 求解链路

```
/load → 拿 challenge/lot/c
  ├─ Canny 边缘检测（图像处理）
  ├─ PoW（pow_msg = version|bits|hashfunc|datetime|captchaId|lot||nonce）
  ├─ w（轨迹）+ /verify
  └─ 产出四凭证 {lotNumber, captchaOutput, passToken, genTime}
```

- `pow_msg` 拼接顺序固定，nonce 参与哈希，不能写错。
- `geevalidate` body 带 `businessType`、`osType`、`uuid` 等；不同场景 captchaId 不同。

### 6.2 captchaId（重要坑）

| 场景 | captchaId | 结论 |
|---|---|---|
| 登录 | `8c065103d81f5fd3efec8ad3e3a84c30` | 正式使用（`login.rs CAPTCHA_ID_BIZ0`） |
| 登录（错误尝试） | `3b02ad39bd099fd3a8336d9347a189ab` | 会导致 geevalidate 返回 10003，**勿用** |
| 发短信/注册 | `d30b3493dd08ff6aedc850648e7e0466` | `REGISTER_CAPTCHA_ID` |

经验教训：`account.rs` 里的 `LOGIN_CAPTCHA_ID` 常量一度写成 `3b02ad39…`，导致
geevalidate 10003；最终回退到 `8c065103…` 才通过。**验证码 captchaId 一定按场景对应，
不能混用。**

### 6.3 发验证码（注销）

```
GT4(REGISTER) → /api/v70270/security/geevalidate（businessType=1, osType=0）
  → /api/v75/users/check/code（type=5 注销码）
注销：/api/v61300/user/userLogout，body {code}
```

注意：发码前先过 GT4，服务端把校验结果绑定到 `uuid`，之后同一个 `uuid` 才能发码。

---

## 七、登录 / 注销 / 账号

- `POST /api/v70100/login`（Basic + 信封）→ 返回 `uid / token / unid / name / weight / device_id / profile`。
- `POST /api/v70270/security/geevalidate`：登录的滑块凭证校验。
- `POST /api/v65/security/checkGeeUse`：登录前探测是否要走人机验证。
- `POST /api/v6/user/logout`。
- `POST /api/v61300/user/userLogout`：强制注销（验证码不可逆），body 只有 `{code}`。

---

## 八、策略 / 点位 / 围栏

| 端点 | 作用 |
|---|---|
| `POST /api/v70103/runModePolicy` | 跑步策略：距离/时长/配速窗口/区域/围栏/必经点 |
| `POST /api/v560/get/1/distance/1` | 校园点位（sportType=4，距离参数会随 policy 变 `distance/2000`） |
| `POST /api/v1/getGeoFenceForRun` | 电子围栏 + `runAreaId`（可能缺省） |

逆向结论：

1. **绿围栏可缺省**：没有围栏的学校 `fence` 是 null，不是错误。
2. **点位不能少报**：前端渲染依赖服务端存的点位集合，少报直接表现"点不显示"。
3. **`isFixed` 必须原样透传**（硬点=1 / 普通点=0），不要按模式改写。
4. **`isPass` 不能兜底 true**：服务端按轨迹复核，假 true 比 false 更糟。
5. 路径里的 `distance/<米>` 是接口版本 + 距离基准，升级时要核对；原来写死 `1` 会点位拉不全。
6. 围栏坐标是 BD-09；服务端 gLat/gLng 用 GCJ-02（上传时转一次，只转一次）。
7. **`runAreaId / geoFences / freedomShowFence`** 三件套由围栏接口带出，校园跑必须带进提交体与 OBS，
   否则服务端关联不到点位配置。围栏失败回落默认值（-1/[]/false）= 自由跑口径。

---

## 九、提交跑步记录（save/record）

端点 `POST /api/v70260/runnings/save/record`，body 45 键，其中 31 键参与签名。

### 9.1 固定值（真机 7.3.93 抓包）

```
sportType=1(校园)   roomId=0          latitude=0.0    longitude=0.0
more=true           goalId=0          themeId=-1      status=0
isUpload=false      unauthorized=0    useMobilityTools=0    faceCheck=0
segmentJson=""      allLocJson=""     speedPerTenSec=[]   stepsPerTenSec=[]
```

### 9.2 计算值

| 字段 | 结论 |
|---|---|
| totalDis / validDis | **截断**取整（93.90→93），不是四舍五入 |
| speed | 配速(分/公里) × 1000（Android/iOS 一致；之前 iOS 曾误乘 1024 或 km/h×1024，已修正） |
| avgStepFreq | totalSteps / totalTime × 60，最小 1 |
| stopTime | start + totalTime×1000 + 1~5s 暂停抖动（uuid 派生，稳定不漂移） |
| startTime | 计划起点（可被 0~4s 随机偏移） |

### 9.3 序列化顺序（经验）

- 反编译 `UploadFormatEntity.java` 字段声明是**字母序**，Gson 按声明序输出。
- 工具用 `sorted_body_json()` 按 key 排序后进信封；签名与 JSON 文本顺序无关（内部自行排序）。

---

## 十、签名（31 字段，逆向关键结论）

来源：`src/crypto/sign.rs` + `neko-re-sources/native/reimpl-rust/sign.rs`（逐字段对齐）。

- 盐：`2slhe02lsfiwowlcixisla_sls-_slaor`
- 字段固定顺序 31 个（UploadSignEntity 声明序），`sportType=3` 时追加 `roomId`。
- 字符串化：String 原值；null→"null"；bool→"true"/"false"；数字→to_string。
- `originalSign` = 31 项按 key **自然序**拼 `k=v&k=v…`（无 MD5、无盐）。
- `signature` = 同一批项按 key **忽略大小写排序**拼接 + 盐，再 MD5。

踩坑：

1. 用 JSON 序列化顺序去签名是错的（必须排序）。
2. `goalId` null vs 0 会改签名串（正式口径是 0）。
3. `roomId` 只在 sportType=3 参与签名，别无条件加入。

---

## 十一、OBS 对象（轨迹明细）

### 11.1 结构（11 键，每值 gzip + Base64 标准表）

真机 `o0OO00O` 的 Map.put 序：

```
rrid, uid, uuid, run_data, step_freq_json, speed_json, segment_json,
fixed_point_json, runFaceCheck, extension_json, laps_json
```

- `run_data` 内层 `{allLocJson:"<点数组 JSON 字符串>", useZip:false}`。
- `run_data` 单点 **27 键**（**没有 steps**；带 steps 的 28 键是 Niko 旧口径，错误）。
- `segment_json` 单段 11 键（真机 `AnalysisStateDB`）；`avgSpeed` 是配速(分/公里)。
- `laps_json` 单圈 14 键，`avgPace` 分/公里、`avgStride` 厘米。
- `runFaceCheck` / `extension_json` 无抽检时为空串 gzip。
- `fixed_point_json` 内层 5 键：`fivePointJson, freedomShowFence, geoFencesJson, runAreaId, useZip`。

### 11.2 objectKey（重要口径变更）

- 官方**只 PUT 一个 key**：`run_data/<北京时间 YYYYMMDDHH>/<uuid>.json`。
- 时间目录必须用北京时间（UTC+8），不能用本机 `Local`（否则非中国时区机器目录会错）。
- 室内是 `indoor_json/...`（`ObsKind::Indoor`，接口 `/api/v68/indoor/save/record` 尚未接线）。

### 11.3 换签与 Content-Type（踩坑）

- 换签：`POST /api/obs/temporary/url`，body `{bucketName:"iydsj-hbase-hot", objectKey, method, contentType}`。
- **contentType 会进 OBS 签名**：Put 用 `text/plain`，Get 用 `application/json`。
- PUT 时 Content-Type 必须与换签一致（写错 403）。
- 之前实现写死 `application/json` 且传双 key，已修正为 text/plain + 单 key。

---

## 十二、坐标系与轨迹生成（算法侧经验）

### 12.1 坐标

- 点位/围栏 = BD-09；上传 gLat/gLng = GCJ-02；内部存 BD、出网转 GCJ **只转一次**。
- 生活点 `lat/lng=-1.0`（哨兵），`coorType="gcj02"`。

### 12.2 加密（1s 插值）

- `densify_locations`：5s 主网格（含丢点 10/15s）按 1s 插值加密。
- 相邻点之间插 `k-1` 个（`k=ceil(dt/1.0)`）；**5s 间隔插 4 个，不是固定 5 个**。
- 位置按路网/环线弧长 `pos(arc)` 插值，不是直线；漂移 offset 线性 + 0.15m 高斯噪声。
- 不插的情况：端点 `ptype=-1`、`dt<=0`、`|d_arc|<0.25`（终点堆叠段）。
- 累计量线性插值、端点不变 → 总量守恒；`count/stepDistance` 最后统一由差分重建。

### 12.3 单向路网

- `crates/route-planner` 用 petgraph **DiGraph**（有向图），A* 只走 `Outgoing`。
- OSM `oneway=yes` 只加正向边、`oneway=-1` 只加反向边、无标签按双向。
- 加密只沿已选路径取弧长、不感知方向 → 不会逆行。

### 12.4 曾有 bug 的经验

- 步数 `count` 曾是随机 20–88、`stepDistance` 恒 0，与 GPS 距离矛盾 → 改为增量 + 段距离/步数。
- 掉头后僵尸不死的类似 bug 属于另一个项目（PVZ），已另处理，不在本协议范围。

---

## 十三、设备档案（电池 / 传感器）与 geeToken

- 电池/传感器**不进跑步提交体**，只进 geeToken 的设备档案：
  - 电池 → `bs/br/plu/hea/btv/btc`（状态/电量/插电/健康/电压/容量）
  - 传感器 → `gpa`（是否有陀螺仪）
- 作用：极验/风控的**设备可信度与自洽性**，不是运动分析。
- 原则：像真机、同一 DeviceId 跨请求稳定；电压随电量、模拟器常缺传感器。
- `errorCode=-999 + geeToken=""` 默认；走完整极验链才是 200。**不要伪造 200**。
- `deviceVerify` 默认不上报（挂在 requestFivePoint.onSuccess 后，缺 upload/originalSign 预登记）。

---

## 十四、平台 / 版本 / 环境变量

| 项 | 值 |
|---|---|
| 对齐客户端版本 | 7.4.0（`--app-version` 可覆盖；抓包基准 7.3.93） |
| HOST | `https://run.gxapp.iydsj.com` |
| DISCOVERY | `https://discovery.gxapp.iydsj.com` |
| GEE_HOST | `https://gcaptcha4.geetest.com` |
| `MRT_GEETOKEN=1` | 本地生成 geeToken |
| `MRT_GEETOKEN_ONLINE=1` | 换服务端 token |
| `MRT_GEE_TOKEN` | deviceVerify 用的现成 token |
| `MRT_PORTAL_USER/PASS` | 静水空间平台授权账号 |

- 版本号是**单一来源**：HTTP 头 appVersion、UA、headerSign 三者必须一致，否则判"参数异常"。

---

## 十五、服务端观测 / 判灰相关（风险理解）

- 服务端一堆字段是**解析型**（按 key 取值），键序、JSON 空白不影响解析，只有字段值/类型才关键。
- 真正会触发风控/判灰/抽检的是"自洽性"：
  1. 步数↔GPS 距离交叉校验；
  2. 步频窗口不能是常数；
  3. 电压/电量/温度/健康度合理；
  4. 同一 DeviceId 跨请求设备档案稳定；
  5. 模拟器缺传感器/给 100% 电量/AC 充电等异常组合。
- 实际后果通常不是直接封号，而是提高风控分 → 触发滑块/人脸抽检或 geeToken 失效。

---

## 十六、已修复 / 已对齐清单（过程结果）

| 项 | 结论 |
|---|---|
| 提交体 45 键 | lat/lon=0、more=true、goalId=0、themeId=-1、roomId=0、空数组、totalDis 截断 |
| speed 口径 | 配速(分/km)×1000，iOS 与 Android 统一 |
| Android 头 23 键 | 全字符串、32hex DeviceId、小写 nonce、isRoot="0"、安装/更新时间 |
| 签名 31 字段 | 与 reimpl-rust 逐字段一致 |
| OBS 11 键 | 补 extension_json、键序对齐、单 objectKey、Put text/plain、北京时间 |
| 跑区三件套 | 围栏接口带出 runAreaId/geoFences/freedomShowFence，接入五点与 OBS |
| isPass | 40m 吸附真实判定，去掉全 true 兜底 |
| segment_json | 11 键分段（AnalysisStateDB），配速分/公里 |
| 室内 objectKey | ObsKind::Indoor / indoor_json 已就绪（提交接口未接线） |
| 步数/步幅 | count=增量、stepDistance=段距离/步数，sum 与 totalSteps 自洽 |

---

## 十七、尚未验证 / 未定案（诚实边界）

1. iOS 请求头全字段（无 iOS 抓包）。
2. 信封 platform=1 还是 2（Java 层构造被 ollvm 掉）。
3. body.latitude/longitude=0.0 在校园跑是否用于校区判定。
4. run_data 27 键点集（缺真机 allLocJson 明文逐点样本）。
5. extension_json 只确认空串 gzip。
6. 室内提交接口未接线。
7. segment_json 的 startTime/endTime 是毫秒还是秒偏移。

**原则：没样本就标未验证，不臆造。**

---

## 十八、工程与协作经验（写给后续 agent）

1. **两个仓库并行**：main（全平台）+ client（Android 发布 + 曲线），协议层文件保持逐字节一致；
   改协议要两边同步。
2. **`session.txt` 是交接文书**：17 节，含硬规则、协议速查、未验证清单、错误清单、验证流程、
   交接检查单。改代码前先读。
3. **本机无 cargo**：编译验证只能靠 GitHub Actions（main 三平台 CI + client APK）。
4. **`Cargo.toml` 版本改了必须同提交带 `Cargo.lock`**，否则 APK 的 `--locked` 构建直接失败。
5. **`Niko源码` 是旧快照**（28 键点集、双 key、position=999、isPass 恒 true），不要当运行副本，
   只作历史参照。
6. **本地资料不入库**：apis.*、抓包原文、identity/session、.trae 均 gitignore。
7. **提交后必看 CI**：main 三平台要过，APK 偶发失败可忽略但 CI 必须绿。
8. **逆向结论以 neko-re-sources 为准**：docs/contract/PROTOCOL.md + research/*.md +
   native/reimpl-rust/*.rs 是权威参考。

---

## 十九、关键参考锚点（本地）

- 协议契约：`neko-re-sources/docs/contract/PROTOCOL.md`
- 真机抓包：`…/docs/research/REAL-CAPTURE-7393.md`（7.3.93）
- 对齐审计：`…/docs/research/ALIGN-7393-AUDIT.md`
- 网络地图：`…/docs/research/APP-NETWORK-MAP.md`
- 签名参考：`…/native/reimpl-rust/sign.rs`
- 反编译 Java：`…/java/jadx-7.4.0/`（UploadFormatEntity / AnalysisStateDB / FivePoint / HttpUtil / CacheDataUtils）
- 工作区 jadx：`C:\Users\ADMIN\Documents\deepseek-harness\default-workspace\jadxout\full\sources`
  - `entity/UploadFormatEntity.java`（45 键字母序声明）
  - `entity/database/FivePoint.java`（flag=会话时间戳）
  - `entity/database/AnalysisStateDB.java`（分段 11 字段）
  - `utils/HttpUtil.java:83/114`（毫秒时间戳 / isRoot 字符串）
  - `utils/Utils.java:680`（nonce 小写 UUID）

---

## 附：声明

本文档与相关工具仅用于自有账号/自有设备的协议研究与兼容性分析；不包含原 APK 分发、
他人账号数据、绕过人机验证的成品，也不提供可用于替代真实运动的业务字段伪造逻辑。
密钥/盐/签名算法/判分阈值等属于逆向结论，仅在源码或授权材料范围内存档。