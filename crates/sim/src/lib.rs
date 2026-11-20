//! Sendit's shared deterministic gameplay. Positions are feet coordinates in meters.
//! One simulation step always advances 1/128 s, regardless of render or packet cadence.
pub mod bots;
pub mod map;
pub mod motion;
pub mod replay;
pub mod rifle;
pub mod rules;
pub mod wire;
pub mod world;

pub use glam::Vec3;
pub use map::Arena;
pub use motion::{step, Body, Input, Tuning};
pub use rifle::{Rifle, Shot};
pub use rules::{Loadout, RoundResult};
pub use wire::Command;
pub use world::{BombView, Mode, Phase, PlayerView, RoundView, World};

pub const TICK_RATE: u32 = 128;
pub const MAX_REWIND: u32 = 25;

/// Both endpoints use the same seed for a given player and input sequence.
pub fn shot_seed(player: u16, seq: u32) -> u32 {
    seq.wrapping_mul(0x9e37_79b9) ^ u32::from(player).wrapping_mul(0x85eb_ca6b)
}
