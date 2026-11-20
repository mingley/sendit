mod config;
mod host;
mod stats;

use config::{Config, HELP};
use host::Host;
use stats::Window;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / sendit_sim::TICK_RATE as u64);

static STOPPING: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn install_shutdown_handlers() -> io::Result<()> {
    extern "C" fn stop(_: libc::c_int) {
        STOPPING.store(true, Ordering::Relaxed);
    }
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // The handler only writes a lock-free flag. Replay I/O happens on the main thread.
        if unsafe { libc::signal(signal, stop as *const () as libc::sighandler_t) } == libc::SIG_ERR
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    #[cfg(unix)]
    install_shutdown_handlers().map_err(|e| e.to_string())?;
    let config = Config::parse(std::env::args().skip(1))?;
    let mut host = Host::bind(config)?;
    let addr = host.address().map_err(|e| e.to_string())?;
    println!(
        "SENDIT_READY addr={addr} hz=128 snapshot_hz=32 map={} mode={:?} cap={}",
        host.config.map, host.config.mode, host.config.capacity
    );
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut stats = Window::new(host.connected(), 0);
    let mut next = Instant::now();
    let mut ticks = 0u64;
    while !STOPPING.load(Ordering::Relaxed) && host.config.ticks.is_none_or(|limit| ticks < limit) {
        let now = Instant::now();
        if let Some(delay) = next.checked_duration_since(now) {
            std::thread::sleep(delay);
        }
        let before = Instant::now();
        host.step(before).map_err(|e| e.to_string())?;
        let after = Instant::now();
        ticks += 1;
        next += TICK;
        let late = after > next;
        let mut skipped = 0;
        // Limit catch-up work after a long process suspension. Simulation ticks
        // still advance one at a time; count discarded wall-clock slots.
        if after.saturating_duration_since(next) > TICK * 8 {
            skipped = (after.duration_since(next).as_nanos() / TICK.as_nanos()) as u64;
            next += Duration::from_secs_f64(TICK.as_secs_f64() * skipped as f64);
        }
        stats.observe(after - before, late, skipped, host.connected(), 0);
        if host.config.report_ticks > 0 && ticks % host.config.report_ticks == 0 {
            stats.report(ticks, &host.traffic);
            host.traffic = Default::default();
            stats = Window::new(host.connected(), 0);
        }
    }
    stats.report(ticks, &host.traffic);
    host.flush().map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("sendit-server: {error}");
        std::process::exit(1);
    }
}
