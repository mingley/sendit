//! Server-owned competitive round clock, equipment, rewards, and bomb interactions.
//! Empty-site arenas run the same round loop as an elimination duel.
use crate::{
    rifle::MAGAZINE, world::Player, Arena, Body, BombView, Phase, PlayerView, Rifle, RoundView,
    Vec3, World, TICK_RATE,
};
use serde::{Deserialize, Serialize};

pub const TEAM_CAP: usize = 5;
pub const BUY_TICKS: u32 = 10 * TICK_RATE;
pub const LIVE_TICKS: u32 = 105 * TICK_RATE;
pub const FUSE_TICKS: u32 = 35 * TICK_RATE;
pub const PLANT_TICKS: u32 = 3 * TICK_RATE;
pub const DEFUSE_TICKS: u32 = 5 * TICK_RATE;
pub const END_TICKS: u32 = 4 * TICK_RATE;
pub const HALF_ROUNDS: u16 = 12;
pub const ACTION_SPEED: f32 = 0.45;
pub const INTERACT_RANGE: f32 = 1.8;
pub const KIT_COST: u16 = 1_200;
pub const START_MONEY: u16 = 1_200;
pub const KILL_REWARD: u16 = 300;
const WIN_REWARD: u16 = 1_600;
const LOSS_REWARD: u16 = 900;
const OBJECTIVE_REWARD: u16 = 300;
const MONEY_LIMIT: u16 = 16_000;
/// Game-space radius for the end-of-round blast damage falloff.
pub const BLAST_RADIUS: f32 = 14.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Loadout {
    #[default]
    Standard,
    Light,
    Armored,
}

impl Loadout {
    pub fn rifle(self) -> Rifle {
        Rifle::with_magazine(if self == Self::Light { 12 } else { MAGAZINE })
    }

    pub fn armor(self) -> u8 {
        if self == Self::Armored {
            50
        } else {
            0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundResult {
    Elimination,
    Time,
    Defused,
    Detonated,
}

#[derive(Default)]
pub(crate) struct RuleState {
    pub dm_loadout: Loadout,
    action_site: u8,
    held_ticks: u32,
}

/// Use the same range and visibility test for prompts and the server action.
pub fn can_reach_objective(arena: &Arena, body: &Body, pos: Vec3) -> bool {
    if body.pos.distance_squared(pos) > INTERACT_RANGE * INTERACT_RANGE {
        return false;
    }
    let delta = pos + Vec3::Y * 0.18 - body.eye();
    let length = delta.length();
    length < 0.001 || arena.ray_distance(body.eye(), delta / length, length) >= length - 0.005
}

pub(crate) fn credit(view: &mut PlayerView, amount: u16) {
    view.money = view.money.saturating_add(amount).min(MONEY_LIMIT);
}

pub(crate) fn buy_kit(view: &mut PlayerView) {
    if view.money >= KIT_COST
        && (!view.rifle.owns(crate::Weapon::Ak) || view.armor < Loadout::Armored.armor())
    {
        view.money -= KIT_COST;
        view.rifle = Loadout::Armored.rifle();
        view.armor = Loadout::Armored.armor();
    }
}

impl World {
    pub fn set_dm_loadout(&mut self, loadout: Loadout) {
        self.rules.dm_loadout = loadout;
    }

    fn teams_present(&self) -> bool {
        let mut present = [false; 2];
        for player in self.players.values() {
            present[player.view.team as usize] = true;
        }
        present == [true, true]
    }

    fn begin_round(&mut self) {
        let old_number = self.round.number;
        let new_match = old_number == 0;
        let swap = old_number == HALF_ROUNDS;
        let mut score = self.round.score;
        if swap {
            score.swap(0, 1);
        }
        self.round = RoundView {
            phase: Phase::Buy,
            remaining: BUY_TICKS,
            number: old_number + 1,
            score,
            ..RoundView::default()
        };
        self.clear_hold();
        for p in self.players.values_mut() {
            if swap {
                p.view.team ^= 1;
            }
        }
        let ids: Vec<_> = self.players.keys().copied().collect();
        for id in ids {
            let previous = self.players[&id].view;
            self.respawn(id);
            let player = self.players.get_mut(&id).unwrap();
            player.playing_round = true;
            if new_match || swap {
                player.view.money = START_MONEY;
                if new_match {
                    player.view.kills = 0;
                    player.view.deaths = 0;
                }
            } else if previous.hp > 0 {
                player.view.rifle = previous.rifle.next_round();
                player.view.armor = previous.armor;
            }
        }
        if !self.arena.sites.is_empty() {
            let attackers: Vec<_> = self
                .players
                .values()
                .filter(|p| p.view.team == 0)
                .map(|p| p.view.id)
                .collect();
            if !attackers.is_empty() {
                self.round.bomb =
                    BombView::Carried(attackers[usize::from(old_number) % attackers.len()]);
            }
        }
    }

    fn finish_round(&mut self, winner: u8, reason: RoundResult) {
        self.clear_hold();
        self.round.phase = Phase::Ended;
        self.round.winner = Some(winner);
        self.round.result = Some(reason);
        self.round.score[winner as usize] += 1;
        self.round.match_over =
            self.round.score[winner as usize] > HALF_ROUNDS || self.round.number >= 2 * HALF_ROUNDS;
        self.round.remaining = if self.round.match_over {
            2 * END_TICKS
        } else {
            END_TICKS
        };
        for player in self.players.values_mut().filter(|p| p.playing_round) {
            let amount = if player.view.team == winner {
                WIN_REWARD
            } else {
                LOSS_REWARD
            };
            credit(&mut player.view, amount);
        }
    }

    pub(crate) fn release_bomb(&mut self, id: u16) {
        if self.round.bomb == BombView::Carried(id) {
            // Return an out-of-bounds carrier's pack to the attacking spawn.
            let pos = self
                .view(id)
                .map(|p| p.body.pos)
                .filter(|p| p.y >= -32.0)
                .or_else(|| {
                    self.arena
                        .spawns
                        .iter()
                        .find(|s| s.team == 0)
                        .map(|s| s.pos)
                })
                .or_else(|| self.arena.spawns.first().map(|s| s.pos));
            self.round.bomb = pos.map_or(BombView::None, BombView::Dropped);
        }
        if self.round.action_by == Some(id) {
            self.clear_hold();
        }
    }

    fn clear_hold(&mut self) {
        self.round.action_by = None;
        self.round.progress = 0.0;
        self.rules.held_ticks = 0;
    }

    fn hold(&mut self, actor: u16, site: u8, duration: u32) -> bool {
        if self.round.action_by != Some(actor) || self.rules.action_site != site {
            self.rules.held_ticks = 0;
        }
        self.round.action_by = Some(actor);
        self.rules.action_site = site;
        self.rules.held_ticks += 1;
        self.round.progress = self.rules.held_ticks as f32 / duration as f32;
        self.rules.held_ticks >= duration
    }

    fn using(&self, p: &Player) -> bool {
        p.view.hp > 0 && p.input.interact && self.tick.wrapping_sub(p.received_at) <= 8
    }

    fn stationary(&self, p: &Player) -> bool {
        self.using(p)
            && !p.input.fire
            && p.view.rifle.reload_left == 0
            && p.view.body.grounded
            && p.view.body.speed() <= ACTION_SPEED
    }

    fn objective(&mut self) {
        match self.round.bomb {
            BombView::Carried(id) => {
                let Some(p) = self.players.get(&id) else {
                    return;
                };
                if p.received_at == self.tick && p.input.drop_bomb {
                    self.release_bomb(id);
                    return;
                }
                let pos = p.view.body.pos;
                let site = self
                    .arena
                    .sites
                    .iter()
                    .position(|s| pos.distance_squared(s.pos) <= s.radius * s.radius);
                if self.stationary(p) {
                    if let Some(site) = site {
                        if self.hold(id, site as u8, PLANT_TICKS) {
                            self.round.bomb = BombView::Planted(site as u8, pos);
                            self.round.phase = Phase::Planted;
                            self.round.remaining = FUSE_TICKS;
                            credit(
                                &mut self.players.get_mut(&id).unwrap().view,
                                OBJECTIVE_REWARD,
                            );
                            self.clear_hold();
                        }
                        return;
                    }
                }
                self.clear_hold();
            }
            BombView::Dropped(pos) => {
                if let Some(p) = self.players.values().find(|p| {
                    p.view.team == 0
                        && self.using(p)
                        && can_reach_objective(&self.arena, &p.view.body, pos)
                }) {
                    self.round.bomb = BombView::Carried(p.view.id);
                }
            }
            BombView::Planted(site, pos) => {
                // Keep the current defuser if still eligible. An additional CT
                // entering range cannot interrupt or accelerate the timer.
                let eligible = |p: &Player| {
                    p.view.team == 1
                        && self.stationary(p)
                        && can_reach_objective(&self.arena, &p.view.body, pos)
                };
                let actor = self
                    .round
                    .action_by
                    .and_then(|id| self.players.get(&id))
                    .filter(|p| eligible(p))
                    .or_else(|| self.players.values().find(|p| eligible(p)))
                    .map(|p| p.view.id);
                if let Some(actor) = actor {
                    if self.hold(actor, site, DEFUSE_TICKS) {
                        credit(
                            &mut self.players.get_mut(&actor).unwrap().view,
                            OBJECTIVE_REWARD,
                        );
                        self.round.bomb = BombView::None;
                        self.finish_round(1, RoundResult::Defused);
                    }
                } else {
                    self.clear_hold();
                }
            }
            BombView::None => {}
        }
    }

    fn detonate(&mut self) {
        if let BombView::Planted(_, pos) = self.round.bomb {
            let hits: Vec<_> = self
                .players
                .values()
                .filter(|p| p.view.hp > 0)
                .filter_map(|p| {
                    let distance = p.view.body.pos.distance(pos);
                    let damage = (220.0 * (1.0 - distance / BLAST_RADIUS)).round() as u8;
                    (damage > 0).then_some((p.view.id, damage))
                })
                .collect();
            for (id, damage) in hits {
                self.apply_damage(id, damage, None, pos, None);
            }
        }
        self.round.bomb = BombView::None;
        self.finish_round(0, RoundResult::Detonated);
    }

    pub(crate) fn advance_round(&mut self) {
        let phase = self.round.phase;
        if phase == Phase::Warmup {
            if self.teams_present() {
                self.begin_round();
            }
            return;
        }
        self.round.remaining = self.round.remaining.saturating_sub(1);
        if phase == Phase::Ended {
            if self.round.remaining == 0 {
                if self.round.match_over {
                    self.round = RoundView::default();
                }
                if self.teams_present() {
                    self.begin_round();
                } else {
                    self.round.phase = Phase::Warmup;
                    self.round.bomb = BombView::None;
                    self.round.result = None;
                    self.round.winner = None;
                }
            }
            return;
        }
        if phase == Phase::Buy {
            if self.round.remaining == 0 {
                self.round.phase = Phase::Live;
                self.round.remaining = LIVE_TICKS;
            }
            return;
        }
        let mut alive = [false; 2];
        for p in self.players.values().filter(|p| p.view.hp > 0) {
            alive[p.view.team as usize] = true;
        }
        // Shots and deaths from this tick have already resolved. After plant,
        // eliminating every attacker never substitutes for a defuse.
        if !alive[1] {
            self.finish_round(0, RoundResult::Elimination);
        } else if phase == Phase::Live && !alive[0] {
            self.finish_round(1, RoundResult::Elimination);
        } else if self.round.remaining == 0 && phase == Phase::Planted {
            self.detonate();
        } else {
            self.objective();
            if self.round.phase == Phase::Live && self.round.remaining == 0 {
                self.finish_round(1, RoundResult::Time);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Arena, Body, Command, Input, Mode, Tuning};

    fn world(map: &str) -> World {
        let mut world = World::new(
            Arena::load(map).unwrap(),
            Mode::Competitive,
            Tuning::default(),
        );
        assert!(world.add_team(1, 0));
        world.advance(&[]);
        assert_eq!(world.round.phase, Phase::Warmup);
        assert!(world.add_team(2, 1));
        world.advance(&[]);
        assert_eq!(world.round.phase, Phase::Buy);
        world
    }

    fn live(world: &mut World) {
        world.round.remaining = 1;
        world.advance(&[]);
        assert_eq!(world.round.phase, Phase::Live);
        assert_eq!(world.round.remaining, LIVE_TICKS);
    }

    fn drive(world: &mut World, id: u16, input: Input) {
        world.advance(&[(
            id,
            Command {
                seq: world.tick + 1,
                seen_tick: world.tick,
                input,
            },
        )]);
    }

    fn use_for(world: &mut World, id: u16, ticks: u32) {
        let input = Input {
            interact: true,
            ..Input::default()
        };
        for _ in 0..ticks {
            drive(world, id, input);
        }
    }

    fn place(world: &mut World, id: u16, pos: Vec3) {
        world.players.get_mut(&id).unwrap().view.body = Body::at(pos);
    }

    fn planted(site: usize) -> (World, Vec3) {
        let mut w = world("relay");
        live(&mut w);
        let pos = w.arena.sites[site].pos;
        place(&mut w, 1, pos);
        use_for(&mut w, 1, PLANT_TICKS - 1);
        assert_eq!(w.round.phase, Phase::Live);
        assert_eq!(w.round.action_by, Some(1));
        assert!(w.round.progress > 0.99 && w.round.progress < 1.0);
        use_for(&mut w, 1, 1);
        assert_eq!(w.round.phase, Phase::Planted);
        assert!(matches!(w.round.bomb, BombView::Planted(s, _) if s == site as u8));
        assert_eq!(w.round.remaining, FUSE_TICKS);
        (w, pos)
    }

    #[test]
    fn killing_last_attacker_after_plant_still_requires_defuse() {
        let (mut w, pos) = planted(1);
        w.damage(2, 1, 255);
        assert_eq!(w.view(1).unwrap().hp, 0);
        w.advance(&[]);
        assert_eq!(w.round.phase, Phase::Planted);
        assert_eq!(w.round.score, [0, 0]);
        place(&mut w, 2, pos);
        use_for(&mut w, 2, DEFUSE_TICKS - 1);
        assert_eq!(w.round.phase, Phase::Planted);
        assert_eq!(w.round.action_by, Some(2));
        use_for(&mut w, 2, 1);
        assert_eq!(w.round.result, Some(RoundResult::Defused));
        assert_eq!(w.round.winner, Some(1));
        assert_eq!(w.round.score, [0, 1]);
    }

    #[test]
    fn fuse_expires_after_last_attacker_leaves_and_wins_same_tick_as_a_late_defuse() {
        let (mut w, pos) = planted(0);
        w.remove(1);
        place(&mut w, 2, pos);
        use_for(&mut w, 2, DEFUSE_TICKS - 1);
        w.round.remaining = 1;
        use_for(&mut w, 2, 1);
        assert_eq!(w.round.result, Some(RoundResult::Detonated));
        assert_eq!(w.round.bomb, BombView::None);
        assert_eq!(w.view(2).unwrap().hp, 0);
        assert_eq!(w.round.winner, Some(0));
        assert_eq!(w.round.score, [1, 0]);
    }

    #[test]
    fn a_player_saving_outside_the_site_keeps_equipment() {
        let (mut w, pos) = planted(0);
        w.players.get_mut(&2).unwrap().view.rifle = Loadout::Armored.rifle();
        place(&mut w, 2, pos + Vec3::X * (BLAST_RADIUS + 3.0));
        w.round.remaining = 1;
        w.advance(&[]);
        assert_eq!(w.view(1).unwrap().hp, 0);
        assert_eq!(w.view(2).unwrap().hp, 100);
        w.round.remaining = 1;
        w.advance(&[]);
        assert_eq!(w.view(2).unwrap().rifle.magazine, 30);
    }

    #[test]
    fn interrupted_holds_restart_and_other_defenders_cannot_accelerate_or_steal_progress() {
        let mut w = world("relay");
        assert!(w.add_team(3, 1));
        live(&mut w);
        let pos = w.arena.sites[0].pos;
        place(&mut w, 1, pos);
        use_for(&mut w, 1, 20);
        drive(
            &mut w,
            1,
            Input {
                interact: true,
                fire: true,
                ..Input::default()
            },
        );
        assert_eq!(w.round.action_by, None);
        use_for(&mut w, 1, 20);
        for _ in 0..9 {
            w.advance(&[]);
        }
        assert_eq!(w.round.progress, 0.);
        assert_eq!(w.round.action_by, None);
        use_for(&mut w, 1, PLANT_TICKS);
        place(&mut w, 2, pos);
        place(&mut w, 3, pos);
        use_for(&mut w, 3, 1);
        for _ in 0..5 {
            let cmd = Command {
                seq: w.tick + 1,
                seen_tick: w.tick,
                input: Input {
                    interact: true,
                    ..Input::default()
                },
            };
            w.advance(&[(2, cmd), (3, cmd)]);
        }
        assert_eq!(w.round.action_by, Some(3));
        assert_eq!(w.rules.held_ticks, 6);
        w.remove(3);
        use_for(&mut w, 2, 1);
        assert_eq!(w.rules.held_ticks, 1);
    }

    #[test]
    fn drop_recover_disconnect_fall_and_late_join_preserve_one_life() {
        let mut w = world("relay");
        assert!(w.add_team(3, 0));
        live(&mut w);
        let origin = w.view(1).unwrap().body.pos;
        drive(
            &mut w,
            1,
            Input {
                drop_bomb: true,
                ..Input::default()
            },
        );
        assert_eq!(w.round.bomb, BombView::Dropped(origin));
        place(&mut w, 2, origin);
        use_for(&mut w, 2, 1);
        assert_eq!(w.round.bomb, BombView::Dropped(origin));
        place(&mut w, 3, origin);
        use_for(&mut w, 3, 1);
        assert_eq!(w.round.bomb, BombView::Carried(3));
        w.remove(3);
        assert_eq!(w.round.bomb, BombView::Dropped(origin));
        use_for(&mut w, 1, 1);
        assert_eq!(w.round.bomb, BombView::Carried(1));
        assert!(w.add_team(4, 0));
        assert_eq!(w.view(4).unwrap().hp, 0);
        place(&mut w, 1, Vec3::new(0., -50., 0.));
        w.advance(&[]);
        assert_eq!(w.view(1).unwrap().hp, 0);
        assert_eq!(w.view(1).unwrap().deaths, 1);
        assert!(matches!(w.round.bomb, BombView::Dropped(p) if p.y >= -32.));
        assert_eq!(w.round.winner, Some(1));
        let waiting_money = w.view(4).unwrap().money;
        for _ in 0..TICK_RATE {
            w.advance(&[]);
        }
        assert_eq!(w.view(4).unwrap().hp, 0);
        assert_eq!(w.view(4).unwrap().money, waiting_money);
        w.round.remaining = 1;
        w.advance(&[]);
        assert_eq!(w.view(4).unwrap().hp, 100);
    }

    #[test]
    fn economy_freeze_gear_retention_and_friendly_fire() {
        let mut w = world("drill");
        assert!(w.add_team(3, 0));
        assert_eq!(w.view(1).unwrap().rifle.magazine, 12);
        let spawn = w.view(1).unwrap().body.pos;
        let buy = Input {
            buy: true,
            fire: true,
            strafe: 1.,
            ..Input::default()
        };
        drive(&mut w, 1, buy);
        let buyer = w.view(1).unwrap();
        assert_eq!(
            (buyer.money, buyer.rifle.magazine, buyer.armor),
            (0, 30, 50)
        );
        assert_eq!(buyer.body.pos, spawn);
        assert!(buyer.shot.is_none());
        assert_eq!(w.round.bomb, BombView::None);
        live(&mut w);
        drive(&mut w, 2, buy);
        assert_eq!(w.view(2).unwrap().rifle.magazine, 12);
        w.damage(3, 1, 34);
        assert_eq!(w.view(1).unwrap().hp, 100);
        w.damage(2, 1, 34);
        assert_eq!((w.view(1).unwrap().hp, w.view(1).unwrap().armor), (77, 39));
        w.damage(1, 2, 255);
        w.advance(&[]);
        assert_eq!(w.round.result, Some(RoundResult::Elimination));
        assert_eq!(w.view(1).unwrap().money, KILL_REWARD + WIN_REWARD);
        assert_eq!(w.view(2).unwrap().money, START_MONEY + LOSS_REWARD);
        w.round.remaining = 1;
        w.advance(&[]);
        assert_eq!(
            (w.view(1).unwrap().rifle.magazine, w.view(1).unwrap().armor),
            (30, 39)
        );
        assert_eq!(w.view(2).unwrap().rifle.magazine, 12);
        assert_eq!(w.view(1).unwrap().hp, 100);
    }

    #[test]
    fn five_per_side_live_timeout_halftime_and_full_match() {
        let mut w = world("drill");
        for id in 3..=10 {
            assert!(w.add(id));
        }
        assert!(!w.add(11));
        assert!(!w.add_team(12, 1));
        assert!(w.players.values().all(|p| p.view.hp == 100));
        live(&mut w);
        w.round.remaining = 1;
        w.advance(&[]);
        assert_eq!(w.round.result, Some(RoundResult::Time));
        assert_eq!(w.round.score, [0, 1]);
        w.round.number = HALF_ROUNDS;
        w.round.score = [7, 5];
        w.players.get_mut(&1).unwrap().view.money = MONEY_LIMIT;
        w.round.remaining = 1;
        w.advance(&[]);
        assert_eq!(w.round.number, 13);
        assert_eq!(w.round.score, [5, 7]);
        assert_eq!((w.view(1).unwrap().team, w.view(2).unwrap().team), (1, 0));
        assert_eq!(w.view(1).unwrap().money, START_MONEY);
        assert_eq!(w.view(1).unwrap().rifle.magazine, 12);
        w.round.score = [10, 12];
        live(&mut w);
        w.round.remaining = 1;
        w.advance(&[]);
        assert!(w.round.match_over);
        assert_eq!(w.round.score, [10, 13]);
        w.round.remaining = 1;
        w.advance(&[]);
        assert_eq!(w.round.number, 1);
        assert_eq!(w.round.score, [0, 0]);
        w.round.number = 24;
        w.round.score = [12, 11];
        live(&mut w);
        w.round.remaining = 1;
        w.advance(&[]);
        assert!(w.round.match_over);
        assert_eq!(w.round.score, [12, 12]);
    }

    #[test]
    fn deathmatch_spawns_and_reloads_with_selected_kit() {
        for kit in [Loadout::Standard, Loadout::Light, Loadout::Armored] {
            let mut w = World::new(
                Arena::load("drill").unwrap(),
                Mode::Deathmatch,
                Tuning::default(),
            );
            w.set_dm_loadout(kit);
            assert!(w.add(1) && w.add(2));
            let mag = if kit == Loadout::Light { 12 } else { 30 };
            assert_eq!(
                (w.view(2).unwrap().rifle.ammo, w.view(2).unwrap().armor),
                (mag, kit.armor())
            );
            w.damage(1, 2, 255);
            for _ in 1..TICK_RATE {
                w.advance(&[]);
            }
            assert_eq!(w.view(2).unwrap().hp, 0);
            w.advance(&[]);
            let p = w.view(2).unwrap();
            assert_eq!((p.hp, p.rifle.ammo, p.armor), (100, mag, kit.armor()));
            let mut rifle = p.rifle;
            rifle.ammo = 0;
            let tune = Tuning {
                reload_ticks: 1,
                ..Tuning::default()
            };
            rifle.tick(
                Input {
                    reload: true,
                    ..Input::default()
                },
                &p.body,
                0,
                1,
                &tune,
            );
            rifle.tick(Input::default(), &p.body, 1, 1, &tune);
            assert_eq!(rifle.ammo, mag);
        }
    }
}
