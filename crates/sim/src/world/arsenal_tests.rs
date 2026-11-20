use super::*;
use crate::map::{Material, Solid};

fn lane(distance: f32, cover: bool, mode: Mode) -> World {
    let mut boxes = vec![Solid {
        min: Vec3::new(-20., -1., -25.),
        max: Vec3::new(20., 0., 25.),
        material: Material::Floor,
    }];
    if cover {
        boxes.push(Solid {
            min: Vec3::new(-1., 0., -1.1),
            max: Vec3::new(1., 3., -1.0),
            material: Material::Wall,
        });
    }
    let arena = Arena {
        name: "lane".into(),
        boxes,
        spawns: vec![
            Spawn {
                pos: Vec3::ZERO,
                yaw: 0.,
                team: 0,
            },
            Spawn {
                pos: Vec3::new(0., 0., -distance),
                yaw: std::f32::consts::PI,
                team: 1,
            },
        ],
        ..Arena::default()
    };
    let mut world = World::new(arena, mode, Tuning::default());
    assert!(world.add(1) && world.add(2));
    world
}

fn send(world: &mut World, input: Input) {
    let next = world.tick + 1;
    world.advance(&[(
        1,
        Command {
            seq: next,
            seen_tick: next,
            input,
        },
    )]);
}

fn draw_and_fire(world: &mut World, weapon: Weapon) -> Shot {
    send(
        world,
        Input {
            equip: Some(weapon),
            ..Input::default()
        },
    );
    for _ in 0..weapon.spec().draw_ticks {
        send(world, Input::default());
    }
    send(
        world,
        Input {
            fire: true,
            ..Input::default()
        },
    );
    world.view(1).unwrap().shot.unwrap()
}

#[test]
fn every_weapon_reports_authoritative_impact_range_source_and_real_health_loss() {
    for weapon in Weapon::ALL {
        let range = if weapon == Weapon::Knife { 1.8 } else { 10. };
        let mut world = lane(range, false, Mode::Deathmatch);
        let shot = draw_and_fire(&mut world, weapon);
        assert_eq!(
            (shot.weapon, shot.hit, shot.tick),
            (weapon, Some(2), world.tick)
        );
        assert!((shot.distance - (range - RADIUS)).abs() < 0.001);
        assert_eq!(shot.origin, world.view(1).unwrap().body.eye());
        let victim = world.view(2).unwrap();
        let expected = weapon.spec().damage.min(100);
        assert_eq!(victim.hp, 100 - expected);
        let hit = victim.last_hit.unwrap();
        assert_eq!(
            hit,
            Hit {
                tick: world.tick,
                source: Some(1),
                origin: shot.origin,
                weapon: Some(weapon),
                damage: expected
            }
        );
        for _ in 0..4 {
            world.advance(&[]);
        }
        assert_eq!(world.view(2).unwrap().last_hit, Some(hit));
        world.respawn(2);
        assert!(world.view(2).unwrap().last_hit.is_none());
    }
}

#[test]
fn knife_stops_at_contact_range_or_cover_and_gun_tracers_end_on_static_geometry() {
    let mut outside = lane(3., false, Mode::Practice);
    let swing = draw_and_fire(&mut outside, Weapon::Knife);
    assert_eq!(
        (swing.hit, swing.distance),
        (None, Weapon::Knife.spec().range)
    );
    assert_eq!(outside.view(2).unwrap().hp, 100);
    for weapon in [Weapon::Knife, Weapon::M4, Weapon::Sniper] {
        let mut blocked = lane(1.8, true, Mode::Practice);
        let shot = draw_and_fire(&mut blocked, weapon);
        assert_eq!(shot.hit, None);
        assert!((shot.distance - 1.0).abs() < 0.001);
        assert!(blocked.view(2).unwrap().last_hit.is_none());
    }
}

#[test]
fn competitive_equipment_respects_purchase_while_surf_issues_no_weapon() {
    let mut world = lane(8., false, Mode::Competitive);
    world.advance(&[]);
    assert_eq!(world.round.phase, Phase::Buy);
    assert_eq!(world.view(1).unwrap().rifle.magazine, 12);
    assert!(!world.view(1).unwrap().rifle.owns(Weapon::Sniper));
    send(
        &mut world,
        Input {
            equip: Some(Weapon::Sniper),
            ..Input::default()
        },
    );
    assert_eq!(world.view(1).unwrap().rifle.weapon, Weapon::M4);
    send(
        &mut world,
        Input {
            equip: Some(Weapon::Pistol),
            ..Input::default()
        },
    );
    assert_eq!(world.view(1).unwrap().rifle.weapon, Weapon::Pistol);
    send(
        &mut world,
        Input {
            buy: true,
            equip: Some(Weapon::Sniper),
            fire: true,
            ..Input::default()
        },
    );
    let player = world.view(1).unwrap();
    assert_eq!(
        (player.rifle.weapon, player.rifle.switch_left, player.money),
        (Weapon::Sniper, 48, 0)
    );
    assert!(player.shot.is_none());
    for _ in 0..48 {
        world.advance(&[]);
    }
    assert_eq!(world.view(1).unwrap().rifle.switch_left, 0);
    // Survival retains the purchase and active slot on a new round.
    world.round.remaining = 1;
    world.advance(&[]);
    world.damage(1, 2, 255);
    world.advance(&[]);
    world.round.remaining = 1;
    world.advance(&[]);
    assert_eq!(world.view(1).unwrap().rifle.weapon, Weapon::Sniper);
    assert!(!world.view(2).unwrap().rifle.owns(Weapon::Sniper));

    let mut surf = lane(8., false, Mode::Surf);
    for _ in 0..160 {
        send(
            &mut surf,
            Input {
                equip: Some(Weapon::Smg),
                fire: true,
                ..Input::default()
            },
        );
    }
    assert!(!surf.view(1).unwrap().armed);
    assert!(Weapon::ALL
        .iter()
        .all(|&w| !surf.view(1).unwrap().rifle.owns(w)));
    assert!(surf.view(1).unwrap().shot.is_none());
    assert_eq!(surf.view(2).unwrap().hp, 100);
}

#[test]
fn deathmatch_respawns_with_the_last_chosen_weapon_and_fresh_magazines() {
    let mut world = lane(15., false, Mode::Deathmatch);
    draw_and_fire(&mut world, Weapon::Ak);
    assert_eq!(world.view(1).unwrap().rifle.ammo, 23);
    world.damage(2, 1, 255);
    for _ in 0..TICK_RATE {
        world.advance(&[]);
    }
    let respawn = world.view(1).unwrap();
    assert_eq!(respawn.hp, 100);
    assert_eq!(
        (
            respawn.rifle.weapon,
            respawn.rifle.ammo,
            respawn.rifle.switch_left
        ),
        (Weapon::Ak, 24, 0)
    );
    assert_eq!(respawn.rifle.ammo_for(Weapon::M4), 30);
    assert!(respawn.last_hit.is_none());
}
