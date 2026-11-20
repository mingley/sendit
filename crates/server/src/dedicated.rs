use crate::clock::TickClock;
use crate::config::{Config, HELP};
use crate::host::Host;
use crate::stats::Window;
use crate::{console, discovery};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

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

/// Run the dedicated-server CLI, including operator console and process signal handling.
pub fn run_dedicated() -> Result<(), String> {
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
        "SENDIT_READY addr={addr} hz=128 snapshot_hz=32 map={} mode={:?} cap={} bots={}",
        host.config.map,
        host.config.mode,
        host.config.capacity,
        host.bot_count()
    );
    io::stdout().flush().map_err(|e| e.to_string())?;
    let console = console::read_commands();
    let mut stats = Window::new(host.connected(), host.bot_count());
    let mut clock = TickClock::new();
    let mut ticks = 0u64;
    'running: while !stopping() && host.config.ticks.is_none_or(|limit| ticks < limit) {
        if let Some(delay) = clock.delay() {
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
        let (late, skipped) = clock.finish(after);
        stats.observe(
            after - before,
            late,
            skipped,
            host.connected(),
            host.bot_count(),
            host.world.round_view().phase,
        );
        if host.config.report_ticks > 0 && ticks % host.config.report_ticks == 0 {
            stats.report(ticks, &host.traffic);
            host.traffic = Default::default();
            stats = Window::new(host.connected(), host.bot_count());
        }
    }
    stats.report(ticks, &host.traffic);
    host.flush().map_err(|e| e.to_string())?;
    Ok(())
}
