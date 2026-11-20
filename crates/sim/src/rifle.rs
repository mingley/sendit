//! A single game rifle with a learnable pattern and independently tunable spread.
use crate::motion::{look_direction, Body, Input, Tuning};
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub const MAGAZINE: u8 = 30;

/// Angles in milliradians, from the player's unmodified crosshair. The tail
/// repeats during a long burst. The first sample fires along the crosshair.
const PATTERN: [(f32, f32); 16] = [
    (0.0, 0.0),
    (0.0, 4.0),
    (-1.0, 9.0),
    (1.0, 15.0),
    (3.0, 22.0),
    (5.0, 29.0),
    (1.0, 33.0),
    (-5.0, 36.0),
    (-11.0, 38.0),
    (-15.0, 37.0),
    (-9.0, 40.0),
    (-1.0, 41.0),
    (8.0, 39.0),
    (15.0, 37.0),
    (11.0, 40.0),
    (4.0, 41.0),
];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub tick: u32,
    pub origin: Vec3,
    pub direction: Vec3,
    /// Maximum angular deviation from random spread in radians.
    pub spread: f32,
    /// Magnitude of the fixed recoil offset, in radians.
    pub recoil: f32,
    pub recoil_yaw: f32,
    pub recoil_pitch: f32,
    pub spread_yaw: f32,
    pub spread_pitch: f32,
    pub velocity: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rifle {
    pub magazine: u8,
    pub ammo: u8,
    pub reload_left: u16,
    pub cooldown_left: u16,
    pub recoil_index: u8,
    pub shots_fired: u32,
    quiet_ticks: u16,
}

impl Default for Rifle {
    fn default() -> Self {
        Self {
            magazine: MAGAZINE,
            ammo: MAGAZINE,
            reload_left: 0,
            cooldown_left: 0,
            recoil_index: 0,
            shots_fired: 0,
            quiet_ticks: 0,
        }
    }
}

/// A small integer hash supplies repeatable samples, independent of other
/// players, render frames, network ticks, and bot decision order.
fn sample(mut key: u32) -> f32 {
    key ^= key >> 16;
    key = key.wrapping_mul(0x7feb352d);
    key ^= key >> 15;
    key = key.wrapping_mul(0x846ca68b);
    key ^= key >> 16;
    (key >> 8) as f32 / 16_777_216.0
}

impl Rifle {
    pub fn with_magazine(magazine: u8) -> Self {
        let magazine = magazine.clamp(1, MAGAZINE);
        Self {
            magazine,
            ammo: magazine,
            ..Self::default()
        }
    }

    pub fn accurate(&self, body: &Body, tuning: &Tuning) -> bool {
        body.grounded
            && body.speed() <= tuning.accuracy_speed
            && self.recoil_index == 0
            && self.reload_left == 0
            && self.cooldown_left == 0
            && self.ammo > 0
    }

    pub fn spread(&self, body: &Body, tuning: &Tuning) -> f32 {
        let movement = ((body.speed() - tuning.accuracy_speed)
            / (tuning.run_speed - tuning.accuracy_speed).max(0.001))
        .clamp(0.0, 1.0);
        let airborne = if body.grounded {
            0.0
        } else {
            tuning.air_spread
        };
        let burst = (f32::from(self.recoil_index) / 8.0).min(1.0);
        (tuning.rest_spread
            + movement * tuning.moving_spread
            + airborne
            + burst * tuning.burst_spread)
            .max(0.0)
    }

    pub fn tick(
        &mut self,
        input: Input,
        body: &Body,
        tick: u32,
        seed: u32,
        tuning: &Tuning,
    ) -> Option<Shot> {
        let input = input.sanitized();
        self.cooldown_left = self.cooldown_left.saturating_sub(1);
        self.quiet_ticks = self.quiet_ticks.saturating_add(1);
        if self.quiet_ticks >= tuning.recoil_recovery_ticks.max(1) {
            self.recoil_index = 0;
        }
        if self.reload_left > 0 {
            self.reload_left -= 1;
            if self.reload_left != 0 {
                return None;
            }
            self.ammo = self.magazine;
        }
        if self.ammo < self.magazine && (input.reload || (input.fire && self.ammo == 0)) {
            self.reload_left = tuning.reload_ticks.max(1);
            return None;
        }
        if !input.fire || self.cooldown_left != 0 {
            return None;
        }

        let (pattern_yaw, pattern_pitch) = PATTERN[self.recoil_index as usize];
        let recoil_yaw = pattern_yaw * 0.001 * tuning.recoil_scale;
        let recoil_pitch = pattern_pitch * 0.001 * tuning.recoil_scale;
        let spread = self.spread(body, tuning);
        let key = seed ^ self.shots_fired.wrapping_mul(0x9e3779b9);
        let angle = std::f32::consts::TAU * sample(key ^ 0x51ed270b);
        let radius = spread * sample(key ^ 0x2c1b3c6d).sqrt();
        let (s, c) = angle.sin_cos();
        let spread_yaw = radius * c;
        let spread_pitch = radius * s;

        // Place offsets in an orthonormal camera basis so horizontal spread
        // keeps its angular size even while looking up a ramp.
        let forward = look_direction(input.yaw, input.pitch);
        let right = Vec3::new(input.yaw.cos(), 0.0, input.yaw.sin());
        let up = right.cross(forward);
        let direction = (forward
            + right * (recoil_yaw + spread_yaw).tan()
            + up * (recoil_pitch + spread_pitch).tan())
        .normalize_or_zero();
        let shot = Shot {
            tick,
            origin: body.eye(),
            direction,
            spread,
            recoil: recoil_yaw.hypot(recoil_pitch),
            recoil_yaw,
            recoil_pitch,
            spread_yaw,
            spread_pitch,
            velocity: body.vel,
        };
        self.ammo -= 1;
        self.shots_fired = self.shots_fired.wrapping_add(1);
        self.cooldown_left = tuning.shot_interval_ticks.max(1);
        self.recoil_index = if self.recoil_index as usize + 1 == PATTERN.len() {
            8
        } else {
            self.recoil_index + 1
        };
        self.quiet_ticks = 0;
        Some(shot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing() -> Body {
        let mut body = Body::at(Vec3::ZERO);
        body.grounded = true;
        body
    }

    #[test]
    fn first_shot_follows_raw_aim_and_settles_again() {
        let mut rifle = Rifle::default();
        let body = standing();
        let tuning = Tuning::default();
        let input = Input {
            fire: true,
            yaw: 1.1,
            pitch: 0.2,
            ..Input::default()
        };
        let first = rifle.tick(input, &body, 0, 123, &tuning).unwrap();
        assert!((first.direction - look_direction(input.yaw, input.pitch)).length() < 1e-6);
        assert_eq!(first.spread, 0.0);
        assert_eq!(first.recoil, 0.0);
        for tick in 1..tuning.shot_interval_ticks as u32 {
            assert!(rifle.tick(input, &body, tick, 123, &tuning).is_none());
        }
        let second = rifle
            .tick(
                input,
                &body,
                tuning.shot_interval_ticks as u32,
                123,
                &tuning,
            )
            .unwrap();
        assert!(second.recoil_pitch > 0.0);
        for tick in 0..tuning.recoil_recovery_ticks as u32 {
            rifle.tick(Input::default(), &body, 100 + tick, 123, &tuning);
        }
        assert!(rifle.accurate(&body, &tuning));
    }

    #[test]
    fn replay_matches_and_random_spread_has_an_independent_seed() {
        let mut moving = standing();
        moving.vel.x = 5.8;
        let tuning = Tuning::default();
        let burst = |seed| {
            let mut rifle = Rifle::default();
            let input = Input {
                fire: true,
                ..Input::default()
            };
            (0..160)
                .filter_map(|tick| rifle.tick(input, &moving, tick, seed, &tuning))
                .collect::<Vec<_>>()
        };
        let a = burst(42);
        assert_eq!(a, burst(42));
        let b = burst(8);
        assert!(a.len() >= 10);
        for (one, two) in a.iter().zip(&b) {
            assert_eq!(one.recoil_yaw, two.recoil_yaw);
            assert_eq!(one.recoil_pitch, two.recoil_pitch);
            assert_ne!(one.direction, two.direction);
            assert!(one.spread_yaw.hypot(one.spread_pitch) <= one.spread + 1e-6);
        }
    }

    #[test]
    fn reload_obeys_tick_deadline() {
        let body = standing();
        let mut rifle = Rifle {
            ammo: 0,
            ..Rifle::default()
        };
        let tune = Tuning {
            reload_ticks: 3,
            ..Tuning::default()
        };
        let fire = Input {
            fire: true,
            ..Input::default()
        };
        for tick in 0..3 {
            assert!(rifle.tick(fire, &body, tick, 9, &tune).is_none());
        }
        assert_eq!(rifle.tick(fire, &body, 3, 9, &tune).unwrap().tick, 3);
        assert_eq!(rifle.ammo, MAGAZINE - 1);
    }
}
