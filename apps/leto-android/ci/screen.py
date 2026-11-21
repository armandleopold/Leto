"""Looks at the emulator's screen for the emulator check (emulator.sh).

  screen.py shot NAME           saves the screen as shots/NAME.png
  screen.py tap NAME WORD...    saves it, finds the words and taps them
  screen.py pin NAME            saves it and prints the four-digit PIN on it

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
    subprocess.run(["convert", path, "-negate", "-colorspace", "Gray", flipped], check=True)
    table = subprocess.run(
        ["tesseract", flipped, "-", "--psm", "11", "tsv"],
        capture_output=True, text=True, check=True,
    ).stdout
    found = []
    for line in table.splitlines()[1:]:
        cells = line.split("\t")
        if len(cells) < 12 or not cells[11].strip():
            continue
        left, top, width, height = map(int, cells[6:10])
        found.append((cells[11].strip(), left + width // 2, top + height // 2))
    return found


def main():
    command, name, *wanted = sys.argv[1:]
    path = shot(name)
    if command == "shot":
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
