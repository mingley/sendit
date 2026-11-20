//! Portable, tick-indexed input traces. The first JSON line describes the session.
//! Subsequent lines contain one player's input or a named timing marker.
use crate::{Command, Input, Mode, PlayerView, Tuning, MAX_REWIND};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::Path,
};

const VERSION: u16 = 3;
const TICK_HZ: u16 = 128;

#[derive(Debug, Serialize, Deserialize)]
pub struct TraceHeader {
    pub version: u16,
    pub tick_hz: u16,
    pub map: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<Tuning>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Event {
    Sample {
        tick: u32,
        id: u16,
        input: Input,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rewind: Option<u8>,
    },
    Start {
        tick: u32,
        state: PlayerView,
    },
    Marker {
        tick: u32,
        marker: String,
    },
}

pub struct TraceWriter {
    file: BufWriter<File>,
    pending: usize,
    last: BTreeMap<u16, (u32, u32)>,
}

impl TraceWriter {
    pub fn create(path: impl AsRef<Path>, map: &str, source: &str) -> io::Result<Self> {
        Self::open(
            path,
            TraceHeader {
                version: VERSION,
                tick_hz: TICK_HZ,
                map: map.to_owned(),
                source: source.to_owned(),
                mode: None,
                tuning: None,
            },
        )
    }

    pub fn create_configured(
        path: impl AsRef<Path>,
        map: &str,
        source: &str,
        mode: Mode,
        tuning: Tuning,
    ) -> io::Result<Self> {
        Self::open(
            path,
            TraceHeader {
                version: VERSION,
                tick_hz: TICK_HZ,
                map: map.to_owned(),
                source: source.to_owned(),
                mode: Some(mode),
                tuning: Some(tuning),
            },
        )
    }

    fn open(path: impl AsRef<Path>, header: TraceHeader) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut writer = Self {
            file: BufWriter::new(file),
            pending: 0,
            last: BTreeMap::new(),
        };
        writer.line(&header)?;
        writer.flush()?;
        Ok(writer)
    }

    pub fn record(&mut self, tick: u32, id: u16, input: Input) -> io::Result<()> {
        self.line(&Event::Sample {
            tick,
            id,
            input,
            seq: None,
            rewind: None,
        })
    }

    fn start(&mut self, tick: u32, seq: u32, state: PlayerView) -> io::Result<()> {
        let previous = self.last.insert(state.id, (state.life, seq));
        if previous.is_none_or(|(life, last_seq)| {
            life != state.life || (seq.wrapping_sub(last_seq) as i32) <= 0
        }) {
            self.line(&Event::Start { tick, state })?;
        }
        Ok(())
    }

    /// The host calls this immediately before consuming a newly accepted command.
    pub fn record_command(
        &mut self,
        before_tick: u32,
        cmd: Command,
        state: PlayerView,
    ) -> io::Result<()> {
        self.start(before_tick, cmd.seq, state)?;
        let age = before_tick.wrapping_add(1).wrapping_sub(cmd.seen_tick);
        let rewind = if age > i32::MAX as u32 {
            0
        } else {
            age.min(MAX_REWIND) as u8
        };
        self.line(&Event::Sample {
            tick: before_tick,
            id: state.id,
            input: cmd.input,
            seq: Some(cmd.seq),
            rewind: Some(rewind),
        })
    }

    /// Local predictions use the client's command sequence and pre-input pose.
    /// The host decides the eventual rewind age after network transit.
    pub fn record_client(&mut self, cmd: Command, state: PlayerView) -> io::Result<()> {
        self.start(cmd.seq, cmd.seq, state)?;
        self.line(&Event::Sample {
            tick: cmd.seq,
            id: state.id,
            input: cmd.input,
            seq: Some(cmd.seq),
            rewind: None,
        })
    }

    pub fn mark(&mut self, tick: u32, marker: &str) -> io::Result<()> {
        self.line(&Event::Marker {
            tick,
            marker: marker.to_owned(),
        })
    }

    fn line(&mut self, value: &impl Serialize) -> io::Result<()> {
        serde_json::to_writer(&mut self.file, value).map_err(io::Error::other)?;
        self.file.write_all(b"\n")?;
        self.pending += 1;
        if self.pending >= 128 {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.pending = 0;
        self.file.flush()
    }
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    tick: u32,
    input: Input,
    seq: Option<u32>,
    rewind: Option<u8>,
}

#[derive(Debug)]
struct Track {
    id: u16,
    starts: BTreeMap<u32, PlayerView>,
    samples: Vec<Sample>,
}

#[derive(Debug)]
pub struct Trace {
    pub header: TraceHeader,
    tracks: Vec<Track>,
    markers: BTreeMap<String, u32>,
    first: u32,
    duration: u32,
}

impl Trace {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let file =
            File::open(path.as_ref()).map_err(|e| format!("{}: {e}", path.as_ref().display()))?;
        Self::read(BufReader::new(file))
    }

    pub fn read(reader: impl BufRead) -> Result<Self, String> {
        let mut lines = reader.lines();
        let line = lines
            .next()
            .ok_or("empty replay")?
            .map_err(|e| e.to_string())?;
        let header: TraceHeader =
            serde_json::from_str(&line).map_err(|e| format!("replay header: {e}"))?;
        if !(1..=VERSION).contains(&header.version) || header.tick_hz != TICK_HZ {
            return Err(format!(
                "unsupported replay version {} or tick rate {}",
                header.version, header.tick_hz
            ));
        }
        let mut tracks = BTreeMap::<u16, Vec<Sample>>::new();
        let mut starts = BTreeMap::<u16, BTreeMap<u32, PlayerView>>::new();
        let mut markers = BTreeMap::new();
        let (mut first, mut last) = (u32::MAX, 0);
        for (idx, line) in lines.enumerate() {
            let line = line.map_err(|e| e.to_string())?;
            let row = idx + 2;
            let event =
                serde_json::from_str(&line).map_err(|e| format!("replay line {row}: {e}"))?;
            match event {
                Event::Start { tick, state } => {
                    if state.team > 1
                        || !state.body.pos.is_finite()
                        || !state.body.vel.is_finite()
                        || !state.yaw.is_finite()
                        || !state.pitch.is_finite()
                        || !state.rifle.valid_state()
                        || starts
                            .entry(state.id)
                            .or_default()
                            .insert(tick, state)
                            .is_some()
                    {
                        return Err(format!(
                            "replay line {row}: invalid or duplicate starting state"
                        ));
                    }
                }
                Event::Sample {
                    tick,
                    id,
                    input,
                    seq,
                    rewind,
                } => {
                    let samples = tracks.entry(id).or_default();
                    if samples.last().is_some_and(|sample| sample.tick >= tick) {
                        return Err(format!(
                            "replay line {row}: ticks for player {id} must increase"
                        ));
                    }
                    if !input.yaw.is_finite()
                        || !input.pitch.is_finite()
                        || !(-1.0..=1.0).contains(&input.strafe)
                        || !(-1.0..=1.0).contains(&input.forward)
                    {
                        return Err(format!("replay line {row}: invalid input axes or angles"));
                    }
                    samples.push(Sample {
                        tick,
                        input,
                        seq,
                        rewind,
                    });
                    first = first.min(tick);
                    last = last.max(tick);
                }
                Event::Marker { tick, marker } => {
                    if markers.insert(marker.clone(), tick).is_some() {
                        return Err(format!("replay line {row}: duplicate marker {marker}"));
                    }
                }
            }
        }
        if tracks.is_empty() {
            return Err("replay contains no input samples".into());
        }
        let duration = (last - first).checked_add(1).ok_or("replay too long")?;
        if markers.values().any(|tick| !(first..=last).contains(tick)) {
            return Err("marker lies outside the recorded tick interval".into());
        }
        Ok(Self {
            header,
            tracks: tracks
                .into_iter()
                .map(|(id, samples)| Track {
                    id,
                    starts: starts.remove(&id).unwrap_or_default(),
                    samples,
                })
                .collect(),
            markers,
            first,
            duration,
        })
    }

    pub fn duration(&self) -> u32 {
        self.duration
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    pub fn track_id(&self, slot: usize) -> u16 {
        self.tracks[slot].id
    }

    pub fn start_at(&self, slot: usize, elapsed: u32) -> Option<PlayerView> {
        self.tracks[slot]
            .starts
            .get(&self.first.checked_add(elapsed)?)
            .copied()
    }

    pub fn first_tick(&self) -> u32 {
        self.first
    }

    /// Read the actual received command. An absent sample must reach the server
    /// as a packet gap: the last accepted sequence is also part of the shot seed.
    pub fn command_at(&self, slot: usize, elapsed: u32, simulation_tick: u32) -> Option<Command> {
        if elapsed >= self.duration {
            return None;
        }
        let samples = &self.tracks[slot].samples;
        let tick = self.first + elapsed;
        let index = samples
            .binary_search_by_key(&tick, |sample| sample.tick)
            .ok()?;
        let sample = samples[index];
        let fallback = if self.header.source == "human-client" {
            tick
        } else {
            elapsed + 1
        };
        Some(Command {
            seq: sample.seq.unwrap_or(fallback),
            seen_tick: simulation_tick
                .wrapping_add(1)
                .wrapping_sub(u32::from(sample.rewind.unwrap_or(1))),
            input: sample.input,
        })
    }

    /// Tick relative to the first input in the recording.
    pub fn marker(&self, name: &str) -> Option<u32> {
        self.markers.get(name).map(|tick| tick - self.first)
    }

    /// Map slots to the original tracks in ascending player-ID order. Additional
    /// slots reuse tracks with a deterministic phase offset. Original tracks keep
    /// their relative timings. All tracks loop at the recorded session duration.
    pub fn input_at(&self, slot: usize, elapsed_ticks: u32) -> Input {
        let track = &self.tracks[slot % self.tracks.len()].samples;
        let repeat = (slot / self.tracks.len()) as u64;
        let offset = (u64::from(elapsed_ticks) + repeat * 97) % u64::from(self.duration);
        let tick = self.first + offset as u32;
        let end = track.partition_point(|sample| sample.tick <= tick);
        let Some(index) = end.checked_sub(1) else {
            return Input::default();
        };
        let sample = track[index];
        sample.input.at_age(tick - sample.tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> &'static str {
        r#"{"version":1,"tick_hz":128,"map":"drill","source":"test"}"#
    }

    #[test]
    fn sparse_tracks_keep_recorded_offsets_and_loop() {
        let mut text = format!("{}\n", header());
        for (tick, id, forward) in [(44, 2, 1.0), (46, 1, -1.0), (48, 2, 0.0)] {
            let event = Event::Sample {
                tick,
                id,
                seq: None,
                rewind: None,
                input: Input {
                    forward,
                    ..Input::default()
                },
            };
            text.push_str(&serde_json::to_string(&event).unwrap());
            text.push('\n');
        }
        let trace = Trace::read(text.as_bytes()).unwrap();
        assert_eq!(trace.duration(), 5);
        assert_eq!(trace.input_at(0, 0).forward, 0.0);
        assert_eq!(trace.input_at(0, 2).forward, -1.0);
        assert_eq!(trace.input_at(1, 3).forward, 1.0);
        assert_eq!(trace.input_at(1, 4).forward, 0.0);
        assert_eq!(trace.input_at(1, 5).forward, 1.0);
    }

    #[test]
    fn rejects_duplicate_ticks_and_incompatible_tick_rate() {
        let entry = serde_json::to_string(&Event::Sample {
            tick: 7,
            id: 2,
            seq: None,
            rewind: None,
            input: Input::default(),
        })
        .unwrap();
        assert!(Trace::read(format!("{}\n{entry}\n{entry}", header()).as_bytes()).is_err());
        assert!(Trace::read(header().replace("128", "64").as_bytes()).is_err());
    }

    #[test]
    fn original_v2_desktop_trace_keeps_its_m4_and_axes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/relay-desktop.jsonl");
        let trace = Trace::load(path).unwrap();
        assert_eq!(trace.header.version, 2);
        let start = trace.start_at(0, 0).unwrap();
        assert_eq!(
            (start.rifle.weapon, start.rifle.ammo),
            (crate::Weapon::M4, 30)
        );
        assert!(start.last_hit.is_none());
        assert!(trace.command_at(0, 0, 10).unwrap().input.equip.is_none());
        assert!(trace.duration() > 900);
    }

    #[test]
    fn arsenal_trace_restores_melee_empty_mag_and_large_smg_mag() {
        use crate::{Arena, Weapon, World};
        let mut world = World::new(
            Arena::load("drill").unwrap(),
            Mode::Deathmatch,
            Tuning::default(),
        );
        assert!(world.add(1));
        let header = r#"{"version":3,"tick_hz":128,"map":"drill","source":"test"}"#;
        for weapon in [Weapon::Knife, Weapon::Smg] {
            let command = Command {
                seq: world.tick + 1,
                seen_tick: world.tick,
                input: Input {
                    equip: Some(weapon),
                    ..Input::default()
                },
            };
            world.advance(&[(1, command)]);
            let state = *world.view(1).unwrap();
            assert_eq!(state.rifle.weapon, weapon);
            let start = serde_json::to_string(&Event::Start { tick: 10, state }).unwrap();
            let sample = serde_json::to_string(&Event::Sample {
                tick: 10,
                id: 1,
                input: command.input,
                seq: Some(10),
                rewind: Some(0),
            })
            .unwrap();
            let trace = Trace::read(format!("{header}\n{start}\n{sample}").as_bytes()).unwrap();
            assert_eq!(trace.start_at(0, 0).unwrap().rifle, state.rifle);
            assert_eq!(trace.input_at(0, 0).equip, Some(weapon));
        }
    }
}
