use super::*;
use crate::{Command, RoundResult, Tuning, World};

fn setup(map: &str, mode: Mode) -> (World, BotNav) {
    let arena = Arena::load(map).unwrap();
    let nav = BotNav::new(&arena);
    let mut world = World::new(arena, mode, Tuning::default());
    assert!(world.add_team(1, 0));
    assert!(world.add_team(2, 1));
    (world, nav)
}

fn drive(world: &mut World, nav: &BotNav, brain: &mut BotBrain) {
    let tick = world.tick + 1;
    let views = world.views();
    let me = views.iter().find(|p| p.id == brain.id).unwrap();
    let input = brain.command(
        tick,
        world.mode,
        me,
        &views,
        &world.round_view(),
        &world.arena,
        nav,
    );
    world.advance(&[(
        me.id,
        Command {
            seq: tick,
            seen_tick: world.tick,
            input,
        },
    )]);
}

#[test]
fn reaction_delay_and_aim_error_depend_on_skill() {
    let (world, nav) = setup("drill", Mode::Practice);
    let players = world.views();
    let first_shot = |skill| {
        let mut bot = BotBrain::new(1, skill, 6619);
        let mut me = *world.view(1).unwrap();
        for tick in 1..150 {
            let input = bot.command(
                tick,
                world.mode,
                &me,
                &players,
                &world.round_view(),
                &world.arena,
                &nav,
            );
            me.yaw = input.yaw;
            me.pitch = input.pitch;
            if input.fire {
                return (tick, input.yaw);
            }
        }
        panic!("no shot on an unobstructed target");
    };
    let high = first_shot(1.0);
    let low = first_shot(0.0);
    assert!(
        low.0 >= 50 && low.0 >= high.0 + 20,
        "high {high:?} low {low:?}"
    );
    assert!(low.1.abs() > high.1.abs());
    assert_eq!(high, first_shot(1.0));
}

#[test]
fn silent_hidden_enemies_and_unseen_dropped_bomb_do_not_steer_an_agent() {
    let (world, nav) = setup("relay", Mode::Competitive);
    let me = *world.view(1).unwrap();
    let mut hidden = *world.view(2).unwrap();
    let visible_point = me.body.pos - Vec3::Z * 5.;
    let behind = me.body.pos + Vec3::Z * 2.;
    assert!(!BotBrain::in_view(&me, behind + Vec3::Y, &world.arena));
    hidden.body = Body::at(behind);
    let mut a = BotBrain::new(1, 0.8, 41);
    let mut b = BotBrain::new(1, 0.8, 41);
    let concealed = RoundView {
        number: 1,
        phase: Phase::Live,
        bomb: BombView::Dropped(behind),
        ..RoundView::default()
    };
    let absent = RoundView {
        bomb: BombView::None,
        ..concealed
    };
    for tick in 1..80 {
        assert_eq!(
            a.command(
                tick,
                world.mode,
                &me,
                &[me, hidden],
                &concealed,
                &world.arena,
                &nav
            ),
            b.command(tick, world.mode, &me, &[me], &absent, &world.arena, &nav)
        );
    }
    assert!(a.contact.is_none() && a.dropped.is_none());
    // A visible drop is picked as an interaction goal using that same frustum.
    let mut seen = BotBrain::new(1, 0.8, 41);
    let exposed = RoundView {
        bomb: BombView::Dropped(visible_point),
        ..absent
    };
    seen.command(1, world.mode, &me, &[me], &exposed, &world.arena, &nav);
    assert_eq!(seen.dropped, Some(visible_point));
}

#[test]
fn static_cover_masks_a_target_within_the_view_frustum() {
    let (world, nav) = setup("drill", Mode::Practice);
    let me = *world.view(1).unwrap();
    let mut hidden = *world.view(2).unwrap();
    // This line crosses the solid southeast peek corner; yaw is inside the frustum.
    hidden.body = Body::at(Vec3::new(4.8, 0., 0.4));
    assert!(!has_line(&world.arena, me.body.eye(), hidden.body.eye()));
    let mut bot = BotBrain::new(1, 1., 6619);
    for tick in 1..90 {
        let input = bot.command(
            tick,
            Mode::Practice,
            &me,
            &[me, hidden],
            &world.round_view(),
            &world.arena,
            &nav,
        );
        assert!(!input.fire);
    }
    assert!(bot.contact.is_none());
}

#[test]
fn attacker_routes_from_relay_spawn_plants_then_a_defender_runs_and_defuses() {
    let (mut world, nav) = setup("relay", Mode::Competitive);
    let mut attack = BotBrain::new(1, 0.72, 6619);
    let mut defend = BotBrain::new(2, 0.72, 6619);
    for _ in 0..(40 * TICK_RATE) {
        drive(&mut world, &nav, &mut attack);
        if matches!(world.round_view().phase, Phase::Planted | Phase::Ended) {
            break;
        }
    }
    assert_eq!(
        world.round_view().phase,
        Phase::Planted,
        "attacker pos {:?}, defender hp {}, phase {:?}, target {:?}, path {:?}",
        world.view(1).unwrap().body.pos,
        world.view(2).unwrap().hp,
        world.round_view(),
        attack.contact.map(|c| c.point),
        attack.path
    );
    let plant_position = world.view(1).unwrap().body.pos;
    assert!(world
        .arena
        .sites
        .iter()
        .any(|s| s.pos.distance(plant_position) < s.radius));
    world.remove(1);
    for _ in 0..(32 * TICK_RATE) {
        drive(&mut world, &nav, &mut defend);
        if world.round_view().phase == Phase::Ended {
            break;
        }
    }
    assert_eq!(
        world.round_view().result,
        Some(RoundResult::Defused),
        "defender pos {:?}, plant {:?}, known {:?}, path {:?}, round {:?}",
        world.view(2).unwrap().body.pos,
        plant_position,
        defend.known_plant,
        defend.path,
        world.round_view()
    );
}

#[test]
fn nav_connects_both_relay_sites_with_body_clearance() {
    let (world, nav) = setup("relay", Mode::Competitive);
    for spawn in &world.arena.spawns {
        for site in &world.arena.sites {
            let path = nav.route(&world.arena, spawn.pos, site.pos, 17, 0.5);
            assert_eq!(path.last(), Some(&site.pos));
            let mut prev = spawn.pos;
            for point in path {
                assert!(walkable(&world.arena, prev, point));
                prev = point;
            }
        }
    }
}

#[test]
fn full_height_teammate_occludes_body_samples_until_the_enemy_is_visible() {
    let (world, nav) = setup("drill", Mode::Practice);
    let me = *world.view(1).unwrap();
    let enemy = *world.view(2).unwrap();
    let mut teammate = me;
    teammate.id = 3;
    teammate.body = Body::at((enemy.body.pos + me.body.pos) * 0.5);
    let mut brain = BotBrain::new(1, 1., DEFAULT_BOT_SEED);
    let round = RoundView {
        phase: Phase::Live,
        number: 1,
        ..RoundView::default()
    };
    brain.command(
        1,
        Mode::Competitive,
        &me,
        &[me, enemy, teammate],
        &round,
        &world.arena,
        &nav,
    );
    assert!(brain.contact.is_none());
    teammate.body.pos.x += 1.;
    brain.command(
        6,
        Mode::Competitive,
        &me,
        &[me, enemy, teammate],
        &round,
        &world.arena,
        &nav,
    );
    assert_eq!(brain.contact.map(|c| c.id), Some(enemy.id));
}

#[test]
fn gunfire_behind_the_camera_supplies_only_an_audible_bearing() {
    let (world, nav) = setup("drill", Mode::Practice);
    let me = *world.view(1).unwrap();
    let mut enemy = *world.view(2).unwrap();
    let mut brain = BotBrain::new(me.id, 1., DEFAULT_BOT_SEED);
    let round = world.round_view();
    for (tick, distance) in [(1, SHOT_HEARING_RANGE + 2.), (20, 5.)] {
        enemy.body = Body::at(me.body.pos + Vec3::Z * distance);
        enemy.shot = crate::Rifle::default().tick(
            Input {
                fire: true,
                ..Input::default()
            },
            &enemy.body,
            tick,
            73,
            &world.tuning,
        );
        enemy.shot_tick = tick;
        let input = brain.command(
            tick,
            Mode::Practice,
            &me,
            &[me, enemy],
            &round,
            &world.arena,
            &nav,
        );
        assert!(brain.contact.is_none() && !input.fire);
        if tick == 1 {
            assert!(brain.heard.is_none());
        } else {
            assert_eq!(brain.heard, Some((20, PI)));
        }
    }
}
