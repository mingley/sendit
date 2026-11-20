use bevy::prelude::Resource;
use std::path::PathBuf;

const HELP: &str =
    "sendit-client [--connect HOST:PORT] [--name NAME] [--password-env NAME | --password PASSWORD]
  --sensitivity NUMBER       Radians per raw mouse count (default 0.002)
  --width PX --height PX     Window dimensions (default 1920 x 1080)
  --record FILE              Record live fixed-tick inputs
  --replay FILE              Play the first player's input track
  --benchmark SECONDS        Measure an occupied match after three seconds of warmup
  --live-input               Use live mouse/keyboard during a graphical benchmark
  --expect-players COUNT     Minimum match occupancy for benchmarking (default 2)
  --output FILE              Write the benchmark report as JSON
  --mute                     Disable game sounds

Click to capture the mouse. Escape releases it. F1 shows the controls.";

#[derive(Resource)]
pub struct Options {
    pub address: String,
    pub name: String,
    pub password: String,
    pub sensitivity: f32,
    pub width: u32,
    pub height: u32,
    pub record: Option<PathBuf>,
    pub replay: Option<PathBuf>,
    pub benchmark: Option<f32>,
    pub expect_players: usize,
    pub live_input: bool,
    pub output: PathBuf,
    pub mute: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:27015".into(),
            name: "Player".into(),
            password: String::new(),
            sensitivity: 0.002,
            width: 1920,
            height: 1080,
            record: None,
            replay: None,
            benchmark: None,
            expect_players: 2,
            live_input: false,
            output: "client-benchmark.json".into(),
            mute: false,
        }
    }
}

impl Options {
    pub fn parse() -> Result<Option<Self>, String> {
        let mut opts = Self::default();
        let mut supplied_password = false;
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            if flag == "-h" || flag == "--help" {
                println!("{HELP}");
                return Ok(None);
            }
            if flag == "--live-input" {
                opts.live_input = true;
                continue;
            }
            if flag == "--mute" {
                opts.mute = true;
                continue;
            }
            let value = args
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))?;
            let invalid = || format!("invalid value for {flag}");
            match flag.as_str() {
                "--connect" => opts.address = value,
                "--name" => opts.name = value,
                "--password" | "--password-env" => {
                    if supplied_password {
                        return Err("choose one password option".into());
                    }
                    supplied_password = true;
                    opts.password = if flag == "--password" {
                        value
                    } else {
                        if value.is_empty()
                            || !value.bytes().enumerate().all(|(i, b)| {
                                b == b'_'
                                    || b.is_ascii_alphabetic()
                                    || (i > 0 && b.is_ascii_digit())
                            })
                        {
                            return Err("invalid password variable name".into());
                        }
                        std::env::var(&value)
                            .map_err(|_| format!("password variable {value} is unavailable"))?
                    };
                }
                "--sensitivity" => opts.sensitivity = value.parse().map_err(|_| invalid())?,
                "--width" => opts.width = value.parse().map_err(|_| invalid())?,
                "--height" => opts.height = value.parse().map_err(|_| invalid())?,
                "--record" => opts.record = Some(value.into()),
                "--replay" => opts.replay = Some(value.into()),
                "--benchmark" => opts.benchmark = Some(value.parse().map_err(|_| invalid())?),
                "--expect-players" => opts.expect_players = value.parse().map_err(|_| invalid())?,
                "--output" => opts.output = value.into(),
                _ => return Err(format!("unknown option {flag}\n{HELP}")),
            }
        }
        if !(1..=16_384).contains(&opts.width) || !(1..=16_384).contains(&opts.height) {
            return Err("window dimensions must be 1..16384".into());
        }
        if !opts.sensitivity.is_finite() || opts.sensitivity <= 0.0 {
            return Err("sensitivity must be finite and positive".into());
        }
        if opts.benchmark.is_some_and(|v| !v.is_finite() || v <= 0.0) || opts.expect_players == 0 {
            return Err("benchmark duration and population must be positive".into());
        }
        if opts.live_input && (opts.benchmark.is_none() || opts.replay.is_some()) {
            return Err("--live-input requires --benchmark and live mouse/keyboard control".into());
        }
        if opts.record.is_some()
            && (opts.replay.is_some() || (opts.benchmark.is_some() && !opts.live_input))
        {
            return Err("recording requires live human input".into());
        }
        Ok(Some(opts))
    }
}
