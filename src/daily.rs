//! 每日使用限制：本地记录当天是否已成功提交过跑步。
//!
//! 仅依赖本地时间与数据目录中的 daily_usage.json，不经过服务端校验。

use chrono::Local;

const FILE: &str = "daily_usage.json";

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct DailyUsage {
    /// 最近一次成功提交跑步的本地日期（YYYY-MM-DD）。
    #[serde(default)]
    last_used_date: String,
}

fn usage_path() -> std::path::PathBuf {
    crate::platform::data_dir().join(FILE)
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// 今天是否已经成功提交过跑步。
pub fn used_today() -> bool {
    let Ok(text) = std::fs::read_to_string(usage_path()) else {
        return false;
    };
    let usage: DailyUsage = serde_json::from_str(&text).unwrap_or_default();
    usage.last_used_date == today()
}

/// 记录今天已使用；同一天重复调用只覆盖为同一日期。
pub fn mark_used() -> Result<(), String> {
    let usage = DailyUsage {
        last_used_date: today(),
    };
    let json = serde_json::to_string_pretty(&usage)
        .map_err(|e| format!("序列化每日使用记录失败: {e}"))?;
    let path = usage_path();
    std::fs::write(&path, json).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}
