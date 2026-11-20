//! Portable, tick-indexed input traces. The first JSON line describes the session.
//! Subsequent lines contain one player's input or a named timing marker.
use crate::Input;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::Path,
};

const VERSION: u16 = 1;
const TICK_HZ: u16 = 128;

#[derive(Debug, Serialize, Deserialize)]
pub struct TraceHeader {
    pub version: u16,
    pub tick_hz: u16,
    pub map: String,
    pub source: String,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Event {
    Sample { tick: u32, id: u16, input: Input },
    Marker { tick: u32, marker: String },
}

pub struct TraceWriter {
    file: BufWriter<File>,
    pending: usize,
}

impl TraceWriter {
    pub fn create(path: impl AsRef<Path>, map: &str, source: &str) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut writer = Self {
            file: BufWriter::new(file),
            pending: 0,
        };
        writer.line(&TraceHeader {
            version: VERSION,
            tick_hz: TICK_HZ,
            map: map.to_owned(),
            source: source.to_owned(),
        })?;
        writer.flush()?;
        Ok(writer)
    }

    pub fn record(&mut self, tick: u32, id: u16, input: Input) -> io::Result<()> {
        self.line(&Event::Sample { tick, id, input })
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

#[derive(Debug)]
pub struct Trace {
    pub header: TraceHeader,
    tracks: Vec<Vec<(u32, Input)>>,
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
        if header.version != VERSION || header.tick_hz != TICK_HZ {
            return Err(format!(
                "unsupported replay version {} or tick rate {}",
                header.version, header.tick_hz
            ));
        }
        let mut tracks = BTreeMap::<u16, Vec<(u32, Input)>>::new();
        let mut markers = BTreeMap::new();
        let (mut first, mut last) = (u32::MAX, 0);
        for (idx, line) in lines.enumerate() {
            let line = line.map_err(|e| e.to_string())?;
            let row = idx + 2;
            let event =
                serde_json::from_str(&line).map_err(|e| format!("replay line {row}: {e}"))?;
            match event {
                Event::Sample { tick, id, input } => {
                    let samples = tracks.entry(id).or_default();
                    if samples.last().is_some_and(|(t, _)| *t >= tick) {
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
                    samples.push((tick, input));
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
            tracks: tracks.into_values().collect(),
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

    /// Tick relative to the first input in the recording.
    pub fn marker(&self, name: &str) -> Option<u32> {
        self.markers.get(name).map(|tick| tick - self.first)
    }

    /// Map slots to the original tracks in ascending player-ID order. Additional
    /// slots reuse tracks with a deterministic phase offset. Original tracks keep
    /// their relative timings. All tracks loop at the recorded session duration.
    pub fn input_at(&self, slot: usize, elapsed_ticks: u32) -> Input {
        let track = &self.tracks[slot % self.tracks.len()];
        let repeat = (slot / self.tracks.len()) as u64;
        let offset = (u64::from(elapsed_ticks) + repeat * 97) % u64::from(self.duration);
        let tick = self.first + offset as u32;
        let end = track.partition_point(|(t, _)| *t <= tick);
        let Some(index) = end.checked_sub(1) else {
            return Input::default();
        };
        let (sample, mut input) = track[index];
        let age = tick - sample;
        if age > 8 {
            return Input {
                yaw: input.yaw,
                pitch: input.pitch,
                ..Input::default()
            };
        }
        if age > 0 {
            input.buy = false;
            input.drop_bomb = false;
            input.reload = false;
        }
        input
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
            input: Input::default(),
        })
        .unwrap();
        assert!(Trace::read(format!("{}\n{entry}\n{entry}", header()).as_bytes()).is_err());
        assert!(Trace::read(header().replace("128", "64").as_bytes()).is_err());
    }
}
