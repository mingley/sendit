use sendit_sim::{Loadout, Mode, Tuning};
use std::net::SocketAddr;
use std::path::PathBuf;

pub const HELP: &str = "\
sendit-server [options]
  --bind IP:PORT       Listen address (default 0.0.0.0:27015)
  --map NAME           Arena identifier (competitive default relay, otherwise drill)
  --mode MODE          practice, competitive, deathmatch, or surf (default practice)
  --loadout KIT     Deathmatch spawn kit: standard, light, or armored
  --max-players N      Maximum connected players (default 10)
  --password-env VAR   Read the join password from this environment variable
  --friction F         Override ground friction for tuning or replay
  --record FILE        Record accepted user inputs as a replay trace
  --ticks N            Exit after N fixed simulation ticks
  --report-ticks N     Print tick, CPU, RSS and network metrics every N ticks
                       (default 1280; 0 prints only the final summary)
  --help               Show these options";

pub struct Config {
    pub bind: SocketAddr,
    pub map: String,
    pub mode: Mode,
    pub dm_loadout: Loadout,
    pub capacity: u16,
    pub password: String,
    pub tuning: Tuning,
    pub record: Option<PathBuf>,
    pub ticks: Option<u64>,
    pub report_ticks: u64,
}

impl Config {
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut args = args;
        let mut config = Config {
            bind: "0.0.0.0:27015".parse().unwrap(),
            map: "drill".into(),
            mode: Mode::Practice,
            dm_loadout: Loadout::Standard,
            capacity: 10,
            password: String::new(),
            tuning: Tuning::default(),
            record: None,
            ticks: None,
            report_ticks: 1280,
        };
        let mut explicit_map = false;
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--bind" => config.bind = value.parse().map_err(|_| "invalid listen address")?,
                "--map" => {
                    explicit_map = true;
                    config.map = value;
                }
                "--mode" => config.mode = parse_mode(&value)?,
                "--loadout" => {
                    config.dm_loadout = match value.as_str() {
                        "standard" => Loadout::Standard,
                        "light" => Loadout::Light,
                        "armored" => Loadout::Armored,
                        _ => return Err(format!("unknown deathmatch spawn kit {value}")),
                    };
                }
                "--max-players" => {
                    config.capacity = value.parse().map_err(|_| "invalid player cap")?;
                    if !(1..=32).contains(&config.capacity) {
                        return Err("--max-players must be between 1 and 32".into());
                    }
                }
                "--password-env" => {
                    config.password = std::env::var(&value)
                        .map_err(|_| format!("password variable {value} is unavailable"))?;
                    if config.password.is_empty() || config.password.len() > 128 {
                        return Err("password must contain 1 to 128 bytes".into());
                    }
                }
                "--friction" => {
                    let friction: f32 = value.parse().map_err(|_| "invalid friction")?;
                    if !friction.is_finite() || !(0.0..=128.0).contains(&friction) {
                        return Err("--friction must be between 0 and 128".into());
                    }
                    config.tuning.friction = friction;
                }
                "--ticks" => config.ticks = Some(value.parse().map_err(|_| "invalid tick limit")?),
                "--report-ticks" => {
                    config.report_ticks = value.parse().map_err(|_| "invalid report interval")?
                }
                "--record" => config.record = Some(PathBuf::from(value)),
                _ => return Err(format!("unknown option {flag}")),
            }
        }
        if config.mode == Mode::Competitive {
            if config.capacity > 10 {
                return Err("competitive servers support up to five players per team".into());
            }
            if !explicit_map {
                config.map = "relay".into();
            }
        }
        Ok(config)
    }
}

pub fn parse_mode(name: &str) -> Result<Mode, String> {
    match name {
        "practice" => Ok(Mode::Practice),
        "competitive" => Ok(Mode::Competitive),
        "deathmatch" => Ok(Mode::Deathmatch),
        "surf" => Ok(Mode::Surf),
        _ => Err(format!("unknown ruleset {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn competitive_defaults_to_two_sites_and_respects_explicit_arena() {
        let parse = |args: &[&str]| Config::parse(args.iter().map(|arg| arg.to_string()));
        assert_eq!(parse(&["--mode", "competitive"]).unwrap().map, "relay");
        assert_eq!(
            parse(&["--map", "drill", "--mode", "competitive"])
                .unwrap()
                .map,
            "drill"
        );
        assert!(parse(&["--mode", "competitive", "--max-players", "11"]).is_err());
        assert!(matches!(
            parse(&["--mode", "deathmatch", "--loadout", "armored"])
                .unwrap()
                .dm_loadout,
            Loadout::Armored
        ));
    }
}
