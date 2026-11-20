#!/usr/bin/env python3
"""Play a local match using the shipping Apple Silicon download."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile


def smoke(archive, scratch):
    package = scratch / "download"
    work = scratch / "player"
    work.mkdir()
    subprocess.run(["ditto", "-x", "-k", archive, package], check=True)
    app = package / "Sendit.app"
    server = package / "sendit-server"
    probe = package / "sendit-probe"
    bench = package / "sendit-bench"
    subprocess.run([probe, "verify-bots", "--server", server], cwd=work, check=True, timeout=240)
    subprocess.run([bench, "surf"], cwd=work, check=True, stdout=subprocess.DEVNULL)

    metal = subprocess.check_output([
        "xcrun", "swift", "-e", 'import Metal; print(MTLCreateSystemDefaultDevice()?.name ?? "")',
    ], text=True).strip()
    if not metal:
        print("The Mac runner has no Metal device. Signed executables and live bot matches passed.")
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
          f"({match['frame_interval_ms']['samples']} frame intervals).")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    path = parser.parse_args().archive.resolve()
    with tempfile.TemporaryDirectory() as temp:
        smoke(path, Path(temp))
