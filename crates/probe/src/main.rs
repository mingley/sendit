mod arsenal;
mod bots;
mod hosting;
mod net;
mod rules;
mod scenario;

use net::Impairment;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const HELP: &str = "Sendit UDP probe

  sendit-probe verify [--server PATH]
  sendit-probe verify-rules [--server PATH]
  sendit-probe verify-arsenal [--server PATH]
  sendit-probe verify-hosting [--server PATH]
  sendit-probe verify-bots [--server PATH] [--case fill|skill|duel|plant|defuse]
  sendit-probe smoke [--connect HOST:PORT] [--latency-ms N] [--jitter-ms N] [--loss-pct N] [--seed N]
  sendit-probe load [--connect HOST:PORT] [--players N] [--seconds N] [--replay FILE]

verify launches two fresh drill servers, testing both LAN conditions and a seeded impaired link.
verify-rules tests competitive bomb play and deathmatch loadouts on separate local servers.
verify-arsenal uses two ordinary clients to check weapon changes, firing, hits, reload and knife range.
verify-hosting starts a local directory and community hosts to check operator and player workflows.
verify-bots exercises fill, skill and bomb objectives over UDP.
smoke expects an empty practice server on the drill map. Latency and jitter apply in each direction.
load occupies a live server. Without an input trace it uses a synthetic movement and firing pattern.
Use --password-env NAME to join a protected smoke or load server.";

struct Options {
    action: String,
    case: Option<String>,
    addr: String,
    server: PathBuf,
    net: Impairment,
    players: usize,
    seconds: u32,
    replay: Option<PathBuf>,
    password: String,
}

impl Options {
    fn read() -> Result<Self, String> {
        let mut args = env::args().skip(1);
        let action = args.next().unwrap_or_else(|| "help".into());
        let sibling = env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name(format!("sendit-server{}", env::consts::EXE_SUFFIX));
        let mut o = Self {
            action,
            case: None,
            addr: "127.0.0.1:27015".into(),
            server: sibling,
            net: Impairment::default(),
            players: 10,
            seconds: 10,
            replay: None,
            password: String::new(),
        };
        while let Some(key) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {key}"))?;
            match key.as_str() {
                "--connect" => o.addr = value,
                "--server" => o.server = value.into(),
                "--case" => o.case = Some(value),
                "--latency-ms" => {
                    o.net.latency_ms = value.parse().map_err(|_| "invalid latency")?
                }
                "--jitter-ms" => o.net.jitter_ms = value.parse().map_err(|_| "invalid jitter")?,
                "--loss-pct" => {
                    o.net.loss_pct = value.parse().map_err(|_| "invalid loss percentage")?
                }
                "--seed" => o.net.seed = value.parse().map_err(|_| "invalid seed")?,
                "--players" => o.players = value.parse().map_err(|_| "invalid player count")?,
                "--seconds" => o.seconds = value.parse().map_err(|_| "invalid duration")?,
                "--replay" => o.replay = Some(value.into()),
                "--password-env" => {
                    o.password = env::var(&value).map_err(|_| format!("{value} is unset"))?
                }
                _ => return Err(format!("unknown option {key}")),
            }
        }
        if o.players == 0 || o.players > 32 || o.seconds == 0 || o.seconds > 86_400 {
            return Err("players must be 1–32 and seconds 1–86400".into());
        }
        if o.net.latency_ms > 2_000
            || o.net.jitter_ms > 2_000
            || !(0.0..100.0).contains(&o.net.loss_pct)
        {
            return Err("latency and jitter must be 0–2000 ms, loss must be in [0,100)".into());
        }
        Ok(o)
    }
}

struct Server {
    process: Child,
    lines: mpsc::Receiver<String>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Server {
    fn admin(&mut self, command: &str) -> Result<(), String> {
        let input = self.process.stdin.as_mut().ok_or("server stdin closed")?;
        writeln!(input, "{command}")
            .and_then(|()| input.flush())
            .map_err(|e| e.to_string())
    }

    fn wait_line(&self, matches: impl Fn(&str) -> bool, seconds: u64) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let line = self
                .lines
                .recv_timeout(remaining)
                .map_err(|_| "server exited or timed out waiting for output")?;
            if matches(&line) {
                return Ok(line);
            }
        }
        Err("no matching line from server".into())
    }
}

fn start_service(mut command: Command, prefix: &str) -> Result<(Server, SocketAddr), String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("launch {}: {e}", command.get_program().to_string_lossy()))?;
    let pipe = child.stdout.take().ok_or("missing server stdout")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let server = Server {
        process: child,
        lines: rx,
    };
    let line = server.wait_line(|line| line.starts_with(prefix), 10)?;
    let addr = line
        .split_whitespace()
        .find_map(|word| word.strip_prefix("addr="))
        .ok_or("missing listen address")?
        .parse::<SocketAddr>()
        .map_err(|e| e.to_string())?;
    Ok((server, addr))
}

fn start_server(path: &Path, options: &[&str]) -> Result<(Server, SocketAddr), String> {
    let mut command = Command::new(path);
    command.args(["--bind", "127.0.0.1:0"]).args(options);
    start_service(command, "SENDIT_READY ")
}

fn run() -> Result<(), String> {
    let o = Options::read()?;
    match o.action.as_str() {
        "help" | "--help" | "-h" => {
            println!("{HELP}");
            Ok(())
        }
        "verify" => {
            let impaired = Impairment {
                latency_ms: 36,
                jitter_ms: 10,
                loss_pct: 6.0,
                seed: 137,
            };
            for (label, profile) in [("local", Impairment::default()), ("impaired", impaired)] {
                let (_server, addr) =
                    start_server(&o.server, &["--map", "drill", "--mode", "practice"])?;
                eprintln!("Testing {label} on {addr}");
                scenario::smoke(&addr.to_string(), "", profile)?;
            }
            Ok(())
        }
        "verify-rules" => rules::verify(&o.server),
        "verify-arsenal" => arsenal::verify(&o.server),
        "verify-hosting" => hosting::verify(&o.server),
        "verify-bots" => bots::verify(&o.server, o.case.as_deref()),
        "smoke" => scenario::smoke(&o.addr, &o.password, o.net),
        "load" => scenario::load(
            &o.addr,
            &o.password,
            o.net,
            o.players,
            o.seconds,
            o.replay.as_deref(),
        ),
        _ => Err(format!("unknown command '{}'\n{HELP}", o.action)),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sendit-probe: {error}");
            ExitCode::FAILURE
        }
    }
}
