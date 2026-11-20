//! Predictable equipped-weapon state. M4 retains the original rifle tuning and replay behavior.
use crate::motion::{look_direction, Body, Input, Tuning};
use crate::Weapon;
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub const MAGAZINE: u8 = Weapon::M4.spec().magazine;

const fn all_weapons() -> u8 {
    (1 << Weapon::ALL.len()) - 1
}
const fn default_capacity() -> u8 {
    MAGAZINE
}
const fn default_range() -> f32 {
    Weapon::M4.spec().range
}
const fn full_pouches() -> [u8; 6] {
    [
        0,
        Weapon::Pistol.spec().magazine,
        MAGAZINE,
        Weapon::Ak.spec().magazine,
        Weapon::Sniper.spec().magazine,
        Weapon::Smg.spec().magazine,
    ]
}

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

// Different lateral movement and climb, expressed in game milliradians.
#[rustfmt::skip]
const AK_PATTERN: [(f32, f32); 16] = [
    (0.,0.), (-1.,5.), (-3.,11.), (-7.,18.), (-6.,25.), (-2.,30.), (4.,35.), (10.,37.),
    (14.,39.), (17.,36.), (10.,40.), (2.,42.), (-7.,41.), (-13.,38.), (-15.,40.), (-8.,42.),
];
#[rustfmt::skip]
const SMG_PATTERN: [(f32, f32); 16] = [
    (0.,0.), (1.,3.), (3.,6.), (5.,9.), (2.,13.), (-2.,17.), (-5.,20.), (-2.,23.),
    (3.,25.), (8.,25.), (5.,28.), (0.,29.), (-6.,28.), (-8.,27.), (-4.,29.), (1.,30.),
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
    #[serde(default)]
    pub weapon: Weapon,
    /// The server clips this ray to the first intersected wall or player.
    #[serde(default = "default_range")]
    pub distance: f32,
    #[serde(default)]
    pub hit: Option<u16>,
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
    #[serde(default)]
    pub weapon: Weapon,
    #[serde(default)]
    pub switch_left: u16,
    #[serde(default = "all_weapons")]
    available: u8,
    #[serde(default = "full_pouches")]
    stowed: [u8; 6],
    #[serde(default = "default_capacity")]
    m4_capacity: u8,
    #[serde(default)]
    trigger_down: bool,
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
            weapon: Weapon::M4,
            switch_left: 0,
            available: all_weapons(),
            stowed: full_pouches(),
            m4_capacity: MAGAZINE,
            trigger_down: false,
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
            m4_capacity: magazine,
            ..Self::default()
        }
    }

    pub fn owns(&self, weapon: Weapon) -> bool {
        self.available & (1 << weapon.index()) != 0
    }

    pub fn ammo_for(&self, weapon: Weapon) -> u8 {
        if weapon == self.weapon {
            self.ammo
        } else {
            self.stowed[weapon.index()]
        }
    }

    fn capacity(&self, weapon: Weapon) -> u8 {
        if weapon == Weapon::M4 {
            if self.weapon == Weapon::M4 {
                self.magazine
            } else {
                self.m4_capacity
            }
        } else {
            weapon.spec().magazine
        }
    }

    /// Validate the coherent inventory a trace/snapshot can restore, including
    /// a legacy M4 trace with only the original fields populated.
    pub fn valid_state(&self) -> bool {
        self.available & !all_weapons() == 0
            && (self.available == 0 || self.owns(self.weapon))
            && (1..=MAGAZINE).contains(&self.m4_capacity)
            && self.ammo <= self.magazine
            && (self.recoil_index as usize) < PATTERN.len()
            && match self.weapon {
                Weapon::M4 => (1..=MAGAZINE).contains(&self.magazine),
                w => self.magazine == w.spec().magazine,
            }
            && Weapon::ALL
                .iter()
                .all(|&w| w == self.weapon || self.stowed[w.index()] <= self.capacity(w))
    }

    pub(crate) fn competitive_base(mut self) -> Self {
        self.available = (1 << Weapon::Knife.index())
            | (1 << Weapon::Pistol.index())
            | (1 << Weapon::M4.index());
        self
    }

    pub(crate) fn unarmed(mut self) -> Self {
        self.available = 0;
        self
    }

    /// Surviving a round restores magazines in the retained kit and clears the
    /// previous round's cooldowns. Dead players receive a new issue kit.
    pub(crate) fn next_round(self) -> Self {
        let mut fresh = Self::with_magazine(self.capacity(Weapon::M4));
        fresh.available = self.available;
        fresh.ready_with(self.weapon)
    }

    pub(crate) fn ready_with(mut self, weapon: Weapon) -> Self {
        self.equip(weapon);
        self.switch_left = 0;
        self
    }

    fn equip(&mut self, weapon: Weapon) {
        if weapon == self.weapon || !self.owns(weapon) {
            return;
        }
        if self.weapon == Weapon::M4 {
            self.m4_capacity = self.magazine;
        }
        self.stowed[self.weapon.index()] = self.ammo;
        let capacity = self.capacity(weapon);
        self.ammo = self.stowed[weapon.index()].min(capacity);
        self.magazine = capacity;
        self.weapon = weapon;
        self.switch_left = weapon.spec().draw_ticks;
        self.reload_left = 0;
        self.recoil_index = 0;
        self.quiet_ticks = 0;
    }

    pub fn accurate(&self, body: &Body, tuning: &Tuning) -> bool {
        body.grounded
            && body.speed() <= tuning.accuracy_speed
            && self.recoil_index == 0
            && self.reload_left == 0
            && self.cooldown_left == 0
            && self.switch_left == 0
            && self.owns(self.weapon)
            && (self.weapon == Weapon::Knife || self.ammo > 0)
    }

    pub fn spread(&self, body: &Body, tuning: &Tuning) -> f32 {
        if self.weapon == Weapon::Knife {
            return 0.0;
        }
        let spec = self.weapon.spec();
        let movement = ((body.speed() - tuning.accuracy_speed)
            / (tuning.run_speed - tuning.accuracy_speed).max(0.001))
        .clamp(0.0, 1.0);
        let airborne = if body.grounded {
            0.0
        } else {
            tuning.air_spread * spec.air_spread
        };
        let burst = (f32::from(self.recoil_index) / 8.0).min(1.0);
        (tuning.rest_spread
            + movement * tuning.moving_spread * spec.moving_spread
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
        let pressed = input.fire && !self.trigger_down;
        self.trigger_down = input.fire;
        self.cooldown_left = self.cooldown_left.saturating_sub(1);
        self.switch_left = self.switch_left.saturating_sub(1);
        if let Some(weapon) = input.equip {
            self.equip(weapon);
        }
        if !self.owns(self.weapon) || self.switch_left > 0 {
            return None;
        }
        let spec = self.weapon.spec();
        self.quiet_ticks = self.quiet_ticks.saturating_add(1);
        if self.quiet_ticks >= self.weapon.settle_ticks(tuning) {
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
            self.reload_left = self.weapon.reload_ticks(tuning);
            return None;
        }
        if !input.fire || (!spec.automatic && !pressed) || self.cooldown_left != 0 {
            return None;
        }

        let pattern = match self.weapon {
            Weapon::Ak => &AK_PATTERN,
            Weapon::Smg => &SMG_PATTERN,
            _ => &PATTERN,
        };
        let (pattern_yaw, pattern_pitch) = pattern[self.recoil_index as usize];
        let recoil_yaw = pattern_yaw * 0.001 * tuning.recoil_scale * spec.recoil;
        let recoil_pitch = pattern_pitch * 0.001 * tuning.recoil_scale * spec.recoil;
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
            weapon: self.weapon,
            distance: spec.range,
            hit: None,
        };
        if self.weapon != Weapon::Knife {
            self.ammo -= 1;
        }
        self.shots_fired = self.shots_fired.wrapping_add(1);
        self.cooldown_left = self.weapon.interval_ticks(tuning);
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

    #[test]
    fn equip_draw_cancels_an_interrupted_reload_and_preserves_every_magazine() {
        let body = standing();
        let tune = Tuning::default();
        let mut rifle = Rifle::with_magazine(12);
        let tap = Input {
            fire: true,
            ..Input::default()
        };
        let pick = |weapon| Input {
            equip: Some(weapon),
            ..Input::default()
        };
        assert_eq!(
            rifle.tick(tap, &body, 0, 2, &tune).unwrap().weapon,
            Weapon::M4
        );
        assert!(rifle
            .tick(pick(Weapon::Pistol), &body, 1, 2, &tune)
            .is_none());
        assert_eq!(
            (rifle.switch_left, rifle.magazine, rifle.ammo),
            (13, 14, 14)
        );
        for tick in 2..=13 {
            assert!(rifle
                .tick(pick(Weapon::Pistol), &body, tick, 2, &tune)
                .is_none());
        }
        assert_eq!(
            rifle.tick(tap, &body, 14, 2, &tune).unwrap().weapon,
            Weapon::Pistol
        );
        rifle.tick(
            Input {
                reload: true,
                ..Input::default()
            },
            &body,
            15,
            2,
            &tune,
        );
        assert_eq!(rifle.reload_left, Weapon::Pistol.spec().reload_ticks);
        rifle.tick(pick(Weapon::Smg), &body, 16, 2, &tune);
        assert_eq!(rifle.reload_left, 0);
        assert_eq!(rifle.ammo_for(Weapon::Pistol), 13);
        for (tick, want, capacity, remaining) in [
            (17, Weapon::M4, 12, 11),
            (18, Weapon::Knife, 0, 0),
            (19, Weapon::Pistol, 14, 13),
            (20, Weapon::Smg, 36, 36),
        ] {
            assert!(rifle.tick(pick(want), &body, tick, 2, &tune).is_none());
            assert_eq!(
                (rifle.weapon, rifle.magazine, rifle.ammo),
                (want, capacity, remaining)
            );
            assert!(rifle.valid_state());
        }
    }

    #[test]
    fn semi_auto_requires_release_and_automatic_slots_repeat_on_their_cadence() {
        let tune = Tuning::default();
        let body = standing();
        let press = Input {
            fire: true,
            ..Input::default()
        };
        for weapon in Weapon::ALL {
            let mut rifle = Rifle::default();
            rifle.equip(weapon);
            rifle.switch_left = 0;
            let interval = u32::from(weapon.interval_ticks(&tune));
            let fired: Vec<_> = (0..=3 * interval)
                .filter_map(|tick| rifle.tick(press, &body, tick, 1, &tune))
                .collect();
            if weapon.spec().automatic {
                assert_eq!(
                    fired.iter().map(|s| s.tick).collect::<Vec<_>>(),
                    [0, interval, 2 * interval, 3 * interval]
                );
            } else {
                assert_eq!(fired.len(), 1);
                rifle.tick(Input::default(), &body, 3 * interval + 1, 1, &tune);
                assert!(rifle
                    .tick(press, &body, 3 * interval + 2, 1, &tune)
                    .is_some());
            }
            assert!(fired
                .iter()
                .all(|s| s.weapon == weapon && s.distance == weapon.spec().range));
            assert!(rifle.valid_state());
        }
    }

    #[test]
    fn new_fields_read_old_rifle_and_input_as_a_stable_m4() {
        let legacy = r#"{"magazine":12,"ammo":7,"reload_left":0,"cooldown_left":0,"recoil_index":0,"shots_fired":10,"quiet_ticks":30}"#;
        let mut restored: Rifle = serde_json::from_str(legacy).unwrap();
        let input:Input = serde_json::from_str(r#"{"forward":0,"strafe":0,"yaw":0,"pitch":0,"jump":false,"fire":true,"interact":false,"drop_bomb":false,"buy":false,"reload":false}"#).unwrap();
        assert!(restored.valid_state());
        assert_eq!(restored.weapon, Weapon::M4);
        assert_eq!(input.equip, None);
        let shot = restored
            .tick(input, &standing(), 88, 2, &Tuning::default())
            .unwrap();
        assert_eq!(shot.direction, Vec3::NEG_Z);
        assert_eq!(shot.recoil, 0.);
        assert_eq!(
            (restored.magazine, restored.ammo, restored.cooldown_left),
            (12, 6, 13)
        );
        restored.equip(Weapon::Knife);
        restored.equip(Weapon::M4);
        assert_eq!((restored.magazine, restored.ammo), (12, 6));
    }
    #[test]
    fn automatic_primaries_have_distinct_repeatable_recoil_paths() {
        let burst = |weapon: Weapon, seed| {
            let mut state = Rifle::default().ready_with(weapon);
            let ticks = u32::from(weapon.interval_ticks(&Tuning::default())) * 12;
            (0..ticks)
                .filter_map(|tick| {
                    state.tick(
                        Input {
                            fire: true,
                            ..Input::default()
                        },
                        &standing(),
                        tick,
                        seed,
                        &Tuning::default(),
                    )
                })
                .map(|s| (s.recoil_yaw.signum() as i8, s.recoil_pitch))
                .collect::<Vec<_>>()
        };
        let patterns = [Weapon::M4, Weapon::Ak, Weapon::Smg].map(|w| {
            let p = burst(w, 31);
            assert_eq!(p, burst(w, 81));
            p.into_iter().map(|(x, _)| x).collect::<Vec<_>>()
        });
        assert_ne!(patterns[0], patterns[1]);
        assert_ne!(patterns[1], patterns[2]);
        assert_ne!(patterns[2], patterns[0]);
    }
}
