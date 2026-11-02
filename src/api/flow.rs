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
use crate::track::generator::build as gen_track;
use crate::track::wire::{build_obs_object, five_point_wrapper, obs_keys};
use rand_distr::{Distribution, Normal};
use serde_json::Value;

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
    // 必经点保持策略顺序置于前端（waypoints[0] 即起点），剩余打卡点去重后按质心角
    // 排序；道路模式下两者都作为「必须经过」的硬约束。
    let route_pts = crate::track::generate_road::merge_route_points(&pts_bd, &pol.must_points);
    if !pol.must_points.is_empty() {
        log(&format!(
            "√ [policy] 必经点 {} 个（保持顺序），合并后路线 waypoint 共 {} 个",
            pol.must_points.len(),
            route_pts.len()
        ));
        // 逐点打印，便于与响应/轨迹核对：所有 must_points 都作为强制必经点，
        // 与起点重合的首点由路由层投影去重，不再被跳过。
        for (i, &(mlat, mlon)) in pol.must_points.iter().enumerate() {
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
    // 随机 0-4 秒偏移（终端上报的 flag 与首点差 <5s），轨迹/提交/OBS/五点统一使用
    let start_ms = params.start_ms + (rand::random::<i64>() % 5) * 1000;
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
                        let fences = match crate::api::fence::fetch_geo_fence(client) {
                            Ok(f) => {
                                let _ = crate::api::model::save_fence_cache(&f);
                                log(&format!("√ [track] 电子围栏 {} 个", f.len()));
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
            let fences = match crate::api::fence::fetch_geo_fence(client) {
                Ok(f) => {
                    let _ = crate::api::model::save_fence_cache(&f);
                    log(&format!("√ [track] 电子围栏 {} 个", f.len()));
                    f
                }
                Err(e) => {
                    log(&format!("⚠ [track] 围栏获取失败，回退缓存: {e}"));
                    crate::api::model::load_fence_cache().unwrap_or_default()
                }
            };
            let fenced = crate::track::generate_road::apply_fences(&net, &fences);
            // 同上：全部必经点强制经过，起点重合点由 plan_track_route 去重
            let must_bd: Vec<(f64, f64)> = pol.must_points.clone();
            match crate::track::generate_road::build_track(
                params.dist,
                params.dur,
                params.seed,
                start_ms,
                &route_pts,
                &must_bd,
                &fenced,
                &params.track_outline,
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
        RouteMode::Legacy => gen_track(
            params.dist,
            params.dur,
            params.seed,
            (anchor.latitude, anchor.longitude),
            start_ms,
            &pts_bd,
        )?,
    };
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

    // ④ 五点 wrapper（跑完态）
    let five = five_point_wrapper(&pts, track.startTime);
    let _ = &five;

    // ⑤ 提交（sportType=1）
    log("[record] 提交跑步记录（sportType=1）…");
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
    };
    let result = submit_record(client, &sp, log)?;
    sleep_secs(1);

    // ⑥ OBS 上传（双 key）
    log("[obs] 上传 OBS 对象（gzip+base64，10 键）…");
    // 从提交结果回填 track.startTime（含随机秒偏移），保证 body/OBS/flag 全链一致
    let mut track_for_obs = sp.track.clone();
    track_for_obs.startTime = result.start_ms;
    let obj = build_obs_object(&track_for_obs, result.rrid, &result.uuid, sess.uid, &pts);
    let payload = obj.to_string().into_bytes();
    let keys = obs_keys(&track_for_obs, result.rrid, &result.uuid);
    let obs_ok = super::obs::upload_both_keys(client, &keys, &payload, log);
    if obs_ok == 2 {
        log("√ [obs] 双 key 上传成功");
    } else {
        log(&format!("⚠ [obs] 上传成功 {obs_ok}/2"));
    }

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
