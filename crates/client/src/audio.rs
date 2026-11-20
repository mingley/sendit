use crate::game::Session;
use bevy::{
    audio::{SpatialScale, Volume},
    prelude::*,
};
use sendit_sim::{
    bots::{sound_gain, MELEE_HEARING_RANGE, SHOT_HEARING_RANGE, STEP_HEARING_RANGE, STEP_STRIDE},
    BombView, Phase, RoundResult, Weapon, TICK_RATE,
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub fn listener() -> SpatialListener {
    // Rodio 0.19's pan law raises channel zero for a source nearer its right
    // input ear. Supply the ear offsets in channel order for this backend.
    SpatialListener::new(-0.28)
}

mod synth;
use synth::{synthesize, Coloration};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Footing {
    Stone,
    Wood,
    Metal,
}

impl Footing {
    /// Sample the collider supporting the character's feet only when its next stride lands.
    fn at(arena: &sendit_sim::Arena, feet: Vec3) -> Self {
        use crate::scene::environment::{box_footing, iron_grate, ramp_footing};
        use sendit_sim::motion::RADIUS;
        let over = |min: Vec3, max: Vec3, margin: f32| {
            feet.x >= min.x - margin
                && feet.x <= max.x + margin
                && feet.z >= min.z - margin
                && feet.z <= max.z + margin
        };
        let boxes = arena.boxes.iter().filter_map(|b| {
            let gap = (b.max.y - feet.y).abs();
            (gap <= 0.025 && over(b.min, b.max, RADIUS - 0.0001))
                .then(|| (gap, !over(b.min, b.max, 0.), box_footing(&arena.name, b)))
        });
        // The movement controller samples wedge height at the center of the body.
        let ramps = arena.ramps.iter().filter_map(|r| {
            let gap = (r.height_at(feet.x, feet.z) - feet.y).abs();
            (gap <= 0.025 && over(r.min, r.max, 0.0001))
                .then(|| (gap, false, ramp_footing(r.material)))
        });
        let contact = boxes
            .chain(ramps)
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, _, material)| material);
        if contact == Some(Self::Stone) && iron_grate(&arena.name, feet) {
            Self::Metal
        } else {
            contact.unwrap_or(Self::Stone)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cue {
    Shot(Weapon),
    Reload(Weapon),
    Step(Footing, bool),
    Pass,
    Damage,
    Objective,
    Use,
    Armed,
    Defused,
    Detonated,
}

impl Cue {
    fn all() -> impl Iterator<Item = Self> {
        [
            Self::Pass,
            Self::Damage,
            Self::Objective,
            Self::Use,
            Self::Armed,
            Self::Defused,
            Self::Detonated,
        ]
        .into_iter()
        .chain(
            [Footing::Stone, Footing::Wood, Footing::Metal]
                .into_iter()
                .flat_map(|underfoot| [Self::Step(underfoot, false), Self::Step(underfoot, true)]),
        )
        .chain(Weapon::ALL.into_iter().map(Self::Shot))
        .chain(
            Weapon::ALL
                .into_iter()
                .filter(|w| *w != Weapon::Knife)
                .map(Self::Reload),
        )
    }

    fn duration(self) -> f32 {
        match self {
            Self::Shot(Weapon::Knife) => 0.17,
            Self::Shot(Weapon::Pistol) => 0.16,
            Self::Shot(Weapon::M4) => 0.19,
            Self::Shot(Weapon::Ak) => 0.25,
            Self::Shot(Weapon::Sniper) => 0.55,
            Self::Shot(Weapon::Smg) => 0.12,
            Self::Reload(_) => 0.30,
            Self::Step(Footing::Metal, _) => 0.19,
            Self::Step(_, _) | Self::Pass => 0.13,
            Self::Damage => 0.16,
            Self::Objective => 0.15,
            Self::Use => 0.13,
            Self::Armed => 0.27,
            Self::Defused => 0.42,
            Self::Detonated => 0.48,
        }
    }

    /// Source volume, full-volume radius, maximum audible distance in meters.
    fn mix(self) -> (f32, f32, f32) {
        match self {
            Self::Shot(Weapon::Knife) => (0.43, 4., MELEE_HEARING_RANGE),
            Self::Shot(Weapon::Sniper) => (0.72, 17., SHOT_HEARING_RANGE),
            Self::Shot(Weapon::Smg) => (0.51, 10., SHOT_HEARING_RANGE),
            Self::Shot(_) => (0.60, 13., SHOT_HEARING_RANGE),
            Self::Reload(_) => (0.36, 4., 13.),
            Self::Step(_, _) => (0.50, 6., STEP_HEARING_RANGE),
            Self::Pass => (0.45, 1.5, 3.),
            Self::Damage => (0.48, 1., 1.),
            Self::Objective => (0.45, 11., 45.),
            Self::Use => (0.4, 4., 13.),
            Self::Armed | Self::Defused => (0.5, 15., 65.),
            Self::Detonated => (0.63, 32., 100.),
        }
    }

    pub fn range(self) -> f32 {
        self.mix().2
    }
}

#[derive(Clone, Copy)]
struct Emission {
    cue: Cue,
    pos: Vec3,
    spatial: bool,
    owner: Option<u16>,
}

#[derive(Resource)]
pub struct Sound {
    enabled: bool,
    clips: HashMap<Cue, [Handle<AudioSource>; 3]>,
    queue: Vec<Emission>,
    strides: HashMap<u16, (f32, bool)>,
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
            clips: HashMap::new(),
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
        self.enqueue(Emission {
            cue,
            pos,
            spatial,
            owner: None,
        });
    }

    pub fn emit_actor(&mut self, cue: Cue, pos: Vec3, owner: u16) {
        self.enqueue(Emission {
            cue,
            pos,
            spatial: cue != Cue::Damage,
            owner: Some(owner),
        });
    }

    fn enqueue(&mut self, event: Emission) {
        if !self.enabled || !event.pos.is_finite() {
            return;
        }
        // Coalesce buffered snapshots arriving in the same audio frame. Keep
        // different participants audible even if they share a weapon.
        if let Some(existing) = self
            .queue
            .iter_mut()
            .rev()
            .find(|e| e.owner == event.owner && e.cue == event.cue)
        {
            *existing = event;
        } else if self.queue.len() < 64 {
            self.queue.push(event);
        }
    }

    pub fn reset_actor(&mut self, id: u16) {
        self.strides.remove(&id);
        self.queue.retain(|event| event.owner != Some(id));
    }

    #[cfg(test)]
    pub(super) fn queued(&self, cue: Cue) -> usize {
        self.queue.iter().filter(|e| e.cue == cue).count()
    }

    pub fn reset_world(&mut self) {
        self.strides.clear();
        self.queue.clear();
        self.last_pulse = None;
        self.phase = None;
        self.actor = None;
        self.last_use = None;
    }

    pub fn walk(&mut self, id: u16, pos: Vec3, previous: Vec3, arena: &sendit_sim::Arena) {
        if !self.enabled {
            return;
        }
        let distance = (pos - previous).with_y(0.).length();
        if !distance.is_finite() || distance > 3. {
            self.strides.remove(&id);
            return;
        }
        let (stride, alternate) = self.strides.entry(id).or_default();
        *stride += distance;
        if *stride >= STEP_STRIDE {
            *stride %= STEP_STRIDE;
            *alternate = !*alternate;
            let cue = Cue::Step(Footing::at(arena, pos), *alternate);
            self.emit_actor(cue, pos + Vec3::Y * 0.15, id);
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

#[cfg(test)]
mod tests;

/// Produce all three filtered variants once; firing creates no sample buffers.
pub fn setup(mut sound: ResMut<Sound>, mut assets: ResMut<Assets<AudioSource>>) {
    if sound.enabled {
        sound.clips = Cue::all()
            .map(|cue| {
                (
                    cue,
                    Coloration::ALL.map(|tone| {
                        assets.add(AudioSource {
                            bytes: synthesize(cue, tone).into(),
                        })
                    }),
                )
            })
            .collect();
    }
}

struct Mix {
    gain: f32,
    radius: f32,
    color: Coloration,
    spatial: bool,
}

fn mix(
    event: Emission,
    listener: &Transform,
    subject: Option<u16>,
    map: &sendit_sim::Arena,
) -> Option<Mix> {
    let (mut gain, radius, cutoff) = event.cue.mix();
    let spatial = event.spatial && (event.owner.is_none() || event.owner != subject);
    if !spatial {
        if matches!(event.cue, Cue::Step(_, _)) {
            gain *= 0.68;
        }
        return Some(Mix {
            gain,
            radius,
            color: Coloration::Clear,
            spatial,
        });
    }
    let offset = event.pos - listener.translation;
    let distance = offset.length();
    if distance >= cutoff {
        return None;
    }
    let direction = listener.rotation.inverse() * offset.normalize_or_zero();
    let transmission = sound_gain(map, listener.translation, event.pos);
    // Rear sources lose treble, with extra acoustic shadow for sources on
    // another floor or beyond cover. The spatial sink still follows head turns.
    let color = if transmission < 0.99 || direction.y < -0.62 {
        Coloration::Muffled
    } else if direction.z > 0.28 || direction.y > 0.62 || distance > radius * 2.3 {
        Coloration::Soft
    } else {
        Coloration::Clear
    };
    gain *= transmission * (1. - 0.14 * direction.z.max(0.));
    Some(Mix {
        gain,
        radius,
        color,
        spatial,
    })
}

#[derive(Component)]
pub(super) struct QueuedAt(Instant);

pub fn play(
    mut commands: Commands,
    mut sound: ResMut<Sound>,
    session: Res<Session>,
    listener: Query<&Transform, With<SpatialListener>>,
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
    let fallback = Transform::from_translation(session.body.eye())
        .with_rotation(Quat::from_rotation_y(-session.input.yaw));
    let ear = listener.get_single().unwrap_or(&fallback);
    let subject = session.spectate.or(session.id);
    let Sound { queue, clips, .. } = &mut *sound;
    for event in queue.drain(..) {
        let Some(mix) = mix(event, ear, subject, &session.map) else {
            continue;
        };
        let Some(variants) = clips.get(&event.cue) else {
            continue;
        };
        let settings = PlaybackSettings::DESPAWN
            .with_spatial(mix.spatial)
            .with_spatial_scale(SpatialScale::new(1.0 / mix.radius))
            .with_volume(Volume::new(mix.gain));
        commands.spawn((
            AudioPlayer::new(variants[mix.color as usize].clone()),
            Transform::from_translation(event.pos),
            settings,
            QueuedAt(Instant::now()),
        ));
    }
}
