//! 轨迹后处理：起终点哨兵（索引1 type=5 全零 / 末点 type=6 / 首点 type∈{0,7}）、
//! 后半段 type=1/2 散布、首点速度取首个非零值。
//!
//! 不再做「结尾断崖」的时间外扩与索引 2 速度修正：累计时长/距离/步数必须与
//! 速度、步频积分保持一致，任何末点改写都会破坏同一份累计数据源。

use super::geom::{round_to, Rng};
use super::model::GenPoint;

pub fn apply_post_fixes(locs: &mut [GenPoint], rng: &mut Rng, _start_ms: i64) {
    if locs.len() > 2 {
        // 起点哨兵（type=5→setStartPoint）：全零累计 + state=1
        let sp = &mut locs[1];
        sp.ptype = 5;
        sp.state = 1;
        sp.locType = 1;
        sp.radius = round_to(rng.uniform(2.0, 3.5), 2);
        sp.speed = 0.0;
        sp.avgSpeed = 0.0;
        sp.bdS = 0.0;
        sp.totalDis = 0.0;
        sp.validDis = 0.0;
        sp.totalTime = 0;
        sp.validTime = 0;
        sp.steps = 0;
        // 终点哨兵
        let last = locs.len() - 1;
        let e = &mut locs[last];
        e.ptype = 6;
        e.locType = 1;
        e.radius = round_to(rng.uniform(1.5, 3.0), 2);
        // 后半段散布 type=1/2/8（67-99% 行程区间；type=8 真人低频出现）
        let from = (locs.len() as f64 * 0.67) as usize;
        for i in from..locs.len().saturating_sub(2) {
            if rng.random() < 0.06 {
                locs[i].ptype = rng.choice(&[1, 1, 2, 8]);
                locs[i].locType = 1;
            }
        }
    }
    // 首点 = 起点：totalTime=0、type∈{0,7}、速度取首个非零值
    if !locs.is_empty() {
        locs[0].totalTime = 0;
        locs[0].validTime = 0;
        locs[0].state = 1;
        locs[0].ptype = rng.choice(&[0, 0, 0, 7]);
        locs[0].locType = 1;
        locs[0].radius = round_to(rng.uniform(1.5, 3.0), 2);
        for j in 1..locs.len() {
            if locs[j].avgSpeed > 0.0 {
                locs[0].avgSpeed = locs[j].avgSpeed;
                locs[0].bdS = locs[j].bdS;
                break;
            }
        }
        if locs[0].speed == 0.0 {
            locs[0].speed = round_to(rng.uniform(7.0, 13.0), 4);
        }
    }
}
