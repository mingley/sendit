//! Recover named map data from individually retryable UDP datagrams.
use sendit_sim::{map::MAX_MAP_BYTES, wire::CHUNK_BYTES, Arena};
use std::time::{Duration, Instant};

const RETRY: Duration = Duration::from_millis(180);
const WINDOW: usize = 8;

pub struct MapDownload {
    name: String,
    token: u64,
    count: Option<u8>,
    pieces: Vec<Option<Vec<u8>>>,
    requested: Vec<Option<Instant>>,
}

impl MapDownload {
    pub fn new(name: &str, token: u64) -> Self {
        Self {
            name: name.to_owned(),
            token,
            count: None,
            pieces: vec![None],
            requested: vec![None],
        }
    }

    pub fn requests(&mut self, now: Instant) -> Vec<u8> {
        let mut requests = Vec::new();
        for (idx, (piece, last)) in self.pieces.iter().zip(&mut self.requested).enumerate() {
            if piece.is_none()
                && last.is_none_or(|sent| now.saturating_duration_since(sent) >= RETRY)
            {
                requests.push(idx as u8);
                *last = Some(now);
                if requests.len() == WINDOW {
                    break;
                }
            }
        }
        requests
    }

    pub fn accept(
        &mut self,
        token: u64,
        name: &str,
        part: u8,
        count: u8,
        bytes: Vec<u8>,
    ) -> Result<Option<Arena>, String> {
        if token != self.token || name != self.name {
            return Ok(None);
        }
        let index = usize::from(part);
        let total = usize::from(count);
        if total == 0
            || total > MAX_MAP_BYTES.div_ceil(CHUNK_BYTES)
            || index >= total
            || bytes.is_empty()
            || bytes.len() > CHUNK_BYTES
            || index * CHUNK_BYTES + bytes.len() > MAX_MAP_BYTES
            || (index + 1 < total && bytes.len() != CHUNK_BYTES)
        {
            return Err("invalid map chunk bounds".into());
        }
        if let Some(known) = self.count {
            if known != count {
                return Err("map chunk count changed during transfer".into());
            }
        } else {
            self.count = Some(count);
            self.pieces.resize(total, None);
            self.requested.resize(total, None);
        }
        if let Some(old) = &self.pieces[index] {
            if *old != bytes {
                return Err("conflicting duplicate map chunk".into());
            }
            return Ok(None);
        }
        self.pieces[index] = Some(bytes);
        if self.pieces.iter().any(Option::is_none) {
            return Ok(None);
        }
        let mut source = Vec::with_capacity(total * CHUNK_BYTES);
        for piece in self.pieces.iter().flatten() {
            source.extend_from_slice(piece);
        }
        let arena = Arena::from_json(&source)?;
        if arena.name != name {
            return Err("map file identifier differs from the host announcement".into());
        }
        Ok(Some(arena))
    }
}
#[cfg(test)]
#[path = "map_transfer_tests.rs"]
mod tests;
