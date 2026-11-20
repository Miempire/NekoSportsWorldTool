//! 全链编排：policy → 实时点位 → 轨迹生成 → 提交 → OBS → 详情验证。
//! 由 UI 后台线程调用，log 闭包回传日志。

use super::client::ApiClient;
use super::model::Session;
use super::points;
use super::policy::fetch_policy;
use super::records::fetch_one_record;
use super::submit::{submit_record, SubmitParams, SubmitResult};
use crate::location::Coordinate;
use crate::track::generate_road::RouteMode;
use crate::track::generator::TrackGenMode;
use crate::track::wire::{
    build_obs_object_with_area, five_point_wrapper_with_area, obs_keys,
};
use rand_distr::{Distribution, Normal};
use serde_json::Value;

/// 极验 scene=6 回执窗口策略（手机端可自行选择）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GeeWindowMode {
    /// 固定 60 秒：提交快，但检查点多时回执密度偏高。
    Fixed60,
    /// 对齐真实 totalTime：最接近真机节奏，但长跑提交要等整场时长。
    RealTotal,
}

impl Default for GeeWindowMode {
    fn default() -> Self {
        GeeWindowMode::Fixed60
    }
}

#[derive(Clone)]
pub struct RunParams {
    /// 距离（米）与时长（秒）已由 UI 参数解析。
    pub dist: f64,
    pub dur: i64,
    /// 开始时间（毫秒）。
    pub start_ms: i64,
    pub face_check: i64,
    /// 用户手动填写的绝对海拔（米）；None 使用生成器海拔。
    pub manual_altitude: Option<f64>,
    /// 用户手动填写的海拔范围；与单值字段兼容，范围优先。
    pub manual_altitude_range: Option<crate::track::altitude::AltitudeRange>,
    pub seed: u64,
    /// 路线算法模式。
    pub route_mode: RouteMode,
    /// 跑道模式人工确认的闭合轮廓（BD, lat/lon）；空表示未确认。
    pub track_outline: Vec<(f64, f64)>,
    /// 用户的配速范围（秒/km，两端任意顺序）；用于把瞬时速度限制在范围内。
    /// `None` 时退回生成器的历史宽窗口（1.9~6.28 m/s）。
    pub pace_range: Option<(f64, f64)>,
    /// 只经过策略必经点的前 N 个；`None`=全部，`Some(0)`=不经过。
    pub must_limit: Option<usize>,
    /// 极验回执窗口策略（60s 固定 / 对齐真实 totalTime）。
    pub gee_window_mode: GeeWindowMode,
    /// 轨迹生成模式（Current / Bowl / Niko），仅经典模式（Legacy）生效。
    pub track_mode: TrackGenMode,
    /// Bowl 海拔是否叠加起始段爬升。
    pub bowl_climb: bool,
    /// 海拔是否使用碗形剖面（与配速模式独立控制）。
    pub bowl_altitude: bool,
}

pub struct RunOutcome {
    pub result: SubmitResult,
    pub obs_ok: usize,
    pub detail_ok: bool,
}

fn sleep_secs(s: u64) {
    std::thread::sleep(std::time::Duration::from_secs(s));
}

/// 跑步全链。
pub fn run_full_flow(
    client: &mut ApiClient,
    params: &RunParams,
    log: &mut dyn FnMut(&str),
) -> Result<RunOutcome, String> {
    let sess: Session = client.login.clone().ok_or("未登录")?;

    // 极验会话回执：scene=4 在「起跑」时发（流程一开始），
    // scene=6/7 在提交前按检查点比例补发（折中：不真实等待整场跑步）。
    let geetoken_on = std::env::var("MRT_GEETOKEN").ok().as_deref() == Some("1");
    let geetoken_online = std::env::var("MRT_GEETOKEN_ONLINE").ok().as_deref() == Some("1");
    let run_uuid = uuid::Uuid::new_v4().to_string().to_uppercase();
    let mut start_receipt: (String, i64) = (String::new(), 0);
    if geetoken_on {
        start_receipt = crate::geetoken::make_token_with_mode(
            &client.identity,
            sess.uid,
            "",
            4,
            geetoken_online,
            &mut *log,
        );
        log(&format!(
            "[gee] scene=4 起跑回执 token {} 字符",
            start_receipt.0.len()
        ));
    }

    // ① policy
    log("[policy] 拉取跑步策略…");
    let pol = fetch_policy(client)?;
    log(&format!(
        "√ [policy] ts={} policy={} minDistance={} validTime={}",
        pol.timestamp, pol.policy, pol.min_distance, pol.valid_time
    ));
    sleep_secs(2);

    // ② 实时点位（拒绝本地样本兜底）
    log("[points] 拉取实时点位…");
    if client.identity.has_unconfigured_default_location() {
        return Err("请先在设备信息页填写本次跑步所在城市和定位锚点，不能使用大连默认配置".into());
    }
    let anchor: Coordinate = client.identity.anchor_coordinate()?;
    let pts = points::fetch_points(client, anchor, log)?;
    if pts.is_empty() {
        return Err("实时点位为空 —— 拒绝本地样本兜底".into());
    }
    log(&format!("√ [points] {} 个点位", pts.len()));
    for p in pts.iter().take(5) {
        log(&format!(
            "  [points] {} BD=({:.6},{:.6}) GCJ=({},{})",
            p.get("pointName").and_then(|v| v.as_str()).unwrap_or(""),
            p.get("lat").and_then(|v| v.as_f64()).unwrap_or(0.0),
            p.get("lon").and_then(|v| v.as_f64()).unwrap_or(0.0),
            p.get("glat").map(|v| v.to_string()).unwrap_or_default(),
            p.get("glon").map(|v| v.to_string()).unwrap_or_default(),
        ));
    }

    // ③ 轨迹生成（必经点 + 打卡点）
    let pts_bd = points::points_bd(&pts);
    // 必经点数量限制：只取策略顺序中的前 N 个（None=全部，0=一个都不经过）。
    let must_points: Vec<(f64, f64)> = match params.must_limit {
        Some(n) => pol.must_points.iter().take(n).copied().collect(),
        None => pol.must_points.clone(),
    };
    if let Some(n) = params.must_limit {
        log(&format!(
            "[policy] 按设置取前 {} 个必经点（策略共 {} 个）",
            n,
            pol.must_points.len()
        ));
    }
    // 必经点保持策略顺序置于前端（waypoints[0] 即起点），剩余打卡点去重后按质心角
    // 排序；道路模式下两者都作为「必须经过」的硬约束。
    let route_pts = crate::track::generate_road::merge_route_points(&pts_bd, &must_points);
    if !must_points.is_empty() {
        log(&format!(
            "√ [policy] 必经点 {} 个（保持顺序），合并后路线 waypoint 共 {} 个",
            must_points.len(),
            route_pts.len()
        ));
        // 逐点打印，便于与响应/轨迹核对：所有 must_points 都作为强制必经点，
        // 与起点重合的首点由路由层投影去重，不再被跳过。
        for (i, &(mlat, mlon)) in must_points.iter().enumerate() {
            log(&format!(
                "  [policy] must_points[{i}] BD=({mlat:.6},{mlon:.6}){}",
                if i == 0 { "（首个，若与起点重合则去重）" } else { "（必经点）" }
            ));
        }
    } else {
        log(&format!(
            "[policy] 响应未含必经点列表，仅用打卡点 {} 个",
            route_pts.len()
        ));
    }
    if let Some(&(slat, slon)) = route_pts.first() {
        log(&format!("√ [track] 起点 BD=({slat:.6},{slon:.6})"));
    }
    // 用打卡点随机偏移更新锚点并持久化：下次拉点位即学校真实坐标，摆脱写死的默认值
    if !pts_bd.is_empty() {
        let idx = (rand::random::<f64>() * pts_bd.len() as f64) as usize;
        let (clat, clng) = pts_bd[idx];
        let mut rng = rand::thread_rng();
        let normal = Normal::<f64>::new(0.0, 120.0).unwrap();
        let dlat =
            normal.sample(&mut rng).clamp(-200.0, 200.0) / crate::track::geom::MET_PER_DEG_LAT;
        let dlng =
            normal.sample(&mut rng).clamp(-200.0, 200.0) / crate::track::geom::MET_PER_DEG_LNG;
        client.identity.anchor_lat = clat + dlat;
        client.identity.anchor_lon = clng + dlng;
        if let Err(e) = super::model::save_identity(&client.identity) {
            log(&format!("⚠ 锚点持久化失败: {e}"));
        }
        // 用漂移后的新锚点重存点位缓存，使缓存锚点与持久化锚点一致，避免预览锚点失配。
        if let Ok(new_anchor) = client.identity.anchor_coordinate() {
            let _ = super::model::save_points_cache(new_anchor, &pts);
        }
    }
    // 平均配速必须落在有效窗口内；不满足时直接报错，不再静默修改时长
    let params = params.clone();
    let avg = params.dist / params.dur as f64;
    let (lo, hi) = (
        crate::track::generator::SPEED_FLOOR,
        crate::track::generator::SPEED_CEIL,
    );
    if avg < lo || avg > hi {
        return Err(format!(
            "距离与时长无法同时满足：{:.0}m / {}s 的平均配速 {:.2} m/s 超出有效窗口 {:.2}-{:.2} m/s，\
             请调整距离或时长",
            params.dist, params.dur, avg, lo, hi
        ));
    }
    log(&format!(
        "[track] 生成轨迹 {:.0}m / {}s（{} 点位）…",
        params.dist,
        params.dur,
        route_pts.len()
    ));
    // 瞬时速度窗口：由用户配速范围换算，避免生成配速跑出设定范围；
    // 未提供范围时退回生成器默认宽窗口。
    let v_bounds = params
        .pace_range
        .and_then(|(a, b)| crate::track::generator::speed_bounds_from_pace(a, b));
    // 随机 0-4 秒偏移（终端上报的 flag 与首点差 <5s），轨迹/提交/OBS/五点统一使用
    let start_ms = params.start_ms + (rand::random::<i64>() % 5) * 1000;
    // 跑区元数据：由围栏接口返回（无围栏时保持默认自由跑口径）。
    let mut area = crate::track::wire::RunAreaMeta::default();
    let mut track = match params.route_mode {
        RouteMode::Road => {
            let cfg = crate::api::model::load_config();
            if cfg.osm_path.is_empty() {
                return Err("道路模式需要 OSM 路网，请先在「路网」页加载（回退会丢掉必经点）".into());
            }
                match crate::track::generate_road::load_network_path(&cfg.osm_path) {
                    Ok(mut net) => {
                        crate::track::generate_road::align_network(&mut net);
                        // 电子围栏：裁剪到围栏内道路（失败/无围栏则跳过）
                        let fences = match crate::api::fence::fetch_geo_fence_with_area(client) {
                            Ok((f, a)) => {
                                let _ = crate::api::model::save_fence_cache(&f);
                                log(&format!("√ [track] 电子围栏 {} 个", f.len()));
                                area = a;
                                f
                            }
                            Err(e) => {
                                log(&format!("⚠ [track] 围栏获取失败，回退缓存: {e}"));
                                crate::api::model::load_fence_cache().unwrap_or_default()
                            }
                        };
                        let fenced = crate::track::generate_road::apply_fences(&net, &fences);
                        let (filtered, dropped, skipped) =
                            crate::track::generate_road::apply_obstacles(&fenced);
                        if skipped {
                            log("⚠ [track] 建筑轮廓与道路重叠过多，跳过建筑避让（检查 OSM 对齐）");
                        } else if dropped > 0 {
                            log(&format!("√ [track] 建筑避让：移除 {dropped} 条穿楼边"));
                        }
                        // 强制经过：必经点 + 打卡点全部作为硬约束（与预览同口径）
                        let must_bd: Vec<(f64, f64)> = route_pts.clone();
                        match crate::track::generate_road::build_road(
                            params.dist,
                            params.dur,
                            params.seed,
                            start_ms,
                            &route_pts,
                            &must_bd,
                            &filtered,
                            v_bounds,
                        ) {
                            Ok(t) => {
                                log(&format!(
                                    "√ [track] 真实道路路由 {} 点 / {} 建筑 / {} 围栏",
                                    t.locations.len(),
                                    filtered.buildings.len(),
                                    fences.len()
                                ));
                                log_must_coverage(log, &t, &must_bd);
                                let report = crate::track::check::check(
                                    &t,
                                    &filtered.buildings,
                                    &fences,
                                    Some(&filtered),
                                    15.0,
                                );
                                for line in report.lines() {
                                    log(&line);
                                }
                                t
                            }
                            // 道路模式拒绝静默回退：回退会丢掉必经点，必须让用户修正路网/围栏
                            Err(e) => {
                                return Err(format!("道路模式生成失败（必经点不可达/断路/自交）：{e}"))
                            }
                        }
                    }
                    Err(e) => return Err(format!("路网加载失败：{e}")),
            }
        }
        RouteMode::Track => {
            if params.track_outline.is_empty() {
                return Err("跑道模式需要人工确认的轮廓，请在地图上确认后再提交".into());
            }
            let cfg = crate::api::model::load_config();
            if cfg.osm_path.is_empty() {
                return Err("跑道模式需要 OSM 路网，请先在「路网」页加载".into());
            }
            let mut net = crate::track::generate_road::load_network_path(&cfg.osm_path)?;
            crate::track::generate_road::align_network(&mut net);
            let fences = match crate::api::fence::fetch_geo_fence_with_area(client) {
                Ok((f, a)) => {
                    let _ = crate::api::model::save_fence_cache(&f);
                    log(&format!("√ [track] 电子围栏 {} 个", f.len()));
                    area = a;
                    f
                }
                Err(e) => {
                    log(&format!("⚠ [track] 围栏获取失败，回退缓存: {e}"));
                    crate::api::model::load_fence_cache().unwrap_or_default()
                }
            };
            let fenced = crate::track::generate_road::apply_fences(&net, &fences);
            // 同上：全部必经点强制经过，起点重合点由 plan_track_route 去重
            let must_bd: Vec<(f64, f64)> = must_points.clone();
            match crate::track::generate_road::build_track(
                params.dist,
                params.dur,
                params.seed,
                start_ms,
                &route_pts,
                &must_bd,
                &fenced,
                &params.track_outline,
                v_bounds,
            ) {
                Ok(t) => {
                    log(&format!(
                        "√ [track] 跑道绕圈 {} 点 / {} 建筑 / {} 围栏",
                        t.locations.len(),
                        fenced.buildings.len(),
                        fences.len()
                    ));
                    log_must_coverage(log, &t, &must_bd);
                    let report = crate::track::check::check(
                        &t,
                        &fenced.buildings,
                        &fences,
                        Some(&fenced),
                        15.0,
                    );
                    for line in report.lines() {
                        log(&line);
                    }
                    t
                }
                // 跑道模式拒绝静默回退：断路 / 自交 / 方向冲突必须让用户改轮廓
                Err(e) => return Err(format!("跑道模式生成失败（断路/自交/方向冲突）：{e}")),
            }
        }
        RouteMode::Legacy => crate::track::generator::build_with_mode(
            params.dist,
            params.dur,
            params.seed,
            (anchor.latitude, anchor.longitude),
            start_ms,
            &pts_bd,
            v_bounds,
            params.track_mode,
            params.bowl_climb,
            params.bowl_altitude,
        )?,
    };
    // 【Bowl 海拔】经典模式由 generator::build_with_mode 内部完成；
    // 路网/跑道模式的形状由路线决定，这里补一道碗形剖面覆盖。
    if params.bowl_altitude && params.route_mode != RouteMode::Legacy {
        crate::track::altitude::apply_bowl_profile(&mut track, params.seed, params.bowl_climb);
        log(&format!(
            "√ [track] 碗形海拔已应用到 {} 个点（模式={:?}，起始爬升={}）",
            track.locations.len(),
            params.route_mode,
            params.bowl_climb
        ));
    }
    if let Some(range) = params.manual_altitude_range {
        crate::track::altitude::override_bd_a_range(&mut track, range)?;
        log(&format!(
            "√ [track] 已将海拔曲线映射到 {:.2}-{:.2}m，覆盖 {} 个点，爬升/圈数据将按覆盖值计算",
            range.min_m,
            range.max_m,
            track.locations.len()
        ));
    } else if let Some(altitude_m) = params.manual_altitude {
        crate::track::altitude::override_bd_a(&mut track, altitude_m)?;
        log(&format!(
            "√ [track] 已用手动海拔 {:.2}m 覆盖 {} 个点，爬升/圈数据将按覆盖值计算",
            altitude_m,
            track.locations.len()
        ));
    }
    log(&format!(
        "√ [track] {} 点 totalDis={:.0}m steps={} 起点={}",
        track.locations.len(),
        track.totalDistance,
        track.totalSteps,
        chrono::Local
            .timestamp_millis_opt(params.start_ms)
            .single()
            .map(|t| t.format("%H:%M:%S").to_string())
            .unwrap_or_default(),
    ));

    // ④ 五点 wrapper（跑完态；校园跑带真实跑区）
    let five = five_point_wrapper_with_area(&pts, track.startTime, &area, Some(&track));
    let _ = &five;

    // ④.5 极验设备回执（deviceVerify）
    //
    // 【真机实测】官方在 `requestFivePoint` 的 onSuccess 里立刻上报这一次，
    // 那一刻记录还没生成，所以 body 里**没有 rrid**（对应本函数 rrid 传 -1），
    // 有 uuid 则带上。实测请求落在记录 startTime 前约 20 秒（起跑时），
    // 而不是提交之后 —— 旧实现放在提交后是错的。
    //
    // 目标主机：`partner.iydsj.com`（不是 run.gxapp 域名）。
    // geeToken 优先取 MRT_GEE_TOKEN（外部注入的 DeepKnow 回执），
    // 没有就退回本次起跑的 scene=4 回执。
    if std::env::var("MRT_DEVICE_VERIFY").ok().as_deref() == Some("1") {
        let injected = std::env::var("MRT_GEE_TOKEN").unwrap_or_default();
        let gee = if injected.trim().is_empty() {
            start_receipt.0.clone()
        } else {
            injected.trim().to_string()
        };
        if gee.is_empty() {
            log("⚠ [deviceverify] 无可用 geeToken（MRT_GEE_TOKEN 为空且未开启 geetoken），跳过");
        } else {
            let code = crate::geetoken::code_for_token(&gee);
            log(&format!("[deviceverify] 起跑后上报设备回执（errorCode={code}）…"));
            match super::deviceverify::device_verify(
                client,
                &gee,
                -1,
                Some(run_uuid.as_str()),
                code,
            ) {
                Ok(_) => log("√ [deviceverify] 上报成功"),
                Err(e) => log(&format!("⚠ [deviceverify] 上报失败: {e}")),
            }
        }
    }

    // ⑤ 提交（sportType=1）
    log("[record] 提交跑步记录（sportType=1）…");
    // scene=6 按检查点顺序在压缩窗口内按比例发；scene=7 最后发，用于提交体。
    let (gee_token, error_code) = if geetoken_on {
        // [可配置] 回执窗口策略：固定 60s / 对齐真实 totalTime（手机端自行选择）。
        let window = match params.gee_window_mode {
            GeeWindowMode::Fixed60 => (pts.len() as f64 * 2.0).clamp(2.0, 60.0),
            GeeWindowMode::RealTotal => {
                let total_secs = (track.totalTime as f64).max(2.0);
                (pts.len() as f64 * 2.0).clamp(2.0, total_secs)
            }
        };
        let mut waited = 0.0f64;
        for (index, _) in pts.iter().enumerate() {
            let target = window * (index + 1) as f64 / pts.len() as f64;
            let wait = (target - waited).max(0.0);
            if wait > 0.02 {
                std::thread::sleep(std::time::Duration::from_millis((wait * 1000.0) as u64));
            }
            waited = target;
            // 循环内每条只用于日志，不跨轮保留（避免 unused_assignments）
            let (receipt6, code6) = crate::geetoken::make_token_with_mode(
                &client.identity,
                sess.uid,
                &run_uuid,
                6,
                geetoken_online,
                &mut *log,
            );
            log(&format!(
                "[gee] scene=6 打卡回执 {}/{} token {} 字符 code={}",
                index + 1,
                pts.len(),
                receipt6.len(),
                code6
            ));
        }
        let receipt = crate::geetoken::make_token_with_mode(
            &client.identity,
            sess.uid,
            &run_uuid,
            7,
            geetoken_online,
            &mut *log,
        );
        log(&format!(
            "[gee] scene=7 提交回执 token {} 字符，errorCode={}",
            receipt.0.len(),
            receipt.1
        ));
        receipt
    } else {
        // [201 修复] 不走极验时，提交体里的 errorCode 必须是 -999（与 geetoken::code_for_token("") 一致），
        // 原来这里给 0 会与空 geeToken 不配对，服务端按「数据异常」拒绝。
        (String::new(), -999)
    };
    let sp = SubmitParams {
        track,
        uid: sess.uid,
        selected_unid: sess.unid.parse().unwrap_or(0),
        policy: pol.policy,
        policy_ts: pol.timestamp,
        min_distance: pol.min_distance,
        weight: if sess.weight > 0.0 { sess.weight } else { 68.0 },
        face_check: params.face_check,
        five_point_json: five,
        address: client.identity.city.clone(),
        uuid: run_uuid,
        gee_token,
        error_code,
    };
    // ⑤ 【真机顺序】先上传 OBS，再提交记录。
    // 实测：真机 OBS 对象里的 `rrid` 字段是 "0"（上传时记录还没生成），
    // 服务端靠 objectKey 里的 uuid + 提交体的 uuid 关联两者。
    // 旧实现是「先提交拿 rrid，再传 OBS(rrid=真实值)」，顺序与取值都错。
    log("[obs] 先上传 OBS 对象（gzip+base64，11 键，rrid=0）…");
    let obj = build_obs_object_with_area(
        &sp.track,
        0,
        &sp.uuid,
        sess.uid,
        &pts,
        &client.identity.platform,
        &area,
    );
    let payload = obj.to_string().into_bytes();
    let keys = obs_keys(&sp.track, 0, &sp.uuid);
    let obs_ok = super::obs::upload_both_keys(client, &keys, &payload, log);
    if obs_ok == keys.len() {
        log("√ [obs] OBS 上传成功");
    } else {
        log(&format!("⚠ [obs] 上传成功 {obs_ok}/{}", keys.len()));
    }
    // [校验] 回读第一个 objectKey，确认 OBS 已真正落盘且对象含 run_data
    if obs_ok > 0 && !keys.is_empty() {
        match super::obs::fetch_object(client, &keys[0], log) {
            Ok(obj) => {
                if obj.get("run_data").is_some() {
                    log(&format!("√ [obs] 回读校验通过 objectKey={}", keys[0]));
                } else {
                    log(&format!("⚠ [obs] 回读对象缺 run_data：objectKey={}", keys[0]));
                }
            }
            Err(e) => log(&format!("⚠ [obs] 回读校验失败: {e}")),
        }
    }

    // ⑥ 提交跑步记录（OBS 已就位，服务端按 uuid 取轨迹）
    let result = submit_record(client, &sp, log)?;
    sleep_secs(1);

    // ⑦ 详情验证
    sleep_secs(2);
    log("[verify] 拉取详情验证…");
    let detail_ok = match fetch_one_record(client, result.rrid) {
        Ok(d) => {
            log(&format!(
                "√ [verify] rrid={} complete={:?} dis={:?} time={:?}",
                result.rrid,
                d.get("complete").and_then(|v| v.as_bool()),
                d.get("totalDis"),
                d.get("totalTime"),
            ));
            if let Ok(mut slot) = VERIFY_DETAIL.lock() {
                slot.replace(d.clone());
            }
            true
        }
        Err(e) => {
            log(&format!(
                "⚠ [verify] 详情拉取失败（提交已成功 rrid={}）：{e}",
                result.rrid
            ));
            false
        }
    };
    Ok(RunOutcome {
        result,
        obs_ok,
        detail_ok,
    })
}

/// AI 提交流（UI 线程用）。
pub fn run_ai_submit(
    client: &mut ApiClient,
    sport_id: i64,
    mode: super::ai::AiMode,
    log: &mut dyn FnMut(&str),
) -> Result<Value, String> {
    log(&format!("[ai] 提交 sportId={sport_id} mode={mode:?}…"));
    let biz = super::ai::upload(client, sport_id, mode, None)?;
    log("√ [ai] 提交成功");
    Ok(biz)
}

/// AI 列表（UI 线程用）。
pub fn run_ai_list(
    client: &mut ApiClient,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<super::ai::AiSport>, String> {
    log("[ai] 拉取项目列表…");
    let list = super::ai::fetch_list(client)?;
    log(&format!("√ [ai] {} 个项目", list.len()));
    Ok(list)
}

/// 记录列表（UI 线程用）。
pub fn run_records(
    client: &mut ApiClient,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<super::records::RecordRow>, String> {
    log("[records] 拉取跑步记录…");
    let rows = super::records::fetch_records(client)?;
    log(&format!("√ [records] {} 条记录", rows.len()));
    Ok(rows)
}

use chrono::TimeZone as _;

/// 最近一次详情验证的完整响应（达标判定明细在 reasonList）。
pub static VERIFY_DETAIL: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// 日志输出每个必经点到轨迹的最近距离（米），用于核对「必达」。
fn log_must_coverage(
    log: &mut dyn FnMut(&str),
    t: &crate::track::model::Track,
    must: &[(f64, f64)],
) {
    if must.is_empty() {
        return;
    }
    let cov = crate::track::generate_road::must_coverage(t, must);
    let text: Vec<String> = cov.iter().map(|d| format!("{d:.0}m")).collect();
    log(&format!(
        "√ [track] 必经点覆盖（最近距离）: {}",
        text.join(" / ")
    ));
}
