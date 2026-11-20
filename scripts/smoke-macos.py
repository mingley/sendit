#!/usr/bin/env python3
"""Verify the extracted Apple Silicon game, its Metal renderer and its foreground controls."""
import argparse
from contextlib import contextmanager
import json
from pathlib import Path
import re
import select
import shutil
import subprocess
import tempfile
import time


def wait_for(client, description, check, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if client.poll() is not None:
            raise RuntimeError(f"Client exited with code {client.returncode} while waiting for {description}")
        value = check()
        if value:
            return value
        time.sleep(0.1)
    raise RuntimeError(f"Timed out waiting for {description}")


def read_trace(path):
    if not path.exists():
        return [], []
    # The recorder can be writing its final line as we read.
    lines = path.read_bytes().split(b"\n")[:-1]
    if not lines:
        return [], []
    rows = [json.loads(line) for line in lines]
    header = rows[0]
    if (header["source"], header["map"], header["mode"]) != ("human-client", "aim", "Deathmatch"):
        raise RuntimeError(f"Unexpected input recording header: {header}")
    return lines, rows


def observed_tick(path, after, condition):
    _, rows = read_trace(path)
    return next((r["tick"] for r in rows if "input" in r and r["tick"] > after and condition(r["input"])), None)


def recorded_controls(path):
    lines, rows = read_trace(path)
    samples = [r for r in rows if "input" in r]
    equip = next((r["tick"] for r in samples if r["input"].get("equip") == "smg"), None)
    if equip is None:
        return None
    forward = [r["tick"] for r in samples if r["tick"] > equip and r["input"]["forward"] > 0.5]
    if not forward:
        return None
    fire = [r["tick"] for r in samples if r["tick"] > forward[-1] and r["input"]["fire"]]
    if not fire or samples[-1]["tick"] == fire[-1]:
        return None
    # End exactly at the observed burst. A later bot kill and respawn can refill the magazine.
    prefix = [line for line, row in zip(lines, rows) if row.get("tick", 0) <= fire[-1]]
    return b"\n".join(prefix) + b"\n", {
        "equip_tick": equip, "forward_samples": len(forward), "fire_samples": len(fire),
    }


@contextmanager
def hold(driver, action, pid):
    process = subprocess.Popen([driver, action, str(pid)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        if not select.select([process.stdout], [], [], 12)[0] or process.stdout.readline() != b"held\n":
            raise RuntimeError(f"Could not post the foreground {action} event")
        yield
    finally:
        try:
            process.communicate(input=b"release\n", timeout=4)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise
        if process.returncode:
            raise RuntimeError(f"The native {action} control exited with code {process.returncode}")


def play_with_os_input(app, bench, driver, work):
    trace = work / "native-input.jsonl"
    client_log = work / "native-client.log"
    # Launch the executable inside the extracted app to observe its exact PID and exit status.
    # The independent five-bot benchmark opens the same bundle via LaunchServices.
    executable = app / "Contents" / "MacOS" / "sendit-client"
    with client_log.open("wb") as output:
        client = subprocess.Popen([
            executable, "--record", trace, "--bots", "1", "--bot-skill", "0",
            "--width", "960", "--height", "540",
        ], cwd=work, stdout=output, stderr=subprocess.STDOUT)
    try:
        def log_matches(pattern):
            return re.search(pattern, client_log.read_text(errors="replace"))

        def gesture(name):
            subprocess.run([driver, name, str(client.pid)], check=True, timeout=15)

        wait_for(client, "the rendered lobby", lambda: log_matches(r"QUICKPLAY_READY map=aim bots=1 "), 70)
        gesture("start")
        wait_for(client, "the local player and one bot", lambda: log_matches(r"QUICKPLAY_PLAYING id=\d+ players=2\b"))
        def observed(description, after, condition):
            return wait_for(client, description, lambda: observed_tick(trace, after, condition))

        observed("the live input stream", -1, lambda _: True)
        gesture("equip")
        equip_tick = observed("a recorded SMG selection", -1, lambda cmd: cmd.get("equip") == "smg")
        with hold(driver, "walk", client.pid):
            walked = observed("recorded forward movement while holding W", equip_tick, lambda cmd: cmd["forward"] > 0.5)
        stopped = observed("the W release", walked, lambda cmd: cmd["forward"] == 0)
        with hold(driver, "fire", client.pid):
            fired = observed("recorded fire while holding the mouse", stopped, lambda cmd: cmd["fire"])
            observed("a sustained firing burst", fired + 12, lambda cmd: cmd["fire"])
        prefix, receipt = wait_for(client, "the recorded fire release", lambda: recorded_controls(trace))
        gesture("pause")
        # The in-game menu suppresses D while the host and the recorder keep ticking.
        # Two recorder flushes ensure the sample interval follows the actual key-down.
        with hold(driver, "strafe", client.pid):
            _, rows = read_trace(trace)
            latest = max(r["tick"] for r in rows if "input" in r)
            end = observed("the open menu across 256 input ticks", latest + 255, lambda _: True)
            _, rows = read_trace(trace)
            paused = [r["input"] for r in rows if "input" in r and latest + 128 < r["tick"] <= end]
            if not paused or any(cmd["strafe"] for cmd in paused):
                raise RuntimeError("The live player moved while Escape was open and D was held")
            receipt["paused_samples"] = len(paused)
        gesture("quit")
        if client.wait(timeout=12) != 0:
            raise RuntimeError(f"The menu quit with exit code {client.returncode}")
        capture = work / "native-burst.jsonl"
        capture.write_bytes(prefix)
        replay = json.loads(subprocess.check_output([bench, "replay", "--file", capture], cwd=work, timeout=30))
        (work / "native-replay.json").write_text(json.dumps(replay, indent=2))
        players = replay["players"]
        if len(players) != 1 or players[0]["equipped"] != "smg" or players[0]["shots_fired"] < 1:
            raise RuntimeError(f"The captured inputs failed to produce SMG fire in replay: {replay}")
        return {**receipt, "players": 2, "replayed_smg_shots": players[0]["shots_fired"],
                "menu_exit_code": client.returncode}
    except Exception:
        print(client_log.read_text(errors="replace")[-6000:], flush=True)
        raise
    finally:
        if client.poll() is None:
            client.terminate()
            try:
                client.wait(timeout=4)
            except subprocess.TimeoutExpired:
                client.kill()
                client.wait()


def smoke(archive, scratch, results):
    package = scratch / "download"
    work = scratch / "player"
    work.mkdir()
    subprocess.run(["ditto", "-x", "-k", archive, package], check=True)
    app = package / "Sendit.app"
    server = package / "sendit-server"
    probe = package / "sendit-probe"
    bench = package / "sendit-bench"
    subprocess.run([probe, "verify-bots", "--server", server], cwd=work, check=True, timeout=240)
    results["live_bot_matches"] = {"status": "passed"}
    subprocess.run([probe, "verify-arsenal", "--server", server], cwd=work, check=True, timeout=240)
    results["live_arsenal"] = {"status": "passed"}
    subprocess.run([bench, "surf"], cwd=work, check=True, stdout=subprocess.DEVNULL)
    results["surf"] = {"status": "passed"}

    driver = scratch / "quartz-playtest"
    source = Path(__file__).with_name("playtest-macos.swift").resolve()
    subprocess.run(["xcrun", "swiftc", source, "-o", driver], check=True, timeout=120)
    capabilities = json.loads(subprocess.check_output([driver, "status"], timeout=15))
    results["capabilities"] = capabilities
    metal = capabilities["metal"]
    if not metal or not capabilities["login_session"]:
        reason = "No Metal device" if not metal else "No macOS login session for a window"
        for check in ["metal_match", "os_input"]:
            results[check] = {"status": "skipped", "reason": reason}
        return
    report = work / "match.json"
    subprocess.run([
        "open", "-n", "-W", app, "--args", "--benchmark", "2", "--expect-players", "6",
        "--width", "960", "--height", "540", "--output", report,
    ], cwd=work, check=True, timeout=150)
    match = json.loads(report.read_text())
    if (match["target"] != "aarch64-apple-darwin" or match["gpu"]["backend"] != "Metal"
            or match["mode"] != "Deathmatch" or match["map"] != "aim"
            or (match["players_min"], match["players_max"]) != (6, 6)
            or match["resolution_px"] != [960, 540]
            or match["frame_interval_ms"]["samples"] < 2):
        raise RuntimeError(f"Unexpected desktop match: {match}")
    print(f"Sendit.app rendered Aim with five bots on {metal} "
          f"({match['frame_interval_ms']['samples']} frame intervals).", flush=True)
    results["metal_match"] = {"status": "passed", "players": 6, "frame_intervals": match["frame_interval_ms"]["samples"]}
    if not capabilities["post_events"]:
        results["os_input"] = {"status": "skipped", "reason": "macOS denied Quartz event posting (Accessibility)"}
    else:
        results["os_input"] = {"status": "failed"}
        receipt = play_with_os_input(app, bench, driver, work)
        results["os_input"] = {"status": "passed", **receipt}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--output-dir", type=Path, help="Save the separate check results, Metal report and native input evidence")
    args = parser.parse_args()
    results = {}
    with tempfile.TemporaryDirectory() as temp:
        scratch = Path(temp)
        try:
            smoke(args.archive.resolve(), scratch, results)
        except Exception as error:
            results["error"] = str(error)
            raise
        finally:
            report = json.dumps(results, sort_keys=True, indent=2)
            print(report, flush=True)
            if args.output_dir:
                args.output_dir.mkdir(parents=True, exist_ok=True)
                for name in ["match.json", "native-client.log", "native-input.jsonl", "native-burst.jsonl", "native-replay.json"]:
                    file = scratch / "player" / name
                    if file.is_file():
                        shutil.copy2(file, args.output_dir / name)
                (args.output_dir / "results.json").write_text(report + "\n")
