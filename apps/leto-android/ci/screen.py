"""Looks at the emulator's screen for the emulator check (emulator.sh).

  screen.py shot NAME           saves the screen as shots/NAME.png
  screen.py tap NAME WORD...    saves it, finds the words and taps them
  screen.py pin NAME            saves it and prints the four-digit PIN on it
  screen.py accent NAME         saves it and taps the largest patch of the
                                accent colour: the main button

The app is drawn by egui, so Android sees no buttons in it: the words are
read from the picture with tesseract.
"""

import os
import glob
import re
import subprocess
import sys

SHOTS = "shots"


def shot(name):
    os.makedirs(SHOTS, exist_ok=True)
    path = os.path.join(SHOTS, name + ".png")
    with open(path, "wb") as picture:
        subprocess.run(["adb", "exec-out", "screencap", "-p"], stdout=picture, check=True)
    return path


def words(path):
    """Every word tesseract reads, with the centre of its box."""
    # The app is light text on a dark ground: tesseract reads dark on light.
    flipped = path + ".flipped.png"
    # Android's screenshots have an alpha channel, which -negate would turn
    # to fully transparent.
    subprocess.run(
        ["convert", path, "-alpha", "off", "-negate", "-colorspace", "Gray", flipped], check=True
    )
    read = subprocess.run(
        ["tesseract", flipped, "-", "--psm", "11", "tsv"],
        capture_output=True, text=True, check=True,
    )
    table = read.stdout
    if read.stderr.strip():
        print("tesseract:", read.stderr.strip())
    found = []
    for line in table.splitlines()[1:]:
        cells = line.split("\t")
        if len(cells) < 12 or not cells[11].strip():
            continue
        left, top, width, height = map(int, cells[6:10])
        found.append((cells[11].strip(), left + width // 2, top + height // 2))
    return found


FONT = "Ubuntu-Light.ttf"
SIDE = (24, 36)


def font():
    """The font the app draws text with: egui's own, in cargo's sources."""
    found = sorted(glob.glob(os.path.expanduser(
        f"~/.cargo/registry/src/*/epaint_default_fonts-*/fonts/{FONT}")))
    if not found:
        raise SystemExit(f"{FONT} is not in cargo's sources")
    return found[-1]


def pixels(command):
    """An image, from an ImageMagick command, as SIDE light-or-dark pixels."""
    raw = subprocess.run(
        command + ["-trim", "+repage", "-resize", f"{SIDE[0]}x{SIDE[1]}!",
                   "-threshold", "50%", "-depth", "8", "gray:-"],
        capture_output=True, check=True,
    ).stdout
    return [b > 127 for b in raw]


def models():
    """Each digit as the app's font draws it."""
    path = font()
    return {
        d: pixels(["convert", "-background", "black", "-fill", "white", "-font", path,
                   "-pointsize", "200", f"label:{d}"])
        for d in "0123456789"
    }


def pin(path):
    """The PIN: four large light glyphs side by side.

    Tesseract took the app's thin 5 for a 3, so each digit is cut out and
    matched against the ten digits drawn in the app's own font.
    """
    light = path + ".digits.png"
    subprocess.run(
        ["convert", path, "-alpha", "off", "-colorspace", "Gray", "-threshold", "60%", light],
        check=True,
    )
    listing = subprocess.run(
        ["convert", light, "-define", "connected-components:verbose=true",
         "-define", "connected-components:area-threshold=100",
         "-connected-components", "8", "null:"],
        capture_output=True, text=True, check=True,
    ).stdout
    width = int(subprocess.run(
        ["identify", "-format", "%w", light], capture_output=True, text=True, check=True
    ).stdout)
    glyphs = []
    for line in listing.splitlines()[1:]:
        fields = line.split()
        if len(fields) < 5 or fields[-1] not in ("srgb(255,255,255)", "gray(255)"):
            continue
        match = re.fullmatch(r"(\d+)x(\d+)\+(\d+)\+(\d+)", fields[1])
        if not match:
            continue
        w, h, x, y = map(int, match.groups())
        # A PIN digit is about a twelfth of the screen's width tall.
        if width * 0.06 < h < width * 0.15 and w < h:
            glyphs.append((y, x, w, h))
    known = None
    # The four whose tops line up, left to right.
    for top, *_ in sorted(glyphs):
        row = sorted((g for g in glyphs if abs(g[0] - top) < width * 0.02), key=lambda g: g[1])
        if len(row) != 4:
            continue
        known = known or models()
        code = ""
        for y, x, w, h in row:
            seen = pixels(["convert", light, "-crop", f"{w}x{h}+{x}+{y}", "+repage"])
            code += min(known, key=lambda d: sum(a != b for a, b in zip(known[d], seen)))
        print("digits read:", code)
        return code
    return None


# Leto's accent, the fill of its main buttons (`leto_core::brand`).
ACCENT = "#FF6B35"


def accent(path):
    """The centre of the largest button of the accent colour, if any: a
    patch wider than tall, so that the logo's round sun on the first
    start's splash is not taken for one."""
    listing = subprocess.run(
        ["convert", path, "-alpha", "off", "-fuzz", "15%", "-fill", "white", "-opaque", ACCENT,
         "-fill", "black", "+opaque", "white",
         "-define", "connected-components:verbose=true",
         "-define", "connected-components:area-threshold=200",
         "-connected-components", "8", "null:"],
        capture_output=True, text=True, check=True,
    ).stdout
    best = None
    for line in listing.splitlines()[1:]:
        fields = line.split()
        if len(fields) < 5 or "255,255,255" not in fields[-1]:
            continue
        size = fields[1].split("+")[0]
        w, h = (int(v) for v in size.split("x"))
        if w < 2 * h:
            continue
        x, y = (float(v) for v in fields[2].split(","))
        area = int(fields[3])
        if best is None or area > best[0]:
            best = (area, int(x), int(y))
    return best


def main():
    command, name, *wanted = sys.argv[1:]
    path = shot(name)
    if command == "shot":
        return 0
    if command == "accent":
        best = accent(path)
        if best is None:
            print("no button of the accent colour")
            return 1
        _, x, y = best
        subprocess.run(["adb", "shell", "input", "tap", str(x), str(y)], check=True)
        print(f"tapped the main button at {x},{y}")
        return 0
    if command == "pin":
        code = pin(path)
        if code is None:
            return 1
        print(code)
        return 0
    found = words(path)
    print("read:", " ".join(word for word, _, _ in found))
    # tap: the first of the words, where the others follow it.
    for index, (word, x, y) in enumerate(found):
        following = [w for w, _, _ in found[index : index + len(wanted)]]
        if [w.lower() for w in following] == [w.lower() for w in wanted]:
            subprocess.run(["adb", "shell", "input", "tap", str(x), str(y)], check=True)
            print(f"tapped {' '.join(wanted)} at {x},{y}")
            return 0
    print(f"{' '.join(wanted)} is not on the screen")
    return 1


if __name__ == "__main__":
    sys.exit(main())
