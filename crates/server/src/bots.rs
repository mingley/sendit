//! Population management is separate from bot decisions. Each bot only submits
//! a sequenced player command to the same authoritative step as a UDP client.
use crate::config::Config;
use sendit_sim::bots::{BotBrain, BotNav};
use sendit_sim::wire::FIRST_BOT_ID;
use sendit_sim::{Arena, Command, World};
use std::collections::BTreeMap;

struct Slot {
    brain: BotBrain,
    sequence: u32,
}

pub struct Bots {
    slots: BTreeMap<u16, Slot>,
    nav: Option<BotNav>,
}

impl Bots {
    pub fn new(arena: &Arena, cfg: &Config) -> Self {
        Self {
            slots: BTreeMap::new(),
            nav: (cfg.bots > 0 || cfg.fill > 0).then(|| BotNav::new(arena)),
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn names(&self) -> impl Iterator<Item = (u16, String)> + '_ {
        self.slots
            .keys()
            .map(|&id| (id, format!("Bot {:02}", id - FIRST_BOT_ID + 1)))
    }

    pub fn desired(humans: usize, cfg: &Config) -> usize {
        usize::from(cfg.bots)
            .max(usize::from(cfg.fill).saturating_sub(humans))
            .min(usize::from(cfg.capacity).saturating_sub(humans))
    }

    pub fn make_room(&mut self, world: &mut World, desired: usize) {
        while self.slots.len() > desired {
            let (id, _) = self.slots.pop_last().unwrap();
            world.remove(id);
        }
    }

    fn add_brain(&mut self, id: u16, cfg: &Config) {
        self.slots.insert(
            id,
            Slot {
                brain: BotBrain::new(id, cfg.bot_skill, cfg.bot_seed),
                sequence: 0,
            },
        );
    }

    pub fn fill(&mut self, world: &mut World, humans: usize, cfg: &Config) {
        let desired = Self::desired(humans, cfg);
        while self.slots.len() < desired {
            let id = (FIRST_BOT_ID..FIRST_BOT_ID + cfg.capacity)
                .find(|id| !self.slots.contains_key(id))
                .unwrap();
            let added = match cfg.bot_team {
                Some(team) => world.add_team(id, team),
                None => world.add(id),
            };
            if !added {
                break; // An explicitly selected team has hit its competitive cap.
            }
            self.add_brain(id, cfg);
        }
    }

    /// Keep actual teams (including halftime swaps) when rotating occupied servers.
    pub fn for_rotation(&self, old: &World, new: &mut World, cfg: &Config) -> Result<Self, String> {
        let mut next = Self::new(&new.arena, cfg);
        for &id in self.slots.keys() {
            let team = old.view(id).ok_or("bot lacks world state")?.team;
            if !new.add_team(id, team) {
                return Err(format!("could not place bot {id} on new map"));
            }
            next.add_brain(id, cfg);
        }
        Ok(next)
    }

    pub fn commands(&mut self, world: &World, commands: &mut Vec<(u16, Command)>) {
        if self.slots.is_empty() {
            return;
        }
        let nav = self
            .nav
            .as_ref()
            .expect("populated bots require navigation");
        let players = world.views();
        let round = world.round_view();
        for (&id, slot) in &mut self.slots {
            let Some(me) = world.view(id) else { continue };
            let input = slot.brain.command(
                world.tick.wrapping_add(1),
                world.mode,
                me,
                &players,
                &round,
                &world.arena,
                nav,
            );
            slot.sequence = slot.sequence.wrapping_add(1);
            commands.push((
                id,
                Command {
                    seq: slot.sequence,
                    seen_tick: world.tick,
                    input,
                },
            ));
        }
    }
}
