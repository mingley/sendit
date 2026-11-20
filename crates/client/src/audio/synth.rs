use super::{Cue, Footing};
use sendit_sim::Weapon;
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Coloration {
    Clear,
    Soft,
    Muffled,
}

impl Coloration {
    pub const ALL: [Self; 3] = [Self::Clear, Self::Soft, Self::Muffled];
}

const RATE: u32 = 24_000;

pub(super) fn synthesize(cue: Cue, color: Coloration) -> Vec<u8> {
    let samples = (cue.duration() * RATE as f32) as u32;
    let mut wav = Vec::with_capacity((44 + samples * 2) as usize);
    wav.extend(b"RIFF");
    wav.extend((36 + samples * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(RATE.to_le_bytes());
    wav.extend((2 * RATE).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((samples * 2).to_le_bytes());
    let mut noise = 0x2bd2_a719u32;
    let mut body_noise = 0.;
    let mut last = 0.;
    let filter = match color {
        Coloration::Clear => 1.,
        Coloration::Soft => 0.42,
        Coloration::Muffled => 0.13,
    };
    for i in 0..samples {
        let t = i as f32 / RATE as f32;
        noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let white = (noise >> 8) as f32 / 8_388_608. - 1.;
        body_noise += 0.19 * (white - body_noise);
        let tone = |hz: f32| (TAU * hz * t).sin();
        let decay = |rate: f32| (-rate * t).exp();
        let transient = |start: f32, rate: f32| {
            if t >= start {
                (-(t - start) * rate).exp()
            } else {
                0.
            }
        };
        let signal = match cue {
            Cue::Shot(Weapon::Knife) => {
                (body_noise * 1.15 + white * 0.19) * (PI * t / cue.duration()).sin().powi(2)
                    + (TAU * (370. * t - 880. * t * t)).sin() * 0.12 * decay(13.)
            }
            Cue::Shot(weapon) => {
                // Each pulse combines a pitched body, short broadband attack
                // and one mechanical transient. Their timing stays in game units.
                let (pitch, tail, crack, action, ring) = match weapon {
                    Weapon::Pistol => (177., 41., 0.77, 0.030, 684.),
                    Weapon::M4 => (111., 29., 0.62, 0.044, 434.),
                    Weapon::Ak => (76., 19., 0.53, 0.059, 295.),
                    Weapon::Sniper => (53., 9., 0.70, 0.112, 168.),
                    Weapon::Smg => (214., 60., 0.56, 0.021, 789.),
                    Weapon::Knife => unreachable!(),
                };
                let thump = (TAU * (pitch * t + 0.77 * (1. - decay(59.)))).sin();
                crack * white * decay(138.)
                    + 0.47 * thump * decay(tail)
                    + 0.62 * body_noise * decay(tail * 0.86)
                    + 0.15 * (white + tone(ring)) * transient(action, 94.)
            }
            Cue::Reload(weapon) => {
                let pitch = 306. + weapon.index() as f32 * 62.;
                let click = 0.20 * tone(pitch) + 0.36 * white;
                click * (decay(81.) + transient(0.163 + 0.009 * weapon.index() as f32, 68.))
                    + 0.31 * body_noise * transient(0.062, 20.)
            }
            Cue::Step(Footing::Stone, alternate) => {
                let (pitch, scuff) = if alternate {
                    (87., 0.039)
                } else {
                    (102., 0.026)
                };
                0.55 * tone(pitch) * decay(37.)
                    + 0.61 * body_noise * decay(30.)
                    + 0.31 * white * transient(scuff, 125.)
            }
            Cue::Step(Footing::Wood, alternate) => {
                let pitch = if alternate { 124. } else { 140. };
                // Hollow board resonance and a second, quieter toe knock.
                let knock = 0.43 * tone(pitch) + 0.24 * tone(pitch * 1.83);
                knock * (decay(34.) + 0.39 * transient(0.035, 62.))
                    + 0.42 * body_noise * decay(45.)
                    + 0.13 * white * decay(110.)
            }
            Cue::Step(Footing::Metal, alternate) => {
                let pitch = if alternate { 460. } else { 504. };
                // The short low sole impact excites two inharmonic plate modes.
                0.23 * tone(96.) * decay(41.)
                    + 0.32 * white * decay(85.)
                    + 0.29 * tone(pitch) * decay(21.)
                    + 0.15 * tone(pitch * 2.38) * decay(28.)
                    + 0.15 * body_noise * transient(0.029, 60.)
            }
            Cue::Pass => {
                (0.43 * (TAU * (1710. * t - 4900. * t * t)).sin() + 0.38 * white)
                    * (PI * t / cue.duration()).sin().powi(3)
            }
            Cue::Damage => (0.42 * tone(182.) + 0.24 * tone(211.) + 0.27 * body_noise) * decay(24.),
            Cue::Use => 0.45 * body_noise * decay(50.) + 0.35 * tone(420.) * decay(75.),
            Cue::Armed => 0.55 * (TAU * (390. * t + 680. * t * t)).sin() * decay(6.),
            Cue::Defused => {
                let note = if t < 0.14 {
                    523.25
                } else if t < 0.28 {
                    659.25
                } else {
                    783.99
                };
                0.52 * tone(note) * (PI * (t % 0.14) / 0.14).sin().powi(2)
            }
            Cue::Detonated => (0.56 * body_noise + 0.41 * tone(63.) + 0.23 * tone(47.)) * decay(8.),
            Cue::Objective => {
                let tap = |start: f32, hz: f32| {
                    let phase = t - start;
                    if (0.0..0.06).contains(&phase) {
                        0.70 * (TAU * hz * phase).sin() * (PI * phase / 0.06).sin().powi(2)
                    } else {
                        0.
                    }
                };
                tap(0., 713.) + tap(0.077, 953.)
            }
        };
        last += filter * (signal - last);
        let attack = (t * 1200.).min(1.);
        let release = ((cue.duration() - t) * 250.).min(1.);
        let sample = (last.clamp(-1., 1.) * attack * release * i16::MAX as f32) as i16;
        wav.extend(sample.to_le_bytes());
    }
    wav
}
