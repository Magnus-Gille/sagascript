#!/usr/bin/env python3
"""Protocol v1 conformance checks for a Sagascript engine host.

Only Python's standard library is used. The checks intentionally exercise the
wire behavior rather than importing or special-casing the CoreML implementation.

  engine-host-conformance.py HOST --no-model         model-free subset (hello, ping,
                                                     status, errors, shutdown, EOF exit)
  engine-host-conformance.py HOST --model-dir DIR    the subset plus load, transcribe
                                                     windows, cancel and unload

CI runs the model-free subset against the staged Core ML host (ci.yml,
test-macos). The full run needs a downloaded Pianissimo model. The release
gate runs the subset against the signed host before bundling it and the full
run, after a Pianissimo smoke that downloads the model, against the host in the
installed signed app (release-build-macos.yml) and in the downloaded draft
(verify-release-draft.sh); test-build.yml rehearses the full run.
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

    def stderr_tail(self, limit=1000, budget=1 << 20):
        """The last `limit` characters the host has written to stderr so far, without blocking.

        A host that is alive but stuck keeps stderr open, so `read()` would wait for EOF (and the
        release job for its timeout). Read only what is already in the pipe, in raw chunks of at
        most 4096 bytes, and at most `budget` bytes for a host that never stops writing.
        """
        if not self.process.stderr:
            return ""
        fd = self.process.stderr.fileno()
        chunks = []
        total = 0
        try:
            while total < budget:
                ready, _, _ = select.select([fd], [], [], 0)
                if not ready:
                    break
                chunk = os.read(fd, 4096)
                if not chunk:
                    break
                chunks.append(chunk)
                total += len(chunk)
        except OSError:
            pass
        return b"".join(chunks).decode("utf-8", "replace")[-limit:]

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


def require_int(obj, key, where):
    value = obj.get(key)
    # JSON integers only: typed clients (Rust u64) reject floats such as 95520.45.
    require(isinstance(value, int) and not isinstance(value, bool) and value >= 0,
            f"{where}.{key} must be a non-negative JSON integer, got {value!r}")


def assert_failure(response, request_id, code):
    require(response.get("id") == request_id, f"response id mismatch: {response}")
    require(response.get("ok") is False, f"request {request_id} unexpectedly succeeded: {response}")
    require(response.get("error", {}).get("code") == code, f"expected {code}: {response}")


def hello(host):
    hello_id = host.send("hello", client={"name": "engine-host-conformance", "version": "1", "git_sha": "0" * 40})
    response = host.wait_for(hello_id, 10)
    assert_success(response, hello_id)
    require(response.get("protocol") == 1, f"wrong protocol: {response}")
    host_info = response.get("host", {})
    require(host_info.get("name") == "sagascript-engine-host", f"wrong host name: {response}")
    require(re.fullmatch(r"[0-9a-f]{40}", host_info.get("git_sha", "")), f"missing full git_sha: {response}")
    for key in ("version", "engine", "engine_version"):
        require(isinstance(host_info.get(key), str), f"hello.host.{key} must be a string: {response}")
    capabilities = response.get("capabilities", {})
    require(capabilities.get("max_in_flight", 0) >= 1, f"invalid capabilities: {response}")
    for key in ("sample_rate", "max_in_flight"):
        require_int(capabilities, key, "hello.capabilities")
    for key in ("max_window_s", "preferred_window_s", "preferred_overlap_s"):
        value = capabilities.get(key)
        require(isinstance(value, (int, float)) and not isinstance(value, bool) and value >= 0,
                f"hello.capabilities.{key} must be a non-negative number, got {value!r}")
    return response


def exercise_without_model(binary, cache_dir, scratch):
    """Every op that needs no model, plus the errors a client relies on."""
    host = Host(binary, cache_dir)
    try:
        before = host.send("ping")
        assert_failure(host.wait_for(before, 5), before, "protocol")
        hello(host)
        ping = host.send("ping")
        ping_response = host.wait_for(ping, 2)
        assert_success(ping_response, ping)
        require(set(ping_response) == {"id", "ok"}, f"ping must answer {{}}: {ping_response}")
        status = host.send("status")
        status_response = host.wait_for(status, 2)
        assert_success(status_response, status)
        require(status_response.get("state") == "idle", f"a fresh host is idle: {status_response}")
        require(status_response.get("model_id") is None, f"no model is loaded yet: {status_response}")
        for key in ("in_flight", "rss_bytes"):
            require_int(status_response, key, "status")
        unknown = host.send("conformance_unknown_op")
        assert_failure(host.wait_for(unknown, 5), unknown, "unsupported")
        missing = host.send(
            "load", model_dir=os.path.join(scratch, "no-such-model"),
            model_id="pianissimo-sv-coreml-conformance", compute_units="ane"
        )
        assert_failure(host.wait_for(missing, 30), missing, "model_missing")
        # The client reloads and resends once on not_loaded, so a host must say so before load.
        unloaded = host.send(
            "transcribe_window", pcm_path=os.path.join(scratch, "unused.f32le"), offset_samples=0,
            num_samples=16_000, sample_rate=16_000, format="f32le", priority="interactive"
        )
        assert_failure(host.wait_for(unloaded, 10), unloaded, "not_loaded")
        cancel = host.send("cancel", target=unloaded)
        cancel_response = host.wait_for(cancel, 2)
        assert_success(cancel_response, cancel)
        require(cancel_response.get("cancelled") is False, f"a finished request cannot be cancelled: {cancel_response}")
        shutdown = host.send("shutdown")
        assert_success(host.wait_for(shutdown, 2), shutdown)
        require(host.shutdown() == 0, "shutdown did not exit 0")
    finally:
        if host.process.poll() is None:
            host.process.kill()
            host.process.wait()

    eof_host = Host(binary, cache_dir)
    try:
        hello(eof_host)
        eof_host.process.stdin.close()
        require(eof_host.process.wait(timeout=3) == 0, "EOF did not make host exit 0")
    finally:
        if eof_host.process.poll() is None:
            eof_host.process.kill()
            eof_host.process.wait()


def hello_and_load(host, model_dir):
    response = hello(host)
    capabilities = response.get("capabilities", {})
    load_id = host.send(
        "load", model_dir=os.path.abspath(model_dir),
        model_id="pianissimo-sv-coreml-conformance", compute_units="ane"
    )
    load = host.wait_for(load_id, 180)
    assert_success(load, load_id)
    require(load.get("window_s", 0) > 0, f"load did not report window_s: {load}")
    require(load.get("blank_id", 0) >= 0, f"load did not report blank_id: {load}")
    require_int(load, "load_ms", "load")
    require_int(capabilities, "max_in_flight", "hello.capabilities")
    return response, load


def exercise_host(binary, model_dir, cache_dir, pcm_path, expect_exit_on_eof=False):
    host = Host(binary, cache_dir)
    try:
        _, load = hello_and_load(host, model_dir)
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
        for key in ("preprocess_ms", "encode_ms", "decode_ms"):
            require_int(busy_response.get("timings", {}), key, "transcribe_window.timings")
        for key in ("in_flight", "rss_bytes"):
            require_int(status_response, key, "status")
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
    model = parser.add_mutually_exclusive_group(required=True)
    model.add_argument("--model-dir", help="run every check against this model directory")
    model.add_argument("--no-model", action="store_true", help="run only the checks that need no model")
    parser.add_argument("--cache-dir")
    args = parser.parse_args()
    require(os.path.isfile(args.host_binary) and os.access(args.host_binary, os.X_OK), "host binary is not executable")
    require(args.no_model or os.path.isdir(args.model_dir), "model directory does not exist")

    with tempfile.TemporaryDirectory(prefix="sagascript-engine-host-") as directory:
        exercise_without_model(args.host_binary, args.cache_dir, directory)
        if args.no_model:
            print("engine-host conformance (model-free subset): PASS")
            return
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
