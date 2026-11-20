use serde_json::{json, Value};
use std::{fs, process::Command};

fn command(exe: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(exe).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn platform() -> Value {
    #[cfg(target_os = "macos")]
    let cpu = command("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"])
        .filter(|model| !model.is_empty());
    #[cfg(not(target_os = "macos"))]
    let cpu = fs::read_to_string("/proc/cpuinfo").ok().and_then(|info| {
        info.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            matches!(key.trim(), "model name" | "Hardware").then(|| value.trim().to_owned())
        })
    });
    let cgroup_cpu = fs::read_to_string("/sys/fs/cgroup/cpu.max")
        .ok()
        .map(|s| s.trim().to_owned());
    json!({
        "cpu_model":cpu,
        "available_parallelism":std::thread::available_parallelism().ok().map(|x|x.get()),
        "cgroup_cpu_max":cgroup_cpu,
        "system":command("uname",&["-srm"]),
        "target":env!("SENDIT_TARGET"),
        "build_profile":env!("SENDIT_PROFILE"),
        "renderer":"headless",
        "resolution":Value::Null
    })
}

#[cfg(unix)]
pub fn process_seconds() -> Option<f64> {
    let mut time = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the entire rusage object on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, time.as_mut_ptr()) } != 0 {
        return None;
    }
    let time = unsafe { time.assume_init() };
    Some(
        (time.ru_utime.tv_sec + time.ru_stime.tv_sec) as f64
            + (time.ru_utime.tv_usec + time.ru_stime.tv_usec) as f64 * 1e-6,
    )
}

#[cfg(not(unix))]
pub fn process_seconds() -> Option<f64> {
    None
}

pub fn resident_kib(key: &str) -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name != key {
                return None;
            }
            value.split_whitespace().next()?.parse().ok()
        })
}

/// Nearest rank, clamped to the actual sample set; returns a measured sample.
pub fn percentile(sorted: &[u64], p: usize) -> u64 {
    sorted[(sorted.len() * p).div_ceil(100).max(1) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn high_tail_is_measured_at_small_sample_counts() {
        assert_eq!(percentile(&[2, 4, 8], 99), 8);
        assert_eq!(percentile(&[2, 4, 8], 50), 4);
    }
}
