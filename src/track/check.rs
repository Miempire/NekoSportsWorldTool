//! 轨迹几何合法性校验：穿建筑 / 越围栏 / 偏离路网。
//!
//! 轨迹与建筑/围栏/路网均须为同一工作系（BD-09）。异常点（type=-1）为零位移
//! 或随机漂移，不代表真实行进，不参与校验。

use route_planner::{point_in_polygon, segment_intersects_polygon, Coord, RoadGraph};

use super::model::{GenPoint, Track};

/// 校验结果；`samples` 保留前几处违规的人读描述，供日志输出。
pub struct TrackCheck {
    pub building_hits: usize,
    pub fence_outside: usize,
    pub off_network: usize,
    pub max_off_m: f64,
    pub samples: Vec<String>,
}

impl TrackCheck {
    pub fn clean(&self) -> bool {
        self.building_hits == 0 && self.fence_outside == 0 && self.off_network == 0
    }

    pub fn lines(&self) -> Vec<String> {
        if self.clean() {
            return vec![format!(
                "√ [check] 几何校验通过（最大离线 {:.1}m）",
                self.max_off_m
            )];
        }
        let mut out = vec![format!(
            "⚠ [check] 穿建筑 {} 段 / 越围栏 {} 点 / 离线 {} 点（最大 {:.1}m）",
            self.building_hits, self.fence_outside, self.off_network, self.max_off_m
        )];
        out.extend(self.samples.iter().take(3).cloned());
        out
    }
}

fn coord(p: &GenPoint) -> Coord {
    Coord::new(p.gLng, p.gLat)
}

fn label(p: &GenPoint) -> String {
    format!("点{} ({:.6},{:.6})", p.id, p.gLat, p.gLng)
}

/// 逐段/逐点校验。
///
/// `buildings` 为 BD-09 建筑外环；`fences` 为 BD-09 围栏环（(lat,lng)）；
/// `net` 为已对齐到 BD-09 的路网；`off_limit_m` 为允许的路网偏离阈值。
pub fn check(
    track: &Track,
    buildings: &[Vec<Coord>],
    fences: &[Vec<(f64, f64)>],
    net: Option<&RoadGraph>,
    off_limit_m: f64,
) -> TrackCheck {
    let mut out = TrackCheck {
        building_hits: 0,
        fence_outside: 0,
        off_network: 0,
        max_off_m: 0.0,
        samples: Vec::new(),
    };
    let locs = &track.locations;
    if locs.len() < 2 {
        return out;
    }
    let fence_polys: Vec<Vec<Coord>> = fences
        .iter()
        .map(|r| r.iter().map(|p| Coord::new(p.1, p.0)).collect())
        .collect();

    for w in locs.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if a.ptype == -1 || b.ptype == -1 {
            continue;
        }
        let (ca, cb) = (coord(a), coord(b));
        if ca == cb {
            continue;
        }
        if !buildings.is_empty()
            && buildings
                .iter()
                .any(|ring| segment_intersects_polygon(ring, ca, cb))
        {
            out.building_hits += 1;
            if out.samples.len() < 3 {
                out.samples.push(format!("穿建筑：{} → {}", label(a), label(b)));
            }
        }
    }

    if !fence_polys.is_empty() {
        for p in locs.iter().filter(|p| p.ptype != -1) {
            let c = coord(p);
            if !fence_polys.iter().any(|poly| point_in_polygon(poly, c)) {
                out.fence_outside += 1;
                if out.samples.len() < 3 {
                    out.samples.push(format!("越围栏：{}", label(p)));
                }
            }
        }
    }

    if let Some(g) = net {
        for p in locs.iter().filter(|p| p.ptype != -1) {
            let d = match g.nearest_edge(g.to_m(coord(p))) {
                Some((_, _, _, d)) => d,
                None => continue,
            };
            if d > out.max_off_m {
                out.max_off_m = d;
            }
            if d > off_limit_m {
                out.off_network += 1;
                if out.samples.len() < 3 {
                    out.samples.push(format!("离线 {:.1}m：{}", d, label(p)));
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(id: i64, lat: f64, lng: f64) -> GenPoint {
        GenPoint {
            id,
            flag: 1,
            lat,
            lng,
            gLat: lat,
            gLng: lng,
            speed: 10.0,
            avgSpeed: 3.0,
            radius: 3.0,
            accuracy: 3.0,
            ptype: 0,
            locType: 0,
            hasAltitude: true,
            totalTime: 5,
            totalDis: 15.0,
            validDis: 15.0,
            validTime: 5,
            steps: 9,
            stepDistance: 1.6,
            gainTime: String::new(),
            gainTimeMs: 5000,
            queueNum: 0,
            coorType: "gcj02".into(),
            bdA: 82.0,
            bdD: 0.0,
            bdS: 1.5,
            bdG: 1,
            count: 20,
            dtr: 0.0,
            state: 0,
            locationId: String::new(),
        }
    }

    fn track(points: Vec<GenPoint>) -> Track {
        let first = points[0].clone();
        Track {
            totalTime: 5,
            totalDistance: 15.0,
            validDistance: 15.0,
            validTime: 5,
            startTime: 1_700_000_000_000,
            startLatitude: first.gLat,
            startLongitude: first.gLng,
            locations: points,
            totalSteps: 9,
            speedPerTenSec: Vec::new(),
            stepsPerTenSec: Vec::new(),
            segments: Vec::new(),
        }
    }

    #[test]
    fn detects_crossing_building() {
        let t = track(vec![point(0, 0.000, 121.000), point(1, 0.001, 121.000)]);
        let ring = vec![
            Coord::new(120.9998, 0.0003),
            Coord::new(121.0002, 0.0003),
            Coord::new(121.0002, 0.0007),
            Coord::new(120.9998, 0.0007),
        ];
        let report = check(&t, &[ring], &[], None, 15.0);
        assert_eq!(report.building_hits, 1);
        assert!(!report.clean());
    }

    #[test]
    fn detects_point_outside_fence() {
        let t = track(vec![point(0, 0.000, 121.000), point(1, 0.001, 121.000)]);
        let fence = vec![(30.0, 120.0), (30.0, 120.1), (30.1, 120.1), (30.1, 120.0)];
        let report = check(&t, &[], &[fence], None, 15.0);
        assert_eq!(report.fence_outside, 2);
    }
}
