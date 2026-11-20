mod net;
mod scenario;

use net::Impairment;
use std::env;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const HELP: &str = "Sendit UDP probe

  sendit-probe verify [--server PATH]
  sendit-probe smoke [--connect HOST:PORT] [--latency-ms N] [--jitter-ms N] [--loss-pct N] [--seed N]
  sendit-probe load [--connect HOST:PORT] [--players N] [--seconds N] [--replay FILE]

verify launches two fresh drill servers, testing both LAN conditions and a seeded impaired link.
smoke expects an empty practice server on the drill map. Latency and jitter apply in each direction.
load occupies a live server. Without an input trace it uses a synthetic movement and firing pattern.
Use --password-env NAME to join a protected smoke or load server.";

struct Options {
    action: String,
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

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_server(path: &Path) -> Result<(Server, SocketAddr), String> {
    let child = Command::new(path)
        .args([
            "--bind",
            "127.0.0.1:0",
            "--map",
            "drill",
            "--mode",
            "practice",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| {
            format!(
                "couldn't launch {}: {e}; build sendit-server first",
                path.display()
            )
        })?;
    let mut owner = Server(child);
    let pipe = owner.0.stdout.take().ok_or("missing server stdout")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if let Some(suffix) = line.strip_prefix("SENDIT_READY ") {
                for word in suffix.split_whitespace() {
                    if let Some(addr) = word.strip_prefix("addr=") {
                        let _ = tx.send(addr.parse::<SocketAddr>().map_err(|e| e.to_string()));
                        break;
                    }
                }
            }
        }
    });
    let addr = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "server exited or did not report readiness")??;
    Ok((owner, addr))
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
                let (_server, addr) = start_server(&o.server)?;
                eprintln!("Testing {label} on {addr}");
                scenario::smoke(&addr.to_string(), "", profile)?;
            }
            Ok(())
        }
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
