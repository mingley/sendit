#!/usr/bin/env python3
"""Package the Linux server, desktop client and measurement tools from a locked release build."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
BINS = ("sendit-server", "sendit-client", "sendit-probe", "sendit-bench")
BUNDLE = "sendit-linux-x86_64"


def run(command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def add(tar, name, data, timestamp, executable=False):
    entry = tarfile.TarInfo(f"{BUNDLE}/{name}")
    entry.size = len(data)
    entry.mode = 0o755 if executable else 0o644
    entry.mtime = timestamp
    tar.addfile(entry, io.BytesIO(data))


def notices(tar, metadata, timestamp):
    inventory = []
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if not package["source"]:
            continue
        name = f"{package['name']}-{package['version']}"
        inventory.append(f"{name}\t{package.get('license') or 'see license file'}")
        source = Path(package["manifest_path"]).parent
        paths = {p for p in source.iterdir() if p.is_file() and
                 p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"))}
        if package.get("license_file"):
            paths.add(source / package["license_file"])
        if package["name"] == "cosmic-text":
            paths.update((source / "fonts").glob("*LICENSE*"))
        for p in sorted(paths):
            if p.is_file():
                add(tar, f"notices/{name}/{p.name}", p.read_bytes(), timestamp)
    add(tar, "notices/packages.txt", ("\n".join(inventory) + "\n").encode(), timestamp)
    if shutil.which("rustc"):
        runtime = Path(run(["rustc", "--print", "sysroot"])) / "share/doc/rust/COPYRIGHT-library.html"
        if runtime.is_file():
            add(tar, "notices/Rust-standard-library.html", runtime.read_bytes(), timestamp)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--out", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        parser.error("run on the native x86_64 Linux build host")
    subprocess.run([args.cargo, "build", "--release", "--locked", "--workspace"], cwd=ROOT, check=True)
    data = json.loads(run([args.cargo, "metadata", "--locked", "--format-version", "1",
                           "--filter-platform", "x86_64-unknown-linux-gnu"]))
    timestamp = int(run(["git", "log", "-1", "--format=%ct"]))
    output = args.out.resolve()
    output.mkdir(exist_ok=True, parents=True)
    filename = f"{BUNDLE}.tar.xz"
    # Prepare a complete archive first so an interrupted build preserves the old download.
    with tempfile.TemporaryDirectory(dir=output) as tmp:
        archive = Path(tmp) / filename
        with tarfile.open(archive, "w:xz", preset=4) as tar:
            for binary in BINS:
                built = Path(data["target_directory"]) / "release" / binary
                stripped = Path(tmp) / binary
                subprocess.run(["strip", "--strip-unneeded", "-o", stripped, built], check=True)
                add(tar, binary, stripped.read_bytes(), timestamp, True)
            for map_file in sorted((ROOT / "maps").glob("*.json")):
                add(tar, "maps/" + map_file.name, map_file.read_bytes(), timestamp)
            add(tar, "README.md", (ROOT / "README.md").read_bytes(), timestamp)
            add(tar, "scripts/benchmark.py", (ROOT / "scripts/benchmark.py").read_bytes(), timestamp)
            for doc in sorted((ROOT / "docs").rglob("*")):
                if doc.is_file():
                    add(tar, doc.relative_to(ROOT), doc.read_bytes(), timestamp)
            for result in sorted((ROOT / "benchmarks/cloud").glob("*.json")):
                add(tar, result.relative_to(ROOT), result.read_bytes(), timestamp)
            fixture = ROOT / "fixtures/relay-desktop.jsonl"
            add(tar, fixture.relative_to(ROOT), fixture.read_bytes(), timestamp)
            notices(tar, data, timestamp)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.replace(output / filename)
    (output / "SHA256SUMS").write_text(f"{digest}  {filename}\n")
    print(f"{output / filename}\nSHA-256 {digest}")


if __name__ == "__main__":
    main()
