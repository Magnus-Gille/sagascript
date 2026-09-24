#!/usr/bin/env python3
"""Offline JSONL worker for a local Klang Pianissimo NeMo checkpoint.

The process is intentionally long lived: it restores one local ``.nemo``
checkpoint, then accepts one 16 kHz mono WAV request per input line.  The Rust
caller owns cancellation by terminating this process; no request is run in a
background thread, so a request cannot outlive its process.

Only the Python standard library is imported at module import time.  NeMo and
PyTorch are imported after the checkpoint path has been validated, which keeps
the protocol helpers usable by lightweight tests and avoids any implicit model
download during those tests.
"""

from __future__ import annotations

import argparse
import json
import logging
import math
import os
import sys
import tempfile
import wave
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence, TextIO


SAMPLE_RATE = 16_000
CHUNK_SECONDS = 120.0
OVERLAP_SECONDS = 1.0

LOGGER = logging.getLogger("sagascript.pianissimo")


class InvalidRequest(ValueError):
    """Raised for malformed JSONL request objects."""


def configure_offline_environment() -> None:
    """Force all model and dataset helpers into local-only mode.

    These are assigned rather than defaulted so a parent process cannot
    accidentally turn a failed local restore into a network download.
    """

    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_DATASETS_OFFLINE"] = "1"
    os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
    os.environ["PYTORCH_ENABLE_MPS_FALLBACK"] = "1"


def chunk_ranges(
    duration_seconds: float,
    chunk_seconds: float = CHUNK_SECONDS,
    overlap_seconds: float = OVERLAP_SECONDS,
) -> list[tuple[float, float]]:
    """Return ``(start, end)`` ranges with a one-second overlap.

    The last range is allowed to be shorter than ``chunk_seconds``.  Ranges
    are expressed in seconds so the function is independent of audio I/O and
    can be tested without importing NeMo.
    """

    if not math.isfinite(duration_seconds) or duration_seconds < 0:
        raise ValueError("duration_seconds must be finite and non-negative")
    if not math.isfinite(chunk_seconds) or chunk_seconds <= 0:
        raise ValueError("chunk_seconds must be finite and positive")
    if not math.isfinite(overlap_seconds) or not 0 <= overlap_seconds < chunk_seconds:
        raise ValueError("overlap_seconds must be finite and smaller than chunk_seconds")
    if duration_seconds == 0:
        return [(0.0, 0.0)]

    step = chunk_seconds - overlap_seconds
    ranges: list[tuple[float, float]] = []
    start = 0.0
    while True:
        end = min(start + chunk_seconds, duration_seconds)
        ranges.append((start, end))
        if end >= duration_seconds:
            break
        start += step
    return ranges


def _finite_timestamp(value: Any, default: float) -> float:
    try:
        timestamp = float(value)
    except (TypeError, ValueError):
        return default
    return timestamp if math.isfinite(timestamp) else default


def _word_fields(word: Mapping[str, Any]) -> tuple[str, float, float] | None:
    value = word.get("word", word.get("text"))
    if not isinstance(value, str) or not value.strip():
        return None
    start = _finite_timestamp(word.get("start"), 0.0)
    end = _finite_timestamp(word.get("end"), start)
    if end < start:
        end = start
    return value, start, end


def deduplicate_chunk_words(
    chunks: Sequence[tuple[float, float, Sequence[Mapping[str, Any]]]],
) -> list[dict[str, Any]]:
    """Merge chunk-local words using the midpoint ownership rule.

    Each word is converted to absolute time using its chunk start.  A word in
    an overlap belongs to the chunk whose start is at or before its midpoint;
    the next chunk owns the interval beginning at its start.  This keeps one
    copy of words repeated by the model while retaining words near either
    side of a boundary.
    """

    result: list[dict[str, Any]] = []
    for index, (chunk_start, chunk_end, words) in enumerate(chunks):
        if index + 1 < len(chunks):
            ownership_end = chunks[index + 1][0]
        else:
            ownership_end = chunk_end
        for raw_word in words:
            parsed = _word_fields(raw_word)
            if parsed is None:
                continue
            text, local_start, local_end = parsed
            start = max(0.0, chunk_start + local_start)
            end = max(start, chunk_start + local_end)
            midpoint = (start + end) / 2.0
            if midpoint < chunk_start or midpoint >= ownership_end:
                continue
            result.append({"word": text, "start": start, "end": end})

    result.sort(key=lambda word: (word["start"], word["end"]))
    return result


def deduplicate_words(
    chunks: Sequence[tuple[float, float, Sequence[Mapping[str, Any]]]],
) -> list[dict[str, Any]]:
    """Compatibility alias for the public midpoint deduplication helper."""

    return deduplicate_chunk_words(chunks)


def _audio_info(path: Path) -> tuple[int, int, int, int, float]:
    if not path.is_absolute() or not path.is_file():
        raise FileNotFoundError
    with wave.open(str(path), "rb") as reader:
        channels = reader.getnchannels()
        sample_width = reader.getsampwidth()
        rate = reader.getframerate()
        frames = reader.getnframes()
    if channels != 1 or rate != SAMPLE_RATE:
        raise ValueError("audio must be 16 kHz mono WAV")
    if sample_width <= 0:
        raise ValueError("audio has an invalid sample width")
    return channels, sample_width, rate, frames, frames / rate


def _write_chunk(
    source: wave.Wave_read,
    destination: Path,
    start_frame: int,
    frame_count: int,
    channels: int,
    sample_width: int,
    rate: int,
) -> None:
    source.setpos(start_frame)
    payload = source.readframes(frame_count)
    with wave.open(str(destination), "wb") as writer:
        writer.setnchannels(channels)
        writer.setsampwidth(sample_width)
        writer.setframerate(rate)
        writer.writeframes(payload)


def _hypothesis_parts(hypothesis: Any) -> tuple[str, list[Mapping[str, Any]]]:
    """Extract text and word timestamps from NeMo's hypothesis variants."""

    if isinstance(hypothesis, Mapping):
        text = hypothesis.get("text", "")
        timestamp = hypothesis.get("timestamp", {})
    else:
        text = getattr(hypothesis, "text", "")
        timestamp = getattr(hypothesis, "timestamp", {})
    if not isinstance(text, str):
        text = str(text) if text is not None else ""
    if not isinstance(timestamp, Mapping):
        timestamp = {}
    words = timestamp.get("word", [])
    if not isinstance(words, Sequence) or isinstance(words, (str, bytes)):
        words = []
    return text, [item for item in words if isinstance(item, Mapping)]


def _transcribe_one(model: Any, audio_path: Path, device: Any) -> tuple[str, list[Mapping[str, Any]]]:
    """Run one short chunk through NeMo with word timestamps enabled."""

    import torch

    # NeMo's Lightning data path accepts this helper's keyword and MPS needs
    # synchronous copies.  The call is kept in one place so future NeMo
    # versions can use the same blocking policy without changing the protocol.
    if getattr(device, "type", None) == "mps":
        from lightning_fabric.utilities.apply_func import move_data_to_device

        move_data_to_device(torch.empty(0), device, non_blocking=False)

    with torch.inference_mode():
        hypotheses = model.transcribe(
            audio=[str(audio_path)],
            batch_size=1,
            return_hypotheses=True,
            timestamps=True,
        )
    if not hypotheses:
        return "", []
    first = hypotheses[0]
    if isinstance(first, Sequence) and not isinstance(first, (str, bytes, Mapping)):
        first = first[0] if first else ""
    return _hypothesis_parts(first)


def _load_model(model_path: Path) -> tuple[Any, Any]:
    configure_offline_environment()
    import torch
    from nemo.collections.asr.models import ASRModel
    from nemo.utils import model_utils

    has_mps = hasattr(torch.backends, "mps") and torch.backends.mps.is_available()
    device = torch.device("mps" if has_mps else "cpu")

    # Restore from the supplied archive and resolve its recorded concrete ASR
    # class.  ``from_pretrained`` is deliberately never used here: this worker
    # must not consult a model hub.
    config = ASRModel.restore_from(restore_path=str(model_path), return_config=True)
    model_class = model_utils.import_class_by_path(config.target)
    model = model_class.restore_from(restore_path=str(model_path), map_location=device)
    model = model.to(device)
    model.eval()
    return model, device


def _transcribe_file(
    model: Any,
    device: Any,
    audio_path: Path,
    emit_progress: Any,
    request_id: Any,
) -> dict[str, Any]:
    channels, sample_width, rate, frames, duration = _audio_info(audio_path)
    ranges = chunk_ranges(duration)
    chunk_results: list[tuple[float, float, Sequence[Mapping[str, Any]]]] = []
    text_parts: list[str] = []
    with tempfile.TemporaryDirectory(prefix="sagascript-pianissimo-") as temporary_directory:
        with wave.open(str(audio_path), "rb") as source:
            for index, (start, end) in enumerate(ranges):
                chunk_path = audio_path
                if len(ranges) > 1:
                    chunk_path = Path(temporary_directory) / f"chunk-{index:04d}.wav"
                    start_frame = int(round(start * rate))
                    frame_count = max(0, int(round(end * rate)) - start_frame)
                    _write_chunk(
                        source,
                        chunk_path,
                        start_frame,
                        frame_count,
                        channels,
                        sample_width,
                        rate,
                    )
                chunk_text, words = _transcribe_one(model, chunk_path, device)
                if chunk_text:
                    text_parts.append(chunk_text)
                chunk_results.append((start, end, words))
                emit_progress(
                    {"type": "progress", "id": request_id, "completed": index + 1, "total": len(ranges)}
                )

    merged_words = deduplicate_chunk_words(chunk_results)
    text = " ".join(word["word"] for word in merged_words).strip()
    if not text:
        # A model without word timestamps still returns useful hypothesis text.
        # This fallback does not affect timestamped Pianissimo output.  For a
        # chunked file, it may contain a repeated overlap, so prefer words when
        # NeMo supplied them and otherwise preserve the model's chunk text.
        text = " ".join(text_parts).strip()
    return {"text": text, "words": merged_words, "duration": duration}


def _emit(stream: TextIO, payload: Mapping[str, Any]) -> None:
    stream.write(json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n")
    stream.flush()


def _error_code(error: BaseException) -> str:
    if isinstance(error, InvalidRequest):
        return "invalid_request"
    if isinstance(error, FileNotFoundError):
        return "audio_not_found"
    if isinstance(error, (ValueError, wave.Error)):
        return "invalid_audio"
    return "transcription_failed"


def serve(model_path: Path, input_stream: TextIO = sys.stdin, output_stream: TextIO = sys.stdout) -> int:
    """Load one model and serve requests until stdin closes."""

    model, device = _load_model(model_path)
    _emit(output_stream, {"type": "ready", "device": str(device)})
    for raw_line in input_stream:
        line = raw_line.strip()
        if not line:
            continue
        request_id: Any = None
        try:
            request = json.loads(line)
            if not isinstance(request, Mapping):
                raise InvalidRequest
            request_id = request.get("id")
            path_value = request.get("path")
            if not isinstance(path_value, str) or not path_value:
                raise InvalidRequest
            audio_path = Path(path_value)
            if not audio_path.is_absolute():
                raise InvalidRequest
            result = _transcribe_file(model, device, audio_path, lambda event: _emit(output_stream, event), request_id)
            _emit(output_stream, {"type": "result", "id": request_id, **result})
        except json.JSONDecodeError:
            _emit(output_stream, {"type": "error", "id": request_id, "error": "invalid_request"})
        except BaseException as error:  # request failures must not kill the model process
            LOGGER.error("request failed (%s)", type(error).__name__)
            _emit(output_stream, {"type": "error", "id": request_id, "error": _error_code(error)})
    return 0


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Offline Pianissimo NeMo JSONL worker")
    parser.add_argument("--model", required=True, type=Path, help="absolute path to the original .nemo checkpoint")
    args = parser.parse_args(argv)
    if not args.model.is_absolute():
        parser.error("--model must be an absolute path")
    if not args.model.is_file():
        parser.error("--model must point to a local checkpoint")
    logging.basicConfig(stream=sys.stderr, level=logging.INFO, format="%(levelname)s %(message)s")
    try:
        return serve(args.model)
    except BaseException as error:
        LOGGER.error("worker initialization failed (%s)", type(error).__name__)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
