//! Authoritative player lifetimes, damage and bounded target rewind.
use crate::map::Spawn;
#[cfg(test)]
mod arsenal_tests;
mod deathmatch;
use crate::motion::{HEIGHT, RADIUS};
use crate::rules::{credit, Loadout, RoundResult, RuleState, KILL_REWARD, START_MONEY, TEAM_CAP};
use crate::{
    shot_seed, step, Arena, Body, Command, Input, Rifle, Shot, Tuning, Vec3, Weapon, MAX_REWIND,
    TICK_RATE,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Practice,
    Competitive,
    Deathmatch,
    Surf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    Warmup,
    Buy,
    Live,
    Planted,
    Ended,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum BombView {
    #[default]
    None,
    Carried(u16),
    Dropped(Vec3),
    Planted(u8, Vec3),
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct RoundView {
    pub phase: Phase,
    pub remaining: u32,
    /// Scores follow the current attacker (0) and defender (1) sides.
    pub score: [u16; 2],
    pub number: u16,
    pub bomb: BombView,
    pub progress: f32,
    pub action_by: Option<u16>,
    pub winner: Option<u8>,
    pub result: Option<RoundResult>,
    pub match_over: bool,
}

/// Latest authoritative health loss. The origin stays fixed even if the source
/// moves, leaves, or is hidden when a victim receives a later snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub tick: u32,
    pub source: Option<u16>,
    pub origin: Vec3,
    pub weapon: Option<Weapon>,
    pub damage: u8,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PlayerView {
    pub id: u16,
    pub team: u8,
    pub body: Body,
    pub rifle: Rifle,
    pub yaw: f32,
    pub pitch: f32,
    pub hp: u8,
    pub armor: u8,
    pub ack: u32,
    pub life: u32,
    pub shot: Option<Shot>,
    pub shot_tick: u32,
    pub rewind_ticks: u8,
    pub money: u16,
    pub kills: u16,
    pub deaths: u16,
    pub armed: bool,
    #[serde(default)]
    pub last_hit: Option<Hit>,
}

pub(crate) struct Player {
    pub view: PlayerView,
    pub spawn_slot: usize,
    pub input: Input,
    pub received_at: u32,
    seen_tick: u32,
    pub died_at: u32,
    pub playing_round: bool,
}

#[derive(Clone, Copy)]
struct Pose {
    id: u16,
    life: u32,
    pos: Vec3,
}

#[derive(Default)]
struct Frame {
    tick: u32,
    poses: Vec<Pose>,
}

pub struct World {
    pub arena: Arena,
    pub mode: Mode,
    pub tuning: Tuning,
    pub tick: u32,
    pub(crate) players: BTreeMap<u16, Player>,
    pub(crate) round: RoundView,
    pub(crate) rules: RuleState,
    history: Vec<Frame>,
    shots: Vec<(u16, u32, Shot)>,
}

impl World {
    pub fn new(arena: Arena, mode: Mode, tuning: Tuning) -> Self {
        Self {
            arena,
            mode,
            tuning,
            tick: 0,
            players: BTreeMap::new(),
            round: RoundView::default(),
            rules: RuleState::default(),
            history: (0..=MAX_REWIND).map(|_| Frame::default()).collect(),
            shots: Vec::new(),
        }
    }

    pub fn add(&mut self, id: u16) -> bool {
        let attackers = self.players.values().filter(|p| p.view.team == 0).count();
        let team = u8::from(attackers > self.players.len() - attackers);
        self.add_team(id, team)
    }

    pub fn add_team(&mut self, id: u16, team: u8) -> bool {
        if team > 1 || self.players.contains_key(&id) {
            return false;
        }
        let mut used: Vec<_> = self
            .players
            .values()
            .filter(|p| p.view.team == team)
            .map(|p| p.spawn_slot)
            .collect();
        if self.mode == Mode::Competitive && used.len() >= TEAM_CAP {
            return false;
        }
        used.sort_unstable();
        let slot = (0..=used.len()).find(|i| !used.contains(i)).unwrap_or(0);
        let Some(spawn) = self.spawn(team, slot) else {
            return false;
        };
        let competitive = self.mode == Mode::Competitive;
        let waiting = competitive
            && matches!(
                self.round.phase,
                Phase::Live | Phase::Planted | Phase::Ended
            );
        let loadout = self.spawn_loadout();
        let rifle = self.spawn_weapon(loadout);
        let view = PlayerView {
            id,
            team,
            body: Body::at(spawn.pos),
            rifle,
            yaw: spawn.yaw,
            pitch: 0.,
            hp: if waiting { 0 } else { 100 },
            armor: loadout.armor(),
            ack: 0,
            life: 1,
            shot: None,
            shot_tick: 0,
            rewind_ticks: 0,
            money: if competitive { START_MONEY } else { 0 },
            kills: 0,
            deaths: 0,
            armed: self.mode != Mode::Surf,
            last_hit: None,
        };
        self.players.insert(
            id,
            Player {
                view,
                spawn_slot: slot,
                input: Input {
                    yaw: spawn.yaw,
                    ..Input::default()
                },
                received_at: self.tick,
                seen_tick: self.tick,
                died_at: 0,
                playing_round: competitive && self.round.phase == Phase::Buy,
            },
        );
        true
    }

    fn spawn(&self, team: u8, slot: usize) -> Option<Spawn> {
        let spawns = self.arena.spawns.iter().filter(|s| s.team == team);
        let n = spawns.clone().count();
        if n > 0 {
            spawns.skip(slot % n).next().copied()
        } else {
            self.arena.spawns.first().copied()
        }
    }

    pub fn remove(&mut self, id: u16) {
        self.release_bomb(id);
        self.players.remove(&id);
        for frame in &mut self.history {
            frame.poses.retain(|pose| pose.id != id);
        }
    }

    pub fn reset_through(&mut self, id: u16, through: u32) {
        if !matches!(self.mode, Mode::Practice | Mode::Surf) {
            return;
        }
        self.respawn(id);
        if let Some(p) = self.players.get_mut(&id) {
            if through.wrapping_sub(p.view.ack) <= i32::MAX as u32 {
                p.view.ack = through;
            }
        }
    }

    pub fn reset(&mut self, id: u16) {
        if matches!(self.mode, Mode::Practice | Mode::Surf) {
            self.respawn(id);
        }
    }

    pub(crate) fn respawn(&mut self, id: u16) {
        let Some(current) = self.players.get(&id) else {
            return;
        };
        let spawn = if self.mode == Mode::Deathmatch {
            self.deathmatch_spawn(current)
        } else {
            self.spawn(current.view.team, current.spawn_slot)
        };
        let Some(spawn) = spawn else {
            return;
        };
        let loadout = self.spawn_loadout();
        let mut rifle = self.spawn_weapon(loadout);
        if self.mode == Mode::Deathmatch {
            rifle = rifle.ready_with(current.view.rifle.weapon);
        }
        let player = self.players.get_mut(&id).unwrap();
        let v = &mut player.view;
        v.life = v.life.wrapping_add(1);
        v.body = Body::at(spawn.pos);
        v.yaw = spawn.yaw;
        v.pitch = 0.;
        v.hp = 100;
        v.rifle = rifle;
        v.armor = loadout.armor();
        v.shot = None;
        v.shot_tick = 0;
        v.rewind_ticks = 0;
        v.armed = self.mode != Mode::Surf;
        v.last_hit = None;
        player.input = Input {
            yaw: spawn.yaw,
            ..Input::default()
        };
        player.received_at = self.tick;
        player.seen_tick = self.tick;
    }

    fn spawn_loadout(&self) -> Loadout {
        match self.mode {
            Mode::Competitive => Loadout::Light,
            Mode::Deathmatch => self.rules.dm_loadout,
            _ => Loadout::Standard,
        }
    }

    fn spawn_weapon(&self, loadout: Loadout) -> Rifle {
        match self.mode {
            Mode::Competitive => loadout.rifle().competitive_base(),
            Mode::Surf => loadout.rifle().unarmed(),
            _ => loadout.rifle(),
        }
    }

    pub fn view(&self, id: u16) -> Option<&PlayerView> {
        self.players.get(&id).map(|p| &p.view)
    }

    pub fn views(&self) -> Vec<PlayerView> {
        self.players.values().map(|p| p.view).collect()
    }

    /// Start an offline input drill at the captured pose and weapon state.
    /// Independently received damage, resets, and objective state are outside an input trace.
    pub fn add_replay_player(&mut self, state: PlayerView) -> bool {
        if !self.add_team(state.id, state.team) {
            return false;
        }
        let player = self.players.get_mut(&state.id).unwrap();
        player.input = Input {
            yaw: state.yaw,
            pitch: state.pitch,
            ..Input::default()
        };
        player.view = state;
        true
    }

    pub fn round_view(&self) -> RoundView {
        self.round
    }

    pub fn advance(&mut self, commands: &[(u16, Command)]) {
        self.tick = self.tick.wrapping_add(1);
        for &(id, command) in commands {
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            let ahead = command.seq.wrapping_sub(player.view.ack);
            if ahead == 0 || ahead > i32::MAX as u32 {
                continue;
            }
            player.view.ack = command.seq;
            player.received_at = self.tick;
            player.seen_tick = command.seen_tick;
            player.input = command.input.sanitized();
        }

        self.shots.clear();
        let mut respawns = Vec::new();
        let mut falls = Vec::new();
        for (&id, player) in &mut self.players {
            let view = &mut player.view;
            if view.hp == 0 {
                if (self.mode != Mode::Competitive || self.round.phase == Phase::Warmup)
                    && self.tick.wrapping_sub(player.died_at) >= TICK_RATE
                {
                    respawns.push(id);
                }
                continue;
            }
            // Briefly carry analog intent over packet gaps. Edge actions require new input.
            let age = self.tick.wrapping_sub(player.received_at);
            let input = player.input.at_age(age);
            view.yaw = input.yaw;
            view.pitch = input.pitch;
            if self.mode == Mode::Competitive
                && matches!(self.round.phase, Phase::Buy | Phase::Ended)
            {
                if self.round.phase == Phase::Buy {
                    if input.buy {
                        crate::rules::buy_kit(view);
                    }
                    view.rifle.tick(
                        Input {
                            equip: input.equip,
                            ..Input::default()
                        },
                        &view.body,
                        self.tick,
                        shot_seed(id, view.ack),
                        &self.tuning,
                    );
                }
                continue;
            }
            step(&mut view.body, input, &self.arena, &self.tuning);
            if view.body.pos.y < -40.0 {
                if self.mode == Mode::Competitive && self.round.phase != Phase::Warmup {
                    falls.push(id);
                } else {
                    respawns.push(id);
                }
                continue;
            }
            if !view.armed || self.mode == Mode::Surf {
                continue;
            }
            if let Some(shot) = view.rifle.tick(
                input,
                &view.body,
                self.tick,
                shot_seed(id, view.ack),
                &self.tuning,
            ) {
                let age = self.tick.wrapping_sub(player.seen_tick);
                let rewind = if age > i32::MAX as u32 {
                    0
                } else {
                    age.min(MAX_REWIND)
                };
                view.shot_tick = self.tick;
                view.rewind_ticks = rewind as u8;
                self.shots.push((id, self.tick.wrapping_sub(rewind), shot));
            }
        }
        for id in respawns {
            self.respawn(id);
        }

        for id in falls {
            self.eliminate(id, None);
        }

        let index = self.tick as usize % self.history.len();
        let frame = &mut self.history[index];
        frame.tick = self.tick;
        frame.poses.clear();
        frame.poses.extend(
            self.players
                .values()
                .filter(|p| p.view.hp > 0)
                .map(|p| Pose {
                    id: p.view.id,
                    life: p.view.life,
                    pos: p.view.body.pos,
                }),
        );
        for i in 0..self.shots.len() {
            let (id, time, mut shot) = self.shots[i];
            (shot.hit, shot.distance) = self.trace(id, time, shot);
            if let Some(victim) = shot.hit {
                self.apply_damage(
                    victim,
                    shot.weapon.spec().damage,
                    Some(id),
                    shot.origin,
                    Some(shot.weapon),
                );
            }
            if let Some(shooter) = self.players.get_mut(&id) {
                shooter.view.shot = Some(shot);
            }
        }
        if self.mode == Mode::Competitive {
            self.advance_round();
        }
    }

    fn trace(&self, id: u16, time: u32, shot: Shot) -> (Option<u16>, f32) {
        let mut limit =
            self.arena
                .ray_distance(shot.origin, shot.direction, shot.weapon.spec().range);
        let frame = &self.history[time as usize % self.history.len()];
        if frame.tick != time {
            return (None, limit);
        }
        let mut target = None;
        for pose in &frame.poses {
            let Some(player) = self.view(pose.id) else {
                continue;
            };
            if pose.id == id || player.hp == 0 || player.life != pose.life {
                continue;
            }
            if self.mode == Mode::Competitive
                && self.view(id).is_some_and(|me| me.team == player.team)
            {
                continue;
            }
            if let Some(distance) = body_distance(shot.origin, shot.direction, pose.pos, limit) {
                limit = distance;
                target = Some(pose.id);
            }
        }
        (target, limit)
    }

    #[cfg(test)]
    pub(crate) fn damage(&mut self, shooter: u16, target: u16, amount: u8) {
        if let Some(p) = self.view(shooter) {
            self.apply_damage(
                target,
                amount,
                Some(shooter),
                p.body.eye(),
                Some(p.rifle.weapon),
            );
        }
    }

    pub(crate) fn apply_damage(
        &mut self,
        target: u16,
        amount: u8,
        source: Option<u16>,
        origin: Vec3,
        weapon: Option<Weapon>,
    ) {
        if self.mode == Mode::Competitive
            && source
                .and_then(|id| self.view(id))
                .zip(self.view(target))
                .is_some_and(|(a, b)| a.team == b.team)
        {
            return;
        }
        let Some(player) = self.players.get_mut(&target) else {
            return;
        };
        if player.view.hp == 0 {
            return;
        }
        let absorbed = (amount / 3).min(player.view.armor);
        player.view.armor -= absorbed;
        let health_loss = (amount - absorbed).min(player.view.hp);
        if health_loss == 0 {
            return;
        }
        player.view.last_hit = Some(Hit {
            tick: self.tick,
            source,
            origin,
            weapon,
            damage: health_loss,
        });
        if health_loss == player.view.hp {
            self.eliminate(target, source);
        } else {
            player.view.hp -= health_loss;
        }
    }

    fn eliminate(&mut self, target: u16, killer: Option<u16>) {
        let Some(player) = self.players.get_mut(&target) else {
            return;
        };
        if player.view.hp == 0 {
            return;
        }
        player.view.hp = 0;
        player.view.deaths = player.view.deaths.saturating_add(1);
        player.died_at = self.tick;
        self.release_bomb(target);
        if let Some(attacker) = killer.and_then(|id| self.players.get_mut(&id)) {
            attacker.view.kills = attacker.view.kills.saturating_add(1);
            if self.mode == Mode::Competitive
                && matches!(self.round.phase, Phase::Live | Phase::Planted)
            {
                credit(&mut attacker.view, KILL_REWARD);
            }
        }
    }
}

/// Nearest intersection with an upright finite cylinder, clipped by opaque map geometry.
pub(crate) fn body_distance(origin: Vec3, dir: Vec3, feet: Vec3, limit: f32) -> Option<f32> {
    let p = origin - feet;
    let a = dir.x * dir.x + dir.z * dir.z;
    let radial = if a < 1e-9 {
        if p.x * p.x + p.z * p.z > RADIUS * RADIUS {
            return None;
        }
        (0.0, limit)
    } else {
        let b = p.x * dir.x + p.z * dir.z;
        let det = b * b - a * (p.x * p.x + p.z * p.z - RADIUS * RADIUS);
        if det < 0.0 {
            return None;
        }
        ((-b - det.sqrt()) / a, (-b + det.sqrt()) / a)
    };
    let vertical = if dir.y.abs() < 1e-6 {
        if !(0.0..=HEIGHT).contains(&p.y) {
            return None;
        }
        (0.0, limit)
    } else {
        let h1 = -p.y / dir.y;
        let h2 = (HEIGHT - p.y) / dir.y;
        (h1.min(h2), h1.max(h2))
    };
    let near = radial.0.max(vertical.0).max(0.);
    (near < limit && near <= radial.1.min(vertical.1)).then_some(near)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> World {
        let mut world = World::new(
            Arena::load("drill").unwrap(),
            Mode::Practice,
            Tuning::default(),
        );
        assert!(world.add(1) && world.add(2));
        for _ in 0..32 {
            world.advance(&[]);
        }
        world
    }

    fn shoot(world: &mut World, seq: u32, seen_tick: u32) {
        world.advance(&[(
            1,
            Command {
                seq,
                seen_tick,
                input: Input {
                    fire: true,
                    ..Default::default()
                },
            },
        )]);
    }

    #[test]
    fn authoritative_rifle_hits_and_caps_client_supplied_rewind() {
        let mut world = pair();
        shoot(&mut world, 1, 1);
        assert_eq!(world.view(2).unwrap().hp, 66);
        assert_eq!(world.view(1).unwrap().rewind_ticks, MAX_REWIND as u8);
        assert_eq!(world.view(1).unwrap().shot.unwrap().direction, Vec3::NEG_Z);
    }

    #[test]
    fn rewind_cannot_hit_a_new_life_or_a_reused_session() {
        let mut world = pair();
        let before = world.tick;
        world.reset(2);
        shoot(&mut world, 1, before);
        assert_eq!(world.view(2).unwrap().hp, 100);
        world.reset(1);
        world.remove(2);
        assert!(world.add(2));
        shoot(&mut world, 2, before);
        assert_eq!(world.view(2).unwrap().hp, 100);
    }

    #[test]
    fn reset_discards_stale_accepted_sequences() {
        let mut world = pair();
        world.reset_through(1, 64);
        let before = world.view(1).unwrap().body.pos;
        world.advance(&[(
            1,
            Command {
                seq: 63,
                seen_tick: world.tick,
                input: Input {
                    strafe: 1.,
                    fire: true,
                    ..Input::default()
                },
            },
        )]);
        assert_eq!(world.view(1).unwrap().body.pos, before);
        assert_eq!(world.view(1).unwrap().ack, 64);
        assert!(world.view(1).unwrap().shot.is_none());
    }
}
