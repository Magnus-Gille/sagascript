#!/usr/bin/env python3
"""POSIX pipe regressions for the engine-host conformance JSONL reader."""

import importlib.util
import os
import signal
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(os.environ.get("ENGINE_HOST_CONFORMANCE_SCRIPT", Path(__file__).with_name("engine-host-conformance.py")))
SPEC = importlib.util.spec_from_file_location("engine_host_conformance", SCRIPT)
CONFORMANCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CONFORMANCE)
Host = CONFORMANCE.Host


FAKE_HOST = r'''#!/usr/bin/env python3
import json
import os
import sys
import time

MODE = {mode!r}


def response(request, **fields):
    result = {{"id": request["id"], "ok": True}}
    result.update(fields)
    return (json.dumps(result, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")


def write(data):
    os.write(sys.stdout.fileno(), data)


requests = []
for wire in sys.stdin:
    request = json.loads(wire)
    requests.append(request)
    if MODE in ("coalesced", "out-of-order"):
        if len(requests) == 2:
            lines = [response(requests[0]), response(requests[1])]
            if MODE == "out-of-order":
                lines.reverse()
            write(b"".join(lines))
            time.sleep(3)
            break
    elif MODE == "fragmented":
        payload = response(request, text="å")
        marker = "å".encode("utf-8")
        split = payload.index(marker) + 1
        write(payload[:split])
        time.sleep(0.03)
        write(payload[split:])
        break
    elif MODE == "partial":
        write(b'{{"id":1')
        time.sleep(3)
        break
    elif MODE == "silent":
        time.sleep(3)
        break
    elif MODE == "eof-final":
        write(response(request).rstrip(b"\n"))
        break
    elif MODE == "eof-empty":
        break
    elif MODE == "malformed-json":
        write(b"not-json\n")
        break
    elif MODE == "malformed-utf8":
        write(b"\xff\n")
        break
'''


def make_fake_host(directory, mode):
    path = Path(directory) / "fake-engine-host.py"
    path.write_text(FAKE_HOST.format(mode=mode), encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def worker(mode):
    with tempfile.TemporaryDirectory(prefix="sagascript-conformance-reader-") as directory:
        host = Host(str(make_fake_host(directory, mode)))
        try:
            first = host.send("ping")
            if mode in ("coalesced", "out-of-order"):
                second = host.send("status")
                first_response = host.wait_for(first, 1)
                second_response = host.wait_for(second, 1)
                if first_response.get("id") != first or second_response.get("id") != second:
                    raise AssertionError(f"responses were not matched by id: {first_response!r}, {second_response!r}")
            elif mode == "fragmented":
                response = host.wait_for(first, 1)
                if response.get("text") != "å":
                    raise AssertionError(f"fragmented response was decoded incorrectly: {response!r}")
            elif mode == "eof-final":
                response = host.wait_for(first, 1)
                if response.get("id") != first:
                    raise AssertionError(f"final response id mismatch: {response!r}")
            else:
                expected = {
                    "partial": "timed out waiting for host response",
                    "silent": "timed out waiting for host response",
                    "eof-empty": "host exited before response",
                    "malformed-json": "stdout was not JSONL",
                    "malformed-utf8": "stdout was not UTF-8",
                }[mode]
                try:
                    host.wait_for(first, 0.5)
                except AssertionError as error:
                    message = str(error)
                    if expected not in message:
                        raise AssertionError(f"failure lacked {expected!r}: {message}") from error
                    if mode in ("partial", "silent") and ("id 1" not in message or "op 'ping'" not in message):
                        raise AssertionError(f"timeout lacked request context: {message}") from error
                else:
                    raise AssertionError(f"{mode} unexpectedly produced a response")
        finally:
            if host.process.poll() is None:
                host.process.kill()
            host.process.wait()


class ConformanceReaderTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix", "select cannot monitor subprocess pipes on Windows")
    def run_worker(self, mode, timeout=2):
        process = subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), "--worker", mode],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
            text=True,
            start_new_session=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
            self.fail(f"{mode} worker exceeded outer timeout: {error}")
        self.assertEqual(process.returncode, 0, f"{mode} worker failed:\nstdout={stdout}\nstderr={stderr}")

    def test_coalesced_lines_are_drained_from_raw_buffer(self):
        self.run_worker("coalesced")

    def test_out_of_order_pending_responses_survive_coalesced_write(self):
        self.run_worker("out-of-order")

    def test_fragmented_utf8_and_json_wait_for_complete_line(self):
        self.run_worker("fragmented")

    def test_partial_line_timeout_is_bounded(self):
        self.run_worker("partial")

    def test_silent_timeout_has_request_context(self):
        self.run_worker("silent")

    def test_final_non_newline_line_is_accepted_at_eof(self):
        self.run_worker("eof-final")

    def test_eof_before_response_is_clear(self):
        self.run_worker("eof-empty")

    def test_malformed_json_is_clear(self):
        self.run_worker("malformed-json")

    def test_malformed_utf8_is_clear(self):
        self.run_worker("malformed-utf8")


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--worker":
        try:
            worker(sys.argv[2])
        except (AssertionError, OSError, subprocess.SubprocessError) as error:
            print(error, file=sys.stderr)
            sys.exit(1)
    else:
        unittest.main()
