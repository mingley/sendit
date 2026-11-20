use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Traffic {
    pub received: u64,
    pub sent: u64,
    pub send_drops: u64,
}

// Linux reports CPU time for the process across all of its threads.
#[cfg(unix)]
fn cpu_seconds() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // getrusage initializes its output on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    Some(
        (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64
            + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 * 1e-6,
    )
}

#[cfg(not(unix))]
fn cpu_seconds() -> Option<f64> {
    None
}

fn rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn field(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.3}"))
        .unwrap_or_else(|| "n/a".into())
}

pub struct Window {
    start: Instant,
    cpu_start: Option<f64>,
    // Sparse microsecond buckets retain long stalls without an overflow bin.
    histogram: BTreeMap<u64, u64>,
    samples: u64,
    maximum_us: f64,
    late: u64,
    skipped: u64,
    last_observed: Instant,
    humans: usize,
    players: usize,
    players_min: usize,
    players_max: usize,
    human_seconds: f64,
    player_seconds: f64,
}

impl Window {
    pub fn new(humans: usize, bots: usize) -> Self {
        let start = Instant::now();
        Self {
            start,
            cpu_start: cpu_seconds(),
            histogram: BTreeMap::new(),
            samples: 0,
            maximum_us: 0.0,
            late: 0,
            skipped: 0,
            last_observed: start,
            humans,
            players: humans + bots,
            players_min: usize::MAX,
            players_max: 0,
            human_seconds: 0.0,
            player_seconds: 0.0,
        }
    }

    pub fn observe(
        &mut self,
        duration: Duration,
        late: bool,
        skipped: u64,
        humans: usize,
        bots: usize,
    ) {
        let us = duration.as_nanos().div_ceil(1000) as u64;
        *self.histogram.entry(us).or_default() += 1;
        self.samples += 1;
        self.maximum_us = self.maximum_us.max(duration.as_secs_f64() * 1e6);
        self.late += u64::from(late);
        self.skipped += skipped;
        let observed = Instant::now();
        let elapsed = (observed - self.last_observed).as_secs_f64();
        self.human_seconds += elapsed * self.humans as f64;
        self.player_seconds += elapsed * self.players as f64;
        self.last_observed = observed;
        self.humans = humans;
        self.players = humans + bots;
        self.players_min = self.players_min.min(self.players);
        self.players_max = self.players_max.max(self.players);
    }

    fn percentile_us(&self, p: u64) -> f64 {
        let target = (p * self.samples).div_ceil(100);
        let mut count = 0;
        for (us, samples) in &self.histogram {
            count += samples;
            if count >= target {
                return (*us as f64).min(self.maximum_us);
            }
        }
        self.maximum_us
    }

    pub fn report(&self, tick: u64, traffic: &Traffic) {
        if self.samples == 0 {
            return;
        }
        let wall = self.start.elapsed().as_secs_f64();
        let cpu = self
            .cpu_start
            .zip(cpu_seconds())
            .map(|(a, b)| (b - a) / wall * 100.0);
        let rate = |bytes: u64| {
            (self.human_seconds > 0.0).then(|| bytes as f64 / self.human_seconds / 1024.0)
        };
        println!(
            "stats tick={tick} ticks={} players={} players_min={} players_max={} humans={} human_seconds={:.3} player_seconds={:.3} tick_p50_us={:.1} tick_p95_us={:.1} tick_p99_us={:.1} tick_max_us={:.1} missed_ticks={} skipped_slots={} cpu_pct={} rss_kib={} rx_bytes={} tx_bytes={} rx_kib_s_per_human={} tx_kib_s_per_human={} send_drops={}",
            self.samples, self.players, self.players_min, self.players_max, self.humans, self.human_seconds, self.player_seconds, self.percentile_us(50), self.percentile_us(95), self.percentile_us(99),
            self.maximum_us, self.late + self.skipped, self.skipped, field(cpu),
            rss_kib().map(|rss| rss.to_string()).unwrap_or_else(|| "n/a".into()),
            traffic.received, traffic.sent, field(rate(traffic.received)), field(rate(traffic.sent)), traffic.send_drops,
        );
    }
}
