mod config;
mod console;
mod discovery;
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

pub(crate) fn stopping() -> bool {
    STOPPING.load(Ordering::Relaxed)
}

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
    #[cfg(unix)]
    install_shutdown_handlers().map_err(|e| e.to_string())?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let Some(result) = discovery::command(&args) {
        return result;
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    let config = Config::parse(args.into_iter())?;
    let mut host = Host::bind(config)?;
    let addr = host.address().map_err(|e| e.to_string())?;
    println!(
        "SENDIT_READY addr={addr} hz=128 snapshot_hz=32 map={} mode={:?} cap={}",
        host.config.map, host.config.mode, host.config.capacity
    );
    io::stdout().flush().map_err(|e| e.to_string())?;
    let console = console::read_commands();
    let mut stats = Window::new(host.connected(), 0);
    let mut next = Instant::now();
    let mut ticks = 0u64;
    'running: while !stopping() && host.config.ticks.is_none_or(|limit| ticks < limit) {
        let now = Instant::now();
        if let Some(delay) = next.checked_duration_since(now) {
            std::thread::sleep(delay);
        }
        let before = Instant::now();
        for command in console.try_iter().take(8) {
            match host.command(&command, before) {
                Ok(true) => break 'running,
                Ok(false) => (),
                Err(error) => eprintln!("console: {error}"),
            }
        }
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
        stats.observe(
            after - before,
            late,
            skipped,
            host.connected(),
            0,
            host.world.round_view().phase,
        );
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
