//! Black-box bot checks. An ordinary network player observes the same snapshots and
//! objective transitions as a desktop client. No console edits to match state.
use crate::{
    hosting::cli,
    net::{Impairment, Peer},
    rules::{arrived, steer},
    scenario::{connect, pace},
    start_server,
};
use sendit_sim::{wire::FIRST_BOT_ID, BombView, Input, Phase, PlayerView, Vec3};
use serde_json::json;
use std::{
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};

fn join(addr: SocketAddr, n: usize) -> Result<Vec<Peer>, String> {
    connect(&addr.to_string(), "", Impairment::default(), n, None)
}

fn one(addr: SocketAddr) -> Result<Peer, String> {
    Ok(join(addr, 1)?.pop().unwrap())
}

fn watch(
    p: &mut Peer,
    seconds: u64,
    action: impl Fn(&Peer) -> Input,
    mut done: impl FnMut(&Peer) -> Result<bool, String>,
) -> Result<(), String> {
    let start = Instant::now();
    let mut next = start;
    while start.elapsed() < Duration::from_secs(seconds) {
        p.poll(None)?;
        if done(p)? {
            return Ok(());
        }
        p.input(action(p), None)?;
        p.poll(None)?;
        pace(&mut next);
    }
    let f = p.frame()?;
    let actors: Vec<_> = f
        .players
        .iter()
        .map(|p| (p.id, p.team, p.hp, p.body.pos, p.shot_tick))
        .collect();
    Err(format!(
        "timed out on {} at tick {}: {:?}, actors {actors:?}",
        p.map, f.tick, f.round
    ))
}

fn idle(_: &Peer) -> Input {
    Input::default()
}

fn bot(p: &Peer) -> Result<&PlayerView, String> {
    p.frame()?
        .players
        .iter()
        .find(|v| v.id >= FIRST_BOT_ID)
        .ok_or_else(|| "expected a bot in the snapshot".into())
}

fn fill(server: &Path) -> Result<(), String> {
    let (_s, addr) = start_server(
        server,
        &[
            "--map",
            "aim",
            "--mode",
            "deathmatch",
            "--max-players",
            "4",
            "--fill",
            "4",
            "--bot-skill",
            "0.65",
        ],
    )?;
    let roster = cli(server, &["discover", "--target", &addr.to_string()])?;
    if !roster.contains("players=4/4") {
        return Err(format!(
            "unattended fill was absent from discovery: {roster}"
        ));
    }
    let mut peers = join(addr, 4)?;
    let mut spectator = peers.remove(0);
    watch(&mut spectator, 3, idle, |p| {
        let players = &p.frame()?.players;
        Ok(players.len() == 4
            && players.iter().all(|v| v.id < FIRST_BOT_ID)
            && p.roster.len() == 4
            && p.roster.iter().all(|(id, _)| *id < FIRST_BOT_ID))
    })?;
    drop(peers);
    watch(&mut spectator, 3, idle, |p| {
        let f = p.frame()?;
        Ok(f.players.len() == 4
            && f.players.iter().filter(|v| v.id >= FIRST_BOT_ID).count() == 3
            && p.roster
                .iter()
                .filter(|(id, name)| *id >= FIRST_BOT_ID && name.starts_with("Bot "))
                .count()
                == 3)
    })?;
    let first = bot(&spectator)?.id;
    let origin = bot(&spectator)?.body.pos;
    let start_tick = spectator.frame()?.tick;
    watch(&mut spectator, 5, idle, |p| {
        let f = p.frame()?;
        Ok(f.player(first)
            .is_some_and(|v| v.body.pos.distance(origin) > 0.45)
            && f.players
                .iter()
                .any(|v| v.id >= FIRST_BOT_ID && v.shot_tick > start_tick))
    })?;
    println!(
        "{}",
        json!({"test":"aim_deathmatch_fill","passed":true,"initial_bots":4,"joined_humans":4,"after_three_leaves":{"bots":3,"humans":1},"bot_motion_and_shot":true})
    );
    Ok(())
}

fn reaction(server: &Path, level: &str) -> Result<(u32, f32), String> {
    let (_s, addr) = start_server(
        server,
        &[
            "--map",
            "drill",
            "--mode",
            "deathmatch",
            "--max-players",
            "2",
            "--bots",
            "1",
            "--bot-team",
            "t",
            "--bot-skill",
            level,
        ],
    )?;
    let mut peer = one(addr)?;
    let start = peer.frame()?.tick;
    watch(&mut peer, 6, idle, |p| Ok(bot(p)?.shot_tick > start))?;
    let v = bot(&peer)?;
    if v.team != 0 || peer.own()?.team != 1 {
        return Err("fixed bot did not take the requested attacking side".into());
    }
    let shot = v.shot.ok_or("missing observed first shot")?;
    let toward = (peer.own()?.body.pos + Vec3::Y * 1.15 - shot.origin).normalize();
    let error = toward
        .dot(shot.direction)
        .clamp(-1., 1.)
        .acos()
        .to_degrees();
    Ok((v.shot_tick - start, error))
}

fn skill(server: &Path) -> Result<(), String> {
    let (low, low_error) = reaction(server, "0")?;
    let (high, high_error) = reaction(server, "1")?;
    if low < high + 12 {
        return Err(format!("visible-target first-shot timings failed to distinguish skills: low {low}, high {high} ticks"));
    }
    println!(
        "{}",
        json!({"test":"drill_bot_skill","passed":true,"fixed_team":"T","skill_0_first_shot_ticks":low,
        "skill_1_first_shot_ticks":high,"first_shot_torso_center_error_deg":[low_error,high_error]})
    );
    Ok(())
}

fn duel(server: &Path) -> Result<(), String> {
    let (_s, addr) = start_server(
        server,
        &[
            "--map",
            "aim",
            "--mode",
            "competitive",
            "--bots",
            "1",
            "--bot-team",
            "ct",
            "--bot-skill",
            "1",
        ],
    )?;
    let mut peer = one(addr)?;
    watch(&mut peer, 26, idle, |p| {
        let f = p.frame()?;
        let b = bot(p)?;
        Ok(b.team == 1
            && f.round.phase == Phase::Ended
            && f.round.score[1] == 1
            && b.kills > 0
            && p.own()?.hp == 0)
    })?;
    println!(
        "{}",
        json!({"test":"aim_competitive_duel","passed":true,"winning_side":"CT","score":peer.frame()?.round.score})
    );
    Ok(())
}

fn plant(server: &Path) -> Result<(), String> {
    let (_s, addr) = start_server(
        server,
        &[
            "--map",
            "relay",
            "--mode",
            "competitive",
            "--bots",
            "1",
            "--bot-team",
            "t",
            "--bot-skill",
            "0.8",
        ],
    )?;
    let mut peer = one(addr)?;
    let mut in_progress = false;
    watch(&mut peer, 48, idle, |p| {
        let f = p.frame()?;
        let b = bot(p)?;
        if f.round.phase == Phase::Ended {
            return Err(format!(
                "attacking bot ended round before plant: {:?}, T {:?}, CT {:?}",
                f.round,
                b.body.pos,
                p.own()?.body.pos
            ));
        }
        in_progress |= f.round.action_by == Some(b.id) && f.round.progress > 0.;
        Ok(f.round.phase == Phase::Planted
            && matches!(f.round.bomb, BombView::Planted(_, _))
            && in_progress
            && b.team == 0)
    })?;
    let BombView::Planted(site, _) = peer.frame()?.round.bomb else {
        unreachable!()
    };
    println!(
        "{}",
        json!({"test":"relay_bot_plant","passed":true,"map":"relay","site_index":site,"plant_tick":peer.frame()?.tick})
    );
    Ok(())
}

fn defuse(server: &Path) -> Result<(), String> {
    let (_s, addr) = start_server(
        server,
        &[
            "--map",
            "relay",
            "--mode",
            "competitive",
            "--bots",
            "1",
            "--bot-team",
            "ct",
            "--bot-skill",
            "0.8",
        ],
    )?;
    let mut peer = one(addr)?;
    watch(&mut peer, 15, idle, |p| {
        Ok(p.frame()?.round.phase == Phase::Live)
    })?;
    for goal in [Vec3::new(14., 0., 17.), Vec3::new(14., 0., -5.5)] {
        watch(
            &mut peer,
            9,
            |p| {
                p.own()
                    .map_or_else(|_| Input::default(), |me| steer(me.body, goal))
            },
            |p| {
                if p.own()?.hp == 0 {
                    return Err("player's approach to B ended before the plant".into());
                }
                Ok(arrived(p.own()?.body, goal))
            },
        )?;
    }
    watch(
        &mut peer,
        21,
        |_| Input {
            interact: true,
            ..Input::default()
        },
        |p| {
            let r = p.frame()?.round;
            if r.phase == Phase::Ended {
                return Err("retake round ended before the player planted".into());
            }
            Ok(r.phase == Phase::Planted)
        },
    )?;
    let planted = peer.frame()?.tick;
    let mut in_progress = false;
    watch(&mut peer, 38, idle, |p| {
        let f = p.frame()?;
        let b = bot(p)?;
        in_progress |= f.round.phase == Phase::Planted
            && f.round.action_by == Some(b.id)
            && f.round.progress > 0.;
        if f.round.phase == Phase::Ended && f.round.score[1] != 1 {
            return Err(format!(
                "defender failed to disarm in time: CT {:?}, objective {:?}",
                b.body.pos, f.round.bomb
            ));
        }
        Ok(b.team == 1 && in_progress && f.round.phase == Phase::Ended && f.round.score[1] == 1)
    })?;
    println!(
        "{}",
        json!({"test":"relay_bot_defuse","passed":true,"map":"relay","site":"B","plant_to_defuse_ticks":peer.frame()?.tick-planted,"defender_score":1})
    );
    Ok(())
}

type Case = (&'static str, fn(&Path) -> Result<(), String>);

pub fn verify(server: &Path, only: Option<&str>) -> Result<(), String> {
    let cases: [Case; 5] = [
        ("fill", fill),
        ("skill", skill),
        ("duel", duel),
        ("plant", plant),
        ("defuse", defuse),
    ];
    if only.is_some_and(|name| !cases.iter().any(|(label, _)| *label == name)) {
        return Err("unknown bot verification case".into());
    }
    let mut failures = Vec::new();
    for (name, run) in cases {
        if only.is_none_or(|value| value == name) {
            eprintln!("Testing {name} with live bots");
            if let Err(e) = run(server) {
                eprintln!("bot test {name}: {e}");
                failures.push(name);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("bot verification failed: {}", failures.join(", ")))
    }
}
