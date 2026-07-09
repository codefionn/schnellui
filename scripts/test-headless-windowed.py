#!/usr/bin/env python3
"""Exercise the live offscreen host and debug CLI without a desktop display.

Build first with `cargo build -p counter -p todo -p schnellui-debug`, then run
`python3 scripts/test-headless-windowed.py`. Requires a WGPU rendering adapter;
Mesa software Vulkan works on Linux CI. All session files are removed on exit.
"""

import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import time


class Session:
    def __init__(self, binaries, directory, name, app, scenario, env_mode=False,
                 autoclose_ms=30000, scale=1):
        self.directory = directory
        self.info = directory / f"{name}.json"
        self.socket = directory / f"{name}.sock"
        self.debug = binaries / "schnellui-debug"
        self.log_path = directory / f"{name}.log"
        self.log = self.log_path.open("wb")
        env = os.environ.copy()
        env.update(
            DISPLAY=":65535",
            WAYLAND_DISPLAY="schnellui-no-display",
            SCHNELLUI_DEBUG_SERVER="on",
            SCHNELLUI_DEBUG_TRANSPORT="unix",
            SCHNELLUI_DEBUG_INFO=str(self.info),
            SCHNELLUI_DEBUG_SOCKET=str(self.socket),
            SCHNELLUI_AUTOCLOSE_MS=str(autoclose_ms),
        )
        args = [str(binaries / app), "--scenario", scenario, "--scale", str(scale)]
        if env_mode:
            env["SCHNELLUI_HEADLESS_WINDOWED"] = "1"
            args.append("--windowed")
        else:
            env.pop("SCHNELLUI_HEADLESS_WINDOWED", None)
            args.append("--headless-windowed")
        self.process = subprocess.Popen(args, env=env, stdout=self.log, stderr=self.log)

    def command(self, *args):
        result = subprocess.run(
            [str(self.debug), "--info", str(self.info), *args],
            capture_output=True,
            text=True,
            timeout=20,
        )
        if result.returncode:
            raise RuntimeError(f"{' '.join(args)} failed: {result.stderr.strip()}")
        if args[0] == "screenshot":
            return None
        return json.loads(result.stdout) if result.stdout.strip() else None

    def ready(self):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"app exited during startup: {self.log_path.read_text()[-8000:]}")
            if self.info.exists():
                status = self.command("status")
                assert status["mode"] == "headless_windowed", status
                return
            time.sleep(0.02)
        raise RuntimeError(f"app did not become ready: {self.log_path.read_text()[-8000:]}")

    def screenshot(self, name, size):
        path = self.directory / name
        self.command("screenshot", str(path))
        data = path.read_bytes()
        assert data[:8] == b"\x89PNG\r\n\x1a\n", "invalid PNG signature"
        assert struct.unpack(">II", data[16:24]) == size, "wrong screenshot dimensions"

    def quit(self):
        self.command("quit")
        self.wait_closed()

    def wait_closed(self):
        assert self.process.wait(timeout=5) == 0, self.log_path.read_text()[-8000:]
        assert not self.info.exists(), "discovery file survived shutdown"
        assert not self.socket.exists(), "socket survived shutdown"

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.log.close()


def run(binaries, directory):
    sessions = []
    try:
        counter = Session(binaries, directory, "counter", "counter", "counter_zero")
        sessions.append(counter)
        other = Session(binaries, directory, "other", "counter", "counter_zero",
                        env_mode=True, scale=2)
        sessions.append(other)
        counter.ready()
        other.ready()
        counter.command("click", "--role", "button", "--name", "increment")
        counter.command("wait", "--role", "status", "--value", "1")
        other.command("wait", "--role", "status", "--value", "0")
        tree = counter.command("tree")

        def nodes(value):
            if isinstance(value, dict):
                yield value
                for child in value.values():
                    yield from nodes(child)
            elif isinstance(value, list):
                for child in value:
                    yield from nodes(child)

        button = next(node for node in nodes(tree) if node.get("name") == "increment")
        rect = button["rect"]
        x, y = rect["x"] + rect["width"] / 2, rect["y"] + rect["height"] / 2
        counter.command("move", str(x), str(y))
        counter.command("click-at", str(x), str(y))
        counter.command("wait", "--role", "status", "--value", "2")
        counter.command("action", "focus", "--role", "button", "--name", "increment")
        counter.command("key", "enter")
        counter.command("wait", "--role", "status", "--value", "3")
        counter.screenshot("counter.png", (400, 200))
        other.screenshot("scaled-counter.png", (800, 400))

        todo = Session(binaries, directory, "todo", "todo", "day_plan")
        sessions.append(todo)
        todo.ready()
        todo.command("action", "focus", "--role", "text_input", "--name", "Add a next step…")
        todo.command("type", "Test offscreen remount")
        todo.command("click", "--role", "button", "--name", "Add task")
        todo.command("wait", "--role", "checkbox", "--name", "Complete Test offscreen remount",
                     "--remount-count", "1")
        todo.screenshot("todo.png", (940, 700))
        for session in sessions:
            session.quit()

        autoclose = Session(binaries, directory, "autoclose", "counter", "counter_zero",
                            autoclose_ms=50)
        sessions.append(autoclose)
        autoclose.wait_closed()

        disabled_env = os.environ.copy()
        disabled_env.update(SCHNELLUI_DEBUG_SERVER="off", DISPLAY=":65535",
                            WAYLAND_DISPLAY="schnellui-no-display")
        disabled = subprocess.run(
            [str(binaries / "counter"), "--scenario", "counter_zero", "--headless-windowed"],
            env=disabled_env, capture_output=True, text=True, timeout=15,
        )
        assert disabled.returncode != 0, "headless host started without instrumentation"
        assert "requires debug instrumentation" in disabled.stderr, disabled.stderr
        print("Headless windowed smoke passed: isolated sessions, input, remount, scaled PNG, "
              "shutdown, autoclose, disabled instrumentation")
    finally:
        for session in sessions:
            session.close()


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, default=root / "target/debug")
    args = parser.parse_args()
    binaries = args.bin_dir.resolve()
    for name in ("counter", "todo", "schnellui-debug"):
        if not (binaries / name).is_file():
            parser.error(f"missing {binaries / name}; build the three packages first")
    scratch_root = Path.home() / ".cache/agent-tmp"
    scratch_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="schnellui-live-", dir=scratch_root) as scratch:
        run(binaries, Path(scratch))


if __name__ == "__main__":
    main()
