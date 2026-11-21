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


# Leto's accent, the fill of its main buttons (`leto_core::brand`).
ACCENT = "#FF6B35"


def accent(path):
    """The centre of the largest patch of the accent colour, if any."""
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
    found = words(path)
    print("read:", " ".join(word for word, _, _ in found))
    if command == "pin":
        for word, _, _ in found:
            if re.fullmatch(r"\d{4}", word):
                print(word)
                return 0
        digits = [word for word, _, _ in found if re.fullmatch(r"\d", word)]
        if len(digits) >= 4:
            print("".join(digits[:4]))
            return 0
        return 1
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
