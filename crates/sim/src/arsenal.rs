//! Sendit's fictional game balance. Distances are map units, timing is in 128 Hz ticks.
use crate::Tuning;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Weapon {
    Knife,
    Pistol,
    #[default]
    M4,
    Ak,
    Sniper,
    Smg,
}

#[derive(Clone, Copy, Debug)]
pub struct WeaponSpec {
    pub label: &'static str,
    pub magazine: u8,
    pub damage: u8,
    pub range: f32,
    pub automatic: bool,
    pub interval_ticks: u16,
    pub draw_ticks: u16,
    pub reload_ticks: u16,
    pub(crate) settle_ticks: u16,
    pub(crate) recoil: f32,
    pub(crate) moving_spread: f32,
    pub(crate) air_spread: f32,
}

const SPECS: [WeaponSpec; 6] = [
    WeaponSpec {
        label: "Knife",
        magazine: 0,
        damage: 60,
        range: 2.1,
        automatic: true,
        interval_ticks: 56,
        draw_ticks: 8,
        reload_ticks: 0,
        settle_ticks: 1,
        recoil: 0.,
        moving_spread: 0.,
        air_spread: 0.,
    },
    WeaponSpec {
        label: "Pistol",
        magazine: 14,
        damage: 27,
        range: 95.,
        automatic: false,
        interval_ticks: 16,
        draw_ticks: 13,
        reload_ticks: 145,
        settle_ticks: 22,
        recoil: 0.65,
        moving_spread: 0.75,
        air_spread: 0.8,
    },
    WeaponSpec {
        label: "M4",
        magazine: 30,
        damage: 34,
        range: 200.,
        automatic: true,
        interval_ticks: 13,
        draw_ticks: 24,
        reload_ticks: 220,
        settle_ticks: 30,
        recoil: 1.,
        moving_spread: 1.,
        air_spread: 1.,
    },
    WeaponSpec {
        label: "AK",
        magazine: 24,
        damage: 42,
        range: 180.,
        automatic: true,
        interval_ticks: 17,
        draw_ticks: 32,
        reload_ticks: 240,
        settle_ticks: 40,
        recoil: 1.55,
        moving_spread: 1.35,
        air_spread: 1.25,
    },
    WeaponSpec {
        label: ".50 Sniper",
        magazine: 4,
        damage: 110,
        range: 300.,
        automatic: false,
        interval_ticks: 122,
        draw_ticks: 48,
        reload_ticks: 285,
        settle_ticks: 75,
        recoil: 3.0,
        moving_spread: 4.5,
        air_spread: 4.0,
    },
    WeaponSpec {
        label: "SMG",
        magazine: 36,
        damage: 22,
        range: 105.,
        automatic: true,
        interval_ticks: 8,
        draw_ticks: 16,
        reload_ticks: 170,
        settle_ticks: 25,
        recoil: 0.65,
        moving_spread: 0.7,
        air_spread: 0.85,
    },
];

impl Weapon {
    pub const ALL: [Self; 6] = [
        Self::Knife,
        Self::Pistol,
        Self::M4,
        Self::Ak,
        Self::Sniper,
        Self::Smg,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }
    pub const fn spec(self) -> &'static WeaponSpec {
        &SPECS[self.index()]
    }
    pub const fn label(self) -> &'static str {
        self.spec().label
    }

    pub fn interval_ticks(self, tuning: &Tuning) -> u16 {
        if self == Self::M4 {
            tuning.shot_interval_ticks.max(1)
        } else {
            self.spec().interval_ticks
        }
    }

    pub fn reload_ticks(self, tuning: &Tuning) -> u16 {
        if self == Self::M4 {
            tuning.reload_ticks.max(1)
        } else {
            self.spec().reload_ticks
        }
    }

    pub(crate) fn settle_ticks(self, tuning: &Tuning) -> u16 {
        if self == Self::M4 {
            tuning.recoil_recovery_ticks.max(1)
        } else {
            self.spec().settle_ticks
        }
    }
}
