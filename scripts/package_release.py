#!/usr/bin/env python3
"""Build a downloadable x86-64 Sendit release from a Linux host."""
import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
BINS = ("sendit-server", "sendit-client", "sendit-probe", "sendit-bench")
PLATFORMS = {
    "linux": ("x86_64-unknown-linux-gnu", ".tar.xz"),
    "windows": ("x86_64-pc-windows-gnu", ".zip"),
}


def run(command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def notices(metadata):
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
                yield f"notices/{name}/{p.name}", p
    yield "notices/packages.txt", ("\n".join(inventory) + "\n").encode()
    if shutil.which("rustc"):
        runtime = Path(run(["rustc", "--print", "sysroot"])) / "share/doc/rust/COPYRIGHT-library.html"
        if runtime.is_file():
            yield "notices/Rust-standard-library.html", runtime


def contents(binary_dir, temp, windows, strip, metadata, toolchain_notices):
    for binary in BINS:
        filename = binary + (".exe" if windows else "")
        stripped = temp / filename
        subprocess.run([strip, "--strip-unneeded", "-o", stripped, binary_dir / filename], check=True)
        yield filename, stripped
    for notice in toolchain_notices:
        yield f"notices/toolchain/{notice.parent.name}/{notice.name}", notice
    for pattern in ("README.md", "scripts/benchmark.py", "maps/*.json", "docs/**/*",
                    "benchmarks/cloud/*.json", "fixtures/relay-desktop.jsonl"):
        for path in sorted(ROOT.glob(pattern)):
            if path.is_file():
                yield path.relative_to(ROOT).as_posix(), path
    yield from notices(metadata)


def write_archive(path, bundle, files, timestamp, windows):
    if windows:
        zip_date = datetime.fromtimestamp(timestamp, timezone.utc).timetuple()[:6]
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=8) as archive:
            for name, source in files:
                entry = zipfile.ZipInfo(f"{bundle}/{name}", zip_date)
                data = source.read_bytes() if isinstance(source, Path) else source
                archive.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=8)
    else:
        with tarfile.open(path, "w:xz", preset=4) as archive:
            for name, source in files:
                entry = tarfile.TarInfo(f"{bundle}/{name}")
                entry.mode = 0o755 if name in BINS else 0o644
                entry.mtime = timestamp
                data = source.read_bytes() if isinstance(source, Path) else source
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--platform", choices=PLATFORMS, default="linux")
    parser.add_argument("--strip", help="target strip executable")
    parser.add_argument("--toolchain-notice", type=Path, action="append", default=[],
                        help="redistribution notice for a linked toolchain component")
    parser.add_argument("--out", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        parser.error("run on the native x86_64 Linux build host")
    target, extension = PLATFORMS[args.platform]
    windows = args.platform == "windows"
    command = [args.cargo, "build", "--release", "--locked", "--workspace"]
    if windows:
        command.extend(["--target", target])
    subprocess.run(command, cwd=ROOT, check=True)
    data = json.loads(run([args.cargo, "metadata", "--locked", "--format-version", "1",
                           "--filter-platform", target]))
    timestamp = int(run(["git", "log", "-1", "--format=%ct"]))
    output = args.out.resolve()
    output.mkdir(exist_ok=True, parents=True)
    bundle = f"sendit-{args.platform}-x86_64"
    filename = bundle + extension
    # Prepare a complete archive first so an interrupted build preserves the old download.
    with tempfile.TemporaryDirectory(dir=output) as tmp:
        temp = Path(tmp)
        archive = temp / filename
        target_dir = Path(data["target_directory"])
        binary_dir = (target_dir / target if windows else target_dir) / "release"
        strip = args.strip or ("x86_64-w64-mingw32-strip" if windows else "strip")
        files = contents(binary_dir, temp, windows, strip, data, args.toolchain_notice)
        write_archive(archive, bundle, files, timestamp, windows)
        archive.replace(output / filename)
        checksums = []
        for system, (_, suffix) in PLATFORMS.items():
            path = output / f"sendit-{system}-x86_64{suffix}"
            if path.is_file():
                checksums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
        sums = temp / "SHA256SUMS"
        sums.write_text("".join(checksums))
        sums.replace(output / "SHA256SUMS")
    print(output / filename)


if __name__ == "__main__":
    main()
