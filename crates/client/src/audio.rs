use crate::game::Session;
use bevy::{
    audio::{SpatialScale, Volume},
    prelude::*,
};
use sendit_sim::{
    bots::{SHOT_HEARING_RANGE, STEP_HEARING_RANGE, STEP_STRIDE},
    BombView, Phase, RoundResult, TICK_RATE,
};
use std::{
    collections::HashMap,
    f32::consts::TAU,
    time::{Duration, Instant},
};

pub fn listener() -> SpatialListener {
    // Rodio 0.19's pan law raises channel zero for a source nearer its right
    // input ear. Supply the ear offsets in channel order for this backend.
    SpatialListener::new(-0.28)
}

#[derive(Clone, Copy)]
pub enum Cue {
    Shot,
    Step,
    Damage,
    Objective,
    Use,
    Armed,
    Defused,
    Detonated,
}

const CUES: [Cue; 8] = [
    Cue::Shot,
    Cue::Step,
    Cue::Damage,
    Cue::Objective,
    Cue::Use,
    Cue::Armed,
    Cue::Defused,
    Cue::Detonated,
];

impl Cue {
    fn duration(self) -> f32 {
        match self {
            Self::Shot => 0.19,
            Self::Step => 0.13,
            Self::Damage => 0.16,
            Self::Objective => 0.15,
            Self::Use => 0.13,
            Self::Armed => 0.27,
            Self::Defused => 0.42,
            Self::Detonated => 0.48,
        }
    }

    /// Gain, radius of the inner full-volume region, cutoff radius in world meters.
    fn mix(self) -> (f32, f32, f32) {
        match self {
            Self::Shot => (0.56, 13.0, SHOT_HEARING_RANGE),
            Self::Step => (0.44, 6.0, STEP_HEARING_RANGE),
            Self::Damage => (0.45, 1.0, 1.0),
            Self::Objective => (0.45, 11.0, 45.0),
            Self::Use => (0.4, 4.0, 13.0),
            Self::Armed | Self::Defused => (0.5, 15., 65.),
            Self::Detonated => (0.63, 32., 100.),
        }
    }
}

struct Emission {
    cue: Cue,
    pos: Vec3,
    spatial: bool,
}

#[derive(Resource)]
pub struct Sound {
    enabled: bool,
    clips: Option<[Handle<AudioSource>; CUES.len()]>,
    queue: Vec<Emission>,
    strides: HashMap<u16, f32>,
    last_pulse: Option<(u16, u32)>,
    phase: Option<(u16, Phase)>,
    actor: Option<u16>,
    last_use: Option<Instant>,
    planted_pos: Vec3,
}

impl Sound {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            clips: None,
            queue: Vec::new(),
            strides: HashMap::new(),
            last_pulse: None,
            phase: None,
            actor: None,
            last_use: None,
            planted_pos: Vec3::ZERO,
        }
    }

    pub fn emit(&mut self, cue: Cue, pos: Vec3, spatial: bool) {
        if self.enabled && self.queue.len() < 64 && pos.is_finite() {
            self.queue.push(Emission { cue, pos, spatial });
        }
    }

    pub fn walk(&mut self, id: u16, pos: Vec3, previous: Vec3) {
        if !self.enabled {
            return;
        }
        let distance = (pos - previous).with_y(0.0).length();
        if !distance.is_finite() || distance > 3.0 {
            self.strides.remove(&id);
            return;
        }
        let stride = self.strides.entry(id).or_default();
        *stride += distance;
        if *stride >= STEP_STRIDE {
            *stride %= STEP_STRIDE;
            self.emit(Cue::Step, pos + Vec3::Y * 0.1, true);
        }
    }

    fn objective(&mut self, session: &Session) {
        let Some(round) = session.round else { return };
        if let BombView::Planted(_, pos) = round.bomb {
            self.planted_pos = pos;
        }
        let stage = (round.number, round.phase);
        if let Some(prior) = self.phase {
            if prior != stage {
                if round.phase == Phase::Planted {
                    self.emit(Cue::Armed, self.planted_pos, true);
                }
                if round.phase == Phase::Ended {
                    let cue = match round.result {
                        Some(RoundResult::Defused) => Some(Cue::Defused),
                        Some(RoundResult::Detonated) => Some(Cue::Detonated),
                        _ => None,
                    };
                    if let Some(cue) = cue {
                        self.emit(cue, self.planted_pos, true);
                    }
                }
            }
        }
        self.phase = Some(stage);
        if round.action_by != self.actor {
            if let Some(player) = session
                .players
                .iter()
                .find(|p| Some(p.id) == round.action_by)
            {
                if self
                    .last_use
                    .is_none_or(|when| when.elapsed() > Duration::from_millis(250))
                {
                    self.emit(Cue::Use, player.body.eye(), Some(player.id) != session.id);
                    self.last_use = Some(Instant::now());
                }
            }
            self.actor = round.action_by;
        }
        let BombView::Planted(_, point) = round.bomb else {
            self.last_pulse = None;
            return;
        };
        let period = if round.remaining <= TICK_RATE * 5 {
            TICK_RATE / 4
        } else if round.remaining <= TICK_RATE * 12 {
            TICK_RATE / 2
        } else {
            TICK_RATE
        };
        if self.last_pulse.is_none_or(|(number, tick)| {
            number != round.number || session.server_tick.wrapping_sub(tick) >= period
        }) {
            self.emit(Cue::Objective, point + Vec3::Y * 0.3, true);
            self.last_pulse = Some((round.number, session.server_tick));
        }
    }
}

/// Synthesize the original mono clips once. Rodio spatializes the same source at each emitter.
pub fn setup(mut sound: ResMut<Sound>, mut assets: ResMut<Assets<AudioSource>>) {
    if sound.enabled {
        sound.clips = Some(CUES.map(|cue| {
            assets.add(AudioSource {
                bytes: synth(cue).into(),
            })
        }));
    }
}

#[derive(Component)]
pub(super) struct QueuedAt(Instant);

pub fn play(
    mut commands: Commands,
    mut sound: ResMut<Sound>,
    session: Res<Session>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    unstarted: Query<(Entity, &QueuedAt), (Without<AudioSink>, Without<SpatialAudioSink>)>,
) {
    if !sound.enabled {
        return;
    }
    // Bevy skips its playback/cleanup systems when the machine has no audio output.
    for (entity, queued) in &unstarted {
        if queued.0.elapsed() > Duration::from_secs(1) {
            commands.entity(entity).despawn();
        }
    }
    sound.objective(&session);
    if sound.strides.len() > session.players.len() + 4 {
        sound
            .strides
            .retain(|id, _| session.players.iter().any(|p| p.id == *id));
    }
    if sound.queue.is_empty() {
        return;
    }
    let ear = listener
        .get_single()
        .map_or_else(|_| session.body.eye(), GlobalTransform::translation);
    let Sound { queue, clips, .. } = &mut *sound;
    let Some(clips) = clips else { return };
    for event in queue.drain(..) {
        let (mut gain, inner_radius, cutoff) = event.cue.mix();
        if event.spatial {
            let offset = event.pos - ear;
            let distance = offset.length();
            if distance >= cutoff {
                continue;
            }
            if distance > 0.1 && session.map.ray_distance(ear, offset, distance) < distance - 0.15 {
                gain *= 0.52;
            }
        }
        let settings = PlaybackSettings::DESPAWN
            .with_spatial(event.spatial)
            .with_spatial_scale(SpatialScale::new(1.0 / inner_radius))
            .with_volume(Volume::new(gain));
        commands.spawn((
            AudioPlayer::new(clips[event.cue as usize].clone()),
            Transform::from_translation(event.pos),
            settings,
            QueuedAt(Instant::now()),
        ));
    }
}

fn synth(cue: Cue) -> Vec<u8> {
    const RATE: u32 = 24_000;
    let samples = (cue.duration() * RATE as f32) as u32;
    let data_size = 2 * samples;
    let mut wav = Vec::with_capacity((44 + data_size) as usize);
    // Mono signed 16-bit linear PCM.
    wav.extend(b"RIFF");
    wav.extend((36 + data_size).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(RATE.to_le_bytes());
    wav.extend((2 * RATE).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(data_size.to_le_bytes());

    let mut noise = 0x2bd2_a719u32;
    let mut filtered = 0.0f32;
    for i in 0..samples {
        let t = i as f32 / RATE as f32;
        noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let white = (noise >> 8) as f32 / 8_388_608.0 - 1.0;
        filtered += 0.16 * (white - filtered);
        let tone = |hz: f32| (TAU * hz * t).sin();
        let decay = |rate: f32| (-rate * t).exp();
        let signal = match cue {
            Cue::Shot => {
                0.66 * white * decay(83.0)
                    + 0.52 * filtered * decay(20.0)
                    + 0.29 * (TAU * (79.0 * t + (1.0 - decay(44.0)))).sin() * decay(27.0)
            }
            Cue::Step => {
                0.62 * tone(92.0) * decay(34.0)
                    + 0.60 * filtered * decay(31.0)
                    + 0.25 * white * (-150.0 * (t - 0.032).abs()).exp()
            }
            Cue::Damage => {
                (0.42 * tone(182.0) + 0.24 * tone(211.0) + 0.27 * filtered) * decay(24.0)
            }
            Cue::Use => 0.45 * filtered * decay(50.) + 0.35 * tone(420.) * decay(75.),
            Cue::Armed => 0.55 * (TAU * (390. * t + 680. * t * t)).sin() * decay(6.),
            Cue::Defused => {
                let pitch = if t < 0.14 {
                    523.25
                } else if t < 0.28 {
                    659.25
                } else {
                    783.99
                };
                let envelope = (std::f32::consts::PI * (t % 0.14) / 0.14).sin().powi(2);
                0.52 * tone(pitch) * envelope
            }
            Cue::Detonated => (0.56 * filtered + 0.41 * tone(63.0) + 0.23 * tone(47.)) * decay(8.),
            Cue::Objective => {
                let tap = |start: f32, hz: f32| {
                    let phase = t - start;
                    if (0.0..0.060).contains(&phase) {
                        0.70 * (TAU * hz * phase).sin()
                            * (std::f32::consts::PI * phase / 0.06).sin().powi(2)
                    } else {
                        0.0
                    }
                };
                tap(0.0, 713.0) + tap(0.077, 953.0)
            }
        };
        let attack = (t * 1200.0).min(1.0);
        let tail = ((cue.duration() - t) * 250.0).min(1.0);
        let sample = (signal.clamp(-1.0, 1.0) * attack * tail * i16::MAX as f32) as i16;
        wav.extend(sample.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::audio::Decodable;

    #[test]
    fn synthesized_clips_decode_and_have_signal() {
        for cue in CUES {
            let source = AudioSource {
                bytes: synth(cue).into(),
            };
            let mut decoder = source.decoder();
            assert!(decoder.any(|sample| sample != 0));
        }
    }

    #[test]
    fn footsteps_pan_toward_the_source_when_the_listener_turns() {
        let ears = listener();
        let scale = 1.0 / Cue::Step.mix().1;
        let source = AudioSource {
            bytes: synth(Cue::Step).into(),
        };
        let emitter = Vec3::X * 8.0 * scale;
        for (yaw, expect_right) in [(0.0, true), (std::f32::consts::PI, false)] {
            let rotate = Quat::from_rotation_y(yaw);
            let mut stereo = rodio::source::Spatial::new(
                source.decoder(),
                emitter.to_array(),
                (rotate * ears.left_ear_offset * scale).to_array(),
                (rotate * ears.right_ear_offset * scale).to_array(),
            );
            let mut left_energy = 0.0;
            let mut right_energy = 0.0;
            while let (Some(left), Some(right)) = (stereo.next(), stereo.next()) {
                left_energy += f64::from(left).powi(2);
                right_energy += f64::from(right).powi(2);
            }
            assert!(left_energy > 0.0 && right_energy > 0.0);
            assert_eq!(right_energy > left_energy, expect_right);
        }
    }
}
