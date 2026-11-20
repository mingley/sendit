//! Normal operator and client paths against short-lived local hosts and an actual directory.
use crate::{
    net::{Impairment, Peer},
    scenario::{connect, pace},
    start_server, start_service,
};
use sendit_sim::{
    map::MAX_MAP_BYTES,
    wire::{decode, encode, ClientMsg, ServerMsg, PROTOCOL},
    Arena, Input,
};
use serde_json::json;
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CUSTOM: &str = "probe_quay";
const PASSWORD: &str = "local-only-game-test";

struct MapDir(PathBuf);

impl MapDir {
    fn create() -> Result<(Self, Arena), String> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("sendit-map-probe-{}-{unique}", std::process::id()));
        fs::create_dir(&path).map_err(|e| e.to_string())?;
        let owner = Self(path);
        let mut map = Arena::load("drill")?;
        map.name = CUSTOM.into();
        // Make the downloaded map distinguishable from its built-in template.
        map.spawns[0].pos.x += 0.75;
        let bytes = serde_json::to_vec_pretty(&map).map_err(|e| e.to_string())?;
        fs::write(owner.0.join(format!("{CUSTOM}.json")), bytes).map_err(|e| e.to_string())?;
        Ok((owner, map))
    }
}

impl Drop for MapDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cli(server: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(server)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "{} exited {}: {}",
            args[0],
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// A real peer can receive its return-address challenge and abandon the join.
fn unconfirmed_join(addr: &str) -> Result<UdpSocket, String> {
    let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    socket.connect(addr).map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    socket
        .send(&encode(&ClientMsg::Join {
            version: PROTOCOL,
            name: "PendingProbe".into(),
            password: PASSWORD.into(),
        })?)
        .map_err(|e| e.to_string())?;
    let mut bytes = [0; 4096];
    let n = socket.recv(&mut bytes).map_err(|e| e.to_string())?;
    if !matches!(
        decode::<ServerMsg>(&bytes[..n])?,
        ServerMsg::Challenge { .. }
    ) {
        return Err("unconfirmed join received game state before its address challenge".into());
    }
    socket.set_nonblocking(true).map_err(|e| e.to_string())?;
    Ok(socket)
}

fn check_pending(socket: &UdpSocket) -> Result<(), String> {
    let mut bytes = [0; 65_535];
    loop {
        match socket.recv(&mut bytes) {
            Ok(n) => match decode::<ServerMsg>(&bytes[..n])? {
                ServerMsg::Challenge { .. } | ServerMsg::Rejected(_) => {}
                _ => {
                    return Err(
                        "an unconfirmed connection received an active session message".into(),
                    )
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn denied(addr: &str, password: &str, reason: &str) -> Result<(), String> {
    match connect(addr, password, Impairment::default(), 1, None) {
        Err(e) if e.to_lowercase().contains(reason) => Ok(()),
        Err(e) => Err(format!("expected rejection for {reason}, got {e}")),
        Ok(_) => Err(format!(
            "server accepted a join that should be rejected for {reason}"
        )),
    }
}

fn wait_peer(
    p: &mut Peer,
    condition: impl Fn(&Peer) -> bool,
    description: &str,
    seconds: u64,
) -> Result<(), String> {
    let start = Instant::now();
    let mut clock = start;
    while start.elapsed() < Duration::from_secs(seconds) {
        p.poll(None)?;
        if condition(p) {
            return Ok(());
        }
        p.input(Input::default(), None)?;
        p.poll(None)?;
        pace(&mut clock);
    }
    Err(format!(
        "{description} timed out (map {}, last sequence {})",
        p.map, p.seq
    ))
}

fn transferred_map(p: &mut Peer) -> Result<(Arena, u8), String> {
    let limit = Instant::now() + Duration::from_secs(5);
    let mut assembled = Vec::new();
    let mut expected = None;
    let mut index = 0;
    let mut retry = Instant::now();
    while Instant::now() < limit {
        let now = Instant::now();
        if now >= retry {
            p.send(ClientMsg::MapRequest {
                token: p.token,
                part: index,
            })?;
            retry = now + Duration::from_millis(80);
        }
        p.poll(None)?;
        if let Some(part) = p.map_part.take() {
            if part.index != index {
                continue;
            }
            if part.count == 0
                || index >= part.count
                || expected.is_some_and(|n| n != part.count)
                || part.bytes.is_empty()
                || part.bytes.len() > 900
                || (index + 1 < part.count && part.bytes.len() != 900)
            {
                return Err("server sent an inconsistent custom map chunk".into());
            }
            expected = Some(part.count);
            assembled.extend_from_slice(&part.bytes);
            if assembled.len() > MAX_MAP_BYTES {
                return Err("transferred map exceeds map format limit".into());
            }
            if index + 1 == part.count {
                return Ok((Arena::from_json(&assembled)?, part.count));
            }
            index += 1;
            retry = now;
        }
        thread::sleep(Duration::from_millis(2));
    }
    Err(format!("timed out obtaining map chunk {index}"))
}

fn rotated(
    peer: &mut Peer,
    map: &str,
    old_token: u64,
    min_tick: u32,
    seconds: u64,
) -> Result<(), String> {
    wait_peer(
        peer,
        |p| {
            p.token != old_token
                && p.map == map
                && p.frame().is_ok_and(|f| f.tick > min_tick)
                && p.own().is_ok()
        },
        "fresh map session",
        seconds,
    )
}

pub fn verify(server: &Path) -> Result<(), String> {
    let (maps, template) = MapDir::create()?;
    let mut directory = Command::new(server);
    directory.args(["directory", "--bind", "127.0.0.1:0"]);
    let (directory, directory_addr) = start_service(directory, "SENDIT_DIRECTORY_READY ")?;
    let dir = directory_addr.to_string();
    let (_private, private_addr) =
        start_server(server, &["--name", "PrivateFixture", "--mode", "practice"])?;
    let mut command = Command::new(server);
    command
        .env("SENDIT_MAP_DIR", &maps.0)
        .env("SENDIT_PROBE_PASSWORD", PASSWORD)
        .args([
            "--bind",
            "0.0.0.0:0",
            "--name",
            "HarborFixture",
            "--mode",
            "practice",
            "--map",
            CUSTOM,
            "--rotation",
            "probe_quay,drill",
            "--max-players",
            "2",
            "--password-env",
            "SENDIT_PROBE_PASSWORD",
            "--public-list",
            &dir,
        ]);
    let (mut host, listen) = start_service(command, "SENDIT_READY ")?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, listen.port())).to_string();
    denied(&addr, "wrong-local-test-password", "password")?;
    let pending = unconfirmed_join(&addr)?;
    let mut peers = connect(&addr, PASSWORD, Impairment::default(), 2, None)?;
    denied(&addr, PASSWORD, "full")?;
    let mut second = peers.pop().unwrap();
    let mut first = peers.pop().unwrap();
    let (map, parts) = transferred_map(&mut first)?;
    check_pending(&pending)?;
    if parts < 2
        || serde_json::to_value(&map).map_err(|e| e.to_string())?
            != serde_json::to_value(&template).map_err(|e| e.to_string())?
    {
        return Err("download did not reproduce the separately installed map".into());
    }
    if first.own()?.body.pos.distance(template.spawns[0].pos) > 0.05 {
        return Err("host did not spawn on the custom geometry".into());
    }

    let broadcast =
        SocketAddr::from((Ipv4Addr::new(127, 255, 255, 255), listen.port())).to_string();
    let discovered = cli(server, &["discover", "--target", &broadcast])?;
    if !discovered.contains("HarborFixture") || !discovered.contains("locked=true") {
        return Err(format!(
            "LAN broadcast did not find the protected server: {discovered}"
        ));
    }
    let stop = Instant::now() + Duration::from_secs(12);
    loop {
        let listed = cli(server, &["list", "--directory", &dir])?;
        if listed.contains("PrivateFixture") || listed.contains(&format!("addr={private_addr} ")) {
            return Err("directory listed a host that never opted in".into());
        }
        if listed.contains("HarborFixture") && listed.contains(&addr) {
            break;
        }
        if Instant::now() >= stop {
            return Err(format!("opt-in registration never appeared: {listed}"));
        }
        thread::sleep(Duration::from_millis(100));
    }
    host.admin("status")?;
    let _status = host.wait_line(
        |line| line.to_lowercase().contains("status") && line.contains(CUSTOM),
        3,
    )?;
    let kicked = second.id.unwrap();
    host.admin(&format!("kick {kicked}"))?;
    wait_peer(
        &mut first,
        |p| p.frame().is_ok_and(|f| f.player(kicked).is_none()),
        "admin kick",
        3,
    )?;
    // The kicked peer can receive a final rejection before its socket closes.
    let _ = second.poll(None);
    drop(second);
    drop(directory);
    // A fresh player uses the direct address after the directory process has exited.
    let direct = connect(&addr, PASSWORD, Impairment::default(), 1, None)?;
    drop(direct);
    let old_token = first.token;
    let old_tick = first.frame()?.tick;
    host.admin("next")?;
    rotated(&mut first, "drill", old_token, old_tick, 4)?;
    let old_token = first.token;
    let old_tick = first.frame()?.tick;
    host.admin(&format!("map {CUSTOM}"))?;
    rotated(&mut first, CUSTOM, old_token, old_tick, 4)?;
    let old_token = first.token;
    let old_tick = first.frame()?.tick;
    host.admin("restart")?;
    rotated(&mut first, CUSTOM, old_token, old_tick, 4)?;
    host.admin("quit")?;
    let quit = Instant::now();
    loop {
        if let Some(status) = host.process.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                return Err(format!("operator quit exited {status}"));
            }
            break;
        }
        if quit.elapsed() > Duration::from_secs(3) {
            return Err("operator quit failed to stop host".into());
        }
        thread::sleep(Duration::from_millis(10));
    }

    let (_timed, timed_addr) = start_server(
        server,
        &[
            "--mode",
            "deathmatch",
            "--map",
            "drill",
            "--rotation",
            "drill,relay",
            "--rotate-seconds",
            "1",
        ],
    )?;
    let mut timed = connect(&timed_addr.to_string(), "", Impairment::default(), 1, None)?
        .pop()
        .unwrap();
    let old_token = timed.token;
    let old_tick = timed.frame()?.tick;
    rotated(&mut timed, "relay", old_token, old_tick, 5)?;
    println!(
        "{}",
        json!({"test":"community_hosting","passed":true,"password_and_capacity":true,
        "unconfirmed_join_reserved_no_slot_or_state":true,"lan_loopback_broadcast":true,"opt_in_directory":true,"private_host_absent":true,
        "direct_join_after_directory_shutdown":true,"custom_map_chunks":parts,
        "admin":["status","kick","next","map","restart","quit"],"automatic_rotation":"drill to relay"})
    );
    Ok(())
}
