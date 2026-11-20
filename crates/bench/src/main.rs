mod metrics;
mod peek;
mod server;

use sendit_sim::{replay::Trace, Arena, Mode, Tuning};
use serde_json::{json, Value};
use std::{collections::BTreeMap, env, process::ExitCode, str::FromStr};

const USAGE: &str = "\
Sendit benchmark and input replay
  sendit-bench peek [--friction N]
  sendit-bench record-peek --out FILE
  sendit-bench replay --file FILE [--friction N]
  sendit-bench server [--players 10] [--ticks 8192] [--warmup 512]
                      [--map drill] [--mode practice] [--friction N]
                      [--replay FILE]

Input recordings are versioned JSONL at 128 fixed ticks per second. The server
benchmark advances time without sleeping and prints a JSON report. Use a
release build for timing. The probe load command drives a separate UDP server.";

pub struct Options(BTreeMap<String, String>);

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut opts = BTreeMap::new();
        while let Some(key) = args.next() {
            if !key.starts_with("--") {
                return Err(format!("expected a flag; got {key}"));
            }
            let val = args
                .next()
                .filter(|s| !s.starts_with("--"))
                .ok_or_else(|| format!("{key} needs a value"))?;
            if opts.insert(key.clone(), val).is_some() {
                return Err(format!("duplicate {key}"));
            }
        }
        Ok(Self(opts))
    }

    fn take(&mut self, key: &str) -> Option<String> {
        self.0.remove(key)
    }

    fn value<T: FromStr>(&mut self, key: &str, fallback: T) -> Result<T, String> {
        match self.take(key) {
            None => Ok(fallback),
            Some(s) => s.parse().map_err(|_| format!("invalid {key}: {s}")),
        }
    }

    fn required(&mut self, key: &str) -> Result<String, String> {
        self.take(key).ok_or_else(|| format!("missing {key}"))
    }

    fn tuning(&mut self) -> Result<Tuning, String> {
        let mut tuning = Tuning::default();
        tuning.friction = self.value("--friction", tuning.friction)?;
        if !tuning.friction.is_finite() || !(0.0..=64.0).contains(&tuning.friction) {
            return Err("--friction must be in [0, 64]".into());
        }
        Ok(tuning)
    }

    fn finish(&self) -> Result<(), String> {
        self.0
            .keys()
            .next()
            .map_or(Ok(()), |key| Err(format!("unknown flag {key}")))
    }
}

fn mode(s: &str) -> Result<Mode, String> {
    match s {
        "practice" => Ok(Mode::Practice),
        "competitive" => Ok(Mode::Competitive),
        "deathmatch" => Ok(Mode::Deathmatch),
        "surf" => Ok(Mode::Surf),
        _ => Err(format!("unknown mode {s}")),
    }
}

fn run() -> Result<Option<Value>, String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "--help".into());
    if matches!(command.as_str(), "--help" | "-h" | "help") {
        println!("{USAGE}");
        return Ok(None);
    }
    let mut opts = Options::parse(args)?;
    let report = match command.as_str() {
        "peek" => {
            let tuning = opts.tuning()?;
            opts.finish()?;
            peek::measure(
                &Arena::load("drill")?,
                &tuning,
                peek::TICKS,
                peek::BRAKE_TICK,
                peek::input,
            )
        }
        "record-peek" => {
            let path = opts.required("--out")?;
            opts.finish()?;
            peek::record(&path).map_err(|e| format!("{path}: {e}"))?;
            json!({"saved":path,"map":"drill","tracks":2,"fixed_ticks":peek::TICKS,
                   "brake_tick":peek::BRAKE_TICK,"source":"scripted-peek"})
        }
        "replay" => {
            let path = opts.required("--file")?;
            let trace = Trace::load(path)?;
            let tuning = opts.tuning()?;
            opts.finish()?;
            let arena = Arena::load(&trace.header.map)?;
            if trace.header.source == "scripted-peek" {
                let brake = trace
                    .marker("brake")
                    .ok_or("peek trace is missing brake marker")?;
                if trace.track_count() != 2 || brake == 0 {
                    return Err("peek trace needs two tracks and a run-up".into());
                }
                peek::measure(&arena, &tuning, trace.duration(), brake, |slot, tick| {
                    trace.input_at(slot, tick)
                })
            } else {
                server::replay_run(arena, tuning, &trace)?
            }
        }
        "server" => server::run(opts)?,
        _ => return Err(format!("unknown command {command}\n{USAGE}")),
    };
    Ok(Some(report))
}

fn main() -> ExitCode {
    match run() {
        Ok(Some(report)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("JSON report")
            );
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("sendit-bench: {err}");
            ExitCode::FAILURE
        }
    }
}
