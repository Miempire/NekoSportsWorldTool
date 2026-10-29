//! 命令行入口：子命令分发 + 共享助手。
//! 子命令实现见 ；数据文件与 GUI 共用（exe 同目录）。
//!
//! 青龙定时任务：首次 `login --user .. --pass .. --remember`，之后定时 `run`
//! 即可（会话失效且已记住凭据时自动重登）。

mod cmds;

use crate::api::client::ApiClient;
use crate::api::model::{self, Session};
use crate::api::records::RecordRow;

pub fn main(args: Vec<String>) -> i32 {
    cmds::dispatch(args)
}

fn usage() {
    println!(
        r#"NekoSportsWorldTool CLI v{}（full 构建无参数启动 GUI）

用法：nekosportsworldtool <命令> [选项]
      nekosportsworldtool help [命令]   查看命令详情
      nekosportsworldtool --version     显示版本

账号
  login    --user <手机号> --pass <密码> [--remember]
           登录并保存会话；--remember 同时保存凭据，供会话失效时自动重登
           未给 --user/--pass 时读取环境变量 NEKO_USER / NEKO_PASS（适合青龙等定时任务）
  logout    登出并清理本地会话

跑步
  run      [--dist km] [--pace 秒/km|分:秒] [--altitude 米|最小-最大]
           [--ago 分钟 | --days-ago 0-3 --time HH:MM]
           [--face 0|1] [--seed n] [--route legacy|road]
           跑步全链：策略-点位-轨迹-提交-OBS-验证
  template --file <GPX/JSON>
           本地读取真实记录并分析海拔（不会上传）

AI 运动
  ai       --sport <id> [--mode min|count] [--score n] [--minutes n|--count n]
           min：按分钟 1-30；count：按次 5-1000（步长 5）
  ai-list                    AI 运动项目列表
  ai-records --sport <id>    AI 运动记录（按天分组）
  ai-info    --id <记录id>   AI 记录详情（JSON）

记录与统计
  records                    跑步记录列表
  record-info --rrid <id>    单条跑步记录详情（JSON）
  records-raw [--body JSON] [--out 文件]
                             记录接口原始探测（默认 body 为 {{}}）
  semester                   学期完成度
  cheat    [--page n]        违规自查
  rank     main --type 1|2|3 --sort 1|2 [--gender 0|1] [--date YYYY-MM-DD]
           排行榜（1个人 2班级 3院系；1日榜 2月榜）
  rank     indoor --range 1|2|3 [--gender 0|1]    室内榜（1日 2周 3月）
  rank     history --sort 1|2 [--gender 0|1]      历史榜

调试与维护
  obs-get    --key <对象键> [--out 文件]    拉取 OBS 轨迹对象并解码
  obs-sample [--dist m] [--dur 秒] [--rrid n] [--seed n] [--out 文件]
                                            本地生成 OBS 样本（不提交）
  update  [--check]                         检查更新；默认下载并自替换

通用
  --help, -h      显示帮助；可跟在任意命令后查看该命令详情
  --version, -V   显示版本

当天榜单通常在有效里程产生后才有数据；run 默认随机 1.0~1.5 km / 6~8 分配速 / 30-300 分钟前。"#,
        crate::update::current_version()
    );
}

/// 单条命令的详细用法；未知命令返回 false。
pub(crate) fn usage_cmd(cmd: &str) -> bool {
    let text = match cmd {
        "login" => {
            "login --user <手机号> --pass <密码> [--remember]\n\
             \n  --user      账号（手机号）\n\
             \n  --pass      密码\n\
             \n  --remember  同时把凭据落盘，之后会话失效会自动重登\n\
             \n  未给 --user/--pass 时读取环境变量 NEKO_USER / NEKO_PASS。"
        }
        "logout" => "logout\n\n  登出并清理本地会话（凭据保留与否见 login --remember）。",
        "run" => {
            "run [--dist km] [--pace 秒/km|分:秒] [--altitude 米|最小-最大]\n\
             \x20   [--ago 分钟 | --days-ago 0-3 --time HH:MM]\n\
             \x20   [--face 0|1] [--seed n] [--route legacy|road]\n\
             \n  --dist       目标距离（km），缺省随机 1.0~1.5\n\
             \n  --pace       目标配速，支持 360 或 6:00 两种写法，缺省随机 6:00~8:00\n\
             \n  --altitude   固定海拔（米）或 最小-最大 区间\n\
             \n  --ago        开跑于多少分钟前；与 --time 互斥\n\
             \n  --days-ago   0=今天 1=昨天 …，需配合 --time\n\
             \n  --time       开跑时刻 HH:MM（配合 --days-ago）\n\
             \n  --face       是否做人脸校验 0|1，缺省 1\n\
             \n  --seed       随机种子；0 表示按当前时间取\n\
             \n  --route      legacy=打卡点环，road=真实道路路由（缺省取配置）"
        }
        "template" => "template --file <GPX/JSON>\n\n  本地读取真实记录，分析海拔范围与累计升降（不会上传）。",
        "ai" => {
            "ai --sport <id> [--mode min|count] [--minutes n | --count n]\n\
             \n  --mode min     按分钟（1-30，默认 1），用 --minutes 指定\n\
             \n  --mode count   按次（5-1000，且为 5 的倍数），用 --count 指定\n\
             \n  --score n      当前模式下 --minutes / --count 的别名\n\
             \n  --sport 可用 ai-list 查看"
        }
        "ai-list" => "ai-list\n\n  列出可用的 AI 运动项目及 id。",
        "ai-records" => "ai-records --sport <id>\n\n  按天分组的 AI 运动记录。",
        "ai-info" => "ai-info --id <记录id>\n\n  单条 AI 记录全量详情（JSON）。",
        "records" => "records\n\n  跑步记录列表（最近一页）。",
        "record-info" => "record-info --rrid <id>\n\n  单条跑步记录原始详情（JSON）。",
        "records-raw" => {
            "records-raw [--body JSON] [--out 文件]\n\
             \n  --body  自定义请求体（缺省 {}，必须是合法 JSON）\n\
             \n  --out   把返回的数组写入文件，否则只打印首末条"
        }
        "semester" => "semester\n\n  学期有效次数 / 有效里程 / 完成度。",
        "cheat" => "cheat [--page n]\n\n  违规自查：是否被标记 + 全校违规列表（默认第 1 页）。",
        "rank" => {
            "rank main --type 1|2|3 --sort 1|2 [--gender 0|1] [--date YYYY-MM-DD]\n\
             rank indoor --range 1|2|3 [--gender 0|1]\n\
             rank history --sort 1|2 [--gender 0|1]\n\
             \n  --type    1 个人 2 班级 3 院系\n\
             \n  --sort    1 日榜 2 月榜\n\
             \n  --gender  0 女 1 男；不写则由服务端按协议缺省处理\n\
             \n  --date    仅主榜，格式 YYYY-MM-DD，缺省当天\n\
             \n  --range   仅室内榜：1 日 2 周 3 月"
        }
        "obs-get" => "obs-get --key <对象键> [--out 文件]\n\n  以签名 GET 拉取 OBS 轨迹对象，gzip+base64 字段会解码后保存（缺省 obs_real.json）。",
        "obs-sample" => {
            "obs-sample [--dist m] [--dur 秒] [--rrid n] [--seed n] [--out 文件]\n\
             \n  本地生成一份 OBS 对象样本用于结构对照，不会提交（缺省 obs_ours.json）。"
        }
        "update" => "update [--check]\n\n  检查更新；默认下载并自替换，--check 只打印结果与下载地址。",
        "help" => "help [命令]\n\n  查看总览或指定命令的详细用法。",
        _ => return false,
    };
    println!("{text}");
    true
}

/// 解析 `--key value`、`--key=value`、布尔开关 `--flag`。
///
/// 规则：
/// - 值可以是负数（`--seed -1` 不会被当成开关）；
/// - 不以 `-` 开头的裸词视为位置参数，跳过（如 `rank main`）；
/// - `--` 之后全部视为位置参数，不再解析。
pub(crate) fn parse_flags(rest: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let token = rest[i];
        i += 1;
        if token == "--" {
            break;
        }
        let Some(body) = token.strip_prefix('-') else {
            continue; // 位置参数
        };
        let body = body.trim_start_matches('-');
        if body.is_empty() {
            continue;
        }
        if let Some((k, v)) = body.split_once('=') {
            out.push((k.to_string(), v.trim_matches('"').to_string()));
            continue;
        }
        let next = rest.get(i).copied();
        let is_value = match next {
            Some(n) if !n.starts_with('-') => true,
            // 负数（-1、-0.5）仍算值
            Some(n) if n.parse::<f64>().is_ok() => true,
            _ => false,
        };
        if is_value {
            out.push((body.to_string(), rest[i].to_string()));
            i += 1;
        } else {
            out.push((body.to_string(), "1".into()));
        }
    }
    out
}

/// 拒绝命令未声明的参数，避免 `--distn 3` 这类拼写错误被静默忽略。
pub(crate) fn reject_unknown(flags: &[(String, String)], allowed: &[&str]) -> Result<(), String> {
    for (k, _) in flags {
        if !allowed.contains(&k.as_str()) {
            let list = allowed
                .iter()
                .map(|a| format!("--{a}"))
                .collect::<Vec<_>>()
                .join(" ");
            let hint = if list.is_empty() {
                "该命令不接受任何参数".to_string()
            } else {
                format!("可用：{list}")
            };
            return Err(format!("未知参数 --{k}（{hint}）"));
        }
    }
    Ok(())
}

pub(crate) fn get<'a>(flags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

pub(crate) fn logger() -> impl FnMut(&str) {
    |s: &str| println!("{}", crate::textlog::clean(s))
}

pub(crate) fn silent() -> impl FnMut(&str) {
    |_s: &str| {}
}

/// 自动登录用的凭据：已记住的账号优先，其次环境变量 NEKO_USER / NEKO_PASS。
pub(crate) fn creds() -> Option<(String, String)> {
    let cfg = model::load_config();
    if cfg.remember && !cfg.username.is_empty() && !cfg.password.is_empty() {
        return Some((cfg.username, cfg.password));
    }
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
    Some((env("NEKO_USER")?, env("NEKO_PASS")?))
}

/// 自动登录（GUI 记住密码 / login --remember 落盘的凭据 / 环境变量）。
pub(crate) fn auto_login(identity: &model::HeaderIdentity) -> Result<Session, String> {
    let Some((user, pass)) = creds() else {
        return Err(
            "未登录：先执行 login 子命令，或设置环境变量 NEKO_USER / NEKO_PASS，或在 GUI 勾选记住密码"
                .into(),
        );
    };
    println!("[login] 使用已保存的账号或环境变量自动登录…");
    let mut client = ApiClient::new(identity.clone(), None);
    crate::api::login::login(&mut client, &user, &pass, &mut silent())
}

/// 构造已登录客户端；本地会话失效时自动重登一次。
pub(crate) fn make_client() -> Result<ApiClient, String> {
    let identity = model::load_identity();
    let client = {
        let sess = model::load_session();
        if sess.is_logged_in() {
            ApiClient::new(identity, Some(sess))
        } else {
            let s = auto_login(&identity)?;
            ApiClient::new(identity, Some(s))
        }
    };
    // 不做前置健康检查：业务错误（如新账号无学期数据）不等于会话过期，
    // 贸然重登会把好 session 删掉并触发 10121。会话真正过期时业务接口
    // 自然返回 401，由调用方处理。
    Ok(client)
}

pub(crate) fn now_ms() -> i64 {
    crate::crypto::envelope::now_ms()
}

pub(crate) fn fmt_hms(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

pub(crate) fn print_rows(rows: &[RecordRow]) {
    println!(
        "{:<16} {:>8} {:>8} {:>6} {:>6} {:>4} rrid",
        "时间", "距离m", "时长", "配速", "步频", "达标"
    );
    for r in rows {
        let full = fmt_hms(r.start_time);
        // 去掉年份（"YYYY-MM-DD HH:MM:SS" → "MM-DD HH:MM:SS"）；空串时原样输出，避免越界 panic
        let t = full.get(5..).unwrap_or(&full).to_string();
        let pace = if r.total_dis > 0.0 && r.total_time > 0 {
            let p = r.total_time as f64 / (r.total_dis / 1000.0);
            format!("{}:{:02}", (p / 60.0) as i64, (p as i64) % 60)
        } else {
            "-".into()
        };
        println!(
            "{:<16} {:>8.0} {:>8} {:>6} {:>6} {:>4} {}",
            t,
            r.total_dis,
            format!(
                "{}:{:02}:{:02}",
                r.total_time / 3600,
                r.total_time % 3600 / 60,
                r.total_time % 60
            ),
            pace,
            r.avg_step_freq,
            if r.complete { "是" } else { "否" },
            r.rrid
        );
    }
}

pub(crate) fn jstr(v: &serde_json::Value, keys: &[&str]) -> String {
    for k in keys {
        match v.get(k) {
            Some(serde_json::Value::String(s)) => return s.clone(),
            Some(serde_json::Value::Number(n)) => return n.to_string(),
            _ => {}
        }
    }
    "-".into()
}

/// 配速解析：`360` 与 `6:00` 等价；非法或非正数返回 None。
pub(crate) fn parse_pace(v: &str) -> Option<f32> {
    let p = if let Some((m, s)) = v.split_once(':') {
        m.parse::<f32>().ok()? * 60.0 + s.parse::<f32>().ok()?
    } else {
        v.parse::<f32>().ok()?
    };
    (p.is_finite() && p > 0.0).then_some(p)
}

/// 按字符截断，避免在 UTF-8 多字节边界上切片 panic。
pub(crate) fn clip(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}
