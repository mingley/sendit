use super::*;
use sendit_sim::Phase;

impl Host {
    /// A failed load leaves the current match, tickets and trace intact.
    fn change_map(&mut self, name: &str, now: Instant) -> Result<(), String> {
        let (arena, source) = Arena::load_with_source(name)?;
        let mut world = World::new(arena, self.config.mode, self.config.tuning);
        world.tick = self.world.tick.wrapping_add(1);
        world.set_dm_loadout(self.config.dm_loadout);
        let mut tickets = Vec::with_capacity(self.connected());
        for (&addr, session) in &self.sessions {
            let id = session.id;
            let team = self
                .world
                .view(id)
                .ok_or("connected player lacks world state")?
                .team;
            if !world.add_team(id, team) {
                return Err(format!("could not place player {id} on {name}"));
            }
            tickets.push((addr, new_token().map_err(|e| e.to_string())?));
        }
        let bots = self
            .bots
            .for_rotation(&self.world, &mut world, &self.config)?;
        self.flush().map_err(|e| e.to_string())?;
        let next_trace = if let Some(path) = self.config.record.as_ref() {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let file =
                path.with_file_name(format!("{stem}.{:03}.{name}.jsonl", self.trace_serial + 1));
            Some(
                TraceWriter::create_configured(
                    &file,
                    name,
                    "server-session",
                    self.config.mode,
                    self.config.tuning,
                )
                .map_err(|e| format!("open {}: {e}", file.display()))?,
            )
        } else {
            None
        };
        self.trace_serial += 1;
        self.trace = next_trace;
        self.world = world;
        self.bots = bots;
        self.custom_source = source;
        self.config.map = name.into();
        self.listing.map = name.into();
        self.map_started = now;
        self.roster_changed = true;
        for &(addr, token) in &tickets {
            let session = self.sessions.get_mut(&addr).unwrap();
            session.token = token;
            session.last_seq = 0;
            session.last_reset = None;
            session.deathmatch_input = DeathmatchInput::default();
            session.pending.clear();
            session.pending_welcome = true;
            session.chunk_count = 0;
        }
        for (addr, _) in tickets {
            self.welcome(addr).map_err(|e| e.to_string())?;
        }
        println!(
            "SENDIT_MAP map={} mode={:?} tick={}",
            self.config.map, self.config.mode, self.world.tick
        );
        Ok(())
    }

    fn next_map(&mut self, now: Instant) -> Result<(), String> {
        let maps = &self.config.rotation;
        if maps.is_empty() {
            return Err("no --rotation configured".into());
        }
        let index = maps
            .iter()
            .position(|name| name == &self.config.map)
            .map_or(0, |i| (i + 1) % maps.len());
        self.change_map(&maps[index].clone(), now)
    }

    pub(super) fn rotate_if_due(&mut self, now: Instant) {
        if self.config.rotation.is_empty() || self.config.rotate_seconds == 0 {
            return;
        }
        if now.duration_since(self.map_started).as_secs() < u64::from(self.config.rotate_seconds) {
            return;
        }
        if self.config.mode == Mode::Competitive
            && !matches!(self.world.round_view().phase, Phase::Warmup | Phase::Ended)
        {
            return;
        }
        if let Err(error) = self.next_map(now) {
            // Retry only at the next interval. An operator can correct the file and use 'next'.
            self.map_started = now;
            eprintln!("rotation: {error}");
        }
    }

    /// Returns true for a graceful operator-requested shutdown.
    pub fn command(&mut self, line: &str, now: Instant) -> Result<bool, String> {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.as_slice() {
            [] => (),
            ["help"] => println!("status | kick ID | map NAME | next | restart | quit"),
            ["status"] => {
                println!(
                    "status map={} mode={:?} players={} cap={} humans={} bots={}",
                    self.config.map,
                    self.config.mode,
                    self.connected() + self.bots.len(),
                    self.config.capacity,
                    self.connected(),
                    self.bots.len()
                );
                for session in self.sessions.values() {
                    println!("player id={} name={:?}", session.id, session.name);
                }
                for (id, name) in self.bots.names() {
                    println!("bot id={id} name={name:?}");
                }
            }
            ["next"] => self.next_map(now)?,
            ["restart"] => self.change_map(&self.config.map.clone(), now)?,
            ["map", name] => self.change_map(name, now)?,
            ["kick", text] => {
                let id = text
                    .parse::<u16>()
                    .map_err(|_| "expected numeric player ID")?;
                let addr = self
                    .sessions
                    .iter()
                    .find(|(_, s)| s.id == id)
                    .map(|(addr, _)| *addr)
                    .ok_or("player is not connected")?;
                self.send(addr, ServerMsg::Rejected("removed by operator".into()))
                    .map_err(|e| e.to_string())?;
                self.sessions.remove(&addr);
                self.world.remove(id);
                self.roster_changed = true;
                println!("kick id={id}");
            }
            ["quit"] => return Ok(true),
            _ => return Err("unknown command or arguments; use 'help'".into()),
        }
        Ok(false)
    }
}
