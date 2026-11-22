#!/usr/bin/env python3
"""Drive a schnellui Android app over adb. See SKILL.md beside this file.

Device: ANDROID_SERIAL, as adb reads it. App: DRIVE_PKG.
Every command that sends input runs the guard first and exits 3 when the app
is not the foreground app or the device is asleep.
"""
import os
import re
import shutil
import subprocess
import sys
import time

PKG = os.environ.get("DRIVE_PKG", "io.github.codefionn.schnellui.demo")
DUMP = "/sdcard/schnellui-drive.xml"
STOP = 3
NO_DUMP = 4


def adb_path():
    for root in (os.environ.get("ANDROID_HOME"), os.path.expanduser("~/Android/Sdk")):
        if root and os.path.exists(os.path.join(root, "platform-tools", "adb")):
            return os.path.join(root, "platform-tools", "adb")
    found = shutil.which("adb")
    if not found:
        sys.exit("adb not found: set ANDROID_HOME")
    return found


ADB = adb_path()


def adb(*args, binary=False):
    out = subprocess.run([ADB, *args], capture_output=True, timeout=60).stdout
    return out if binary else out.decode("utf-8", "replace")


def shell(command):
    return adb("shell", command)


def guard():
    top, _, rest = shell(
        "dumpsys activity activities | grep topResumedActivity | head -1; echo;"
        " dumpsys power | grep mWakefulness= | head -1;"
        " dumpsys window | grep -o 'isKeyguardShowing=[a-z]*' | head -1"
    ).partition("\n")
    power = lock = rest
    if PKG + "/" not in top:
        stop("foreground is not " + PKG + ": " + (top.strip() or "nothing"))
    if "Awake" not in power:
        stop("device is asleep")
    if "isKeyguardShowing=true" in lock:
        stop("device is locked")


def stop(why):
    print("STOP: " + why, file=sys.stderr)
    sys.exit(STOP)


def size():
    match = re.search(r"cur=(\d+)x(\d+)", shell("dumpsys window displays"))
    return int(match.group(1)), int(match.group(2))


ATTR = re.compile(r' ([\w-]+)="([^"]*)"')
ENTITY = {"&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&apos;": "'"}


def unescape(text):
    text = re.sub(r"&#(\d+);", lambda m: chr(int(m.group(1))), text)
    return re.sub(r"&\w+;", lambda m: ENTITY.get(m.group(0), m.group(0)), text)


def nodes(required=True):
    """The views uiautomator reports, top to bottom. None when it gives no dump."""
    xml = ""
    for _ in range(2):
        # The old file goes first, so a failed dump cannot be read as a fresh one.
        xml = shell(f"rm -f {DUMP}; uiautomator dump {DUMP} 2>&1; cat {DUMP} 2>/dev/null")
        if "<node" in xml:
            break
        time.sleep(0.5)
    else:
        if not required:
            return None
        print("NO DUMP: " + " ".join(xml.split())[:200], file=sys.stderr)
        sys.exit(NO_DUMP)
    found = []
    for tag in re.findall(r"<node[^>]*>", xml):
        attrs = {key: unescape(value) for key, value in ATTR.findall(tag)}
        box = re.match(r"\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]", attrs.get("bounds", ""))
        if not box:
            continue
        found.append({
            "cls": attrs.get("class", "").rsplit(".", 1)[-1],
            "text": attrs.get("text", ""),
            "desc": attrs.get("content-desc", ""),
            "checked": attrs.get("checked") == "true",
            "selected": attrs.get("selected") == "true",
            "enabled": attrs.get("enabled") == "true",
            "pkg": attrs.get("package", ""),
            "box": tuple(int(part) for part in box.groups()),
        })
    return found


def matches(node, want, cls):
    if cls and node["cls"] != cls:
        return False
    return want in (node["text"], node["desc"])


def find(all_nodes, want, cls):
    hits = [node for node in all_nodes if matches(node, want, cls)]
    return hits[0] if hits else None


def line(node):
    flags = "".join(flag for flag, on in (("c", node["checked"]), ("s", node["selected"]), ("d", not node["enabled"])) if on)
    return "\t".join((node["cls"], node["text"], node["desc"], flags or "-", ",".join(map(str, node["box"]))))


def centre(node):
    left, top, right, bottom = node["box"]
    return (left + right) // 2, (top + bottom) // 2


def signature(all_nodes):
    # Positions only: text that changes by itself, a clock, must not read as a scroll.
    return tuple((node["cls"], node["box"][1], node["box"][3]) for node in all_nodes)


def hide_keyboard():
    # BACK closes the activity when no keyboard is up, so look first.
    if "mInputShown=true" in shell("dumpsys input_method | grep -o 'mInputShown=[a-z]*' | head -1"):
        guard()
        shell("input keyevent 4")
        time.sleep(1)


def swipe(down, width, height):
    # Swipes stay in the middle of the screen, clear of the gesture edges.
    start, end = (0.75, 0.3) if down else (0.3, 0.75)
    shell(f"input swipe {width // 2} {int(height * start)} {width // 2} {int(height * end)} 400")
    time.sleep(0.4)


def show(want, cls):
    """Scrolls until the view sits in the middle of the screen, or the page ends."""
    # A swipe across the keyboard types words.
    hide_keyboard()
    width, height = size()
    down = True
    turned = False
    current = nodes(required=False)
    blind = 0
    for _ in range(40):
        guard()
        if current is None:
            # An animation is on screen. Scroll on until it has left.
            blind += 1
            if blind > 4:
                break
            swipe(down, width, height)
            current = nodes(required=False)
            continue
        blind = 0
        node = find(current, want, cls)
        if node:
            y = centre(node)[1]
            if height * 0.15 < y < height * 0.8:
                return node
            down = y >= height * 0.8
        swipe(down, width, height)
        before, current = current, nodes(required=False)
        if current is not None and signature(current) == signature(before):
            # The page did not move: this is its end.
            if node and 0 < centre(node)[1] < height * 0.95:
                return node
            if node or turned:
                break
            down, turned = not down, True
    sys.exit(f"not found: {want!r}" + (f" ({cls})" if cls else ""))


def split(args):
    """[--class Name] text"""
    cls = None
    if args and args[0] == "--class":
        cls, args = args[1], args[2:]
    if len(args) != 1:
        sys.exit(__doc__ + "\nusage: see SKILL.md")
    return args[0], cls


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: drive.py guard|dump|show|tap|tap-xy|scroll|drag|type|hide-keyboard|shot|awake ...")
    command, args = sys.argv[1], sys.argv[2:]
    if command == "guard":
        guard()
        print("ok", "x".join(map(str, size())))
    elif command == "dump":
        for node in nodes():
            if node["text"] or node["desc"] or node["cls"] in ("SeekBar", "EditText", "Spinner", "ProgressBar"):
                print(line(node))
    elif command == "show":
        print(line(show(*split(args))))
    elif command == "tap":
        node = show(*split(args))
        guard()
        x, y = centre(node)
        shell(f"input tap {x} {y}")
        time.sleep(1)
    elif command == "scroll":
        if args not in (["down"], ["up"]):
            sys.exit("usage: drive.py scroll down|up")
        hide_keyboard()
        guard()
        swipe(args == ["down"], *size())
    elif command == "tap-xy":
        guard()
        shell(f"input tap {int(args[0])} {int(args[1])}")
        time.sleep(1)
    elif command == "drag":
        # drag [--class Name] text from to: fractions of the view's width.
        want, cls = split(args[:-2])
        node = show(want, cls)
        guard()
        left, _, right, _ = node["box"]
        y = centre(node)[1]
        start = int(left + (right - left) * float(args[-2]))
        end = int(left + (right - left) * float(args[-1]))
        shell(f"input swipe {start} {y} {end} {y} 600")
        time.sleep(1)
    elif command == "type":
        guard()
        if not re.fullmatch(r"[A-Za-z0-9_.,@-]+", args[0]):
            sys.exit("type takes letters, digits and _.,@- only")
        shell(f"input text {args[0]}")
        time.sleep(1)
    elif command == "hide-keyboard":
        hide_keyboard()
    elif command == "shot":
        with open(args[0], "wb") as out:
            out.write(adb("exec-out", "screencap", "-p", binary=True))
    elif command == "awake":
        if args == ["on"]:
            print("was", shell("settings get global stay_on_while_plugged_in").strip())
            shell("svc power stayon usb")
        elif len(args) == 2 and args[0] == "restore" and args[1].isdigit():
            shell(f"settings put global stay_on_while_plugged_in {args[1]}")
        else:
            sys.exit("usage: drive.py awake on | awake restore <was>")
    else:
        sys.exit("unknown command: " + command)


if __name__ == "__main__":
    main()
