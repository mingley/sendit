//! Server-controlled players. Perception feeds remembered observations into a
//! controller which can only emit the same tick input as a connected player.
use crate::{
    motion::{look_direction, RADIUS},
    Arena, Body, BombView, Input, Mode, Phase, PlayerView, RoundView, Vec3, Weapon, TICK_RATE,
};
use std::f32::consts::{FRAC_PI_4, PI, TAU};

pub const DEFAULT_BOT_SEED: u32 = 6619;
pub const SHOT_HEARING_RANGE: f32 = 65.;
pub const STEP_HEARING_RANGE: f32 = 23.;
pub const STEP_STRIDE: f32 = 1.85;
pub const MELEE_HEARING_RANGE: f32 = 12.;
pub const OCCLUDED_GAIN: f32 = 0.52;

pub fn sound_gain(arena: &Arena, listener: Vec3, source: Vec3) -> f32 {
    let delta = source - listener;
    let distance = delta.length();
    if distance > 0.15 && arena.ray_distance(listener, delta, distance) < distance - 0.15 {
        OCCLUDED_GAIN
    } else {
        1.0
    }
}

fn mix(mut value: u32) -> u32 {
    value = (value ^ (value >> 16)).wrapping_mul(0x85eb_ca6b);
    value = (value ^ (value >> 13)).wrapping_mul(0xc2b2_ae35);
    value ^ (value >> 16)
}

fn unit(value: u32) -> f32 {
    (mix(value) >> 8) as f32 / 16_777_216.0
}
fn flat(v: Vec3) -> Vec3 {
    v.with_y(0.)
}
fn angle(v: Vec3) -> f32 {
    v.x.atan2(-v.z)
}
fn turn(a: f32, b: f32) -> f32 {
    (b - a + PI).rem_euclid(TAU) - PI
}
fn has_line(arena: &Arena, a: Vec3, b: Vec3) -> bool {
    let delta = b - a;
    let distance = delta.length();
    distance < 0.015 || arena.ray_distance(a, delta, distance) >= distance - 0.012
}

fn supported(arena: &Arena, point: Vec3) -> bool {
    let near = |y: f32| (y - point.y).abs() <= 0.55;
    let inside = |lo: Vec3, hi: Vec3| {
        point.x >= lo.x && point.x <= hi.x && point.z >= lo.z && point.z <= hi.z
    };
    arena
        .boxes
        .iter()
        .any(|s| inside(s.min, s.max) && near(s.max.y))
        || arena
            .ramps
            .iter()
            .any(|r| inside(r.min, r.max) && near(r.height_at(point.x, point.z)))
}

fn walkable(arena: &Arena, a: Vec3, b: Vec3) -> bool {
    let horizontal = flat(b - a);
    let length = horizontal.length();
    if length < 0.15 {
        return (b.y - a.y).abs() < 0.5;
    }
    let side = Vec3::new(-horizontal.z, 0., horizontal.x) * ((RADIUS + 0.075) / length);
    for offset in [Vec3::Y * 0.4 + side, Vec3::Y * 0.4 - side, Vec3::Y * 1.60] {
        if !has_line(arena, a + offset, b + offset) {
            return false;
        }
    }
    let checks = (length / 1.4).ceil().clamp(1., 128.) as usize;
    (1..=checks).all(|i| supported(arena, a.lerp(b, i as f32 / checks as f32)))
}

/// Static authored-map navigation. The graph includes spawn and site landmarks
/// even on maps whose authors omit optional route hints.
pub struct BotNav {
    points: Vec<Vec3>,
    edges: Vec<Vec<(usize, f32)>>,
}

impl BotNav {
    pub fn new(arena: &Arena) -> Self {
        let mut points = Vec::new();
        for point in arena
            .nav
            .iter()
            .copied()
            .chain(arena.spawns.iter().map(|s| s.pos))
            .chain(arena.sites.iter().map(|s| s.pos))
        {
            if !points
                .iter()
                .any(|p: &Vec3| p.distance_squared(point) < 0.04)
            {
                points.push(point);
            }
        }
        let mut edges = vec![Vec::new(); points.len()];
        for i in 0..points.len() {
            for j in i + 1..points.len() {
                let distance = points[i].distance(points[j]);
                if distance <= 45. && walkable(arena, points[i], points[j]) {
                    edges[i].push((j, distance));
                    edges[j].push((i, distance));
                }
            }
        }
        Self { points, edges }
    }

    fn route(&self, arena: &Arena, start: Vec3, goal: Vec3, style: u32, skill: f32) -> Vec<Vec3> {
        if walkable(arena, start, goal) {
            return vec![goal];
        }
        let count = self.points.len();
        let mut costs = vec![f32::INFINITY; count];
        let mut previous = vec![None; count];
        let mut visited = vec![false; count];
        for (i, &point) in self.points.iter().enumerate() {
            if walkable(arena, start, point) {
                costs[i] = start.distance(point);
            }
        }
        let mut finish = None;
        let mut best = f32::INFINITY;
        while let Some(i) = (0..count)
            .filter(|&i| !visited[i] && costs[i] < best)
            .min_by(|&a, &b| costs[a].total_cmp(&costs[b]))
        {
            visited[i] = true;
            let point = self.points[i];
            let total = costs[i] + point.distance(goal);
            if total < best && walkable(arena, point, goal) {
                finish = Some(i);
                best = total;
            }
            for &(j, distance) in &self.edges[i] {
                // Low-skill players take less economical paths. This bias stays
                // fixed throughout a round so an actor does not oscillate.
                let variability = 0.06 + (1. - skill) * 0.36;
                let weight =
                    1. + variability * unit(style ^ ((i.min(j) as u32) << 16) ^ i.max(j) as u32);
                let via = costs[i] + distance * weight;
                if via < costs[j] {
                    costs[j] = via;
                    previous[j] = Some(i);
                }
            }
        }
        let mut path = Vec::new();
        let mut cursor = finish;
        while let Some(i) = cursor {
            path.push(self.points[i]);
            cursor = previous[i];
        }
        path.reverse();
        if finish.is_some() {
            path.push(goal);
        }
        path
    }

    fn post(&self, site: Vec3, facing: Vec3, id: u16, skill: f32) -> Vec3 {
        if skill < 0.25 {
            return site;
        }
        let mut candidates = self.points.iter().copied().filter(|&p| {
            let offset = flat(p - site);
            (2.8f32.powi(2)..=9.5f32.powi(2)).contains(&offset.length_squared())
                && offset.dot(facing) < 2.
        });
        let count = candidates.clone().count();
        if count == 0 {
            site
        } else {
            candidates
                .nth((usize::from(id) / 2) % count)
                .unwrap_or(site)
        }
    }
}

#[derive(Clone, Copy)]
struct Contact {
    id: u16,
    point: Vec3,
    ready: u32,
    seen: u32,
}

#[derive(Clone, Copy)]
struct Steps {
    id: u16,
    life: u32,
    pos: Vec3,
    stride: f32,
    shot: u32,
}

#[derive(Clone, Copy)]
struct Goal {
    pos: Vec3,
    use_radius: f32,
    hold: bool,
}

pub struct BotBrain {
    id: u16,
    skill: f32,
    seed: u32,
    life: Option<(u32, u16)>,
    next_scan: u32,
    contact: Option<Contact>,
    heard: Option<(u32, f32)>,
    steps: Vec<Steps>,
    dropped: Option<Vec3>,
    known_plant: Option<Vec3>,
    path: Vec<Vec3>,
    waypoint: usize,
    destination: Option<Vec3>,
    retry_path: u32,
    patrol: u32,
}

impl BotBrain {
    pub fn new(id: u16, skill: f32, seed: u32) -> Self {
        Self {
            id,
            skill: if skill.is_finite() {
                skill.clamp(0., 1.)
            } else {
                0.5
            },
            seed: mix(seed ^ u32::from(id)),
            life: None,
            next_scan: 0,
            contact: None,
            heard: None,
            steps: Vec::new(),
            dropped: None,
            known_plant: None,
            path: Vec::new(),
            waypoint: 0,
            destination: None,
            retry_path: 0,
            patrol: 0,
        }
    }

    fn in_view(me: &PlayerView, point: Vec3, arena: &Arena) -> bool {
        let delta = point - me.body.eye();
        let forward = look_direction(me.yaw, me.pitch);
        let depth = delta.dot(forward);
        let right = Vec3::new(me.yaw.cos(), 0., me.yaw.sin());
        let up = right.cross(forward);
        // Perspective frustum of the native 73-degree vertical camera at 16:9.
        depth > 0.
            && depth <= 200.
            && delta.dot(up).abs() <= depth * 0.740
            && delta.dot(right).abs() <= depth * 1.315
            && has_line(arena, me.body.eye(), point)
    }

    fn hear(&mut self, tick: u32, me: &PlayerView, players: &[PlayerView], arena: &Arena) {
        self.steps.retain(|state| {
            players
                .iter()
                .any(|p| p.id == state.id && p.life == state.life)
        });
        let mut loudest = 0.0;
        let mut bearing = None;
        for p in players.iter().filter(|p| p.id != self.id && p.hp > 0) {
            let at = self.steps.iter().position(|state| state.id == p.id);
            let idx = at.unwrap_or_else(|| {
                self.steps.push(Steps {
                    id: p.id,
                    life: p.life,
                    pos: p.body.pos,
                    stride: 0.,
                    shot: 0,
                });
                self.steps.len() - 1
            });
            let state = &mut self.steps[idx];
            let distance = flat(p.body.pos - state.pos).length();
            state.pos = p.body.pos;
            if distance < 3. && p.body.grounded {
                state.stride += distance;
            }
            let stepped = state.stride >= STEP_STRIDE;
            if stepped {
                state.stride %= STEP_STRIDE;
            }
            let new_shot = p.shot.is_some()
                && p.shot_tick != state.shot
                && tick.wrapping_sub(p.shot_tick) <= 6;
            state.shot = p.shot_tick;
            // The only output of hidden movement is a coarse direction *when*
            // that player actually crosses an audible footfall or fires.
            let event = if new_shot {
                p.shot.map(|shot| {
                    let range = if shot.weapon == Weapon::Knife {
                        MELEE_HEARING_RANGE
                    } else {
                        SHOT_HEARING_RANGE
                    };
                    (shot.origin, range)
                })
            } else if stepped {
                Some((p.body.pos, STEP_HEARING_RANGE))
            } else {
                None
            };
            if let Some((point, range)) = event {
                let delta = point - me.body.eye();
                let proximity = 1. - delta.length() / range;
                if proximity <= 0. {
                    continue;
                }
                let loudness = proximity * sound_gain(arena, me.body.eye(), point);
                if loudness > loudest {
                    loudest = loudness;
                    bearing = Some((angle(delta) / FRAC_PI_4).round() * FRAC_PI_4);
                }
            }
        }
        if let Some(yaw) = bearing {
            self.heard = Some((tick, yaw));
        }
    }

    fn scan(
        &mut self,
        tick: u32,
        mode: Mode,
        me: &PlayerView,
        players: &[PlayerView],
        round: &RoundView,
        arena: &Arena,
    ) {
        self.next_scan = tick + 4 + ((1. - self.skill) * 9.) as u32;
        let mut best = None;
        let mut best_cost = f32::INFINITY;
        for p in players.iter().filter(|p| {
            p.id != self.id && p.hp > 0 && (mode != Mode::Competitive || p.team != me.team)
        }) {
            let target = [p.body.pos + Vec3::Y * 1.23, p.body.eye()]
                .into_iter()
                .find(|&aim| {
                    if !Self::in_view(me, aim, arena) {
                        return false;
                    }
                    let delta = aim - me.body.eye();
                    let length = delta.length();
                    let direction = delta / length;
                    !players
                        .iter()
                        .filter(|b| b.hp > 0 && b.id != me.id && b.id != p.id)
                        .any(|b| {
                            crate::world::body_distance(
                                me.body.eye(),
                                direction,
                                b.body.pos,
                                length,
                            )
                            .is_some()
                        })
                });
            if let Some(point) = target {
                let current = self.contact.is_some_and(|c| c.id == p.id);
                let cost = me.body.pos.distance_squared(point) * if current { 0.74 } else { 1. };
                if cost < best_cost {
                    best_cost = cost;
                    best = Some((p.id, point));
                }
            }
        }
        if let Some((id, point)) = best {
            let reaction =
                14 + ((1. - self.skill) * 38.) as u32 + (mix(self.seed ^ u32::from(id)) % 4);
            let ready = self
                .contact
                .filter(|c| c.id == id && tick.wrapping_sub(c.seen) < 100)
                .map_or(tick + reaction, |c| c.ready);
            self.contact = Some(Contact {
                id,
                point,
                seen: tick,
                ready,
            });
        }
        if let BombView::Dropped(point) = round.bomb {
            if me.team == 0 && Self::in_view(me, point + Vec3::Y * 0.3, arena) {
                self.dropped = Some(point);
            }
        } else {
            self.dropped = None;
        }
        if let BombView::Planted(_, point) = round.bomb {
            // The announcement identifies the site. Only store a precise
            // interaction location once the device itself has come into view.
            if Self::in_view(me, point + Vec3::Y * 0.32, arena) {
                self.known_plant = Some(point);
            }
        } else {
            self.known_plant = None;
        }
    }

    fn mission(
        &mut self,
        tick: u32,
        mode: Mode,
        me: &PlayerView,
        round: &RoundView,
        arena: &Arena,
        nav: &BotNav,
    ) -> Goal {
        let roam = |point| Goal {
            pos: point,
            use_radius: 0.,
            hold: false,
        };
        if mode == Mode::Competitive && !arena.sites.is_empty() {
            let site_no = (usize::from(round.number.saturating_sub(1))
                + usize::from(self.id.saturating_sub(1)) / 2)
                % arena.sites.len();
            let default_site = &arena.sites[site_no];
            let enemy_spawn = arena
                .spawns
                .iter()
                .find(|s| s.team != me.team)
                .map_or(Vec3::ZERO, |s| s.pos);
            let sense_post = |site: Vec3| {
                nav.post(
                    site,
                    flat(enemy_spawn - site).normalize_or_zero(),
                    self.id,
                    self.skill,
                )
            };
            match round.bomb {
                BombView::Carried(id) if id == me.id => {
                    return Goal {
                        pos: default_site.pos,
                        use_radius: default_site.radius * 0.70,
                        hold: false,
                    }
                }
                BombView::Dropped(_) if me.team == 0 => {
                    if let Some(known) = self.dropped {
                        return Goal {
                            pos: known,
                            use_radius: 1.05,
                            hold: false,
                        };
                    }
                }
                BombView::Planted(id, _) => {
                    let site = arena.sites.get(id as usize).unwrap_or(default_site);
                    if me.team == 0 {
                        return Goal {
                            pos: sense_post(site.pos),
                            use_radius: 0.,
                            hold: true,
                        };
                    }
                    if let Some(point) = self.known_plant {
                        return Goal {
                            pos: point,
                            use_radius: 1.1,
                            hold: false,
                        };
                    }
                    // Look around the actual site. The search doesn't use
                    // the hidden coordinate from the network snapshot.
                    return Goal {
                        pos: site.pos,
                        use_radius: 0.,
                        hold: true,
                    };
                }
                _ => {}
            }
            if me.team == 1 {
                return Goal {
                    pos: sense_post(default_site.pos),
                    use_radius: 0.,
                    hold: true,
                };
            }
            return Goal {
                pos: default_site.pos,
                use_radius: 0.,
                hold: true,
            };
        }
        if nav.points.is_empty() {
            return roam(me.body.pos);
        }
        let mut idx = mix(self.seed ^ self.patrol) % nav.points.len() as u32;
        if me.body.pos.distance_squared(nav.points[idx as usize]) < 1.2
            || tick.is_multiple_of(8 * TICK_RATE)
        {
            self.patrol = self.patrol.wrapping_add(1);
            idx = mix(self.seed ^ self.patrol) % nav.points.len() as u32;
        }
        roam(nav.points[idx as usize])
    }

    fn waypoint(
        &mut self,
        tick: u32,
        body: &Body,
        goal: Goal,
        arena: &Arena,
        nav: &BotNav,
    ) -> Option<Vec3> {
        let distance = body.pos.distance(goal.pos);
        if distance <= goal.use_radius.max(0.65) {
            return None;
        }
        let changed = self
            .destination
            .is_none_or(|p| p.distance_squared(goal.pos) > 0.16);
        if changed
            || (tick >= self.retry_path && (self.waypoint == self.path.len() || body.speed() < 0.3))
        {
            self.path = nav.route(
                arena,
                body.pos,
                goal.pos,
                self.seed ^ self.patrol,
                self.skill,
            );
            self.waypoint = 0;
            self.destination = Some(goal.pos);
            self.retry_path = tick + TICK_RATE;
        }
        while self.waypoint + 1 < self.path.len()
            && flat(self.path[self.waypoint] - body.pos).length_squared() < 0.75 * 0.75
        {
            self.waypoint += 1;
        }
        self.path.get(self.waypoint).copied()
    }

    /// Run once per authoritative tick, using the same player views, rules and
    /// map as a native client. No server state is mutated here.
    #[allow(clippy::too_many_arguments)]
    pub fn command(
        &mut self,
        tick: u32,
        mode: Mode,
        me: &PlayerView,
        players: &[PlayerView],
        round: &RoundView,
        arena: &Arena,
        nav: &BotNav,
    ) -> Input {
        let mut input = Input {
            yaw: me.yaw,
            pitch: me.pitch,
            ..Input::default()
        };
        if me.hp == 0 {
            return input;
        }
        if self.life != Some((me.life, round.number)) {
            self.life = Some((me.life, round.number));
            self.contact = None;
            self.heard = None;
            self.steps.clear();
            self.known_plant = None;
            self.dropped = None;
            self.destination = None;
            self.next_scan = tick;
        }
        if mode == Mode::Competitive && matches!(round.phase, Phase::Buy | Phase::Ended) {
            input.buy = round.phase == Phase::Buy && tick % 16 == u32::from(self.id) % 16;
            return input;
        }
        if mode == Mode::Deathmatch {
            // Stable assignments vary the lobby on spawn; a new life rotates the kit.
            let slot = if self.id >= crate::wire::FIRST_BOT_ID {
                self.id - crate::wire::FIRST_BOT_ID
            } else {
                self.id.saturating_sub(1)
            };
            let choices = [
                Weapon::M4,
                Weapon::Pistol,
                Weapon::Ak,
                Weapon::Smg,
                Weapon::Sniper,
            ];
            let rotation = u32::from(slot).wrapping_add(me.life.wrapping_sub(1));
            let preferred = choices[(rotation % choices.len() as u32) as usize];
            if me.rifle.weapon != preferred && me.rifle.owns(preferred) {
                input.equip = Some(preferred);
            }
        }
        self.hear(tick, me, players, arena);
        if tick >= self.next_scan {
            self.scan(tick, mode, me, players, round, arena);
        }
        let goal = self.mission(tick, mode, me, round, arena, nav);
        let at_goal = me.body.pos.distance(goal.pos) <= goal.use_radius.max(0.70);
        let waypoint = self.waypoint(tick, &me.body, goal, arena, nav);
        let mut wish = waypoint.map_or(Vec3::ZERO, |p| {
            flat(p - me.body.pos - me.body.vel * 0.070).normalize_or_zero()
        });
        let mut yaw = waypoint.map_or(me.yaw, |p| angle(p - me.body.pos));
        let mut pitch = 0.;
        let mut wants_shot = false;
        if at_goal && goal.use_radius > 0. {
            input.interact = true;
            wish = Vec3::ZERO;
        } else if at_goal && goal.hold {
            let enemy_base = arena
                .spawns
                .iter()
                .find(|s| s.team != me.team)
                .map_or(Vec3::ZERO, |s| s.pos);
            let scan = (tick as f32 * 0.012 + self.id as f32).sin();
            yaw = angle(enemy_base - me.body.pos) + scan * 0.95;
            if me.team == 1
                && matches!(round.bomb, BombView::Planted(..))
                && self.known_plant.is_none()
            {
                // Near the site an agent can hear beeping and sweep its view at the floor.
                yaw = me.yaw + 0.036;
                pitch = -0.55;
                input.interact = true;
            }
        }
        if let Some((heard, bearing)) = self.heard {
            if tick.wrapping_sub(heard) < 54 {
                yaw = bearing;
            }
        }
        if let Some(contact) = self
            .contact
            .filter(|c| tick >= c.ready && tick.wrapping_sub(c.seen) < 110)
        {
            let delta = contact.point - me.body.eye();
            let err = (1. - self.skill).powi(2) * 0.061 + 0.0018;
            let phase = (self.seed % 997) as f32 + tick as f32 * 0.021;
            yaw = angle(delta) + err * phase.sin();
            pitch = delta.y.atan2(flat(delta).length()) + err * (phase * 0.73).cos();
            if me.rifle.recoil_index > 0 {
                if let Some(shot) = me.shot {
                    pitch -= self.skill * 0.84 * shot.recoil_pitch;
                }
            }
            let current = tick.wrapping_sub(contact.seen) <= 4 + ((1. - self.skill) * 9.) as u32;
            if current && !input.interact && mode != Mode::Surf {
                wants_shot = true;
                if self.skill > 0.40 {
                    wish = Vec3::ZERO;
                }
            }
        }
        let max_turn = (2.5 + 6.0 * self.skill) / TICK_RATE as f32;
        input.yaw = me.yaw + turn(me.yaw, yaw).clamp(-max_turn, max_turn);
        input.pitch = (me.pitch + (pitch - me.pitch).clamp(-max_turn, max_turn)).clamp(-1.55, 1.55);
        if wants_shot && turn(input.yaw, yaw).abs() < 0.085 && (input.pitch - pitch).abs() < 0.075 {
            input.fire = if me.rifle.weapon.spec().automatic {
                self.skill < 0.55 || (tick + u32::from(self.id) * 9) % 54 < 34
            } else {
                // A semi-auto needs a released trigger between shots.
                me.rifle.cooldown_left <= 1 && tick % 2 == u32::from(self.id) % 2
            };
        }
        if !input.fire
            && me.rifle.weapon != Weapon::Knife
            && me.rifle.ammo < (me.rifle.magazine / 4).max(1)
            && self.contact.is_none_or(|c| tick.wrapping_sub(c.seen) > 80)
        {
            input.reload = true;
        }
        if wish == Vec3::ZERO && me.body.speed() > 0.26 {
            wish = -flat(me.body.vel).normalize_or_zero() * (me.body.speed() / 1.5).min(1.);
        }
        let (s, c) = input.yaw.sin_cos();
        input.forward = wish.x * s - wish.z * c;
        input.strafe = wish.x * c + wish.z * s;
        input
    }
}

#[cfg(test)]
#[path = "bots_tests.rs"]
mod tests;
