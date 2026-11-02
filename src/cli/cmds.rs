//! CLI 子命令实现。

use super::{
    clip, fmt_hms, get, jstr, logger, make_client, now_ms, parse_flags, parse_pace, print_rows,
    reject_unknown, usage_cmd,
};
use crate::api::ai::AiMode;
use crate::api::client::ApiClient;
use crate::api::model;

/// 各命令允许的参数集合（拼写错误立即报错，而不是被静默忽略）。
fn allowed_flags(cmd: &str) -> Option<&'static [&'static str]> {
    Some(match cmd {
        "login" => &["user", "pass", "remember"],
        "logout" => &[],
        "run" => &[
            "dist",
            "pace",
            "altitude",
            "ago",
            "days-ago",
            "time",
            "face",
            "seed",
            "route",
        ],
        "template" => &["file"],
        "ai" => &["sport", "mode", "score", "count", "minutes"],
        "ai-list" => &[],
        "ai-records" => &["sport"],
        "ai-info" => &["id"],
        "records" => &[],
        "record-info" => &["rrid"],
        "records-raw" => &["body", "out"],
        "obs-get" => &["key", "out"],
        "obs-sample" => &["dist", "dur", "rrid", "seed", "out"],
        "semester" => &[],
        "cheat" => &["page"],
        "rank" => &["type", "sort", "gender", "date", "range"],
        "update" => &["check"],
        "help" | "--help" | "-h" | "version" | "--version" | "-V" | "-v" => &[],
        _ => return None,
    })
}

pub fn dispatch(args: Vec<String>) -> i32 {
    let cmd = args.first().map(|x| x.as_str()).unwrap_or("help").to_string();
    let rest = args.iter().skip(1).map(|x| x.as_str()).collect::<Vec<_>>();

    // 任意命令后跟 --help / -h 都只看该命令的用法，不执行
    if rest.iter().any(|a| *a == "--help" || *a == "-h") {
        if !usage_cmd(&cmd) {
            super::usage();
        }
        return 0;
    }
    // 未知参数提前拦截
    if let Some(allowed) = allowed_flags(&cmd) {
        let flags = parse_flags(&rest);
        if let Err(e) = reject_unknown(&flags, allowed) {
            eprintln!("{e}");
            eprintln!("查看用法：nekosportsworldtool help {cmd}");
            return 1;
        }
    }

    match cmd.as_str() {
        "login" => cmd_login(&rest),
        "logout" => cmd_logout(),
        "run" => cmd_run(&rest),
        "template" => cmd_template(&rest),
        "ai" => cmd_ai(&rest),
        "ai-list" => cmd_ai_list(),
        "records" => cmd_records(),
        "records-raw" => cmd_records_raw(&rest),
        "record-info" => cmd_record_info(&rest),
        "obs-get" => cmd_obs_get(&rest),
        "obs-sample" => cmd_obs_sample(&rest),
        "ai-records" => cmd_ai_records(&rest),
        "ai-info" => cmd_ai_info(&rest),
        "semester" => cmd_semester(),
        "cheat" => cmd_cheat(&rest),
        "rank" => cmd_rank(&rest),
        "update" => cmd_update(&rest),
        "version" | "--version" | "-V" | "-v" => {
            println!(
                "NekoSportsWorldTool v{}（{}）",
                crate::update::current_version(),
                if cfg!(feature = "gui") { "full" } else { "cli" }
            );
            0
        }
        "help" | "--help" | "-h" => {
            match rest.first() {
                Some(c) if !usage_cmd(c) => {
                    eprintln!("未知命令: {c}");
                    super::usage();
                    1
                }
                _ => 0,
            }
        }
        other => {
            eprintln!("未知命令: {other}");
            super::usage();
            1
        }
    }
}

fn cmd_login(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    // --user/--pass 优先，其次环境变量（青龙等定时任务里不必把密码写进命令）
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
    let user = get(&flags, "user")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| env("NEKO_USER"));
    let pass = get(&flags, "pass")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| env("NEKO_PASS"));
    let (Some(user), Some(pass)) = (user, pass) else {
        eprintln!("缺少 --user / --pass（也可用环境变量 NEKO_USER / NEKO_PASS）");
        eprintln!("查看用法：nekosportsworldtool help login");
        return 1;
    };
    let identity = model::load_identity();
    let mut client = ApiClient::new(identity, None);
    let mut log = logger();
    match crate::api::login::login(&mut client, &user, &pass, &mut log) {
        Ok(s) => {
            if get(&flags, "remember").is_some() {
                let mut cfg = model::load_config();
                cfg.username = user;
                cfg.password = pass;
                cfg.remember = true;
                match model::save_config(&cfg) {
                    Ok(()) => println!("凭据已保存（会话失效时自动重登）"),
                    Err(e) => println!("× 凭据保存失败：{e}（本次登录仍可用，重启后需重新登录）"),
                }
            }
            println!("登录成功 uid={} unid={} name={}", s.uid, s.unid, s.name);
            0
        }
        Err(e) => {
            eprintln!("登录失败: {e}");
            1
        }
    }
}

fn cmd_logout() -> i32 {
    let identity = model::load_identity();
    let sess = model::load_session();
    let mut client = ApiClient::new(identity, sess.is_logged_in().then_some(sess));
    let mut log = logger();
    crate::api::login::logout(&mut client, &mut log);
    0
}

fn cmd_run(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let dist_km: f32 = match get(&flags, "dist") {
        Some(v) => match v.parse::<f32>() {
            Ok(x) if x.is_finite() && (0.1..=50.0).contains(&x) => x,
            _ => {
                eprintln!("--dist 需为 0.1~50 之间的公里数（收到 {v}）");
                return 1;
            }
        },
        None => 0.0,
    };
    let pace: f32 = match get(&flags, "pace") {
        Some(v) => match parse_pace(v) {
            Some(p) if (120.0..=1200.0).contains(&p) => p,
            _ => {
                eprintln!("--pace 需为 120~1200 秒/km，可写 360 或 6:00（收到 {v}）");
                return 1;
            }
        },
        None => 0.0,
    };
    let ago_min: Option<i64> = match get(&flags, "ago") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if (0..=4320).contains(&x) => Some(x),
            _ => {
                eprintln!("--ago 需为 0~4320 之间的分钟数（收到 {v}）");
                return 1;
            }
        },
        None => None,
    };
    let days_ago: i64 = match get(&flags, "days-ago") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if (0..=3).contains(&x) => x,
            _ => {
                eprintln!("--days-ago 需为 0~3（今天/昨天/前天/大前天，收到 {v}）");
                return 1;
            }
        },
        None => 0,
    };
    let time_hm: Option<(u32, u32)> = match get(&flags, "time") {
        Some(spec) => {
            let parsed = spec
                .split_once(':')
                .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
                .filter(|(h, m)| *h < 24 && *m < 60);
            let Some(hm) = parsed else {
                eprintln!("--time 需为 HH:MM（24 小时制，如 07:30，收到 {spec}）");
                return 1;
            };
            if ago_min.is_some() {
                eprintln!("--ago 与 --time 互斥：前者按“多少分钟前”，后者按具体时刻，请二选一");
                return 1;
            }
            Some(hm)
        }
        None => None,
    };
    if time_hm.is_none() && days_ago != 0 {
        eprintln!("--days-ago 需配合 --time 使用（如 --days-ago 1 --time 07:30）");
        return 1;
    }
    let face = match get(&flags, "face") {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        Some(v) => {
            eprintln!("--face 只能是 0 或 1（收到 {v}）");
            return 1;
        }
        None => true,
    };
    let (manual_altitude, manual_altitude_range) = match get(&flags, "altitude") {
        Some(value) => match crate::track::altitude::parse_spec(value) {
            Ok(Some(crate::track::altitude::AltitudeSpec::Single(value))) => (Some(value), None),
            Ok(Some(crate::track::altitude::AltitudeSpec::Range(range))) => (None, Some(range)),
            Ok(None) => (None, None),
            Err(e) => {
                eprintln!("--altitude {e}");
                return 1;
            }
        },
        None => (None, None),
    };
    let seed: u64 = match get(&flags, "seed") {
        Some(v) => match v.parse::<u64>() {
            Ok(x) => x,
            Err(_) => {
                eprintln!("--seed 需为非负整数（收到 {v}）");
                return 1;
            }
        },
        None => 0,
    };
    let seed = if seed == 0 {
        (now_ms() % 2_147_483_647) as u64
    } else {
        seed
    };
    let cfg = model::load_config();
    let route_mode = match get(&flags, "route") {
        Some("legacy") => crate::track::generate_road::RouteMode::Legacy,
        Some("road") => crate::track::generate_road::RouteMode::Road,
        Some("track") => {
            eprintln!("--route track 需要 GUI 人工确认跑道轮廓，CLI 不支持");
            return 1;
        }
        Some(v) => {
            eprintln!("--route 只能是 legacy / road / track（收到 {v}）");
            return 1;
        }
        None => match crate::track::generate_road::RouteMode::from_str(&cfg.route_mode) {
            // 配置里的跑道模式同样依赖人工确认，CLI 回退经典算法
            crate::track::generate_road::RouteMode::Track => {
                eprintln!("配置为跑道模式（需 GUI 确认轮廓），CLI 回退经典算法");
                crate::track::generate_road::RouteMode::Legacy
            }
            m => m,
        },
    };

    let dist = if dist_km > 0.0 {
        dist_km as f64 * 1000.0
    } else {
        (1.0 + rand::random::<f32>() * 0.5) as f64 * 1000.0
    };
    let pace_s = if pace > 0.0 {
        pace
    } else {
        360.0 + rand::random::<f32>() * 120.0
    };
    let dur = ((dist as f32 / 1000.0 * pace_s) as i64).max(60);
    let start_ms = match time_hm {
        // --time "HH:MM" 配合 --days-ago（0-3）
        Some((h, m)) => {
            let base = chrono::Local::now() - chrono::Duration::days(days_ago);
            use chrono::{Datelike, TimeZone};
            chrono::Local
                .with_ymd_and_hms(base.year(), base.month(), base.day(), h, m, 0)
                .single()
                .map(|x| x.timestamp_millis())
                .unwrap_or_else(|| now_ms())
        }
        None => match ago_min {
            Some(m) => now_ms() - m * 60_000,
            None => now_ms() - 30 * 60_000 - (rand::random::<f64>() * 270.0 * 60_000.0) as i64,
        },
    };
    // 「开始 + 用时」不能越过当前时刻，否则提交会被服务端判为未来时间
    let start_ms = start_ms.min(now_ms() - dur * 1000 - 5_000);

    println!(
        "参数：{:.0}m / {}s / 配速 {}:{:02}/km / 开始 {}",
        dist,
        dur,
        pace_s as i64 / 60,
        pace_s as i64 % 60,
        fmt_hms(start_ms)
    );

    let mut log = logger();
    let params = crate::api::flow::RunParams {
        dist,
        dur,
        start_ms,
        face_check: face as i64,
        manual_altitude,
        manual_altitude_range,
        seed,
        route_mode,
        track_outline: Vec::new(),
    };
    match crate::api::flow::run_full_flow(&mut client, &params, &mut log) {
        Ok(out) => {
            println!(
                "跑步提交成功 rrid={} uuid={} obs={}/2 verify={}",
                out.result.rrid,
                out.result.uuid,
                out.obs_ok,
                if out.detail_ok { "通过" } else { "未通过" }
            );
            0
        }
        Err(e) => {
            eprintln!("跑步提交失败: {e}");
            1
        }
    }
}

fn cmd_template(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let Some(path) = get(&flags, "file") else {
        eprintln!("缺少 --file <GPX/JSON>");
        return 1;
    };
    match crate::template::load(path) {
        Ok(samples) => {
            let summary = crate::template::summarize(path, &samples);
            println!("本地模板分析（仅预览，不上传）");
            println!("文件：{}", summary.source);
            println!("采样点：{}", summary.samples);
            println!("海拔范围：{:.1}–{:.1} m", summary.min_m, summary.max_m);
            println!(
                "累计上升：{:.1} m；累计下降：{:.1} m",
                summary.gain_m, summary.loss_m
            );
            0
        }
        Err(e) => {
            eprintln!("模板读取失败：{e}");
            1
        }
    }
}

fn cmd_ai(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let sport: i64 = match get(&flags, "sport") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if x > 0 => x,
            _ => {
                eprintln!("--sport 需为正整数项目 ID（收到 {v}），可用 ai-list 查看");
                return 1;
            }
        },
        None => {
            eprintln!("缺少 --sport（可用 ai-list 查看项目 ID）");
            return 1;
        }
    };
    // 按分钟（1-30）或按次（5-1000，5 的倍数）；--score 作为当前模式的别名
    let mode = match get(&flags, "mode").unwrap_or("min") {
        "min" | "minutes" => {
            let minutes = match get(&flags, "minutes").or_else(|| get(&flags, "score")) {
                Some(v) => match v.parse::<i64>() {
                    Ok(x) if (1..=30).contains(&x) => x,
                    _ => {
                        eprintln!("--minutes 需为 1~30 的整数分钟（收到 {v}）");
                        return 1;
                    }
                },
                None => 1,
            };
            AiMode::Minutes { minutes }
        }
        "count" => {
            let reps = match get(&flags, "count").or_else(|| get(&flags, "score")) {
                Some(v) => match v.parse::<i64>() {
                    Ok(x) if (5..=1000).contains(&x) && x % 5 == 0 => x,
                    _ => {
                        eprintln!("--count 需为 5~1000 且是 5 的倍数（收到 {v}）");
                        return 1;
                    }
                },
                None => 5,
            };
            AiMode::Count { reps }
        }
        other => {
            eprintln!("--mode 只能是 min 或 count（收到 {other}）");
            return 1;
        }
    };
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut log = logger();
    match crate::api::flow::run_ai_submit(&mut client, sport, mode, &mut log) {
        Ok(biz) => {
            println!(
                "AI 提交成功 服务器={} {}",
                biz.get("error").and_then(|e| e.as_i64()).unwrap_or(0),
                biz.get("message").and_then(|m| m.as_str()).unwrap_or("")
            );
            0
        }
        Err(e) => {
            eprintln!("AI 提交失败: {e}");
            1
        }
    }
}

fn cmd_ai_list() -> i32 {
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut log = logger();
    match crate::api::flow::run_ai_list(&mut client, &mut log) {
        Ok(list) => {
            for s in list {
                println!("id={:<4} {}", s.id, s.name);
            }
            0
        }
        Err(e) => {
            eprintln!("拉取失败: {e}");
            1
        }
    }
}

fn cmd_records() -> i32 {
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut log = logger();
    match crate::api::flow::run_records(&mut client, &mut log) {
        Ok(rows) => {
            print_rows(&rows);
            0
        }
        Err(e) => {
            eprintln!("拉取失败: {e}");
            1
        }
    }
}

/// 记录列表原始探测：支持自定义 body，打印条数与日期范围。
fn cmd_records_raw(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let body = get(&flags, "body").unwrap_or("{}");
    if let Err(e) = serde_json::from_str::<serde_json::Value>(body) {
        eprintln!("--body 必须是合法 JSON（收到 {body}）：{e}");
        return 1;
    }
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match client.call("POST", crate::api::records::RECORDS_PATH, body, &[]) {
        Ok(biz) => {
            let data = crate::api::client::parse_data_field(&biz);
            let arr = data.as_array().cloned().unwrap_or_default();
            let n = arr.len();
            println!("条数: {n}");
            if let Some(out) = get(&flags, "out") {
                std::fs::write(out, serde_json::to_string_pretty(&arr).unwrap_or_default())
                    .map_err(|e| eprintln!("写入失败: {e}"))
                    .ok();
                println!("已保存 {out}");
            } else if n > 0 {
                println!("首条: {}", clip(&arr[0].to_string(), 300));
                println!("末条: {}", clip(&arr[n - 1].to_string(), 300));
            } else {
                println!("data: {}", clip(&data.to_string(), 500));
            }
            0
        }
        Err(e) => {
            eprintln!("拉取失败: {e}");
            1
        }
    }
}

/// 拉取 OBS 对象（签名 GET），解码 gzip+base64 字段后保存。
fn cmd_obs_get(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let Some(key) = get(&flags, "key").filter(|k| !k.trim().is_empty()) else {
        eprintln!("缺少 --key <对象键>（可从 record-info 的 obsKey 字段获取）");
        return 1;
    };
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut log = |s: &str| eprintln!("{s}");
    match crate::api::obs::fetch_object(&mut client, key, &mut log) {
        Ok(v) => {
            let out = get(&flags, "out").unwrap_or("obs_real.json");
            // 解码 gzip+base64 值便于直接阅读
            let decoded = decode_gz_fields(&v);
            std::fs::write(
                out,
                serde_json::to_string_pretty(&decoded).unwrap_or_default(),
            )
            .map_err(|e| eprintln!("写入失败: {e}"))
            .ok();
            println!("已保存 {out}");
            0
        }
        Err(e) => {
            eprintln!("拉取失败: {e}");
            1
        }
    }
}

/// 解码 OBS 对象中 gzip+base64 的字段值（尽力而为）。
fn decode_gz_fields(v: &serde_json::Value) -> serde_json::Value {
    use base64::Engine;
    let mut out = v.clone();
    if let Some(obj) = out.as_object_mut() {
        for (_k, val) in obj.iter_mut() {
            if let Some(s) = val.as_str() {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(s)
                    .ok()
                    .and_then(|raw| {
                        let mut dec = flate2::read::GzDecoder::new(raw.as_slice());
                        use std::io::Read;
                        let mut txt = String::new();
                        dec.read_to_string(&mut txt).ok()?;
                        Some(txt)
                    });
                if let Some(txt) = decoded {
                    let parsed = serde_json::from_str::<serde_json::Value>(&txt)
                        .map(|x| x.to_string())
                        .unwrap_or(txt);
                    *val = serde_json::Value::String(parsed);
                }
            }
        }
    }
    out
}

/// 本地生成一份 OBS 对象样本（不提交；结构对照用）。
fn cmd_obs_sample(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let dist = match get(&flags, "dist") {
        Some(v) => match v.parse::<f64>() {
            Ok(x) if x.is_finite() && x > 0.0 => x,
            _ => {
                eprintln!("--dist 需为正数米数（收到 {v}）");
                return 1;
            }
        },
        None => 1050.0,
    };
    let dur = match get(&flags, "dur") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if x >= 1 => x,
            _ => {
                eprintln!("--dur 需为 ≥1 的秒数（收到 {v}）");
                return 1;
            }
        },
        None => 480,
    };
    let rrid = match get(&flags, "rrid") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if x > 0 => x,
            _ => {
                eprintln!("--rrid 需为正整数记录 ID（收到 {v}）");
                return 1;
            }
        },
        None => 1320000000,
    };
    let seed = match get(&flags, "seed") {
        Some(v) => match v.parse::<u64>() {
            Ok(x) => x,
            Err(_) => {
                eprintln!("--seed 需为非负整数（收到 {v}）");
                return 1;
            }
        },
        None => 7,
    };
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut log = |s: &str| eprintln!("{s}");
    let anchor = match client.identity.anchor_coordinate() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("定位锚点无效: {e}");
            return 1;
        }
    };
    let pts = match crate::api::points::fetch_points(&mut client, anchor, &mut log) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("点位拉取失败: {e}");
            return 1;
        }
    };
    let pts_bd = crate::api::points::points_bd(&pts);
    let start_ms = crate::crypto::envelope::now_ms() - dur * 1000;
    let track = match crate::track::generator::build(
        dist,
        dur,
        seed,
        (anchor.latitude, anchor.longitude),
        start_ms,
        &pts_bd,
    ) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("轨迹生成失败: {e}");
            return 1;
        }
    };
    let sess = client.login.clone().unwrap_or_default();
    let uuid = uuid::Uuid::new_v4().to_string().to_uppercase();
    let obj = crate::track::wire::build_obs_object(&track, rrid, &uuid, sess.uid, &pts);
    let out = get(&flags, "out").unwrap_or("obs_ours.json");
    let decoded = decode_gz_fields(&obj);
    std::fs::write(
        out,
        serde_json::to_string_pretty(&decoded).unwrap_or_default(),
    )
    .map_err(|e| eprintln!("写入失败: {e}"))
    .ok();
    println!("已保存 {out}");
    0
}

/// 单条跑步记录原始详情。
fn cmd_record_info(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let rrid = match get(&flags, "rrid") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if x > 0 => x,
            _ => {
                eprintln!("--rrid 需为正整数记录 ID（收到 {v}）");
                return 1;
            }
        },
        None => {
            eprintln!("缺少 --rrid（可在 records 输出中查看）");
            return 1;
        }
    };
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match crate::api::records::fetch_one_record(&mut client, rrid) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => {
            eprintln!("拉取失败: {e}");
            return 1;
        }
    }
    0
}

/// 单条 AI 记录全量详情。
fn cmd_ai_info(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let id = match get(&flags, "id") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if x > 0 => x,
            _ => {
                eprintln!("--id 需为正整数 AI 记录 ID（收到 {v}）");
                return 1;
            }
        },
        None => {
            eprintln!("缺少 --id（可在 ai-records 输出中查看）");
            return 1;
        }
    };
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match crate::api::ai::fetch_record_detail(&mut client, id) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => {
            eprintln!("拉取失败: {e}");
            return 1;
        }
    }
    0
}

fn cmd_ai_records(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let sport: i64 = match get(&flags, "sport") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if x > 0 => x,
            _ => {
                eprintln!("--sport 需为正整数项目 ID（收到 {v}），可用 ai-list 查看");
                return 1;
            }
        },
        None => {
            eprintln!("缺少 --sport（可用 ai-list 查看项目 ID）");
            return 1;
        }
    };
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match crate::api::ai::fetch_records(&mut client, sport, 50) {
        Ok(page) => {
            use chrono::TimeZone;
            for g in page.groups {
                let date = chrono::Local
                    .timestamp_millis_opt(g.score_date)
                    .single()
                    .map(|t| t.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                println!("{date} ×{}：", g.frequency);
                for r in g.records {
                    let grade = if r.rtype == 2 {
                        format!("{:.1} 秒", r.score.parse::<f64>().unwrap_or(0.0) / 1000.0)
                    } else {
                        format!("{} 个", r.score)
                    };
                    let finish = chrono::Local
                        .timestamp_millis_opt(r.score_date)
                        .single()
                        .map(|t| t.format("%H:%M:%S").to_string())
                        .unwrap_or_default();
                    let video = if r.has_video { "有" } else { "-" };
                    println!("  {} {} 完成 {finish} 视频 {video}", r.name, grade);
                }
            }
            0
        }
        Err(e) => {
            eprintln!("拉取失败: {e}");
            1
        }
    }
}

fn cmd_semester() -> i32 {
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut log = logger();
    match crate::api::semester::query(&mut client, &mut log) {
        Ok(r) => {
            if let Some(s) = r.summary {
                println!(
                    "学期：{}  有效次数：{}/{}  有效里程：{:.2} km（总 {:.2} km）",
                    s.sname,
                    s.semester_valid_count,
                    s.semester_count,
                    s.semester_valid_dis / 1000.0,
                    s.semester_dis / 1000.0
                );
            }
            if !r.personal_raw.is_null() {
                println!("个人完成度：{}", r.personal_raw);
            }
            0
        }
        Err(e) => {
            eprintln!("拉取失败: {e}");
            1
        }
    }
}

fn cmd_cheat(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let page: i64 = match get(&flags, "page") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if x >= 1 => x,
            _ => {
                eprintln!("--page 需为 ≥1 的整数（收到 {v}）");
                return 1;
            }
        },
        None => 1,
    };
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut log = logger();
    match crate::api::cheat::query(&mut client, page, &mut log) {
        Ok(r) => {
            if r.is_clean() {
                println!("自查：干净（self=null）");
            } else {
                println!("已被标记：{}", r.self_brief());
            }
            println!("全校违规 {} 条", r.list.len());
            for item in r.list.iter().take(20) {
                println!(
                    "  {} | {} | {}",
                    jstr(item, &["name", "userName"]),
                    jstr(item, &["reason", "punishReason", "cause"]),
                    jstr(item, &["createTime", "time", "date"]),
                );
            }
            0
        }
        Err(e) => {
            eprintln!("检查失败: {e}");
            1
        }
    }
}

fn cmd_rank(rest: &[&str]) -> i32 {
    // 首个位置参数是子榜名（缺省 main）
    let kind = match rest.first() {
        Some(first) if !first.starts_with('-') => *first,
        _ => "main",
    };
    let kind = match kind {
        "main" | "indoor" | "history" => kind,
        other => {
            eprintln!("未知榜单类型: {other}（可选 main | indoor | history）");
            eprintln!("查看用法：nekosportsworldtool help rank");
            return 1;
        }
    };
    let flags = parse_flags(rest);
    // 子榜之间参数不通用（如 indoor 没有 --date），按子榜再拦一次
    let allowed: &[&str] = match kind {
        "indoor" => &["range", "gender"],
        "history" => &["sort", "gender"],
        _ => &["type", "sort", "gender", "date"],
    };
    if let Err(e) = reject_unknown(&flags, allowed) {
        eprintln!("{e}");
        eprintln!("查看用法：nekosportsworldtool help rank");
        return 1;
    }
    let gender: Option<i64> = match get(&flags, "gender") {
        Some(v) => match v.parse::<i64>() {
            Ok(x) if x == 0 || x == 1 => Some(x),
            _ => {
                eprintln!("--gender 只能是 0（女）或 1（男），收到 {v}");
                return 1;
            }
        },
        None => None,
    };
    let mut client = match make_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let res = match kind {
        "indoor" => {
            let range: i64 = match get(&flags, "range") {
                Some(v) => match v.parse::<i64>() {
                    Ok(x) if (1..=3).contains(&x) => x,
                    _ => {
                        eprintln!("--range 只能是 1（日）/2（周）/3（月），收到 {v}");
                        return 1;
                    }
                },
                None => 1,
            };
            crate::api::rank::indoor_rank(&mut client, range, gender.unwrap_or(-1))
        }
        "history" => {
            let sort: i64 = match get(&flags, "sort") {
                Some(v) => match v.parse::<i64>() {
                    Ok(x) if x == 1 || x == 2 => x,
                    _ => {
                        eprintln!("--sort 只能是 1（日榜）或 2（月榜），收到 {v}");
                        return 1;
                    }
                },
                None => 1,
            };
            crate::api::rank::history_rank(&mut client, sort, gender.unwrap_or(-1))
        }
        _ => {
            let rtype: i64 = match get(&flags, "type") {
                Some(v) => match v.parse::<i64>() {
                    Ok(x) if (1..=3).contains(&x) => x,
                    _ => {
                        eprintln!("--type 只能是 1（个人）/2（班级）/3（院系），收到 {v}");
                        return 1;
                    }
                },
                None => 1,
            };
            let sort: i64 = match get(&flags, "sort") {
                Some(v) => match v.parse::<i64>() {
                    Ok(x) if x == 1 || x == 2 => x,
                    _ => {
                        eprintln!("--sort 只能是 1（日榜）或 2（月榜），收到 {v}");
                        return 1;
                    }
                },
                None => 1,
            };
            let date = match get(&flags, "date") {
                Some(v) => {
                    if chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").is_err() {
                        eprintln!("--date 需为 YYYY-MM-DD（收到 {v}）");
                        return 1;
                    }
                    Some(v.to_string())
                }
                None => None,
            };
            crate::api::rank::main_rank(&mut client, rtype, sort, gender, date)
        }
    };
    match res {
        Ok(rows) => {
            for r in rows {
                println!("{:<4} {}  {:.2} km", r.sort, r.name, r.length / 1000.0);
            }
            0
        }
        Err(e) => {
            eprintln!("查询失败: {e}");
            1
        }
    }
}

fn cmd_update(rest: &[&str]) -> i32 {
    let flags = parse_flags(rest);
    let check_only = get(&flags, "check").is_some();
    crate::update::cleanup_residue();
    let mut log = logger();
    let release = match crate::update::check_latest(&mut log) {
        Ok(r) => r,
        Err(e) => {
            println!("× 检查更新失败: {e}");
            return 1;
        }
    };
    let Some(rel) = release else {
        println!("√ 已是最新版本（v{}）", crate::update::current_version());
        return 0;
    };
    println!(
        "发现新版本 {}（当前 v{}，{:.1} MB）",
        rel.tag,
        crate::update::current_version(),
        rel.asset_size as f64 / 1024.0 / 1024.0
    );
    if !rel.notes.is_empty() {
        for line in rel.notes.lines().take(8) {
            println!("  {line}");
        }
    }
    if check_only {
        println!("仅检查（--check），未下载。下载地址：{}", rel.asset_url);
        return 0;
    }
    #[cfg(target_os = "android")]
    {
        println!("Android 版请在界面（关于页）中下载并安装更新");
        0
    }
    #[cfg(not(target_os = "android"))]
    {
        println!("开始下载 {}…", rel.asset_name);
        let mut last_pct = u64::MAX;
        let bytes = match crate::update::download(&rel.asset_url, |done, total| {
            if let Some(t) = total.filter(|t| *t > 0) {
                let pct = done * 100 / t;
                if pct != last_pct && pct % 5 == 0 {
                    println!("  {pct}%");
                    last_pct = pct;
                }
            }
        }) {
            Ok(b) => b,
            Err(e) => {
                println!("× 下载失败: {e}");
                return 1;
            }
        };
        println!("下载完成（{} 字节），解包替换…", bytes.len());
        let bin = match crate::update::extract(&rel.asset_name, &bytes) {
            Ok(b) => b,
            Err(e) => {
                println!("× {e}");
                return 1;
            }
        };
        match crate::update::apply(&bin) {
            Ok(()) => {
                println!("√ 已更新到 {}，请重新运行命令", rel.tag);
                0
            }
            Err(e) => {
                println!("× 替换失败: {e}");
                1
            }
        }
    }
}
