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
/// （每日一次限制已移除，始终返回 false）
pub fn used_today() -> bool {
    false
}

/// 记录今天已使用；同一天重复调用只覆盖为同一日期。
/// （每日一次限制已移除，空实现）
pub fn mark_used() -> Result<(), String> {
    Ok(())
}
