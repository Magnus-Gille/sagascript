#!/usr/bin/env python3
"""Prepare synthetic public-audio fixtures and run a cold/warm CLI matrix.

The runner uses one CLI process per backend/duration cell. It passes two distinct
paths containing byte-identical audio so the second item exercises the already
loaded model without file-path de-duplication.
"""
from __future__ import annotations

import argparse
import csv
from datetime import datetime, timezone
import hashlib
import io
import json
import os
import psutil
from pathlib import Path
import queue
import re
import shutil
import subprocess
import sys
import tarfile
import threading
import time
import wave
from typing import Any

SAMPLE_RATE = 16_000
MODEL_ID = "pianissimo-sv"
LONG_SECONDS = 180
PROGRESS_NOTE = (
    "--progress-json reports decoding/resampling/transcribing/completed phases; "
    "model verification and load are in result.performance fields."
)
WORD_RE = re.compile(r"\w+", re.UNICODE)
INHERITED_ENV = (
    "PATH", "SystemRoot", "WINDIR", "TEMP", "TMP", "LOCALAPPDATA", "APPDATA",
    "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "ComSpec", "PATHEXT", "OS",
    "PROCESSOR_ARCHITECTURE", "PROCESSOR_IDENTIFIER", "NUMBER_OF_PROCESSORS",
)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def wer(reference: str, hypothesis: str) -> dict[str, int | float]:
    """Word error counts matching the existing accelerator-lab benchmark."""
    ref = WORD_RE.findall(reference.lower())
    hyp = WORD_RE.findall(hypothesis.lower())
    previous = list(range(len(hyp) + 1))
    for i, expected in enumerate(ref, 1):
        current = [i]
        for j, actual in enumerate(hyp, 1):
            current.append(min(
                previous[j] + 1,
                current[-1] + 1,
                previous[j - 1] + (expected != actual),
            ))
        previous = current
    errors = previous[-1]
    return {"errors": errors, "words": len(ref), "wer": errors / max(len(ref), 1)}


def wav_to_pcm16(data: bytes) -> tuple[int, bytes, dict[str, int]]:
    """Decode mono 16 kHz PCM16 or IEEE float32 WAV to PCM16 samples."""
    if len(data) < 12 or data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise ValueError("expected a little-endian RIFF/WAVE file")
    offset = 12
    fmt: tuple[int, int, int, int, int, int] | None = None
    audio: bytes | None = None
    while offset + 8 <= len(data):
        name, size = struct_unpack_chunk(data, offset)
        start = offset + 8
        end = start + size
        if end > len(data):
            raise ValueError("truncated WAV chunk")
        chunk = data[start:end]
        if name == b"fmt ":
            if size < 16:
                raise ValueError("short WAV fmt chunk")
            fmt = __import__("struct").unpack_from("<HHIIHH", chunk)
        elif name == b"data":
            audio = chunk
        offset = end + (size & 1)
    if fmt is None or audio is None:
        raise ValueError("WAV is missing fmt or data chunk")
    kind, channels, rate, _byte_rate, _align, bits = fmt
    if (channels, rate) != (1, SAMPLE_RATE):
        raise ValueError(f"expected mono {SAMPLE_RATE} Hz audio; got {channels}ch/{rate}Hz")
    import struct
    if kind == 1 and bits == 16:
        frames = len(audio) // 2
        if len(audio) != frames * 2:
            raise ValueError("unaligned PCM16 data")
        pcm = audio
    elif kind == 3 and bits == 32:
        frames = len(audio) // 4
        if len(audio) != frames * 4:
            raise ValueError("unaligned float32 data")
        samples = struct.unpack(f"<{frames}f", audio)
        pcm_buf = bytearray(frames * 2)
        for i, sample in enumerate(samples):
            value = max(-1.0, min(1.0, sample))
            integer = max(-32768, min(32767, round(value * 32768.0)))
            struct.pack_into("<h", pcm_buf, i * 2, integer)
        pcm = bytes(pcm_buf)
    else:
        raise ValueError(f"unsupported WAV encoding kind={kind}, bits={bits}")
    return frames, pcm, {"format": kind, "bits_per_sample": bits, "channels": channels, "sample_rate": rate}


def struct_unpack_chunk(data: bytes, offset: int) -> tuple[bytes, int]:
    import struct
    return struct.unpack_from("<4sI", data, offset)


def pcm16_wav(pcm: bytes, frames: int) -> bytes:
    if len(pcm) != frames * 2:
        raise ValueError("PCM byte count does not match sample count")
    output = io.BytesIO()
    with wave.open(output, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(SAMPLE_RATE)
        wav.writeframes(pcm)
    return output.getvalue()


def parse_fleurs_tsv(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8", newline="") as stream:
        for row_number, row in enumerate(csv.reader(stream, delimiter="\t"), 1):
            if len(row) < 7:
                continue
            try:
                samples = int(row[5])
            except ValueError:
                continue  # tolerate an optional header row
            if samples <= 0 or not Path(row[1]).name == row[1]:
                continue
            rows.append({
                "id": row[0],
                "filename": row[1],
                "reference": row[2],
                "samples": samples,
                "gender": row[6],
                "row": row_number,
            })
    if not rows:
        raise ValueError(f"no valid FLEURS rows in {path}")
    return rows


def select_short_row(rows: list[dict[str, Any]], unavailable: set[str]) -> dict[str, Any]:
    for row in rows:
        if row["filename"] in unavailable:
            continue
        if 3 * SAMPLE_RATE <= row["samples"] <= 5 * SAMPLE_RATE:
            return row
    raise ValueError("no unused complete 3–5 second utterance in the FLEURS manifest")


def build_long_sources(lab: Path) -> list[dict[str, Any]]:
    """Read the curated unique clips and extend with unique FLEURS rows."""
    corpus = json.loads((lab / "corpus.json").read_text(encoding="utf-8"))
    lab_root = lab.resolve()
    sources: list[dict[str, Any]] = []
    seen: set[str] = set()
    for row in corpus:
        if not row.get("gender") or row.get("id") == "hongkong-test":
            continue
        name = Path(row["file"]).name
        candidate = (lab / "audio" / name).resolve()
        if not candidate.is_relative_to(lab_root):
            raise ValueError(f"corpus audio escapes LAB: {name}")
        if name in seen or not candidate.is_file():
            continue
        data = candidate.read_bytes()
        frames, pcm, fmt = wav_to_pcm16(data)
        sources.append({
            "id": str(row["id"]), "filename": name,
            "duration_seconds": frames / SAMPLE_RATE,
            "reference": row["reference"], "sha256": sha256_bytes(data),
            "frames": frames, "pcm16": pcm, "format": fmt,
            "gender": row.get("gender"), "source_kind": "corpus.json",
        })
        seen.add(name)
    if not sources:
        raise ValueError("corpus.json has no usable public FLEURS audio files in LAB/audio")
    return sources


def select_long_additions(rows: list[dict[str, Any]], used_names: set[str],
                          short_name: str, current_frames: int) -> list[dict[str, Any]]:
    remaining = LONG_SECONDS * SAMPLE_RATE - current_frames
    candidates = [row for row in rows
                  if row["filename"] not in used_names
                  and row["filename"] != short_name
                  and row["samples"] <= LONG_SECONDS * SAMPLE_RATE]
    # Deterministic largest-fit packing minimizes trailing silence without repeating speech.
    candidates.sort(key=lambda row: (-row["samples"], row["row"], row["filename"]))
    selected: list[dict[str, Any]] = []
    while remaining > 3 * SAMPLE_RATE:
        fit_index = next((i for i, row in enumerate(candidates)
                          if row["samples"] <= remaining), None)
        if fit_index is None:
            break
        row = candidates.pop(fit_index)
        selected.append(row)
        remaining -= row["samples"]
    if remaining > 5 * SAMPLE_RATE:
        # Continue with any clip that fits; repeat fallback is handled after archive reads.
        while remaining > 5 * SAMPLE_RATE:
            fit_index = next((i for i, row in enumerate(candidates)
                              if row["samples"] <= remaining), None)
            if fit_index is None:
                break
            row = candidates.pop(fit_index)
            selected.append(row)
            remaining -= row["samples"]
    return selected


def read_tar_wavs(lab: Path, filenames: set[str]) -> dict[str, bytes]:
    found: dict[str, bytes] = {}
    with tarfile.open(lab / "fleurs-sv-dev.tar.gz", mode="r:gz") as tar:
        for member in tar:
            filename = Path(member.name).name
            if filename not in filenames or filename in found or not member.isfile():
                continue
            handle = tar.extractfile(member)
            if handle is None:
                continue
            found[filename] = handle.read()
            if len(found) == len(filenames):
                break
    missing = filenames - found.keys()
    if missing:
        raise FileNotFoundError("FLEURS archive is missing required audio: " + ", ".join(sorted(missing)))
    return found


def _safe_output_dir(lab: Path, output: Path) -> Path:
    lab = lab.resolve()
    output = output.resolve()
    if not output.is_relative_to(lab):
        raise ValueError("fixture output must be inside LAB")
    return output


def prepare_fixtures(lab: Path, output_dir: Path) -> Path:
    lab = lab.resolve()
    output_dir = _safe_output_dir(lab, output_dir)
    rows = parse_fleurs_tsv(lab / "fleurs-sv-dev.tsv")
    long_sources = build_long_sources(lab)
    used_names = {item["filename"] for item in long_sources}
    short_row = select_short_row(rows, used_names)
    base_frames = sum(item["frames"] for item in long_sources)
    additions = select_long_additions(rows, used_names, short_row["filename"], base_frames)
    needed = {short_row["filename"]} | {row["filename"] for row in additions}
    archive_audio = read_tar_wavs(lab, needed)

    short_bytes = archive_audio[short_row["filename"]]
    short_frames, _short_pcm, short_format = wav_to_pcm16(short_bytes)
    if short_frames != short_row["samples"]:
        raise ValueError(f"FLEURS sample-count mismatch for {short_row['filename']}")
    for row in additions:
        data = archive_audio[row["filename"]]
        frames, pcm, fmt = wav_to_pcm16(data)
        if frames != row["samples"]:
            raise ValueError(f"FLEURS sample-count mismatch for {row['filename']}")
        long_sources.append({
            "id": str(row["id"]), "filename": row["filename"],
            "duration_seconds": frames / SAMPLE_RATE,
            "reference": row["reference"], "sha256": sha256_bytes(data),
            "frames": frames, "pcm16": pcm, "format": fmt,
            "gender": row["gender"], "source_kind": "fleurs-sv-dev.tsv/tar.gz",
        })
    long_frames = sum(item["frames"] for item in long_sources)
    target_frames = LONG_SECONDS * SAMPLE_RATE
    silence_frames = target_frames - long_frames
    repeated_sources: list[dict[str, Any]] = []
    repeat_index = 1
    while silence_frames > 5 * SAMPLE_RATE and short_frames <= silence_frames:
        _frames, short_pcm, _fmt = wav_to_pcm16(short_bytes)
        repeated_sources.append({
            "id": str(short_row["id"]), "filename": short_row["filename"],
            "duration_seconds": short_frames / SAMPLE_RATE,
            "reference": short_row["reference"], "sha256": sha256_bytes(short_bytes),
            "frames": short_frames, "pcm16": short_pcm, "format": short_format,
            "gender": short_row["gender"], "source_kind": "fleurs-sv-dev.tsv/tar.gz",
            "repeat_index": repeat_index,
        })
        repeat_index += 1
        long_frames += short_frames
        silence_frames = target_frames - long_frames
    if long_frames < 175 * SAMPLE_RATE or not 0 <= silence_frames <= 5 * SAMPLE_RATE:
        raise ValueError("could not compose at least 175 seconds of speech and at most 5 seconds of silence")
    long_sources.extend(repeated_sources)
    long_pcm = b"".join(item["pcm16"] for item in long_sources)
    long_pcm += b"\0\0" * silence_frames
    long_bytes = pcm16_wav(long_pcm, target_frames)

    if output_dir.exists():
        raise FileExistsError(f"refusing to overwrite existing fixture directory: {output_dir}")
    output_dir.mkdir(parents=True)
    inputs: dict[str, dict[str, Any]] = {}
    short_composition = [{
        "id": str(short_row["id"]), "filename": short_row["filename"],
        "gender": short_row["gender"], "reference": short_row["reference"],
        "samples": short_frames, "duration_seconds": short_frames / SAMPLE_RATE,
        "sha256": sha256_bytes(short_bytes), "source_format": short_format,
    }]
    long_composition = [{
        key: item[key] for key in (
            "id", "filename", "gender", "reference", "duration_seconds",
            "sha256", "format", "source_kind", "repeat_index",
        ) if key in item
    } for item in long_sources]
    for case, payload, duration, reference, composition in (
        ("short", short_bytes, short_frames / SAMPLE_RATE, short_row["reference"], short_composition),
        ("long", long_bytes, float(LONG_SECONDS),
         " ".join(item["reference"].strip() for item in long_sources), long_composition),
    ):
        cold_path = output_dir / f"{case}-cold.wav"
        warm_path = output_dir / f"{case}-warm.wav"
        cold_path.write_bytes(payload)
        shutil.copyfile(cold_path, warm_path)
        digest = sha256_bytes(payload)
        inputs[case] = {
            "duration_seconds": duration,
            "reference": reference,
            "reference_words": len(WORD_RE.findall(reference.lower())),
            "sha256": digest,
            "cold_path": str(cold_path),
            "warm_path": str(warm_path),
            "cold_sha256": sha256_file(cold_path),
            "warm_sha256": sha256_file(warm_path),
            "composition": composition,
            "synthetic": case == "long",
        }
        if inputs[case]["cold_sha256"] != inputs[case]["warm_sha256"]:
            raise RuntimeError(f"cold/warm copy mismatch for {case}")
    manifest = {
        "schema_version": 1,
        "created_utc": datetime.now(timezone.utc).isoformat(),
        "sample_rate": SAMPLE_RATE,
        "long_seconds_exact": LONG_SECONDS,
        "long_public_speech_seconds": long_frames / SAMPLE_RATE,
        "long_trailing_silence_seconds": silence_frames / SAMPLE_RATE,
        "long_note": "Synthetic concatenation of complete, unique public FLEURS utterances plus trailing silence.",
        "corpus_manifest_sha256": sha256_file(lab / "corpus.json"),
        "fixtures": inputs,
    }
    manifest_path = output_dir / "fixtures.json"
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return manifest_path


def _child_env(overrides: dict[str, str]) -> dict[str, str]:
    env = {key: os.environ[key] for key in INHERITED_ENV if key in os.environ}
    for key, value in overrides.items():
        if not isinstance(key, str) or not isinstance(value, str):
            raise ValueError("backend env must map string names to string values")
        env[key] = value
    return env


def _cli_prefix(value: str | list[str]) -> list[str]:
    prefix = [value] if isinstance(value, str) else value
    if not prefix or any(not isinstance(part, str) or not part for part in prefix):
        raise ValueError("backend cli must be a path or non-empty string array")
    return prefix


def _normal_source(value: str) -> str:
    return os.path.normcase(os.path.abspath(value)).casefold()


def _stream_reader(pipe: Any, name: str, messages: queue.Queue[Any]) -> None:
    try:
        for line in iter(pipe.readline, ""):
            messages.put((name, line, time.perf_counter_ns()))
    finally:
        messages.put((name, None, time.perf_counter_ns()))


def _parse_result_line(line: str) -> dict[str, Any]:
    item = json.loads(line)
    if not isinstance(item, dict) or item.get("status") not in ("ok", "error"):
        raise ValueError("JSONL item must have status ok/error")
    if "source" not in item:
        raise ValueError("JSONL item is missing source")
    if item["status"] == "ok" and not isinstance(item.get("result"), dict):
        raise ValueError("successful JSONL item is missing result object")
    return item


def _terminate_process_tree(pid: int) -> list[str]:
    """Terminate a timed-out CLI and any engine-host descendants."""
    diagnostics: list[str] = []
    try:
        parent = psutil.Process(pid)
    except psutil.NoSuchProcess:
        return diagnostics
    targets = parent.children(recursive=True) + [parent]
    for process in targets:
        try:
            process.terminate()
        except psutil.NoSuchProcess:
            pass
        except psutil.AccessDenied as exc:
            diagnostics.append(f"terminate pid {process.pid}: {exc}")
    _gone, alive = psutil.wait_procs(targets, timeout=5)
    for process in alive:
        try:
            process.kill()
        except psutil.NoSuchProcess:
            pass
        except psutil.AccessDenied as exc:
            diagnostics.append(f"kill pid {process.pid}: {exc}")
    _gone, still_alive = psutil.wait_procs(alive, timeout=5)
    diagnostics.extend(f"pid {process.pid} remained alive after kill" for process in still_alive)
    return diagnostics


def _run_cell(backend: str, spec: dict[str, Any], case: str, fixture: dict[str, Any],
              output_dir: Path, timeout_s: float) -> dict[str, Any]:
    cold_path = Path(fixture["cold_path"]).resolve()
    warm_path = Path(fixture["warm_path"]).resolve()
    if cold_path == warm_path:
        raise ValueError(f"{case} cold and warm inputs must have distinct paths")
    cold_sha, warm_sha = sha256_file(cold_path), sha256_file(warm_path)
    if cold_sha != warm_sha or cold_sha != fixture["sha256"]:
        raise ValueError(f"{case} cold/warm inputs must be byte-identical and match manifest")
    duration = float(fixture["duration_seconds"])
    if case == "short" and not 3.0 <= duration <= 5.0:
        raise ValueError("short fixture must be between 3 and 5 seconds")
    if case == "long" and duration != LONG_SECONDS:
        raise ValueError("long fixture must be exactly 180 seconds")
    frames, _pcm, _format = wav_to_pcm16(cold_path.read_bytes())
    if frames != round(duration * SAMPLE_RATE):
        raise ValueError(f"{case} fixture sample count does not match its recorded duration")
    command = _cli_prefix(spec.get("cli", spec.get("cli_path", "")))
    argv = command + [
        "transcribe", str(cold_path), str(warm_path),
        "--language", "sv", "--model", MODEL_ID, "--no-vad",
        "--jsonl", "--progress-json",
    ]
    # Pianissimo rejects --beam, so the runner deliberately leaves beam unspecified.
    safe_backend = re.sub(r"[^A-Za-z0-9_.-]", "_", backend)
    stem = f"{safe_backend}-{case}"
    stdout_path = output_dir / f"{stem}.stdout.jsonl"
    stderr_path = output_dir / f"{stem}.stderr.log"
    run_path = output_dir / f"{stem}.run.json"
    env = _child_env(spec.get("env", {}))
    start_utc = datetime.now(timezone.utc).isoformat()
    started_ns = time.perf_counter_ns()
    creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0
    process = subprocess.Popen(
        argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, encoding="utf-8", errors="replace", bufsize=1, env=env,
        cwd=spec.get("cwd"), creationflags=creationflags,
    )
    messages: queue.Queue[Any] = queue.Queue()
    readers = [
        threading.Thread(target=_stream_reader, args=(process.stdout, "stdout", messages), daemon=True),
        threading.Thread(target=_stream_reader, args=(process.stderr, "stderr", messages), daemon=True),
    ]
    for reader in readers:
        reader.start()
    result_items: list[tuple[int, dict[str, Any]]] = []
    progress_events: list[dict[str, Any]] = []
    parse_errors: list[dict[str, Any]] = []
    stderr_lines = 0
    ended: set[str] = set()
    timed_out = False
    termination_errors: list[str] = []
    stdout_index = 0
    stderr_index = 0
    with stdout_path.open("w", encoding="utf-8", newline="") as stdout_file, \
         stderr_path.open("w", encoding="utf-8", newline="") as stderr_file:
        while len(ended) < 2:
            now_ns = time.perf_counter_ns()
            elapsed = (now_ns - started_ns) / 1e9
            if not timed_out and elapsed > timeout_s and process.poll() is None:
                timed_out = True
                termination_errors.extend(_terminate_process_tree(process.pid))
            try:
                stream_name, line, received_ns = messages.get(timeout=0.2)
            except queue.Empty:
                if timed_out and elapsed > timeout_s + 15 and len(ended) < 2:
                    for pipe in (process.stdout, process.stderr):
                        if pipe and not pipe.closed:
                            pipe.close()
                    break
                continue
            if line is None:
                ended.add(stream_name)
                continue
            relative_ms = (received_ns - started_ns) / 1e6
            if stream_name == "stdout":
                stdout_index += 1
                stdout_file.write(line)
                try:
                    item = _parse_result_line(line)
                    item["received_monotonic_ms"] = relative_ms
                    item["stdout_line"] = stdout_index
                    result_items.append((received_ns, item))
                except (json.JSONDecodeError, ValueError) as exc:
                    parse_errors.append({"stream": "stdout", "line": stdout_index, "error": str(exc)})
            else:
                stderr_index += 1
                stderr_lines += 1
                stderr_file.write(line)
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(event, dict) and event.get("event") == "transcription_progress":
                    progress_events.append({"received_monotonic_ms": relative_ms, **event})
        stdout_file.flush()
        stderr_file.flush()
    try:
        return_code = process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        return_code = process.wait()
    for reader in readers:
        reader.join(timeout=1)
    for pipe in (process.stdout, process.stderr):
        if pipe and not pipe.closed:
            pipe.close()

    expected = {"cold": cold_path, "warm": warm_path}
    source_to_slot = {_normal_source(str(path)): slot for slot, path in expected.items()}
    last_result_ns: int | None = None
    item_summaries: list[dict[str, Any]] = []
    for order, (received_ns, item) in enumerate(result_items):
        source = str(item.get("source", ""))
        slot = source_to_slot.get(_normal_source(source))
        entry: dict[str, Any] = {
            "order": order,
            "slot": slot or "unexpected",
            "status": item["status"],
            "source": source,
            "received_monotonic_ms": item["received_monotonic_ms"],
            "process_start_to_result_ms": item["received_monotonic_ms"],
            "input_sha256": cold_sha if slot in ("cold", "warm") else None,
        }
        if last_result_ns is not None:
            entry["previous_result_to_result_ms"] = (received_ns - last_result_ns) / 1e6
        last_result_ns = received_ns
        if item["status"] == "error":
            entry["error"] = item.get("error")
        else:
            result = item["result"]
            performance = result.get("performance") or {}
            transcript = result.get("text") or ""
            counts = wer(fixture["reference"], transcript)
            entry.update({
                "audio_seconds": result.get("duration_seconds"),
                "transcript_chars": len(transcript),
                "quality": counts,
                "performance_total_seconds": performance.get("total_seconds"),
                "model_verification_seconds": performance.get("model_verification_seconds"),
                "model_load_seconds": performance.get("model_load_seconds"),
                "decode_resample_seconds": performance.get("decode_resample_seconds"),
                "engine": performance.get("engine"),
                "engine_warm": performance.get("engine_warm"),
            })
        item_summaries.append(entry)
    run = {
        "backend": backend,
        "case": case,
        "argv": argv,
        "env_keys": sorted(spec.get("env", {}).keys()),
        "started_utc": start_utc,
        "process_start_monotonic_ns": started_ns,
        "exit_code": return_code,
        "timed_out": timed_out,
        "termination_errors": termination_errors,
        "timeout_seconds": timeout_s,
        "stdout_jsonl": str(stdout_path),
        "stderr_log": str(stderr_path),
        "result_item_count": len(result_items),
        "progress_event_count": len(progress_events),
        "progress_events": progress_events,
        "stdout_parse_errors": parse_errors,
        "stderr_line_count": stderr_lines,
        "progress_note": PROGRESS_NOTE,
        "inputs": {slot: {"path": str(path), "sha256": cold_sha} for slot, path in expected.items()},
        "items": item_summaries,
    }
    run_path.write_text(json.dumps(run, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    run["run_json"] = str(run_path)
    return run


def run_matrix(config_path: Path, manifest_path: Path, output_dir: Path) -> Path:
    config = json.loads(config_path.read_text(encoding="utf-8"))
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    backends = config.get("backends")
    fixtures = manifest.get("fixtures")
    if not isinstance(backends, dict) or not backends:
        raise ValueError("config must define a non-empty backends object")
    missing_backends = {"cpu", "gpu", "npu"} - {str(name).lower() for name in backends}
    if missing_backends:
        raise ValueError("config is missing required backend(s): " + ", ".join(sorted(missing_backends)))
    if not isinstance(fixtures, dict) or not {"short", "long"}.issubset(fixtures):
        raise ValueError("fixture manifest must define short and long cases")
    timeout_s = float(config.get("timeout_seconds", 900))
    if timeout_s <= 0:
        raise ValueError("timeout_seconds must be positive")
    if output_dir.exists():
        raise FileExistsError(f"refusing to overwrite existing results directory: {output_dir}")
    output_dir.mkdir(parents=True)
    runs: list[dict[str, Any]] = []
    for backend, spec in backends.items():
        if not isinstance(spec, dict):
            raise ValueError(f"backend config for {backend} must be an object")
        for case in ("short", "long"):
            runs.append(_run_cell(backend, spec, case, fixtures[case], output_dir, timeout_s))
    successful = all(
        run["exit_code"] == 0 and not run["timed_out"]
        and run["result_item_count"] == 2 and not run["stdout_parse_errors"]
        and [item["slot"] for item in run["items"]] == ["cold", "warm"]
        and all(item["status"] == "ok" for item in run["items"])
        for run in runs
    ) and len(runs) == len(backends) * 2
    summary = {
        "schema_version": 1,
        "success": successful,
        "created_utc": datetime.now(timezone.utc).isoformat(),
        "config_path": str(config_path.resolve()),
        "manifest_path": str(manifest_path.resolve()),
        "cli_contract": {
            "model": MODEL_ID,
            "language": "sv",
            "vad": "disabled",
            "beam": "unspecified; Pianissimo rejects --beam",
            "invocation_per_cell": "one CLI process; cold then warm distinct paths with identical bytes",
        },
        "runs": runs,
    }
    summary_path = output_dir / "matrix-results.json"
    summary_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return summary_path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    prepare = subparsers.add_parser("prepare", help="prepare deterministic short and synthetic long fixtures")
    prepare.add_argument("--lab", type=Path, required=True)
    prepare.add_argument("--out", type=Path, help="fixture directory inside LAB (default: LAB/matrix-fixtures)")
    run = subparsers.add_parser("run", help="run configured CLI backends over short/long cold/warm cells")
    run.add_argument("--config", type=Path, required=True, help="JSON backend map with cli path/argv prefix and env")
    run.add_argument("--manifest", type=Path, required=True, help="fixtures.json from prepare")
    run.add_argument("--out", type=Path, required=True, help="new output directory; must not already exist")
    args = parser.parse_args(argv)
    try:
        if args.command == "prepare":
            lab = args.lab.resolve()
            output = args.out if args.out else lab / "matrix-fixtures"
            manifest = prepare_fixtures(lab, output)
            print(json.dumps({"manifest": str(manifest), "sha256": sha256_file(manifest)}))
        else:
            summary = run_matrix(args.config.resolve(), args.manifest.resolve(), args.out.resolve())
            data = json.loads(summary.read_text(encoding="utf-8"))
            print(json.dumps({"summary": str(summary), "success": data["success"], "sha256": sha256_file(summary)}))
            return 0 if data["success"] else 1
    except (OSError, ValueError, json.JSONDecodeError, tarfile.TarError, subprocess.SubprocessError) as exc:
        parser.exit(2, f"matrix_eval: {exc}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
