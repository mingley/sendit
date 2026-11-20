//! Course wayfinding from the map's ordered marker centerline. Alternate bank lines can
//! remain to either side; progress describes position along the course, independent of FPS.
use bevy::prelude::Vec3;
use sendit_sim::Body;

struct Leg {
    next: usize,
    progress: f32,
    start: bool,
    finish: bool,
}

fn locate(points: &[Vec3], pos: Vec3) -> Option<Leg> {
    let mut nearest = f32::INFINITY;
    let mut traveled = 0.;
    let mut total = 0.;
    let mut next = 1;
    for (index, edge) in points.windows(2).enumerate() {
        let delta = edge[1] - edge[0];
        let length = delta.length();
        if length < 0.001 {
            continue;
        }
        let t = ((pos - edge[0]).dot(delta) / delta.length_squared()).clamp(0., 1.);
        let distance = (pos - edge[0].lerp(edge[1], t)).length_squared();
        if distance <= nearest {
            nearest = distance;
            next = index + 1;
            traveled = total + t * length;
        }
        total += length;
    }
    if total <= 0. {
        return None;
    }
    Some(Leg {
        next,
        progress: traveled / total,
        start: traveled < 0.3,
        finish: traveled > total - 0.4 && nearest < 10. * 10.,
    })
}

pub fn readout(points: &[Vec3], body: &Body, yaw: f32) -> (String, String) {
    let Some(leg) = locate(points, body.pos) else {
        return (String::new(), String::new());
    };
    if leg.finish && body.grounded {
        return ("   FINISH   ".into(), "   F5  Ride again   ".into());
    }
    if leg.start {
        return (
            "   LAUNCH   ".into(),
            "   W  Approach the first bank   |   A / D  Steer   ".into(),
        );
    }
    let target = points[leg.next];
    let delta = target - body.pos;
    let turn = Vec3::new(yaw.cos(), 0., yaw.sin()).dot(delta);
    let forward = Vec3::new(yaw.sin(), 0., -yaw.cos()).dot(delta);
    let heading = if forward > turn.abs() {
        "ahead"
    } else if forward < -turn.abs() {
        "behind"
    } else if turn > 0. {
        "right"
    } else {
        "left"
    };
    let gate = if leg.next == points.len() - 1 {
        "FINISH".into()
    } else {
        format!("GATE {}", leg.next)
    };
    let top = format!(
        "   {gate}    {:.0} m {heading}   |   {:.0}%   ",
        delta.length(),
        leg.progress * 100.
    );
    let hint = if body.pos.y < points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min) - 2.5 {
        "   Off course   |   F5  Restart   ".into()
    } else {
        String::new()
    };
    (top, hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_tracks_bends_and_alternate_lateral_lines() {
        let path = [
            Vec3::ZERO,
            Vec3::new(0., -3., -10.),
            Vec3::new(10., -3., -10.),
        ];
        let start = locate(&path, Vec3::Z).unwrap();
        assert!(start.start);
        let leg = locate(&path, Vec3::new(5., -3., -11.)).unwrap();
        assert_eq!(leg.next, 2);
        assert!((0.70..0.80).contains(&leg.progress));
        assert!(!leg.finish);
        assert!(locate(&path, path[2]).unwrap().finish);
        assert!(locate(&[], Vec3::ZERO).is_none());
    }
}
