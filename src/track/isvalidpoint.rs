//! App 端点位有效性评分 —— `libswsport.so@0x10e84` 的精确复现。
//!
//! 来源：`sw-proto-suite-v3.0.2/isvalidpoint.py`（该文件从反汇编逐指令还原，
//! 经 29,232 组 Unicorn 差分验证 100% 等价）。
//!
//! 评分表
//! ------
//! ```text
//! w8  = 基础分(w3)        w3<3→0, <5→5, <8→15, <15→25, ≥15→30
//! w9  = 点位类型分(w2)    w2==1→30, w2==2→20, 其它→15
//! w10 = 距离分(s1)        s1<10→30, <50→20, <100→10, 其它→0
//! w11 = 速度分(s0)        s0>0→10, 否则→0
//!
//! 判定: (w4 & 0xff) != 0  &&  (w5 & 0xff) == 0  &&  (w8+w9+w10+w11) > 65
//! ```
//!
//! 参数含义（AAPCS64 / JNI 静态注册）
//! - `w2`  点位类型（生成器用 1 = 有效采集点）
//! - `s0`  瞬时速度（m/s）
//! - `s1`  与上一点的间距（米）
//! - `w3`  点位类型/序号分档（生成器用 20 = 高档）
//! - `w4`  标志位 A（默认 1）
//! - `w5`  标志位 B（默认 0）
//!
//! 生成器的最保险组合：`w2=1, w3>=15, s0>0, s1<10` → 100 分（阈值 65）。

/// 判定阈值：总分必须 **严格大于** 该值。
pub const PASS_THRESHOLD: i32 = 65;

/// `w8` —— 基础分（由 `w3` 决定）。
pub fn base_score(w3: i64) -> i32 {
    if w3 < 3 {
        0
    } else if w3 < 5 {
        5
    } else if w3 < 8 {
        15
    } else if w3 < 15 {
        25
    } else {
        30
    }
}

/// `w9` —— 点位类型分（由 `w2` 决定）。
pub fn point_type_score(w2: i64) -> i32 {
    match w2 {
        1 => 30,
        2 => 20,
        _ => 15,
    }
}

/// `w10` —— 距离分（由 `s1` 决定，单位米）。
pub fn distance_score(s1: f64) -> i32 {
    if s1 < 10.0 {
        30
    } else if s1 < 50.0 {
        20
    } else if s1 < 100.0 {
        10
    } else {
        0
    }
}

/// `w11` —— 速度分（由 `s0` 决定，单位 m/s）。
pub fn speed_score(s0: f64) -> i32 {
    if s0 > 0.0 {
        10
    } else {
        0
    }
}

/// 总分。
pub fn total_score(w2: i64, s0: f64, s1: f64, w3: i64) -> i32 {
    base_score(w3) + point_type_score(w2) + distance_score(s1) + speed_score(s0)
}

/// 与 `libswsport.so@0x10e84` 完全等价的判定。
pub fn is_valid_point(w2: i64, s0: f64, s1: f64, w3: i64, w4: i64, w5: i64) -> bool {
    if (w4 & 0xFF) == 0 {
        return false;
    }
    if (w5 & 0xFF) != 0 {
        return false;
    }
    total_score(w2, s0, s1, w3) > PASS_THRESHOLD
}

/// 生成器侧的批量自检：返回 `(是否全部通过, 最低分)`。
///
/// `speeds` 为瞬时速度（m/s），`gaps` 为与上一点的间距（米，首点用自身速度×dt 代替）。
/// 固定按 `w2 = 1`（有效采集点）、`w3 = 20`（高档）、`w4 = 1`、`w5 = 0`。
pub fn validate_series(speeds: &[f64], gaps: &[f64]) -> (bool, i32) {
    let mut all_ok = true;
    let mut min_score = i32::MAX;
    for (i, &s0) in speeds.iter().enumerate() {
        let s1 = gaps.get(i).copied().unwrap_or(s0.max(0.0));
        if !is_valid_point(1, s0, s1, 20, 1, 0) {
            all_ok = false;
        }
        let sc = total_score(1, s0, s1, 20);
        min_score = min_score.min(sc);
    }
    (all_ok, if min_score == i32::MAX { 0 } else { min_score })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_table_matches_reference() {
        assert_eq!(base_score(0), 0);
        assert_eq!(base_score(3), 5);
        assert_eq!(base_score(5), 15);
        assert_eq!(base_score(8), 25);
        assert_eq!(base_score(15), 30);
        assert_eq!(base_score(20), 30);

        assert_eq!(point_type_score(1), 30);
        assert_eq!(point_type_score(2), 20);
        assert_eq!(point_type_score(0), 15);

        assert_eq!(distance_score(9.9), 30);
        assert_eq!(distance_score(10.0), 20);
        assert_eq!(distance_score(99.9), 10);
        assert_eq!(distance_score(100.0), 0);
    }

    #[test]
    fn standard_generator_combo_passes() {
        // w2=1(30) + w3=20(30) + s1=5(30) + s0>0(10) = 100 > 65
        assert_eq!(total_score(1, 3.0, 5.0, 20), 100);
        assert!(is_valid_point(1, 3.0, 5.0, 20, 1, 0));
    }

    #[test]
    fn large_gap_still_passes_when_base_high() {
        // s1 数千米也还有 30+30+0+10 = 70 > 65
        assert_eq!(total_score(1, 3.0, 5000.0, 20), 70);
        assert!(is_valid_point(1, 3.0, 5000.0, 20, 1, 0));
    }

    #[test]
    fn flags_can_reject() {
        assert!(!is_valid_point(1, 3.0, 5.0, 20, 0, 0), "w4=0 必须拒绝");
        assert!(!is_valid_point(1, 3.0, 5.0, 20, 1, 1), "w5!=0 必须拒绝");
    }

    /// `validate_series` 固定 w2=1 / w3=20：正常点的分数是 100，
    /// 只有速度项（10 分）可能扣 → 最低 90，**仍高于阈值 65**。
    /// 所以这里验证的是「分数如实反映扣分」，不是「一定判不通过」。
    #[test]
    fn series_validation_reports_min_score() {
        // 全部有速度：满分 100
        let (ok, min) = validate_series(&[3.0, 3.1, 2.9], &[5.0, 5.0, 5.0]);
        assert!(ok, "正常点应全部通过");
        assert_eq!(min, 100, "满分应为 100");

        // 中间一个 0 速度：丢 10 分 → 90，仍然通过（阈值 65）
        let (ok2, min2) = validate_series(&[3.0, 0.0, 2.9], &[5.0, 5.0, 5.0]);
        assert!(ok2, "0 速度只丢 10 分，90 仍高于阈值");
        assert_eq!(min2, 90, "0 速度点应为 90 分");
        assert!(min2 < min, "带 0 速度点时最低分必须变低");
    }

    /// 真正会判不通过的组合：基础分低（w3 小）+ 距离大。
    /// `validate_series` 固定 w3=20，所以这里直接调 `is_valid_point` 验证边界。
    #[test]
    fn low_base_score_with_large_gap_fails() {
        // w3=0 → w8=0，w2=1 → 30，s1=500 → 0，s0>0 → 10，合计 40 ≤ 65
        assert_eq!(total_score(1, 3.0, 500.0, 0), 40);
        assert!(!is_valid_point(1, 3.0, 500.0, 0, 1, 0));
        // 同样条件但 w3=15（30 分）→ 70 > 65，通过
        assert_eq!(total_score(1, 3.0, 500.0, 15), 70);
        assert!(is_valid_point(1, 3.0, 500.0, 15, 1, 0));
    }
}
