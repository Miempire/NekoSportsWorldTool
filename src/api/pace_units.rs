//! `runModePolicy` 的 `speedTop` / `speedBottom` 单位判定。
//!
//! 移植自 `sw-proto-suite-v3.0.2/pace_units.py`（该文件基于两个真实校区实测）。
//!
//! 为什么必须单独做单位判定
//! ------------------------
//! 这两个字段**在不同校区填的不是同一个单位**：
//!
//! · 宝盖校区   `speedTop=4.0   speedBottom=10.0`
//!     按【分/公里】读 → 4'00" ~ 10'00"，与本机 5'30" 记录稳定通过一致；
//!     按【m/s】读     → 4'10" ~ 1'40"，那样 5'30" 必被判无效 —— 与事实矛盾。
//! · 另一校区   `speedTop=2.17  speedBottom=10.0`
//!     按分/公里读 → 2'10" ~ 10'00"，2'10" 比世界纪录还快 ⇒ 不可能；
//!     按 m/s 读   → 7'41" ~ 1'40"，且当天 7'46" 的记录确实被判无效 ⇒ 吻合。
//!
//! 判据：默认按【分/公里】读；只有当这个读法**物理上不可能**
//! （最快边界比 3'00"/km 还快）时，才改按 m/s 读。

/// 单个 policy 数值 ≤ 该阈值时，按「分/公里」读。
const MIN_KM_GUESS_MAX: f64 = 30.0;
/// 校园规则的最快边界不可能快于此（世界纪录约 2'50"/km）。
const IMPLAUSIBLE_FAST: f64 = 180.0;

/// 一对 policy 值按「分/公里」读 → `(最快秒/公里, 最慢秒/公里)`。
///
/// 必须**成对**判断：`speedBottom=100` 单看像「100 秒/公里」，
/// 在一对里其实是「100 分/公里」即「不限」；逐值判会把 `(2.0, 100.0)`
/// 读成 1'40"~2'00" 这种荒唐窗口。
fn bounds_min_km(a: f64, b: f64) -> (f64, f64) {
    if a > MIN_KM_GUESS_MAX && b > MIN_KM_GUESS_MAX {
        // 已经是「秒/公里」形态
        (a.min(b), a.max(b))
    } else {
        (a.min(b) * 60.0, a.max(b) * 60.0)
    }
}

/// 一对 policy 值按「m/s」读 → `(最快秒/公里, 最慢秒/公里)`。
fn bounds_m_s(a: f64, b: f64) -> (f64, f64) {
    // 速度大的更快（秒数更小）
    (1000.0 / a.max(b), 1000.0 / a.min(b))
}

/// 自动判定单位 → `(最快秒/公里, 最慢秒/公里, 单位标签)`。
///
/// 单位标签：`"min_km"`（默认）| `"m_s"`（改判）| `"s_km"`（值本身已是秒）。
pub fn resolve(lo: f64, hi: f64) -> Option<(f64, f64, &'static str)> {
    if !(lo.is_finite() && hi.is_finite()) || lo <= 0.0 || hi <= 0.0 {
        return None;
    }
    let (fast, slow) = bounds_min_km(lo, hi);
    // 默认口径给出的「最快」比世界纪录还快 ⇒ 只可能是 m/s
    if fast < IMPLAUSIBLE_FAST {
        let (f2, s2) = bounds_m_s(lo, hi);
        if f2.is_finite() && s2.is_finite() && s2 > f2 {
            return Some((f2, s2, "m_s"));
        }
    }
    if slow <= fast {
        return None;
    }
    let tag = if lo > MIN_KM_GUESS_MAX && hi > MIN_KM_GUESS_MAX {
        "s_km"
    } else {
        "min_km"
    };
    Some((fast, slow, tag))
}

/// 秒/公里 → `5'40"`；无效值返回 `-`。
pub fn fmt_pace(sec: f64) -> String {
    if !sec.is_finite() || sec <= 0.0 {
        return "-".into();
    }
    let total = sec.round() as i64;
    format!("{}'{:02}\"", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baogai_4_10_is_min_per_km() {
        let (f, s, tag) = resolve(4.0, 10.0).unwrap();
        assert_eq!((f.round() as i64, s.round() as i64), (240, 600));
        assert_eq!(tag, "min_km");
    }

    #[test]
    fn implausible_speed_falls_back_to_m_s() {
        let (f, s, tag) = resolve(2.17, 10.0).unwrap();
        assert_eq!(tag, "m_s");
        assert_eq!(f.round() as i64, 100);
        assert_eq!(s.round() as i64, 461);
    }

    #[test]
    fn already_seconds_passthrough() {
        let (f, s, tag) = resolve(240.0, 600.0).unwrap();
        assert_eq!((f.round() as i64, s.round() as i64), (240, 600));
        assert_eq!(tag, "s_km");
    }

    #[test]
    fn wide_bottom_means_unlimited_not_100_seconds() {
        // 4.0/100.0 必须读成 240~6000（分/公里），不能读成 240~6000 秒以外的形态
        let (f, s, tag) = resolve(4.0, 100.0).unwrap();
        assert_eq!((f.round() as i64, s.round() as i64), (240, 6000));
        assert_eq!(tag, "min_km");
    }

    #[test]
    fn real_account_2_0_100_is_m_s() {
        let (f, s, tag) = resolve(2.0, 100.0).unwrap();
        assert_eq!(tag, "m_s");
        assert_eq!((f.round() as i64, s.round() as i64), (10, 500));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(resolve(0.0, 10.0).is_none());
        assert!(resolve(f64::NAN, 10.0).is_none());
    }

    #[test]
    fn fmt_pace_rounds() {
        assert_eq!(fmt_pace(339.5), "5'40\"");
        assert_eq!(fmt_pace(0.0), "-");
    }
}
