//! End-to-end rules checks. Movement, purchases, shots and objective actions
//! go through ordinary UDP clients on the shipping maps.
use crate::{
    net::{Frame, Impairment, Peer},
    scenario::{aim, connect, pace},
    start_server,
};
use sendit_sim::{BombView, Input, Phase, PlayerView, Vec3};
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

type Inputs = Vec<(u16, Input)>;

struct Squad {
    peers: Vec<Peer>,
    clock: Instant,
}

impl Squad {
    fn join(addr: &str, count: usize) -> Result<Self, String> {
        Ok(Self {
            peers: connect(addr, "", Impairment::default(), count, None)?,
            clock: Instant::now(),
        })
    }

    fn frame(&self) -> &Frame {
        // connect() returns only after every slot has received a player snapshot.
        self.peers[0].frame().unwrap()
    }

    fn player(&self, id: u16) -> Result<&PlayerView, String> {
        self.frame()
            .player(id)
            .ok_or_else(|| format!("missing player {id}"))
    }

    fn tick(&mut self, inputs: &[(u16, Input)]) -> Result<(), String> {
        for p in &mut self.peers {
            p.poll(None)?;
            let input = inputs
                .iter()
                .find(|(id, _)| Some(*id) == p.id)
                .map_or_else(Input::default, |(_, v)| *v);
            p.input(input, None)?;
            p.poll(None)?;
        }
        pace(&mut self.clock);
        for p in &mut self.peers {
            p.poll(None)?;
        }
        Ok(())
    }

    fn until(
        &mut self,
        description: &str,
        seconds: u64,
        inputs: impl Fn(&Squad, u32) -> Result<Inputs, String>,
        done: impl Fn(&Squad) -> Result<bool, String>,
    ) -> Result<(), String> {
        let start = Instant::now();
        let mut tick = 0;
        while start.elapsed() < Duration::from_secs(seconds) {
            if done(self)? {
                return Ok(());
            }
            let commands = inputs(self, tick)?;
            self.tick(&commands)?;
            tick += 1;
        }
        let round = self.frame().round;
        let positions: Vec<_> = self
            .frame()
            .players
            .iter()
            .map(|p| (p.id, p.body.pos, p.hp))
            .collect();
        Err(format!(
            "{description} timed out: {round:?}; players {positions:?}"
        ))
    }

    fn shoot_until_dead(&mut self, shooter: u16, target: u16) -> Result<(), String> {
        self.until(
            "elimination by rifle fire",
            6,
            |s, tick| {
                let shot = aim(s.player(shooter)?.body.pos, s.player(target)?.body.pos);
                Ok(vec![(
                    shooter,
                    Input {
                        fire: tick % 26 < 3,
                        ..shot
                    },
                )])
            },
            |s| Ok(s.player(target)?.hp == 0),
        )
    }

    fn walk(&mut self, goals: &[(u16, Vec3)]) -> Result<(), String> {
        self.until(
            "moving to relay waypoint",
            9,
            |s, _| {
                goals
                    .iter()
                    .map(|&(id, goal)| {
                        let body = s.player(id)?.body;
                        let delta = (goal - body.pos).with_y(0.);
                        // Slow down early and counteract measured velocity on approach.
                        let steer =
                            ((delta * 3.5 - body.vel.with_y(0.) * 0.35) / 5.).clamp_length_max(1.);
                        let input = Input {
                            strafe: steer.x,
                            forward: -steer.z,
                            ..Input::default()
                        };
                        Ok((id, input))
                    })
                    .collect()
            },
            |s| {
                goals.iter().try_fold(true, |ready, &(id, goal)| {
                    let body = s.player(id)?.body;
                    Ok(ready
                        && (body.pos - goal).with_y(0.).length() < 0.18
                        && body.vel.length() < 0.35)
                })
            },
        )
    }
}

fn competitive(addr: &str) -> Result<(), String> {
    let mut game = Squad::join(addr, 10)?;
    let idle = |_: &Squad, _: u32| Ok(vec![]);
    game.until("ten players in buy time", 4, idle, |s| {
        Ok(s.frame().players.len() == 10 && s.frame().round.phase == Phase::Buy)
    })?;
    let roster = &game.frame().players;
    if roster.iter().filter(|p| p.team == 0 && p.hp > 0).count() != 5
        || roster.iter().filter(|p| p.team == 1 && p.hp > 0).count() != 5
    {
        return Err(
            "competitive host did not admit five living players on each side during buy".into(),
        );
    }
    let BombView::Carried(carrier) = game.frame().round.bomb else {
        return Err("buy time did not assign the bomb".into());
    };
    let attacker = roster
        .iter()
        .find(|p| p.team == 0 && p.id != carrier)
        .unwrap()
        .id;
    let defender = roster.iter().find(|p| p.team == 1).unwrap().id;
    let observer = roster
        .iter()
        .find(|p| p.team == 1 && p.id != defender)
        .unwrap()
        .id;
    let starting_pos = game.player(carrier)?.body.pos;
    let start_money = game.player(defender)?.money;
    let start_ammo = game.player(defender)?.rifle.ammo;
    for i in 0..36 {
        game.tick(&[
            (
                carrier,
                Input {
                    forward: 1.,
                    ..Input::default()
                },
            ),
            (
                defender,
                Input {
                    buy: i < 6,
                    ..Input::default()
                },
            ),
        ])?;
    }
    let purchased = game.player(defender)?;
    if purchased.money >= start_money || purchased.rifle.ammo <= start_ammo || purchased.armor != 50
    {
        return Err(format!(
            "buy did not charge and equip kit: money {}, ammunition {}, armor {}",
            purchased.money, purchased.rifle.ammo, purchased.armor
        ));
    }
    if game.player(carrier)?.body.pos.distance(starting_pos) > 0.02 {
        return Err("attacker moved during freeze time".into());
    }
    game.until("round live", 14, idle, |s| {
        Ok(s.frame().round.phase == Phase::Live)
    })?;

    // Teammate receives the dropped bomb, then four attacking volunteers leave.
    // This preserves a genuine last-attacker elimination without any server cheats.
    game.walk(&[(attacker, starting_pos + Vec3::X * 0.85)])?;
    game.until(
        "bomb dropped",
        2,
        |_, _| {
            Ok(vec![(
                carrier,
                Input {
                    drop_bomb: true,
                    ..Input::default()
                },
            )])
        },
        |s| Ok(matches!(s.frame().round.bomb, BombView::Dropped(_))),
    )?;
    game.until(
        "teammate recovered bomb",
        3,
        |_, _| {
            Ok(vec![(
                attacker,
                Input {
                    interact: true,
                    ..Input::default()
                },
            )])
        },
        |s| Ok(s.frame().round.bomb == BombView::Carried(attacker)),
    )?;
    let leaving: Vec<_> = game
        .frame()
        .players
        .iter()
        .filter(|p| p.team == 0 && p.id != attacker)
        .map(|p| p.id)
        .collect();
    game.peers.retain(|p| !leaving.contains(&p.id.unwrap()));
    game.until("one remaining attacker", 3, idle, |s| {
        Ok(s.frame().players.len() == 6)
    })?;

    // Original Relay west approach, followed by a CT hold overlooking site A.
    game.walk(&[
        (attacker, Vec3::new(-14., 0., 17.)),
        (defender, Vec3::new(-14., 0., -16.)),
        (observer, Vec3::new(-13.2, 0., -16.)),
    ])?;
    game.walk(&[
        (attacker, Vec3::new(-14., 0., -5.5)),
        (defender, Vec3::new(-14., 0., -6.8)),
        (observer, Vec3::new(-13.2, 0., -5.5)),
    ])?;
    game.shoot_until_dead(attacker, observer)?;
    let observer_death = game.frame().tick;
    let dead_pos = game.player(observer)?.body.pos;
    game.until(
        "plant at A",
        6,
        |_, _| {
            Ok(vec![(
                attacker,
                Input {
                    interact: true,
                    ..Input::default()
                },
            )])
        },
        |s| {
            Ok(s.frame().round.phase == Phase::Planted
                && matches!(s.frame().round.bomb, BombView::Planted(0, _)))
        },
    )?;
    let t_life = game.player(attacker)?.life;
    let ct_score = game.frame().round.score[1];
    game.shoot_until_dead(defender, attacker)?;
    for _ in 0..150 {
        game.tick(&[(
            observer,
            Input {
                forward: 1.,
                fire: true,
                ..Input::default()
            },
        )])?;
        if game.frame().round.phase != Phase::Planted
            || game.player(attacker)?.hp != 0
            || game.player(attacker)?.life != t_life
        {
            return Err(
                "last attacker elimination ended the planted round or allowed a respawn".into(),
            );
        }
    }
    let spectator = game.peers.iter().find(|p| p.id == Some(observer)).unwrap();
    let ally = spectator
        .frame()?
        .player(defender)
        .ok_or("spectator lost its living teammate")?;
    if spectator.own()?.hp != 0
        || spectator.own()?.body.pos.distance(dead_pos) > 0.02
        || ally.team != spectator.own()?.team
        || ally.hp == 0
        || ally.shot_tick <= observer_death
    {
        return Err(
            "dead CT moved, respawned, or failed to receive its live teammate's actions".into(),
        );
    }
    game.until(
        "defuse and CT score",
        8,
        |_, _| {
            Ok(vec![(
                defender,
                Input {
                    interact: true,
                    ..Input::default()
                },
            )])
        },
        |s| Ok(s.frame().round.phase == Phase::Ended && s.frame().round.score[1] == ct_score + 1),
    )?;
    println!(
        "{}",
        json!({"test":"relay_competitive","passed":true,"initial_team_sizes":[5,5],"site":"A",
        "buy_charged":true,"freeze_verified":true,"bomb_transferred":true,"last_attacker_dead_during_defuse":true,
        "score_after_defuse":game.frame().round.score,"dead_ct_received_live_teammate_shot":true})
    );
    Ok(())
}

fn deathmatch(addr: &str, armored: bool) -> Result<(), String> {
    let mut game = Squad::join(addr, if armored { 2 } else { 1 })?;
    let shooter = game.peers[0].id.unwrap();
    let expect = if armored { (30, 50) } else { (12, 0) };
    let first = game.player(shooter)?;
    if (first.rifle.ammo, first.armor) != expect || first.rifle.magazine != expect.0 {
        return Err(format!(
            "wrong deathmatch spawn loadout: {:?}",
            (first.rifle.ammo, first.rifle.magazine, first.armor)
        ));
    }
    if !armored {
        println!(
            "{}",
            json!({"test":"deathmatch_light","passed":true,"ammo":first.rifle.ammo,"armor":first.armor})
        );
        return Ok(());
    }
    let target = game.peers[1].id.unwrap();
    game.until(
        "second DM slot visible",
        2,
        |_, _| Ok(vec![]),
        |s| Ok(s.frame().player(target).is_some()),
    )?;
    let life = game.player(target)?.life;
    game.shoot_until_dead(shooter, target)?;
    let death_tick = game.frame().tick;
    game.until(
        "deathmatch respawn",
        3,
        |_, _| Ok(vec![]),
        |s| {
            let p = s.player(target)?;
            Ok(p.life > life && p.hp == 100 && (p.rifle.ammo, p.armor) == expect)
        },
    )?;
    let respawn_ticks = game.frame().tick - death_tick;
    if !(120..=136).contains(&respawn_ticks) {
        return Err(format!(
            "deathmatch respawn took {respawn_ticks} ticks between 32 Hz snapshots"
        ));
    }
    println!(
        "{}",
        json!({"test":"deathmatch_armored","passed":true,"restored_ammo":expect.0,"restored_armor":expect.1,
        "snapshot_ticks_between_death_and_respawn":respawn_ticks})
    );
    Ok(())
}

pub fn verify(server: &Path) -> Result<(), String> {
    {
        let (_host, addr) = start_server(server, &["--map", "relay", "--mode", "competitive"])?;
        eprintln!("Testing competitive 5v5 and post-plant defuse on {addr}");
        competitive(&addr.to_string())?;
    }
    for kit in ["light", "armored"] {
        let (_host, addr) = start_server(
            server,
            &["--map", "drill", "--mode", "deathmatch", "--loadout", kit],
        )?;
        deathmatch(&addr.to_string(), kit == "armored")?;
    }
    Ok(())
}
