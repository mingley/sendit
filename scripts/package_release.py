#!/usr/bin/env python3
"""Build a downloadable Sendit release on Linux or an Apple Silicon Mac."""
import argparse
from datetime import datetime, timezone
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
BINS = ("sendit-server", "sendit-client", "sendit-probe", "sendit-bench")
PLATFORMS = {
    "linux": ("x86_64-unknown-linux-gnu", "x86_64", ".tar.xz"),
    "windows": ("x86_64-pc-windows-gnu", "x86_64", ".zip"),
    "macos": ("aarch64-apple-darwin", "arm64", ".zip"),
}
MACOS_MINIMUM = "11.0"
BUNDLE_ID = "io.github.mingley.sendit"


def bundle_name(system):
    return f"sendit-{system}-{PLATFORMS[system][1]}"


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


def support_files(metadata, toolchain_notices):
    for notice in toolchain_notices:
        yield f"notices/toolchain/{notice.parent.name}/{notice.name}", notice
    for pattern in ("README.md", "scripts/benchmark.py", "scripts/playtest-desktop.py", "maps/*.json", "docs/**/*",
                    "benchmarks/cloud/*.json", "fixtures/relay-desktop.jsonl"):
        for path in sorted(ROOT.glob(pattern)):
            relative = path.relative_to(ROOT)
            if path.is_file() and not any(p.startswith(".") or p == "__MACOSX" for p in relative.parts):
                yield relative.as_posix(), path
    yield from notices(metadata)


def contents(binary_dir, temp, windows, strip, metadata, toolchain_notices):
    for binary in BINS:
        filename = binary + (".exe" if windows else "")
        stripped = temp / filename
        subprocess.run([strip, "--strip-unneeded", "-o", stripped, binary_dir / filename], check=True)
        yield filename, stripped
    yield from support_files(metadata, toolchain_notices)


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


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def publish(archive, output):
    """Protect existing downloads and merge checksums for partial output directories."""
    # Both supported build hosts permit advisory locks on directory descriptors.
    # Lock the directory because atomic replacement changes the manifest's inode.
    directory_fd = os.open(output, os.O_RDONLY)
    try:
        fcntl.flock(directory_fd, fcntl.LOCK_EX)
        manifest = output / "SHA256SUMS"
        entries = {}
        if manifest.is_file():
            for line in manifest.read_text(encoding="utf-8").splitlines():
                if not line.strip():
                    continue
                entry = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](.+)", line)
                if not entry:
                    raise ValueError(f"Invalid SHA256SUMS entry: {line!r}")
                value, name = entry.groups()
                entries[name] = value
        for system, (_, _, extension) in PLATFORMS.items():
            name = bundle_name(system) + extension
            candidate = archive if name == archive.name else output / name
            if candidate.is_file():
                entries[name] = digest(candidate)
        new_manifest = archive.parent / "SHA256SUMS"
        new_manifest.write_text("".join(f"{value}  {name}\n" for name, value in entries.items()),
                                encoding="utf-8")
        archive.replace(output / archive.name)
        new_manifest.replace(manifest)
    finally:
        os.close(directory_fd)


def info_plist(version):
    # Apple's UI version keys use the numeric portion of a semantic version.
    numeric = re.fullmatch(r"(\d+\.\d+\.\d+)(?:[-+].*)?", version)
    if not numeric:
        raise ValueError(f"Invalid Sendit version: {version}")
    return plistlib.dumps({
        "CFBundleDevelopmentRegion": "en",
        "CFBundleDisplayName": "Sendit",
        "CFBundleExecutable": "sendit-client",
        "CFBundleIdentifier": BUNDLE_ID,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": "Sendit",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": numeric[1],
        "CFBundleSupportedPlatforms": ["MacOSX"],
        "CFBundleVersion": numeric[1],
        "LSApplicationCategoryType": "public.app-category.action-games",
        "LSMinimumSystemVersion": MACOS_MINIMUM,
        "NSHighResolutionCapable": True,
        "NSPrincipalClass": "NSApplication",
    })


def check_macos_binary(path):
    arch = run(["/usr/bin/lipo", "-archs", path])
    if arch != "arm64":
        raise ValueError(f"Expected a thin arm64 executable at {path}, found {arch}")
    build = run(["/usr/bin/xcrun", "vtool", "-show-build", path])
    minimum = re.search(r"^\s*minos\s+([.\d]+)\s*$", build, re.MULTILINE)
    macos = re.search(r"^\s*platform\s+(?:MACOS|1)\s*$", build, re.MULTILINE | re.IGNORECASE)
    if not macos or not minimum:
        raise ValueError(f"Missing macOS deployment information in {path}")
    if minimum[1] not in (MACOS_MINIMUM, MACOS_MINIMUM + ".0"):
        raise ValueError(f"{path} targets macOS {minimum[1]}, expected {MACOS_MINIMUM}")
    for line in run(["/usr/bin/otool", "-L", path]).splitlines()[1:]:
        dependency = line.strip().split(" (", 1)[0]
        if not dependency.startswith(("/usr/lib/", "/System/Library/")):
            raise ValueError(f"{path} links an unbundled dependency: {dependency}")


def sign(path, identifier):
    subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none",
                    "--identifier", identifier, path], check=True)


def write_macos_zip(path, stage, timestamp):
    date = datetime.fromtimestamp(timestamp, timezone.utc).timetuple()[:6]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=8) as archive:
        for source in sorted(stage.rglob("*")):
            relative = source.relative_to(stage)
            if any(p.startswith(".") or p == "__MACOSX" for p in relative.parts):
                continue
            name = relative.as_posix()
            directory = not source.is_symlink() and source.is_dir()
            entry = zipfile.ZipInfo(name + ("/" if directory else ""), date)
            entry.create_system = 3  # Unix file type and permission bits, also honored by ditto.
            if source.is_symlink():
                entry.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(entry, os.fsencode(os.readlink(source)))
            elif directory:
                entry.external_attr = ((stat.S_IFDIR | 0o755) << 16) | 0x10
                archive.writestr(entry, b"")
            else:
                mode = 0o755 if source.stat().st_mode & 0o111 else 0o644
                entry.external_attr = (stat.S_IFREG | mode) << 16
                archive.writestr(entry, source.read_bytes(), compress_type=zipfile.ZIP_DEFLATED,
                                 compresslevel=8)


def macos_archive(archive, binary_dir, temp, strip, metadata, toolchain_notices, timestamp):
    stage = temp / "stage"
    app = stage / "Sendit.app"
    contents = app / "Contents"
    executables = contents / "MacOS"
    resources = contents / "Resources"
    executables.mkdir(parents=True)
    resources.mkdir()
    version = next(p["version"] for p in metadata["packages"]
                   if p["name"] == "sendit-client" and not p["source"])
    plist = contents / "Info.plist"
    plist.write_bytes(info_plist(version))
    plist.chmod(0o644)
    for name, source in support_files(metadata, toolchain_notices):
        # Notices stay with the client when the user moves just the app to Applications.
        base = resources if name.startswith("notices/") else stage
        destination = base / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(source, Path):
            shutil.copyfile(source, destination)
        else:
            destination.write_bytes(source)
        destination.chmod(0o644)
    cli = [name for name in BINS if name != "sendit-client"]
    for binary in BINS:
        dest = (executables if binary == "sendit-client" else stage) / binary
        subprocess.run([strip, "-x", "-o", dest, binary_dir / binary], check=True)
        dest.chmod(0o755)
        check_macos_binary(dest)
        if binary in cli:
            sign(dest, f"{BUNDLE_ID}.{binary}")
    sign(app, BUNDLE_ID)
    write_macos_zip(archive, stage, timestamp)

    # Verify the actual download after native ZIP extraction. Running from an
    # empty directory catches accidental dependencies on a checkout or maps/.
    extracted = temp / "extracted"
    subprocess.run(["/usr/bin/ditto", "-x", "-k", archive, extracted], check=True)
    isolated = temp / "empty"
    isolated.mkdir()
    restored = extracted / "Sendit.app"
    for path in [restored, *(extracted / name for name in cli)]:
        subprocess.run(["/usr/bin/codesign", "--verify", "--strict", "--verbose=2", path], check=True)
    for path in [restored / "Contents/MacOS/sendit-client", *(extracted / name for name in cli)]:
        if stat.S_IMODE(path.stat().st_mode) & 0o111 != 0o111:
            raise ValueError(f"Executable permission missing after extraction: {path}")
        subprocess.run([path, "--help"], cwd=isolated, check=True, stdout=subprocess.DEVNULL)
    subprocess.run([extracted / "sendit-server", "--bind", "127.0.0.1:0", "--mode", "deathmatch",
                    "--map", "aim", "--bots", "1", "--ticks", "4"],
                   cwd=isolated, check=True, timeout=20, stdout=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--platform", choices=PLATFORMS, default="linux")
    parser.add_argument("--target", help="Rust target triple (must match --platform)")
    parser.add_argument("--strip", help="target strip executable")
    parser.add_argument("--toolchain-notice", type=Path, action="append", default=[],
                        help="redistribution notice for a linked toolchain component")
    parser.add_argument("--out", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    target, _, extension = PLATFORMS[args.platform]
    if args.target and args.target != target:
        parser.error(f"--platform {args.platform} requires --target {target}")
    macos = args.platform == "macos"
    windows = args.platform == "windows"
    if macos:
        if platform.system() != "Darwin" or (platform.machine() != "arm64" and
                                              run(["/usr/sbin/sysctl", "-n", "hw.optional.arm64"]) != "1"):
            parser.error("the macOS package must be built, signed and tested on an Apple Silicon Mac")
    elif platform.system() != "Linux" or platform.machine() != "x86_64":
        parser.error("Linux and Windows packages use the native x86_64 Linux build host")
    explicit_target = macos or windows or bool(args.target)
    command = [args.cargo, "build", "--release", "--locked", "--workspace"]
    if explicit_target:
        command.extend(["--target", target])
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET=MACOS_MINIMUM) if macos else None
    subprocess.run(command, cwd=ROOT, check=True, env=env)
    data = json.loads(run([args.cargo, "metadata", "--locked", "--format-version", "1",
                           "--filter-platform", target]))
    timestamp = int(run(["git", "log", "-1", "--format=%ct"]))
    output = args.out.resolve()
    output.mkdir(exist_ok=True, parents=True)
    bundle = bundle_name(args.platform)
    filename = bundle + extension
    # Prepare a complete archive first so an interrupted build preserves the old download.
    with tempfile.TemporaryDirectory(dir=output) as tmp:
        temp = Path(tmp)
        archive = temp / filename
        target_dir = Path(data["target_directory"])
        binary_dir = (target_dir / target if explicit_target else target_dir) / "release"
        if macos:
            macos_archive(archive, binary_dir, temp, args.strip or "/usr/bin/strip",
                          data, args.toolchain_notice, timestamp)
        else:
            strip = args.strip or ("x86_64-w64-mingw32-strip" if windows else "strip")
            files = contents(binary_dir, temp, windows, strip, data, args.toolchain_notice)
            write_archive(archive, bundle, files, timestamp, windows)
        publish(archive, output)
    print(output / filename)


if __name__ == "__main__":
    main()
