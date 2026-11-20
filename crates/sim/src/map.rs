//! Shared collision, rendering, and navigation geometry. Coordinates are meters, Y up.
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::PathBuf};

pub const MAX_MAP_BYTES: usize = 48 * 1024;
const WORLD_LIMIT: f32 = 2048.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    Floor,
    #[default]
    Wall,
    Cover,
    Trim,
    Target,
    Ramp,
    Step,
    SiteA,
    SiteB,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    #[default]
    X,
    Z,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Solid {
    pub min: Vec3,
    pub max: Vec3,
    pub material: Material,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ramp {
    pub min: Vec3,
    pub max: Vec3,
    pub axis: Axis,
    pub rising: bool,
    pub material: Material,
}

impl Ramp {
    /// Height of the extended top plane. Callers can test the XZ footprint separately.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let (q, lo, hi) = match self.axis {
            Axis::X => (x, self.min.x, self.max.x),
            Axis::Z => (z, self.min.z, self.max.z),
        };
        let t = if self.rising { q - lo } else { hi - q };
        self.min.y + t / (hi - lo) * (self.max.y - self.min.y)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    pub pos: Vec3,
    pub yaw: f32,
    pub team: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    pub pos: Vec3,
    pub radius: f32,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arena {
    pub name: String,
    pub boxes: Vec<Solid>,
    #[serde(default)]
    pub ramps: Vec<Ramp>,
    pub spawns: Vec<Spawn>,
    #[serde(default)]
    pub sites: Vec<Site>,
    /// Traversable landmarks in world space, at foot height.
    #[serde(default)]
    pub nav: Vec<Vec3>,
}

/// Identifiers are also used in the network protocol. A map name is a single filename stem.
pub fn valid_map_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn in_world(p: Vec3) -> bool {
    p.is_finite() && p.abs().max_element() <= WORLD_LIMIT
}

fn valid_bounds(lo: Vec3, hi: Vec3) -> bool {
    in_world(lo) && in_world(hi) && (hi - lo).min_element() >= 0.001
}

fn embedded(name: &str) -> Option<&'static [u8]> {
    match name {
        "drill" => Some(include_bytes!("../../../maps/drill.json")),
        "relay" => Some(include_bytes!("../../../maps/relay.json")),
        _ => None,
    }
}

pub fn is_builtin(name: &str) -> bool {
    embedded(name).is_some()
}

impl Arena {
    /// Built-in arenas are embedded in the server and client. Install additional files in
    /// SENDIT_MAP_DIR, or in maps/ relative to the working directory.
    pub fn load(name: &str) -> Result<Self, String> {
        Self::load_with_source(name).map(|(arena, _)| arena)
    }

    /// Preserve the validated source of installed maps for clients joining this host.
    pub fn load_with_source(name: &str) -> Result<(Self, Option<Vec<u8>>), String> {
        if !valid_map_name(name) {
            return Err("invalid map name (use letters, digits, dash, or underscore)".into());
        }
        let (arena, source) = if let Some(bytes) = embedded(name) {
            (Self::from_json(bytes)?, None)
        } else {
            let dir = std::env::var_os("SENDIT_MAP_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("maps"));
            let path = dir.join(format!("{name}.json"));
            let mut bytes = Vec::new();
            File::open(&path)
                .and_then(|f| f.take(MAX_MAP_BYTES as u64 + 1).read_to_end(&mut bytes))
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            (Self::from_json(&bytes)?, Some(bytes))
        };
        if arena.name != name {
            return Err(format!("map identifier in file must equal '{name}'"));
        }
        Ok((arena, source))
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MAP_BYTES {
            return Err(format!("map exceeds {MAX_MAP_BYTES} bytes"));
        }
        let result: Self = serde_json::from_slice(bytes).map_err(|e| format!("map JSON: {e}"))?;
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_map_name(&self.name) {
            return Err("invalid map identifier".into());
        }
        if self.boxes.len() + self.ramps.len() == 0
            || self.boxes.len() + self.ramps.len() > 512
            || self.spawns.is_empty()
            || self.spawns.len() > 64
            || self.sites.len() > 8
            || self.nav.len() > 256
        {
            return Err("map requires 1–512 solids and 1–64 spawns, up to 8 sites and 256 navigation points".into());
        }
        for (index, b) in self.boxes.iter().enumerate() {
            if !valid_bounds(b.min, b.max) {
                return Err(format!("invalid bounds in box {index}"));
            }
        }
        for (index, r) in self.ramps.iter().enumerate() {
            if !valid_bounds(r.min, r.max) {
                return Err(format!("invalid bounds in ramp {index}"));
            }
        }
        for (index, s) in self.spawns.iter().enumerate() {
            if !in_world(s.pos) || s.team > 1 || !s.yaw.is_finite() {
                return Err(format!("invalid position, team, or yaw in spawn {index}"));
            }
        }
        for (index, s) in self.sites.iter().enumerate() {
            if !in_world(s.pos) || !(0.25..=32.0).contains(&s.radius) || !valid_map_name(&s.name) {
                return Err(format!("invalid site {index}"));
            }
        }
        if self.nav.iter().any(|p| !in_world(*p)) {
            return Err("navigation point outside world bounds".into());
        }
        Ok(())
    }

    /// Returns the distance in meters to the first solid, clipped to max_distance.
    pub fn ray_distance(&self, from: Vec3, direction: Vec3, max_distance: f32) -> f32 {
        let Some(dir) = direction.try_normalize() else {
            return 0.0;
        };
        if !from.is_finite() || max_distance.is_nan() || max_distance <= 0.0 {
            return 0.0;
        }
        let mut best = max_distance;
        for b in &self.boxes {
            if let Some((near, _)) = ray_box(from, dir, b.min, b.max, best) {
                best = near;
            }
        }
        for r in &self.ramps {
            let Some((mut near, mut far)) = ray_box(from, dir, r.min, r.max, best) else {
                continue;
            };
            // Clip the ray segment inside the enclosing box against the sloped top face.
            let (span, velocity) = match r.axis {
                Axis::X => (r.max.x - r.min.x, dir.x),
                Axis::Z => (r.max.z - r.min.z, dir.z),
            };
            let slope = (r.max.y - r.min.y) / span * if r.rising { 1.0 } else { -1.0 };
            let side = from.y - r.height_at(from.x, from.z);
            let speed = dir.y - velocity * slope;
            if speed.abs() < 1e-7 {
                if side > 0.0 {
                    continue;
                }
            } else if speed < 0.0 {
                near = near.max(-side / speed);
            } else {
                far = far.min(-side / speed);
            }
            if near <= far {
                best = near;
            }
        }
        best
    }
}

fn ray_box(from: Vec3, dir: Vec3, min: Vec3, max: Vec3, limit: f32) -> Option<(f32, f32)> {
    let mut near = 0.0f32;
    let mut far = limit;
    for axis in 0..3 {
        let speed = dir[axis];
        if speed.abs() < 1e-7 {
            if from[axis] < min[axis] || from[axis] > max[axis] {
                return None;
            }
        } else {
            let a = (min[axis] - from[axis]) / speed;
            let b = (max[axis] - from[axis]) / speed;
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return None;
            }
        }
    }
    Some((near, far))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drill_has_clear_starting_lane_and_a_covered_peek() {
        let drill = Arena::load("drill").unwrap();
        let eye = Vec3::Y * 1.55;
        let start = drill.spawns[0].pos + eye;
        let end = drill.spawns[1].pos + eye;
        let ray = end - start;
        assert_eq!(drill.ray_distance(start, ray, ray.length()), ray.length());
        let behind_cover = start + Vec3::X * 3.0;
        let ray = end - behind_cover;
        assert!(drill.ray_distance(behind_cover, ray, ray.length()) < 5.0);
    }

    #[test]
    fn wedge_intersections_respect_all_four_slope_orientations() {
        for axis in [Axis::X, Axis::Z] {
            for rising in [true, false] {
                let ramp = Ramp {
                    min: Vec3::ZERO,
                    max: Vec3::splat(2.0),
                    axis,
                    rising,
                    material: Material::Ramp,
                };
                let arena = Arena {
                    ramps: vec![ramp],
                    ..Arena::default()
                };
                let lateral = if axis == Axis::X { Vec3::Z } else { Vec3::X };
                let along_slope = if axis == Axis::X { Vec3::X } else { Vec3::Z };
                let mid = along_slope * 0.5 + lateral;
                let height = if rising { 0.5 } else { 1.5 };
                assert_eq!(
                    arena.ray_distance(mid + Vec3::Y * 3.0, -Vec3::Y, 5.0),
                    3.0 - height
                );
                let above = mid + Vec3::Y * (height + 0.25) - lateral * 2.0;
                assert_eq!(arena.ray_distance(above, lateral, 5.0), 5.0);
                let below = above - Vec3::Y * 0.5;
                assert_eq!(arena.ray_distance(below, lateral, 5.0), 1.0);
            }
        }
    }

    #[test]
    fn unsafe_map_names_and_invalid_geometry_are_rejected() {
        assert!(Arena::load("../drill").is_err());
        assert!(Arena::from_json(&vec![b' '; MAX_MAP_BYTES + 1]).is_err());
        let mut drill = Arena::load("drill").unwrap();
        drill.boxes[0].max = drill.boxes[0].min;
        assert!(drill.validate().is_err());
    }

    #[test]
    fn relay_has_two_walkable_attacks_and_a_shorter_defender_rotation_to_each_site() {
        use crate::{step, Body, Input, Tuning, TICK_RATE};
        let arena = Arena::load("relay").unwrap();
        let tune = Tuning::default();
        let walk = |spawn: Vec3, route: &[Vec3]| {
            let mut body = Body::at(spawn);
            let mut ticks = 0u32;
            for &waypoint in route {
                for _ in 0..TICK_RATE * 8 {
                    let offset = waypoint - body.pos;
                    if offset.length() < 0.23 {
                        break;
                    }
                    let input = Input {
                        yaw: offset.x.atan2(-offset.z),
                        forward: 1.0,
                        ..Input::default()
                    };
                    step(&mut body, input, &arena, &tune);
                    ticks += 1;
                }
                assert!(
                    body.pos.distance(waypoint) < 0.23,
                    "blocked from {waypoint:?} at {:?}",
                    body.pos
                );
            }
            ticks
        };
        for team in [0, 1] {
            assert_eq!(arena.spawns.iter().filter(|s| s.team == team).count(), 5);
        }
        assert_eq!(arena.sites.len(), 2);
        for site in &arena.sites {
            let attacker = arena.spawns[0].pos;
            let defender = arena.spawns[1].pos;
            let outer_attack = walk(
                attacker,
                &[Vec3::new(site.pos.x, 0.0, attacker.z), site.pos],
            );
            let defender_route = walk(
                defender,
                &[Vec3::new(site.pos.x, 0.0, defender.z), site.pos],
            );
            assert!(defender_route + TICK_RATE < outer_attack);
            walk(attacker, &[Vec3::new(0.0, 0.0, -4.7), site.pos]);
        }
    }
}
