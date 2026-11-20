#!/usr/bin/env python3
"""Play the release client through a real X11 window and save evidence.

    cargo build --release --locked --workspace
    python3 scripts/playtest-desktop.py --output-dir /tmp/sendit-playtest

On Debian/Ubuntu install xvfb, x11-utils, libxtst6, libxkbcommon-x11-0, tesseract-ocr-eng,
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
    faces = [amber] + [np.max(np.abs(rgb - color), axis=2) <= 4
                       for color in ([100,69,36], [48,44,34])]
    for index, face in enumerate(faces):
        rows = np.flatnonzero(face.sum(axis=1) > 55)
        for group in np.split(rows, np.flatnonzero(np.diff(rows) > 1) + 1):
            if len(group) < 15:
                continue
            xs = np.flatnonzero(face[group].sum(axis=0) > len(group) * .55)
            for columns in np.split(xs, np.flatnonzero(np.diff(xs) > 3) + 1):
                if len(columns) < 50:
                    continue
                x0, x1 = int(columns[0])+2, int(columns[-1])-1
                y0, y1 = int(group[0])+2, int(group[-1])-1
                patch = rgb[y0:y1, x0:x1]
                letters = patch.max(axis=2) < 120 if index == 0 else patch.min(axis=2) > 130
                ys, xx = np.nonzero(letters)
                if len(xx) < 16:
                    continue
                glyphs = letters[ys.min():ys.max()+1, xx.min():xx.max()+1]
                ink = Image.fromarray(np.uint8(np.where(glyphs, 0, 255)))
                label = ocr(ImageOps.expand(ink, 5, 255), 7)
                if re.search(r"\b(?:" + word + r")\b", label, re.I):
                    return (left + (x0+x1)/2, top + (y0+y1)/2)
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


def selected_slot(frame):
    """Read the selected underline among the six visible, centered inventory cells."""
    w, h = frame.size
    left, right = round(w*.18), round(w*.82)
    rows = np.asarray(frame.crop((left, h-56, right, h-10)), dtype=np.int16)
    amber = np.max(np.abs(rows - [255,194,95]), axis=2) <= 7
    others = np.max(np.abs(rows - [118,101,70]), axis=2) <= 7
    for active, owned in zip(amber, others):
        xx = np.flatnonzero(active | owned)
        runs = [g for g in np.split(xx, np.flatnonzero(np.diff(xx) > 1) + 1) if len(g) >= 25]
        if len(runs) == 6:
            chosen = [i for i, cell in enumerate(runs, 1) if active[cell].mean() > .90]
            if len(chosen) == 1:
                return chosen[0]
    return None


def gun_pixels(frame):
    w, h = frame.size
    region = frame.crop((int(w*.50), int(h*.55), int(w*.92), int(h*.88)))
    return np.asarray(region.resize((80, 48)), dtype=float)


def weapon_caption(frame):
    """OCR the lower-right HUD card, excluding similarly coloured masonry behind it."""
    a = np.asarray(frame, dtype=np.int16)
    h, w = a.shape[:2]
    scale = min(1, max(.53, min(w/1060, h/610)))
    right, bottom = w-round(24*scale), h-round(24*scale)
    x, y = right-4, bottom-3
    background = a[y, x]
    left, top = x, y
    while left > right-260*scale and np.abs(a[y, left] - background).max() < 28:
        left -= 1
    while top > bottom-75*scale and np.abs(a[top, x] - background).max() < 28:
        top -= 1
    card = a[top+1:bottom, left+1:right]
    r, g, b = card.transpose(2, 0, 1)
    pale = (r > 132) & (b > 100) & (r-g >= 1) & (r-g < 24) & (g-b > 5) & (g-b < 40)
    rows = np.flatnonzero(pale.any(axis=1))
    names = []
    for group in np.split(rows, np.flatnonzero(np.diff(rows) > 1) + 1):
        if len(group) < 3:
            continue
        glyphs = pale[group]
        columns = np.flatnonzero(glyphs.any(axis=0))
        glyphs = glyphs[:,columns[0]:columns[-1]+1]
        word = Image.fromarray(np.uint8(np.where(glyphs, 0, 255)))
        names.append(ocr(ImageOps.expand(word.resize((word.width*2, word.height*2),
                                         Image.Resampling.NEAREST), 4, 255), 7))
        break
    ink = Image.fromarray(np.uint8(np.where(card.max(axis=2) > 135, 0, 255)))
    if ink.height > 38:
        ink = ink.crop((0, 0, round(ink.width*.68), round(ink.height*.53)))
    names.append(ocr(ImageOps.expand(ink, 5, 255), 7))
    return " ".join(names)



def defender_on_drill(frame):
    """Locate the blue uniform in the opposing half of this private, two-player drill."""
    w, h = frame.size
    pixels = np.asarray(frame.resize((w//2, h//2)), dtype=np.int16)
    r, g, b = pixels.transpose(2, 0, 1)
    blue = (b > g*1.12) & (g > r*1.2) & (b-r > 21) & (r > 27) & (r < 141)
    blue[:int(.46*len(blue))] = False
    blue[int(.62*len(blue)):] = False
    blue[:, :int(.38*blue.shape[1])] = False
    blue[:, int(.62*blue.shape[1]):] = False
    blue[int(.59*len(blue)):, int(.65*blue.shape[1]):] = False
    # Find the densest patch. The sight line has no other blue combatant.
    column = int(np.convolve(blue.sum(axis=0), np.ones(11), mode="same").argmax())
    left = max(0, column-8)
    yy, xx = np.nonzero(blue[:, left:column+9])
    if len(xx) < 8 or np.ptp(yy) < 3 or np.ptp(yy) > h*.17:
        return None
    return 2*(left + np.median(xx)), 2*np.percentile(yy, 42)


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
        if path.name not in self.report["screenshots"]:
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
        joined = self.log_match(host, log, r'join id=(\d+) name="PlayerQA"')
        self.player_id = int(joined[1])
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
            previous = gun_pixels(d.frame())
            for key, (weapon, label) in enumerate(weapons, 1):
                d.press(str(key))
                def equipped():
                    im = d.frame()
                    return im if selected_slot(im) == key else None
                wait_for(f"visible inventory slot {key} highlighted", equipped, 8)
                time.sleep(.5)  # Wait for the longest draw before comparing the held model.
                im = self.photo("weapon-" + weapon.lower())
                observed = selected_slot(im)
                current = gun_pixels(im)
                change = round(float(np.abs(current-previous).mean()), 3)
                previous = current
                recorded = wait_for(f"host receiving slot {key}", lambda: any(
                    str(row["input"].get("equip")).lower() == weapon.lower() for row in samples(trace)), 5)
                self.check(f"Key {key} equips {label}", recorded and observed == key and change > .3,
                           selected_slot=observed, model_pixel_change=change, rendered=weapon_caption(im))
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
            host, trace = self.host(stack, "duel", "competitive", "--max-players", "2",
                                    "--bots", "1", "--bot-team", "ct", "--bot-skill", "0")
            d = self.desk
            # Both players face the common Drill lane in the normal buy phase.
            # The defender can strafe, shoot back, and win the round.
            d.press("b")
            d.press("3")
            d.move(0, 12)
            time.sleep(.3)
            deadline = now() + 65
            last_sample = 0
            next_buy = now() + 15
            held = False
            attempts = 0
            won = None
            while now() < deadline:
                frame = d.frame()
                target = defender_on_drill(frame)
                if target is not None:
                    x, y = target
                    dx, dy = x - frame.width/2, y - frame.height/2
                    focal = frame.height / (2 * math.tan(math.radians(73/2)))
                    # Aim only within the visible lane; the team marking must
                    # stay close to the reticle before a burst is allowed.
                    if abs(dx) < 68 and abs(dy) < 30:
                        d.move(int(np.clip(math.atan(dx/focal)*255, -30, 30)),
                               int(np.clip(math.atan(dy/focal)*255, -10, 10)))
                        if not held:
                            d.button(True)
                            held = True
                            attempts += 1
                            if attempts == 1:
                                self.photo("duel-opponent", frame)
                        if now()-last_sample > 1.0:
                            self.photo("duel-combat", frame)
                            last_sample = now()
                if now() > next_buy:
                    d.press("b")
                    next_buy = now() + 8
                # A new round emits the real server's starting PlayerView; its
                # persistent K/D cannot be produced by predicted fire alone.
                won = next((row["state"] for row in trace_events(trace)
                            if row.get("state", {}).get("id") == self.player_id
                            and row["state"]["kills"] >= 1), None)
                if won:
                    break
                time.sleep(.045)
            d.button(False)
            self.check("Native mouse fire earns an authoritative kill against the bot",
                       won is not None and attempts > 0,
                       trigger_presses=attempts, kills=won["kills"] if won else 0,
                       deaths=won["deaths"] if won else None)
            d.key("Tab", True)
            time.sleep(.4)
            board = self.photo("duel-scoreboard")
            d.key("Tab", False)
            w, h = board.size
            region = board.crop((w//2-292, int(h*.115), w//2+292, int(h*.34)))
            text = ocr(region, 6)
            # Read the pale player rows independently of the warm team headers.
            glyphs = np.asarray(region).min(axis=2) > 138
            rows = Image.fromarray(np.uint8(np.where(glyphs, 0, 255)))
            row_text = ocr(ImageOps.expand(rows.resize((rows.width*2, rows.height*2)), 6, 255), 6)
            host.stdin.write(b"status\n")
            host.stdin.flush()
            roster = self.log_match(host, self.out / "duel-host.log", r"humans=1 bots=1")
            self.check("Tab shows the two-player match and its recorded result",
                       roster is not None and "ROUND" in text.upper() and
                       "PLAYER" in row_text.upper() and "BOT" in row_text.upper(),
                       player_id=self.player_id, authoritative_kills=won["kills"],
                       authoritative_deaths=won["deaths"], scoreboard=text, rows=row_text)
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
