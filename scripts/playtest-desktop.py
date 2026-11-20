#!/usr/bin/env python3
"""Play the release client through a real X11 window and save evidence.

    cargo build --release --locked --workspace
    python3 scripts/playtest-desktop.py --output-dir /tmp/sendit-playtest

On Debian/Ubuntu install xvfb, x11-utils, libxtst6, tesseract-ocr-eng,
mesa-vulkan-drivers, python3-pil and python3-numpy. Xvfb starts automatically.
Pass --display :N to use a dedicated GPU-backed X11 display instead.
"""
import argparse
import contextlib
import csv
import ctypes
import io
import json
import math
import os
from pathlib import Path
import re
import select
import shutil
import socket
import subprocess
import sys
import tempfile
import time

import numpy as np
from PIL import Image, ImageGrab, ImageOps


ROOT = Path(__file__).resolve().parents[1]
now = time.monotonic


def wait_for(description, check, seconds=12, interval=0.06):
    deadline = now() + seconds
    while True:
        value = check()
        if value:
            return value
        if now() >= deadline:
            raise RuntimeError(f"Timed out waiting for {description}")
        time.sleep(interval)


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=4)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def start_display(stack, name, env):
    if name:
        return name
    proc = subprocess.Popen(
        ["Xvfb", "-displayfd", "1", "-screen", "0", "1280x720x24", "-nolisten", "tcp"],
        stdout=subprocess.PIPE, env=env,
    )
    stack.callback(stop, proc)
    if not select.select([proc.stdout], [], [], 12)[0]:
        raise RuntimeError("Xvfb did not open an X display")
    number = proc.stdout.readline().decode().strip()
    if not number.isdigit():
        raise RuntimeError("Xvfb did not report a display number")
    return ":" + number


class Desktop:
    """XTEST injects the same key, button and raw mouse events as normal input."""
    def __init__(self, name):
        self.name = name
        self.env = dict(os.environ, DISPLAY=name)
        self.x = ctypes.CDLL("libX11.so.6")
        self.xt = ctypes.CDLL("libXtst.so.6")
        self.x.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.x.XOpenDisplay.restype = ctypes.c_void_p
        self.x.XStringToKeysym.argtypes = [ctypes.c_char_p]
        self.x.XStringToKeysym.restype = ctypes.c_ulong
        self.x.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        self.x.XKeysymToKeycode.restype = ctypes.c_uint
        self.conn = ctypes.c_void_p(self.x.XOpenDisplay(name.encode()))
        if not self.conn.value:
            raise RuntimeError(f"Cannot open X display {name}")
        self.window = None
        self.geometry = None

    def command(self, *args):
        return subprocess.check_output(args, env=self.env, text=True, stderr=subprocess.DEVNULL)

    def find(self, client):
        if client.poll() is not None:
            raise RuntimeError(f"Client exited with {client.returncode} before opening its window")
        tree = self.command("xwininfo", "-root", "-tree")
        for wid in re.findall(r'(0x[0-9a-fA-F]+) "Sendit"', tree):
            if self.command("xprop", "-id", wid, "_NET_WM_PID").split()[-1] != str(client.pid):
                continue
            info = self.command("xwininfo", "-id", wid)
            fields = ["Absolute upper-left X", "Absolute upper-left Y", "Width", "Height"]
            self.geometry = tuple(int(re.search(k + r":\s*(-?\d+)", info)[1]) for k in fields)
            self.window = ctypes.c_ulong(int(wid, 16))
            self.x.XRaiseWindow(self.conn, self.window)
            self.x.XSetInputFocus(self.conn, self.window, 1, 0)
            self.sync()
            return True
        return False

    def sync(self):
        self.x.XSync(self.conn, 0)

    def frame(self):
        x, y, w, h = self.geometry
        return ImageGrab.grab(bbox=(x, y, x + w, y + h), xdisplay=self.name).convert("RGB")

    def point(self, x, y):
        self.x.XWarpPointer(self.conn, 0, self.window, 0, 0, 0, 0, int(x), int(y))
        self.sync()

    def move(self, dx, dy):
        self.xt.XTestFakeRelativeMotionEvent(self.conn, int(dx), int(dy), 0)
        self.sync()

    def key(self, name, down):
        sym = self.x.XStringToKeysym(name.encode())
        code = self.x.XKeysymToKeycode(self.conn, sym)
        if not code:
            raise RuntimeError(f"X keyboard lacks key {name}")
        self.xt.XTestFakeKeyEvent(self.conn, code, int(down), 0)
        self.sync()

    def press(self, key, duration=0.12):
        self.key(key, True)
        time.sleep(duration)
        self.key(key, False)

    def button(self, down, number=1):
        self.xt.XTestFakeButtonEvent(self.conn, number, int(down), 0)
        self.sync()

    def click(self, point):
        self.point(*point)
        time.sleep(0.12)
        self.button(True)
        time.sleep(0.14)
        self.button(False)


def ocr(image, psm=6, boxes=False):
    scale = 2 if max(image.size) < 550 or boxes else 1
    image = ImageOps.autocontrast(image.convert("L")).resize((image.width * scale, image.height * scale))
    data = io.BytesIO()
    image.save(data, format="PPM")
    args = ["tesseract", "stdin", "stdout", "-l", "eng", "--psm", str(psm)]
    if boxes:
        args.append("tsv")
    result = subprocess.run(args, input=data.getvalue(), capture_output=True, check=True, timeout=14,
                            env=dict(os.environ, OMP_THREAD_LIMIT="1"))
    return result.stdout.decode(errors="replace").strip()


def panel_button(image, word):
    """Find the visible label; the two passes handle pale text and filled amber buttons."""
    w, h = image.size
    left, top, right, bottom = int(w * .20), int(h * .12), int(w * .80), int(h * .94)
    if word != "Click":
        left, top, right, bottom = w//2 - 205, h//2 - 150, w//2 + 205, h//2 + 150
    panel = image.crop((left, top, right, bottom))
    rgb = np.asarray(panel, dtype=np.int16)
    r, g, b = rgb.transpose(2, 0, 1)
    amber = (r > 210) & (g > 125) & (g < 225) & (b < 155) & (r - g > 30)
    rows = np.flatnonzero((amber.sum(axis=1) > 65))
    for group in np.split(rows, np.flatnonzero(np.diff(rows) > 1) + 1):
        if len(group) < 16:
            continue
        band = amber[group]
        xs = np.flatnonzero(band.sum(axis=0) > len(group) * .55)
        for columns in np.split(xs, np.flatnonzero(np.diff(xs) > 3) + 1):
            if len(columns) < 55:
                continue
            box = (int(columns[0]), int(group[0]), int(columns[-1]) + 1, int(group[-1]) + 1)
            label = ocr(panel.crop(box), 7)
            if re.search(r"\b(?:" + word + r")\b", label, re.I):
                return (left + (box[0] + box[2]) / 2, top + (box[1] + box[3]) / 2)
    for row in csv.DictReader(io.StringIO(ocr(panel, 6 if word != "Click" else 11, boxes=True)), delimiter="\t"):
        if re.fullmatch(word, row["text"].strip("!.,|:"), re.I):
            return (left + (int(row["left"]) + int(row["width"]) / 2) / 2,
                    top + (int(row["top"]) + int(row["height"]) / 2) / 2)
    return None


def trace_events(path):
    if not path.is_file():
        return []
    data = path.read_bytes()
    # Writers flush batches. A screenshot or a live read can land between writes.
    return [json.loads(line) for line in data.split(b"\n")[:-1] if line]


def samples(path):
    return [row for row in trace_events(path) if "input" in row]


def visible_change(a, b):
    w, h = a.size
    crop = (int(w * .02), int(h * .14), int(w * .98), int(h * .80))
    def pixels(image):
        return np.asarray(image.crop(crop).resize((96, 58)).convert("RGB"), dtype=float)
    return round(float(np.abs(pixels(a) - pixels(b)).mean()), 2)


def moving_rival(before, after):
    """Locate one moving silhouette on the private Drill lane, at half resolution."""
    w, h = after.size
    a = np.asarray(before.resize((w//2, h//2)), dtype=np.int16)
    b = np.asarray(after.resize((w//2, h//2)), dtype=np.int16)
    delta = np.abs(a - b).max(axis=2) > 26
    height, width = delta.shape
    delta[:int(.23*height)] = False
    delta[int(.76*height):] = False
    # Exclude the held weapon and the fixed feedback message near the reticle.
    delta[int(.55*height):, int(.62*width):] = False
    delta[int(.56*height):int(.62*height), int(.34*width):int(.66*width)] = False
    delta[:,:int(.03*width)] = False
    delta[:,int(.97*width):] = False
    if delta.sum() < 12 or delta.mean() > .065:
        return None  # camera motion, a respawn or a flash, rather than one actor
    column = int(np.convolve(delta.sum(axis=0), np.ones(12), mode="same").argmax())
    left, right = max(0, column-12), min(width, column+13)
    yy, xx = np.nonzero(delta[:, left:right])
    if len(xx) < 12 or yy.max()-yy.min() < 5:
        return None
    return (2*(left + np.median(xx)), 2*np.percentile(yy, 39), max(8, np.ptp(xx)))


class Playtest:
    def __init__(self, args, desktop, env, scratch):
        self.bin = args.bin_dir.resolve()
        self.out = args.output_dir.resolve()
        self.out.mkdir(parents=True, exist_ok=True)
        self.scratch = scratch
        self.env = env
        self.desk = desktop
        self.report = {"passed": False, "checks": [], "screenshots": []}
        self.client = None

    def check(self, name, condition, **evidence):
        self.report["checks"].append({"check": name, "passed": bool(condition), **evidence})
        self.write()
        print(("PASS " if condition else "FAIL ") + name, flush=True)
        if not condition:
            raise RuntimeError(f"{name}: {evidence}")

    def write(self):
        (self.out / "report.json").write_text(json.dumps(self.report, indent=2) + "\n")

    def photo(self, name, frame=None):
        image = self.desk.frame() if frame is None else frame
        path = self.out / (name + ".png")
        image.save(path)
        self.report["screenshots"].append(path.name)
        return image

    def process(self, stack, binary, arguments, label):
        log = self.out / (label + ".log")
        stream = stack.enter_context(log.open("wb"))
        proc = subprocess.Popen([self.bin / binary, *arguments], cwd=self.scratch,
                                stdin=subprocess.PIPE, stdout=stream, stderr=subprocess.STDOUT,
                                env=self.env)
        stack.callback(stop, proc)
        return proc, log

    def log_match(self, proc, path, pattern, seconds=25):
        def match():
            if proc.poll() is not None:
                raise RuntimeError(f"{proc.args[0]} exited {proc.returncode}; see {path.name}")
            return re.search(pattern, path.read_text(errors="replace"))
        return wait_for(pattern, match, seconds)

    def window(self, stack, label, *args):
        self.client, log = self.process(stack, "sendit-client", ["--mute", *args], label + "-client")
        wait_for("native Sendit window", lambda: self.desk.find(self.client), 45)
        def failed(kind, error, traceback):
            if kind and self.client.poll() is None:
                self.photo(label + "-failure")
        stack.push(failed)
        return self.client, log

    def button(self, label):
        return wait_for(f"visible {label} button",
                        lambda: panel_button(self.desk.frame(), label), seconds=12)

    def quit(self):
        self.desk.press("Escape")
        self.desk.click(self.button("Quit"))
        self.check("Quit exits the native application", self.client.wait(timeout=12) == 0)

    def host(self, stack, label, mode, *args):
        trace = self.scratch / (label + "-server.jsonl")
        host, log = self.process(stack, "sendit-server",
                                 ["--bind", "127.0.0.1:0", "--mode", mode, "--map", "drill",
                                  "--record", trace, "--report-ticks", "0", *args], label + "-host")
        addr = self.log_match(host, log, r"SENDIT_READY addr=(127\.0\.0\.1:\d+)")[1]
        stack.callback(lambda: shutil.copy2(trace, self.out / trace.name) if trace.exists() else None)
        client, _ = self.window(stack, label, "--connect", addr, "--name", "PlayerQA",
                                "--width", "1280", "--height", "720")
        self.log_match(host, log, r'join id=\d+ name="PlayerQA"')
        wait_for("server-recorded native commands", lambda: len(samples(trace)) > 12, 15)
        self.desk.click((self.desk.geometry[2] // 2, self.desk.geometry[3] // 2))
        return host, trace

    def quickplay(self):
        with contextlib.ExitStack() as stack:
            trace = self.scratch / "quickplay-input.jsonl"
            client, log = self.window(stack, "quickplay", "--record", trace)
            self.log_match(client, log, r"QUICKPLAY_READY", 70)
            self.button("Click")
            menu = self.photo("quickplay-welcome")
            w, h = menu.size
            content = ocr(menu.crop((w//2-225,h//2-112,w//2+225,h//2+82)), 6)
            self.check("Default lobby shows five rivals and 40% skill",
                       re.search(r"5\s*bots", content, re.I) and re.search(r"40\s*%", content),
                       text=content)
            self.desk.click(self.button("Click"))
            addr = self.log_match(client, log, r"QUICKPLAY_STARTED addr=(127\.0\.0\.1:\d+)")[1]
            joined = self.log_match(client, log, r"QUICKPLAY_PLAYING id=(\d+) players=(\d+)")
            self.check("One click joins the six-player Aim match", int(joined[2]) == 6,
                       address=addr, players=int(joined[2]))
            time.sleep(.3)
            self.photo("aim-first-person")
            self.desk.move(-115, 21)
            self.desk.press("d", .32)
            self.photo("aim-oblique")
            # The normal human capture emits a fresh, server-confirmed state when life changes.
            def respawn():
                for row in trace_events(trace):
                    player = row.get("state", {})
                    if player.get("deaths", 0) and player.get("hp", 0) > 0:
                        return player
            player = wait_for("a bot eliminating the player and the player respawning", respawn, 65)
            self.check("Bot damage causes death and a live respawn", player["life"] > 1,
                       life=player["life"], hp=player["hp"], deaths=player["deaths"])
            self.photo("aim-respawn")
            self.desk.key("Tab", True)
            time.sleep(.38)
            self.photo("aim-scoreboard")
            self.desk.key("Tab", False)
            self.quit()
            shutil.copy2(trace, self.out / trace.name)
            ip, port = addr.split(":")
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
                udp.bind((ip, int(port)))
            self.check("Quit releases the private host port", True, address=addr)
        with contextlib.ExitStack() as stack:
            client, log = self.window(stack, "welcome-quit")
            self.log_match(client, log, r"QUICKPLAY_READY", 70)
            self.desk.click(self.button("Quit"))
            rc = client.wait(timeout=12)
            self.check("Welcome Quit exits before starting a host", rc == 0 and
                       "QUICKPLAY_STARTED" not in log.read_text(), exit_code=rc)

    def controls(self):
        with contextlib.ExitStack() as stack:
            host, trace = self.host(stack, "controls", "practice")
            d = self.desk
            before = d.frame()
            d.key("w", True)
            time.sleep(.36)
            d.press("space")
            d.key("w", False)
            d.press("d", .42)
            d.move(135, 28)
            time.sleep(.75)
            after = self.photo("drill-look-and-move")
            commands = [r["input"] for r in wait_for("movement delivered to server",
                        lambda: samples(trace) if any(row["input"].get("strafe") for row in samples(trace)) else None)]
            change = visible_change(before, after)
            self.check("WASD, jump and mouse look reach the host and change the view",
                       any(c["forward"] > .5 for c in commands) and any(c["jump"] for c in commands)
                       and any(c["strafe"] > .5 for c in commands)
                       and max(abs(c["pitch"]) for c in commands) > .01 and change > 1.2,
                       world_pixel_change=change)
            weapons = [("Knife", "KNIFE"), ("Pistol", "PISTOL"), ("M4", "M4"),
                       ("Ak", "AK"), ("Sniper", "SNIPER"), ("Smg", "SMG")]
            for key, (weapon, label) in enumerate(weapons, 1):
                d.press(str(key))
                def equipped():
                    im = d.frame()
                    w, h = im.size
                    text = ocr(im.crop((int(w * .82), int(h * .84), w, h)), 6)
                    return (im, text) if re.search(r"\b" + label + r"\b", text, re.I) else None
                im, text = wait_for(f"{label} on the rendered weapon HUD", equipped, 8)
                self.photo("weapon-" + weapon.lower(), im)
                recorded = wait_for(f"host receiving slot {key}", lambda: any(
                    str(row["input"].get("equip")).lower() == weapon.lower() for row in samples(trace)), 5)
                self.check(f"Key {key} equips {label}", recorded, rendered=text)
                if weapon == "Sniper":
                    time.sleep(.55)
                    hip = d.frame()
                    d.button(True, 3)
                    time.sleep(.55)
                    aimed = self.photo("weapon-sniper-scoped")
                    d.button(False, 3)
                    time.sleep(.35)
                    change = visible_change(hip, aimed)
                    restored = visible_change(hip, d.frame())
                    self.check("Holding and releasing RMB scopes the sniper",
                               change > 6 and restored < change * .55,
                               scope_pixel_change=change, release_pixel_change=restored)
            d.press("Escape")
            self.button("Resume")
            self.photo("pause-menu")
            # Choose a spot outside the panel for attempted firing while the menu is open.
            d.point(24, 24)
            time.sleep(.3)
            start = samples(trace)[-1]["tick"] + 8
            d.key("w", True)
            d.button(True)
            d.button(True, 3)
            d.move(100, 24)
            d.press("2")
            time.sleep(1.25)
            d.key("w", False)
            d.button(False)
            d.button(False, 3)
            rows = wait_for("neutral commands over a full second while paused", lambda: [
                r for r in samples(trace) if start <= r["tick"] < start + 128
            ] if samples(trace)[-1]["tick"] >= start + 128 else None, 8)
            cs = [r["input"] for r in rows]
            stable = (max(c["yaw"] for c in cs) - min(c["yaw"] for c in cs) < .001 and
                      max(c["pitch"] for c in cs) - min(c["pitch"] for c in cs) < .001)
            neutral = all(not any(c.get(a) for a in ("forward", "strafe", "jump", "fire",
                                                      "reload", "use_action", "buy", "drop", "equip"))
                          for c in cs)
            self.check("Escape menu suppresses movement, looking, firing and switching",
                       len(rows) > 60 and stable and neutral, sampled_ticks=len(rows))
            d.click(self.button("Resume"))
            d.press("w", .30)
            time.sleep(.25)
            resumed = wait_for("W reaches the host after Resume", lambda: any(
                r["tick"] >= start + 128 and r["input"]["forward"] > .5 for r in samples(trace)))
            self.check("Clicking Resume restores player control", resumed)
            self.quit()
            host.stdin.write(b"quit\n")
            host.stdin.flush()
            self.check("Dedicated host shuts down normally", host.wait(timeout=8) == 0)

    def duel(self):
        with contextlib.ExitStack() as stack:
            host, trace = self.host(stack, "duel", "deathmatch", "--max-players", "2",
                                    "--bots", "1", "--bot-team", "ct", "--bot-skill", "0")
            d = self.desk
            # The opponent moves against a fixed range backdrop. Compare two
            # actual frames while our camera is still; no bot pose is available
            # to this control loop. This case only drives the private local host.
            d.press("5")
            time.sleep(.52)
            deadline = now() + 60
            last_seen = now()
            last_score = last_reload = last_shot = 0
            score = ""
            attempts = 0
            while now() < deadline:
                first = d.frame()
                time.sleep(.10)
                frame = d.frame()
                target = moving_rival(first, frame)
                if target is not None:
                    x, y, extent = target
                    dx, dy = x - frame.width/2, y - frame.height/2
                    focal = frame.height / (2 * math.tan(math.radians(73 / 2)))
                    # Align ordinary raw mouse counts with the projected torso.
                    d.move(round(math.atan(dx / focal) * 485), round(math.atan(dy / focal) * 485))
                    last_seen = now()
                    if abs(dx) < max(24, extent) and abs(dy) < max(21, extent) and now() - last_shot > .29:
                        if attempts == 0:
                            self.photo("duel-opponent", frame)
                        d.button(True)
                        time.sleep(.11)
                        d.button(False)
                        last_shot = now()
                        attempts += 1
                elif now() - last_seen > 1.2:
                    d.move(300, 0)
                    last_seen = now() - .84
                if now() - last_score >= .8:
                    w, h = frame.size
                    crop = frame.crop((int(w*.37), 9, int(w*.63), int(h*.09)))
                    score = ocr(crop, 7)
                    if re.search(r"\b[1-9]\d*\s*KILLS?\b", score, re.I):
                        self.photo("duel-combat", frame)
                        break
                    last_score = now()
                if attempts and attempts % 5 == 0 and now() - last_reload > 4:
                    d.press("r")
                    last_reload = now()
                time.sleep(.12)
            else:
                self.photo("duel-no-frag")
                raise RuntimeError(f"No confirmed frag after {attempts} shots at a visible bot; HUD {score!r}")
            d.key("Tab", True)
            time.sleep(.35)
            board = self.photo("duel-scoreboard")
            d.key("Tab", False)
            text = ocr(board, 6)
            self.check("A real mouse-triggered shot earns a bot kill",
                       bool(re.search("PlayerQA", text.replace(" ", ""), re.I)) and
                       bool(re.search("bot", text, re.I)) and
                       any(r["input"]["fire"] for r in samples(trace)),
                       hud=score, scoreboard=text)
            self.quit()
            host.stdin.write(b"quit\n")
            host.stdin.flush()
            host.wait(timeout=8)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bin-dir", type=Path, default=ROOT / "target/release")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "target/desktop-playtest")
    parser.add_argument("--display", help="use this dedicated X display instead of starting Xvfb")
    parser.add_argument("--case", choices=["all", "quickplay", "controls", "duel"], default="all")
    args = parser.parse_args()
    with contextlib.ExitStack() as stack:
        scratch = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="sendit-players-")))
        env = dict(os.environ)
        env["DISPLAY"] = start_display(stack, args.display, env)
        env["WINIT_UNIX_BACKEND"] = "x11"
        env.pop("WAYLAND_DISPLAY", None)
        env.setdefault("XDG_RUNTIME_DIR", str(scratch))
        env.setdefault("LP_NUM_THREADS", "3")
        env.setdefault("OMP_THREAD_LIMIT", "1")
        desktop = Desktop(env["DISPLAY"])
        stack.callback(desktop.x.XCloseDisplay, desktop.conn)
        test = Playtest(args, desktop, env, scratch)
        try:
            for case in ("quickplay", "controls", "duel"):
                if args.case in ("all", case):
                    getattr(test, case)()
            test.report["passed"] = True
        except (RuntimeError, OSError, subprocess.SubprocessError, ValueError) as error:
            if desktop.geometry and not any(p.endswith("failure.png") for p in test.report["screenshots"]):
                test.photo("failure")
            test.report["error"] = str(error)
            print(f"FAIL {error}", file=sys.stderr)
        finally:
            test.write()
            print(test.out / "report.json", flush=True)
        return 0 if test.report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
