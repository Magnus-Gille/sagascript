#!/usr/bin/env python3
"""Protocol v1 conformance checks for a Sagascript engine host.

Only Python's standard library is used. The checks intentionally exercise the
wire behavior rather than importing or special-casing the CoreML implementation.
"""

import argparse
import json
import os
import re
import select
import struct
import subprocess
import sys
import tempfile
import time


def fail(message):
    raise AssertionError(message)


class Host:
    def __init__(self, binary, cache_dir=None):
        args = [binary, "--protocol", "1"]
        if cache_dir:
            args += ["--cache-dir", cache_dir]
        self.process = subprocess.Popen(
            args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, bufsize=1
        )
        self.next_id = 1
        self.pending = {}

    def send(self, op, **fields):
        request_id = self.next_id
        self.next_id += 1
        request = {"v": 1, "id": request_id, "op": op}
        request.update(fields)
        self.process.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        self.process.stdin.flush()
        return request_id

    def read_line(self, timeout=30):
        ready, _, _ = select.select([self.process.stdout], [], [], timeout)
        if not ready:
            fail(f"timed out waiting for host response; stderr={self.stderr_tail()!r}")
        line = self.process.stdout.readline()
        if not line:
            fail(f"host exited before response; returncode={self.process.poll()} stderr={self.stderr_tail()!r}")
        try:
            return json.loads(line)
        except json.JSONDecodeError as exc:
            fail(f"stdout was not JSONL: {line!r}: {exc}")

    def wait_for(self, request_id, timeout=30):
        if request_id in self.pending:
            return self.pending.pop(request_id)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            response = self.read_line(max(0.01, deadline - time.monotonic()))
            if response.get("id") == request_id and "ok" in response:
                return response
            if "id" in response and "ok" in response:
                self.pending[response["id"]] = response
        fail(f"timed out waiting for id {request_id}")

    def stderr_tail(self):
        if not self.process.stderr:
            return ""
        ready, _, _ = select.select([self.process.stderr], [], [], 0)
        if not ready:
            return ""
        return self.process.stderr.read()[-1000:]

    def shutdown(self):
        if self.process.stdin:
            self.process.stdin.close()
        return self.process.wait(timeout=3)


def require(condition, message):
    if not condition:
        fail(message)


def assert_success(response, request_id):
    require(response.get("id") == request_id, f"response id mismatch: {response}")
    require(response.get("ok") is True, f"request {request_id} failed: {response}")


def assert_failure(response, request_id, code):
    require(response.get("id") == request_id, f"response id mismatch: {response}")
    require(response.get("ok") is False, f"request {request_id} unexpectedly succeeded: {response}")
    require(response.get("error", {}).get("code") == code, f"expected {code}: {response}")


def hello_and_load(host, model_dir):
    hello_id = host.send("hello", client={"name": "engine-host-conformance", "version": "1", "git_sha": "0" * 40})
    hello = host.wait_for(hello_id, 10)
    assert_success(hello, hello_id)
    require(hello.get("protocol") == 1, f"wrong protocol: {hello}")
    host_info = hello.get("host", {})
    require(host_info.get("name") == "sagascript-engine-host", f"wrong host name: {hello}")
    require(re.fullmatch(r"[0-9a-f]{40}", host_info.get("git_sha", "")), f"missing full git_sha: {hello}")
    capabilities = hello.get("capabilities", {})
    require(capabilities.get("max_in_flight", 0) >= 1, f"invalid capabilities: {hello}")
    load_id = host.send(
        "load", model_dir=os.path.abspath(model_dir),
        model_id="pianissimo-sv-coreml-conformance", compute_units="ane"
    )
    load = host.wait_for(load_id, 180)
    assert_success(load, load_id)
    require(load.get("window_s", 0) > 0, f"load did not report window_s: {load}")
    require(load.get("blank_id", 0) >= 0, f"load did not report blank_id: {load}")
    return hello, load


def exercise_host(binary, model_dir, cache_dir, pcm_path, expect_exit_on_eof=False):
    host = Host(binary, cache_dir)
    try:
        hello, load = hello_and_load(host, model_dir)
        window_samples = int(round(load["window_s"] * 16_000))

        first = host.send(
            "transcribe_window", pcm_path=pcm_path, offset_samples=0,
            num_samples=window_samples, sample_rate=16_000, format="f32le", priority="batch"
        )
        second = host.send(
            "transcribe_window", pcm_path=pcm_path, offset_samples=0,
            num_samples=window_samples, sample_rate=16_000, format="f32le", priority="batch"
        )
        first_response = host.wait_for(first, 45)
        second_response = host.wait_for(second, 45)
        assert_success(first_response, first)
        assert_success(second_response, second)

        too_long = host.send(
            "transcribe_window", pcm_path=pcm_path, offset_samples=0,
            num_samples=window_samples + 1, sample_rate=16_000, format="f32le", priority="batch"
        )
        assert_failure(host.wait_for(too_long), too_long, "bad_request")

        busy = host.send(
            "transcribe_window", pcm_path=pcm_path, offset_samples=0,
            num_samples=window_samples, sample_rate=16_000, format="f32le", priority="interactive"
        )
        ping = host.send("ping")
        status = host.send("status")
        assert_success(host.wait_for(ping, 2), ping)
        status_response = host.wait_for(status, 2)
        assert_success(status_response, status)
        busy_response = host.wait_for(busy, 45)
        assert_success(busy_response, busy)
        require(isinstance(busy_response.get("tokens"), list), f"missing tokens: {busy_response}")
        for token in busy_response["tokens"]:
            require("id" in token and "text" in token and "start" in token and "duration" in token, f"bad token: {token}")
            require(token["text"].startswith("▁") or token["text"], f"empty token text: {token}")
            require(token["start"] >= 0, f"negative token start: {token}")

        cancel_target = host.send(
            "transcribe_window", pcm_path=pcm_path, offset_samples=0,
            num_samples=window_samples, sample_rate=16_000, format="f32le", priority="interactive"
        )
        cancel_id = host.send("cancel", target=cancel_target)
        cancel_response = host.wait_for(cancel_id, 2)
        assert_success(cancel_response, cancel_id)
        require(isinstance(cancel_response.get("cancelled"), bool), f"bad cancel response: {cancel_response}")
        target_response = host.wait_for(cancel_target, 45)
        if cancel_response["cancelled"]:
            assert_failure(target_response, cancel_target, "cancelled")
        else:
            assert_success(target_response, cancel_target)

        unload = host.send("unload")
        assert_success(host.wait_for(unload, 10), unload)
        shutdown = host.send("shutdown")
        assert_success(host.wait_for(shutdown, 2), shutdown)
        require(host.shutdown() == 0, "shutdown did not exit 0")
    finally:
        if host.process.poll() is None:
            host.process.kill()
            host.process.wait()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("host_binary")
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--cache-dir")
    args = parser.parse_args()
    require(os.path.isfile(args.host_binary) and os.access(args.host_binary, os.X_OK), "host binary is not executable")
    require(os.path.isdir(args.model_dir), "model directory does not exist")

    with tempfile.TemporaryDirectory(prefix="sagascript-engine-host-") as directory:
        pcm_path = os.path.join(directory, "audio.f32le")
        fd = os.open(pcm_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        try:
            with os.fdopen(fd, "wb") as pcm:
                pcm.write(struct.pack("<f", 0.0) * (30 * 16_000))
        except Exception:
            os.close(fd)
            raise
        exercise_host(args.host_binary, args.model_dir, args.cache_dir, pcm_path)

        eof_host = Host(args.host_binary, args.cache_dir)
        try:
            hello_and_load(eof_host, args.model_dir)
            eof_host.process.stdin.close()
            require(eof_host.process.wait(timeout=3) == 0, "EOF did not make host exit 0")
        finally:
            if eof_host.process.poll() is None:
                eof_host.process.kill()
                eof_host.process.wait()
    print("engine-host conformance: PASS")


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, subprocess.SubprocessError) as error:
        print(f"engine-host conformance: FAIL: {error}", file=sys.stderr)
        sys.exit(1)
