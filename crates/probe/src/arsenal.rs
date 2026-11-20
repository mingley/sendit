//! Drive the shipping deathmatch server exclusively through ordinary UDP inputs.
use crate::{rules::Squad, scenario::aim, start_server};
use sendit_sim::{Input, Shot, Vec3, Weapon};
use serde_json::json;
use std::path::Path;

fn equip(game: &mut Squad, actor: u16, weapon: Weapon) -> Result<(), String> {
    game.until(
        &format!("equip {weapon:?}"),
        3,
        |_, _| {
            Ok(vec![(
                actor,
                Input {
                    equip: Some(weapon),
                    ..Input::default()
                },
            )])
        },
        |s| {
            let gear = s.player(actor)?.rifle;
            Ok(gear.weapon == weapon && gear.switch_left == 0)
        },
    )
}

fn discharge(
    game: &mut Squad,
    actor: u16,
    target: u16,
    weapon: Weapon,
    strike: bool,
) -> Result<Shot, String> {
    let old_tick = game.player(actor)?.shot_tick;
    let old_hp = game.player(target)?.hp;
    let before = game.player(actor)?.rifle.ammo;
    // Release the trigger between single shots. Wait out the previous attack's cooldown.
    game.until(
        "trigger ready",
        3,
        |_, _| Ok(vec![]),
        |s| {
            let gear = s.player(actor)?.rifle;
            Ok(gear.cooldown_left == 0 && gear.reload_left == 0 && gear.switch_left == 0)
        },
    )?;
    game.tick(&[])?;
    game.until(
        &format!("{weapon:?} attack"),
        3,
        |s, tick| {
            let input = aim(s.player(actor)?.body.pos, s.player(target)?.body.pos);
            Ok(vec![(
                actor,
                Input {
                    fire: tick == 0,
                    ..input
                },
            )])
        },
        |s| Ok(s.player(actor)?.shot_tick > old_tick),
    )?;
    let shooter = game.player(actor)?;
    let other = game.player(target)?;
    let shot = shooter.shot.ok_or("attack tick missing its server shot")?;
    if shot.weapon != weapon
        || shot.hit != strike.then_some(target)
        || !shot.distance.is_finite()
        || shot.distance <= 0.
        || shot.distance > weapon.spec().range + 0.05
    {
        return Err(format!(
            "unexpected {weapon:?} trace against player {target}: {shot:?}"
        ));
    }
    if weapon != Weapon::Knife && shooter.rifle.ammo + 1 != before {
        return Err(format!(
            "{weapon:?} shot did not consume precisely one round"
        ));
    }
    if strike {
        let hit = other
            .last_hit
            .ok_or("victim did not receive a damage event")?;
        let expected = old_hp.saturating_sub(weapon.spec().damage);
        if hit.tick != shooter.shot_tick
            || hit.source != Some(actor)
            || hit.weapon != Some(weapon)
            || hit.damage == 0
            || other.hp != expected
            || hit.origin.distance(shot.origin) > 0.01
        {
            return Err(format!(
                "unexpected {weapon:?} damage; {old_hp} -> {}, hit {hit:?}, shot {shot:?}",
                other.hp
            ));
        }
        let direction = (hit.origin - other.body.eye()).normalize_or_zero();
        let toward_actor = (shooter.body.eye() - other.body.eye()).normalize_or_zero();
        if direction.dot(toward_actor) < 0.995 {
            return Err("damage cue points away from the actual attacker".into());
        }
        let endpoint = shot.origin + shot.direction * shot.distance;
        if (endpoint - other.body.pos).with_y(0.).length() > 0.45 {
            return Err(format!(
                "authoritative trace ended outside the hit player: {endpoint:?}"
            ));
        }
    } else if other.hp != old_hp {
        return Err("a blocked or out-of-reach attack changed the target's health".into());
    }
    Ok(shot)
}

fn line_up(game: &mut Squad, actor: u16, target: u16, separation: f32) -> Result<(), String> {
    // The drill arena has a clear center lane. Enter it across each spawn's back line.
    let mid_x = |v: Vec3| Vec3::new(0., 0., v.z);
    game.walk(&[
        (actor, mid_x(game.player(actor)?.body.pos)),
        (target, mid_x(game.player(target)?.body.pos)),
    ])?;
    game.walk(&[
        (actor, Vec3::Z * (separation * 0.5)),
        (target, -Vec3::Z * (separation * 0.5)),
    ])
}

fn respawn(
    game: &mut Squad,
    id: u16,
    old_life: u32,
    old_deaths: u16,
    selected: Weapon,
) -> Result<(), String> {
    if game.player(id)?.hp != 0 {
        return Err("expected an eliminated target".into());
    }
    game.until(
        "deathmatch respawn",
        4,
        |_, _| Ok(vec![]),
        |s| Ok(s.player(id)?.life != old_life),
    )?;
    let new = game.player(id)?;
    if new.deaths != old_deaths + 1
        || new.hp != 100
        || new.rifle.weapon != selected
        || new.rifle.ammo != selected.spec().magazine
        || new.rifle.switch_left != 0
        || new.last_hit.is_some()
    {
        return Err(format!(
            "respawn did not reset the inventory and incoming-hit cue: {new:?}"
        ));
    }
    Ok(())
}

// The two players are on opposite sides of cover for this timed trigger test.
fn hold_trigger(game: &mut Squad, actor: u16, target: u16, weapon: Weapon) -> Result<u8, String> {
    equip(game, actor, weapon)?;
    game.until(
        "ready to hold trigger",
        3,
        |_, _| Ok(vec![]),
        |s| Ok(s.player(actor)?.rifle.cooldown_left == 0),
    )?;
    let start = game.frame().tick;
    let magazine = game.player(actor)?.rifle.ammo;
    let target_health = game.player(target)?.hp;
    game.until(
        "48 server ticks of held fire",
        3,
        |s, _| {
            let point = aim(s.player(actor)?.body.pos, s.player(target)?.body.pos);
            Ok(vec![(
                actor,
                Input {
                    fire: true,
                    ..point
                },
            )])
        },
        |s| Ok(s.frame().tick - start >= 48),
    )?;
    let rounds = magazine
        .checked_sub(game.player(actor)?.rifle.ammo)
        .ok_or("invalid held-trigger ammunition")?;
    if game.player(target)?.hp != target_health {
        return Err("held fire penetrated solid cover".into());
    }
    // A confirmed release separates this burst from any future semi-auto shot.
    let released = game.frame().tick;
    game.until(
        "released trigger",
        2,
        |_, _| Ok(vec![]),
        |s| Ok(s.frame().tick > released + 4),
    )?;
    Ok(rounds)
}

pub fn verify(server: &Path) -> Result<(), String> {
    let (_server, addr) = start_server(
        server,
        &[
            "--mode",
            "deathmatch",
            "--map",
            "drill",
            "--max-players",
            "2",
        ],
    )?;
    let mut game = Squad::join(&addr.to_string(), 2)?;
    game.until(
        "two players admitted",
        2,
        |_, _| Ok(vec![]),
        |s| Ok(s.frame().players.len() == 2),
    )?;
    let actor = game.frame().players[0].id;
    let target = game.frame().players[1].id;
    let issued = game.player(actor)?.rifle;
    if Weapon::ALL.iter().any(|&w| !issued.owns(w)) {
        return Err("deathmatch did not issue every weapon".into());
    }
    line_up(&mut game, actor, target, 8.)?;
    // Equip the target too: its nondefault selection must survive death while rounds refill.
    equip(&mut game, target, Weapon::Smg)?;
    let before_target_shot = game.player(target)?.shot_tick;
    game.until(
        "target spends one SMG round",
        2,
        |_, tick| {
            Ok(vec![(
                target,
                Input {
                    yaw: std::f32::consts::FRAC_PI_2,
                    fire: tick == 0,
                    ..Input::default()
                },
            )])
        },
        |s| Ok(s.player(target)?.shot_tick > before_target_shot),
    )?;
    let first = *game.player(target)?;
    if first.rifle.ammo + 1 != Weapon::Smg.spec().magazine || game.player(actor)?.hp != 100 {
        return Err("target failed to spend exactly one shot into the side wall".into());
    }
    let mut tested_hits = Vec::new();
    for weapon in [Weapon::Pistol, Weapon::M4, Weapon::Ak] {
        equip(&mut game, actor, weapon)?;
        let shot = discharge(&mut game, actor, target, weapon, true)?;
        tested_hits.push(json!({"weapon": weapon, "distance": shot.distance, "victim_hp": game.player(target)?.hp}));
    }
    respawn(
        &mut game,
        target,
        first.life,
        first.deaths,
        first.rifle.weapon,
    )?;

    // Return to two partially spent magazines and reload only the pistol.
    let pistol_remaining = Weapon::Pistol.spec().magazine - 1;
    let ak_remaining = Weapon::Ak.spec().magazine - 1;
    equip(&mut game, actor, Weapon::Pistol)?;
    if game.player(actor)?.rifle.ammo != pistol_remaining {
        return Err("switching back refilled the pistol magazine".into());
    }
    game.until(
        "reload starts",
        2,
        |_, _| {
            Ok(vec![(
                actor,
                Input {
                    reload: true,
                    ..Input::default()
                },
            )])
        },
        |s| Ok(s.player(actor)?.rifle.reload_left > 0),
    )?;
    let reload_start = game.frame().tick;
    game.until(
        "reload finishes",
        5,
        |_, _| Ok(vec![]),
        |s| Ok(s.player(actor)?.rifle.reload_left == 0),
    )?;
    let reload_ticks = game.frame().tick - reload_start;
    let gear = game.player(actor)?.rifle;
    if gear.ammo != Weapon::Pistol.spec().magazine
        || gear.ammo_for(Weapon::Ak) != ak_remaining
        || gear.ammo_for(Weapon::M4) != Weapon::M4.spec().magazine - 1
    {
        return Err("reload or stowed magazine state is incorrect".into());
    }
    line_up(&mut game, actor, target, 8.)?;
    let victim = *game.player(target)?;
    for weapon in [Weapon::Smg, Weapon::Sniper] {
        equip(&mut game, actor, weapon)?;
        let shot = discharge(&mut game, actor, target, weapon, true)?;
        tested_hits.push(json!({"weapon": weapon, "distance": shot.distance, "victim_hp": game.player(target)?.hp}));
    }
    respawn(
        &mut game,
        target,
        victim.life,
        victim.deaths,
        victim.rifle.weapon,
    )?;

    line_up(&mut game, actor, target, 8.)?;
    equip(&mut game, actor, Weapon::Knife)?;
    let miss = discharge(&mut game, actor, target, Weapon::Knife, false)?;
    line_up(&mut game, actor, target, 1.25)?;
    let near = discharge(&mut game, actor, target, Weapon::Knife, true)?;
    tested_hits.push(json!({"weapon": Weapon::Knife, "distance": near.distance, "victim_hp": game.player(target)?.hp}));
    // A solid corner bisects this parallel lane. The endpoint must land on the corner.
    game.walk(&[(actor, Vec3::Z * 6.), (target, Vec3::ZERO)])?;
    game.walk(&[
        (actor, Vec3::new(2.6, 0., 6.)),
        (target, Vec3::new(2.6, 0., 0.)),
    ])?;
    equip(&mut game, actor, Weapon::M4)?;
    let blocked = discharge(&mut game, actor, target, Weapon::M4, false)?;
    let arena = sendit_sim::Arena::load("drill")?;
    let corner = arena.ray_distance(blocked.origin, blocked.direction, Weapon::M4.spec().range);
    if (blocked.distance - corner).abs() > 0.01 || blocked.distance >= 3.0 {
        return Err(format!(
            "wall trace did not stop at the drill corner: {blocked:?}"
        ));
    }
    let pistol_burst = hold_trigger(&mut game, actor, target, Weapon::Pistol)?;
    let smg_burst = hold_trigger(&mut game, actor, target, Weapon::Smg)?;
    if pistol_burst != 1 || smg_burst < 4 {
        return Err(format!(
            "unexpected held trigger counts: pistol {pistol_burst}, SMG {smg_burst}"
        ));
    }
    println!(
        "{}",
        json!({
            "test": "live_udp_arsenal",
            "passed": true,
            "map": "drill",
            "mode": "deathmatch",
            "hits": tested_hits,
            "knife_miss_endpoint": miss.distance,
            "pistol_reload_observed_ticks": reload_ticks,
            "held_fire_48_ticks": {"pistol": pistol_burst, "smg": smg_burst},
            "respawned_equipped": Weapon::Smg,
            "wall_impact_distance": blocked.distance,
            "stowed_ak_rounds": ak_remaining,
            "respawns": 2,
            "attacker_direction_verified": true
        })
    );
    Ok(())
}
