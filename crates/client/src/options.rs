use bevy::{prelude::Resource, window::WindowResolution};
use std::path::PathBuf;

const HELP: &str =
    "sendit-client [--connect HOST:PORT] [--name NAME] [--password-env NAME | --password PASSWORD]
  --bots COUNT               Local Aim deathmatch opponents, 1..9 (default 5)
  --bot-skill NUMBER         Local bot difficulty, 0..1 (default 0.4)
  --sensitivity NUMBER       Radians per raw mouse count (default 0.001)
  --width PX --height PX     Set window dimensions (benchmarks default to 1920 x 1080)
                             Local play opens maximized with a 1280 x 720 fallback
  --record FILE              Record live fixed-tick inputs
  --replay FILE              Play the first player's input track
  --benchmark SECONDS        Measure an occupied match after three seconds of warmup
  --live-input               Use live mouse/keyboard during a graphical benchmark
  --expect-players COUNT     Minimum match occupancy for benchmarking (default 2)
  --output FILE              Write the benchmark report as JSON
  --mute                     Disable game sounds

Launch without arguments to play a local free-for-all. Adjust opponents with Left/Right
and difficulty with Down/Up, then click to play. Escape releases the mouse; F1 shows
the controls. --connect joins a dedicated server. Standalone --replay defaults to
the existing loopback server; adding --bots replays an Aim recording locally.";

#[derive(Resource)]
pub struct Options {
    pub address: String,
    pub local_match: bool,
    pub bots: u16,
    pub bot_skill: f32,
    pub name: String,
    pub password: String,
    pub sensitivity: f32,
    pub width: u32,
    pub height: u32,
    pub(crate) explicit_resolution: bool,
    pub start_maximized: bool,
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
            local_match: true,
            bots: 5,
            bot_skill: 0.4,
            name: "Player".into(),
            password: String::new(),
            sensitivity: 0.001,
            width: 1920,
            height: 1080,
            explicit_resolution: false,
            start_maximized: false,
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
    pub fn window_resolution(&self) -> WindowResolution {
        let resolution = WindowResolution::new(self.width as f32, self.height as f32);
        // AppKit normally takes window sizes in points. CLI dimensions and
        // benchmark resolutions are backing pixels, including on Retina screens.
        if cfg!(target_os = "macos") && (self.explicit_resolution || self.benchmark.is_some()) {
            resolution.with_scale_factor_override(1.0)
        } else {
            resolution
        }
    }

    pub fn parse() -> Result<Option<Self>, String> {
        Self::parse_args(std::env::args().skip(1))
    }

    fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Self>, String> {
        let mut opts = Self::default();
        let mut local_controls = false;
        let mut supplied_password = false;
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
                "--connect" => {
                    opts.address = value;
                    opts.local_match = false;
                }
                "--bots" => {
                    opts.bots = value.parse().map_err(|_| invalid())?;
                    local_controls = true;
                }
                "--bot-skill" => {
                    opts.bot_skill = value.parse().map_err(|_| invalid())?;
                    local_controls = true;
                }
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
                "--width" => {
                    opts.width = value.parse().map_err(|_| invalid())?;
                    opts.explicit_resolution = true;
                }
                "--height" => {
                    opts.height = value.parse().map_err(|_| invalid())?;
                    opts.explicit_resolution = true;
                }
                "--record" => opts.record = Some(value.into()),
                "--replay" => opts.replay = Some(value.into()),
                "--benchmark" => opts.benchmark = Some(value.parse().map_err(|_| invalid())?),
                "--expect-players" => opts.expect_players = value.parse().map_err(|_| invalid())?,
                "--output" => opts.output = value.into(),
                _ => return Err(format!("unknown option {flag}\n{HELP}")),
            }
        }
        if !(1..=9).contains(&opts.bots) || !(0.0..=1.).contains(&opts.bot_skill) {
            return Err("local bot count must be 1..9 and skill must be 0..1".into());
        }
        if !opts.local_match && local_controls {
            return Err(
                "bot count and difficulty are configured by the host for --connect sessions".into(),
            );
        }
        if !local_controls && opts.replay.is_some() {
            opts.local_match = false;
        }
        if opts.local_match
            && !opts.explicit_resolution
            && opts.benchmark.is_none()
            && opts.replay.is_none()
        {
            opts.width = 1280;
            opts.height = 720;
            opts.start_maximized = true;
        }
        if opts.local_match
            && opts.benchmark.is_some()
            && opts.expect_players > usize::from(opts.bots) + 1
        {
            return Err(
                "expected population exceeds this local match; use --bots or --connect".into(),
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Result<Options, String> {
        Options::parse_args(values.iter().map(|v| v.to_string()))?.ok_or("help".into())
    }
    #[test]
    fn normal_launch_is_local_and_legacy_replay_can_join_a_dedicated_host() {
        let solo = args(&[]).unwrap();
        assert!(solo.local_match);
        assert_eq!((solo.bots, solo.bot_skill), (5, 0.4));
        assert!(args(&["--record", "live.jsonl"]).unwrap().local_match);
        assert!(!args(&["--replay", "drill.jsonl"]).unwrap().local_match);
        assert!(
            args(&["--bots", "3", "--replay", "aim.jsonl"])
                .unwrap()
                .local_match
        );
        assert!(!args(&["--connect", "127.0.0.1:27015"]).unwrap().local_match);
    }
    #[test]
    fn quickplay_fits_the_work_area_and_explicit_sizes_and_benchmarks_are_preserved() {
        let local = args(&[]).unwrap();
        assert_eq!(
            (local.width, local.height, local.start_maximized),
            (1280, 720, true)
        );
        let sized = args(&["--width", "800", "--height", "450"]).unwrap();
        assert_eq!(
            (sized.width, sized.height, sized.start_maximized),
            (800, 450, false)
        );
        for params in [&["--benchmark", "2"][..], &["--connect", "localhost:27015"]] {
            let external = args(params).unwrap();
            assert_eq!(
                (external.width, external.height, external.start_maximized),
                (1920, 1080, false)
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn retina_uses_physical_pixels_for_cli_sizes_and_benchmarks() {
        for (params, width, height) in [
            (&["--width", "960", "--height", "540"][..], 960, 540),
            (&["--benchmark", "2"][..], 1920, 1080),
        ] {
            let mut resolution = args(params).unwrap().window_resolution();
            resolution.set_scale_factor(2.);
            assert_eq!(resolution.scale_factor_override(), Some(1.0));
            assert_eq!(
                (resolution.physical_width(), resolution.physical_height()),
                (width, height)
            );
            assert_eq!(
                (resolution.width(), resolution.height()),
                (width as f32, height as f32)
            );
        }
        let mut normal = args(&[]).unwrap().window_resolution();
        normal.set_scale_factor(2.);
        assert_eq!(normal.scale_factor(), 2.);
    }

    #[test]
    fn bot_limits_and_external_host_ownership_are_checked() {
        for cmd in [
            &["--bots", "0"][..],
            &["--bots", "10"],
            &["--bot-skill", "NaN"],
            &["--bot-skill", "1.1"],
            &["--connect", "localhost:27015", "--bots", "4"],
        ] {
            assert!(args(cmd).is_err());
        }
        for cmd in [
            &["--bots", "1", "--bot-skill", "0"][..],
            &["--bots", "9", "--bot-skill", "1"],
        ] {
            assert!(args(cmd).is_ok());
        }
    }
}
