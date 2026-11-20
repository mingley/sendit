#!/usr/bin/env python3
"""Run an occupied match on loopback with the shipping server and clients."""
import argparse
import contextlib
import json
import os
from pathlib import Path
import platform
import queue
import re
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parent.parent


@contextlib.contextmanager
def child(command, **kwargs):
    process = subprocess.Popen([str(s) for s in command], cwd=ROOT, text=True, **kwargs)
    try:
        yield process
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def messages(process):
    output = queue.Queue()

    def read():
        try:
            for line in process.stdout:
                output.put(line.rstrip())
        finally:
            output.put(None)

    threading.Thread(target=read, daemon=True).start()
    return output


def wait_for(output, predicate, timeout, processes):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for proc in processes:
            if proc.poll() is not None:
                raise RuntimeError(f"{Path(proc.args[0]).name} exited with {proc.returncode}")
        try:
            line = output.get(timeout=0.25)
        except queue.Empty:
            continue
        if line is None:
            raise RuntimeError("server closed its statistics stream")
        if predicate(line):
            return line
    raise RuntimeError("timed out waiting for a complete occupied measurement")


def cpu_name():
    if Path("/proc/cpuinfo").exists():
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.partition(":")[2].strip()
    return platform.processor() or "unknown"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=["server", "client"])
    parser.add_argument("--bin-dir", type=Path, default=ROOT / "target" / "release")
    parser.add_argument("--map", default="drill")
    parser.add_argument("--mode", default="practice", choices=["practice", "deathmatch", "competitive", "surf"])
    parser.add_argument("--players", type=int, default=10)
    parser.add_argument("--seconds", type=int, default=10, help="length of the measured window")
    parser.add_argument("--replay", type=Path, help="drive remote players from a recorded input trace")
    parser.add_argument("--output", type=Path, default=Path("results/match.json"))
    parser.add_argument("--width", type=int, default=1920)
    parser.add_argument("--height", type=int, default=1080)
    args = parser.parse_args()
    if not 2 <= args.players <= 32 or not 2 <= args.seconds <= 3600:
        parser.error("--players must be 2..32 and --seconds must be 2..3600")
    bins = args.bin_dir.resolve()
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    interval = args.seconds * 128
    server = [bins / "sendit-server", "--bind", "127.0.0.1:0", "--map", args.map,
              "--mode", args.mode, "--max-players", args.players, "--report-ticks", interval]
    source = "scripted UDP input"
    track_count = None
    if args.replay:
        args.replay = args.replay.resolve()
        with args.replay.open() as stream:
            header = json.loads(stream.readline())
            track_count = len({record["id"] for line in stream if (record := json.loads(line)).get("id") is not None})
        if header.get("map") != args.map:
            parser.error("--map must match the recorded arena")
        source = f"input replay ({header.get('source', 'unknown origin')})"

    with contextlib.ExitStack() as stack:
        host = stack.enter_context(child(server, stdout=subprocess.PIPE, stdin=subprocess.DEVNULL))
        lines = messages(host)
        ready = wait_for(lines, lambda line: line.startswith("SENDIT_READY "), 10, [host])
        addr = re.search(r"addr=(\S+)", ready).group(1)
        count = args.players - int(args.target == "client")
        # Keep peers joined during graphical adapter startup and shader warmup.
        joiners = [bins / "sendit-probe", "load", "--connect", addr, "--players", count,
                   "--seconds", 3 * args.seconds + 120]
        if args.replay:
            joiners.extend(["--replay", args.replay])
        peers = stack.enter_context(child(joiners, stdout=sys.stderr))
        if args.target == "client":
            client = [bins / "sendit-client", "--connect", addr, "--name", "benchmark",
                      "--width", args.width, "--height", args.height, "--benchmark", args.seconds,
                      "--expect-players", args.players, "--output", output]
            if args.replay:
                client.extend(["--replay", args.replay])
            subprocess.run([str(s) for s in client], cwd=ROOT, check=True, timeout=120 + 2 * args.seconds)
            measured = json.loads(output.read_text())
            measured["peer_workload"] = source
            measured["recorded_input_tracks"] = track_count
            output.write_text(json.dumps(measured, indent=2) + "\n")
            return

        def fully_occupied(line):
            fields = dict(re.findall(r"(\w+)=([^ ]+)", line))
            return (line.startswith("stats ") and fields.get("players_min") == str(args.players)
                    and fields.get("ticks") == str(interval))

        line = wait_for(lines, fully_occupied, 3 * args.seconds + 15, [host, peers])
        fields = dict(re.findall(r"(\w+)=([^ ]+)", line))
        stats = {key: float(value) if "." in value else int(value) if value.isdigit() else value
                 for key, value in fields.items()}
        report = dict(workload=source, map=args.map, rules=args.mode, players=args.players,
                      resolution=None, binary_directory_label=bins.name,
                      profile="release" if bins == (ROOT / "target/release").resolve() else "provided binary",
                      cpu=cpu_name(), platform=platform.platform(), recorded_input_tracks=track_count,
                      cpu_quota=Path("/sys/fs/cgroup/cpu.max").read_text().strip()
                      if Path("/sys/fs/cgroup/cpu.max").exists() else None,
                      logical_cpus=os.cpu_count(), ticks_per_second=128, tick_budget_ms=7.8125,
                      bandwidth="UDP application payload; excludes IP/UDP headers", measured=stats)
        output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, RuntimeError, subprocess.SubprocessError) as error:
        sys.exit(f"benchmark: {error}")
