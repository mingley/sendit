use super::{Player, Spawn, World};
use crate::motion::{clearance, HEIGHT, RADIUS};

impl World {
    /// In free-for-all every live body is an opponent, regardless of spawn color.
    /// Rotate equally safe markers by life; the same world always makes the same choice.
    pub(super) fn deathmatch_spawn(&self, current: &Player) -> Option<Spawn> {
        use std::cmp::Reverse;
        const CLOSE: f32 = 16.0;
        const RANGE_BAND: f32 = 2.0;
        let markers = &self.arena.spawns;
        let living = self
            .players
            .values()
            .filter(|p| p.view.id != current.view.id && p.view.hp > 0);
        if living.clone().next().is_none() {
            return self
                .spawn(current.view.team, current.spawn_slot)
                .filter(|s| clearance(&self.arena, s.pos))
                .or_else(|| {
                    markers
                        .iter()
                        .find(|s| clearance(&self.arena, s.pos))
                        .copied()
                });
        }

        let start = usize::from(current.view.id).wrapping_add(current.view.life as usize);
        let mut best = None;
        for n in 0..markers.len() {
            let candidate = markers[start.wrapping_add(n) % markers.len()];
            if !clearance(&self.arena, candidate.pos) {
                continue;
            }
            let mut overlaps = 0u32;
            let mut visible_close = 0u32;
            let mut closest_visible = CLOSE + RANGE_BAND;
            let mut nearest_sq = f32::INFINITY;
            for other in living.clone() {
                let body = &other.view.body;
                let delta = candidate.pos - body.pos;
                let dist_sq = delta.length_squared();
                nearest_sq = nearest_sq.min(dist_sq);
                // The movement hull has a square footprint and full standing height.
                if delta.x.abs() < 2.0 * RADIUS
                    && delta.z.abs() < 2.0 * RADIUS
                    && delta.y.abs() < HEIGHT
                {
                    overlaps += 1;
                }
                if dist_sq <= CLOSE * CLOSE {
                    let dist = dist_sq.sqrt();
                    if dist < 0.001
                        || self.arena.ray_distance(body.eye(), delta / dist, dist) >= dist - 0.001
                    {
                        visible_close += 1;
                        closest_visible = closest_visible.min(dist);
                    }
                }
            }
            // Preserve hull clearance first. One point-blank opponent is more
            // immediate than two distant sightlines. Within a two-metre band,
            // fewer viewers, more space and the marker rotation break ties.
            let rating = (
                Reverse(overlaps),
                (closest_visible / RANGE_BAND) as u32,
                Reverse(visible_close),
                (nearest_sq.sqrt() / RANGE_BAND) as u32,
            );
            if best.is_none_or(|(previous, _)| rating > previous) {
                best = Some((rating, candidate));
            }
        }
        best.map(|(_, spawn)| spawn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        map::{Material, Solid},
        Arena, Body, Mode, Tuning, Vec3, TICK_RATE,
    };

    fn marker(x: f32, z: f32, team: u8) -> Spawn {
        Spawn {
            pos: Vec3::new(x, 0., z),
            yaw: team as f32,
            team,
        }
    }

    fn block(min: [f32; 3], max: [f32; 3]) -> Solid {
        Solid {
            min: min.into(),
            max: max.into(),
            material: Material::Wall,
        }
    }

    fn setup(markers: &[Spawn], walls: &[Solid]) -> World {
        let mut boxes = vec![block([-40., -1., -40.], [40., 0., 40.])];
        boxes.extend_from_slice(walls);
        let arena = Arena {
            name: "spawn-test".into(),
            spawns: markers.to_vec(),
            boxes,
            ..Arena::default()
        };
        arena.validate().unwrap();
        World::new(arena, Mode::Deathmatch, Tuning::default())
    }

    #[test]
    fn covered_marker_beats_a_more_distant_camped_marker_and_uses_authored_facing() {
        let exposed = marker(0., -9., 0);
        let covered = marker(3., 0., 1);
        let mut w = setup(&[exposed, covered], &[block([1., 0., -2.], [1.5, 2.2, 2.])]);
        // Same-team opponents are still threats in free-for-all.
        assert!(w.add_team(1, 0) && w.add_team(2, 0));
        assert_eq!(w.view(1).unwrap().body.pos, exposed.pos);
        w.players.get_mut(&2).unwrap().view.body = Body::at(Vec3::ZERO);
        w.damage(2, 1, 255);
        for _ in 1..TICK_RATE {
            w.advance(&[]);
        }
        assert_eq!(w.view(1).unwrap().hp, 0);
        w.advance(&[]);
        let p = w.view(1).unwrap();
        assert_eq!((p.body.pos, p.yaw, p.hp), (covered.pos, covered.yaw, 100));
        w.damage(2, 1, 34);
        assert_eq!(w.view(1).unwrap().hp, 66);
    }

    #[test]
    fn several_distant_sightlines_are_safer_than_a_point_blank_spawn() {
        let camp = marker(0., 0., 0);
        let distant = marker(0., -25., 1);
        let mut w = setup(&[camp, distant], &[]);
        for id in 1..=4 {
            assert!(w.add(id));
        }
        for (id, pos) in [
            (2, Vec3::new(0., 0., 1.)),
            (3, Vec3::new(-10., 0., -18.)),
            (4, Vec3::new(10., 0., -18.)),
        ] {
            w.players.get_mut(&id).unwrap().view.body = Body::at(pos);
        }
        w.respawn(1);
        assert_eq!(w.view(1).unwrap().body.pos, distant.pos);
    }

    #[test]
    fn concurrent_respawns_avoid_occupied_or_embedded_markers_when_space_exists() {
        let free = [marker(-3., 0., 0), marker(0., 3., 0), marker(3., 0., 1)];
        let embedded = marker(19., 0., 1);
        let mut w = setup(
            &[free[0], free[1], free[2], embedded],
            &[block([18., 0., -1.], [20., 2., 1.])],
        );
        assert!(w.add(1) && w.add(2) && w.add(3));
        w.players.get_mut(&1).unwrap().view.body = Body::at(free[0].pos);
        w.damage(1, 2, 255);
        w.damage(1, 3, 255);
        for _ in 0..TICK_RATE {
            w.advance(&[]);
        }
        let a = w.view(2).unwrap().body.pos;
        let b = w.view(3).unwrap().body.pos;
        assert_ne!(a, b);
        assert!([free[1].pos, free[2].pos].contains(&a));
        assert!([free[1].pos, free[2].pos].contains(&b));
    }

    #[test]
    fn equivalent_markers_rotate_reproducibly_and_solitary_players_keep_their_spawn() {
        let left = marker(-4., 0., 0);
        let right = marker(4., 0., 1);
        let run = || {
            let mut w = setup(&[left, right], &[]);
            assert!(w.add(1) && w.add(2));
            w.players.get_mut(&2).unwrap().view.body = Body::at(Vec3::ZERO);
            w.respawn(1);
            let first = w.view(1).unwrap().body.pos;
            w.respawn(1);
            let second = w.view(1).unwrap().body.pos;
            assert_ne!(first, second);
            w.remove(2);
            for _ in 0..3 {
                w.respawn(1);
                assert_eq!(w.view(1).unwrap().body.pos, left.pos);
            }
            (first, second)
        };
        assert_eq!(run(), run());
    }
}
