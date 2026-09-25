"""Stdlib-only tests for the Pianissimo worker protocol helpers."""

from __future__ import annotations

import importlib.util
import io
import json
import unittest
from pathlib import Path


WORKER_PATH = Path(__file__).with_name("pianissimo_worker.py")
SPEC = importlib.util.spec_from_file_location("pianissimo_worker", WORKER_PATH)
assert SPEC is not None and SPEC.loader is not None
worker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(worker)


class ChunkProtocolTests(unittest.TestCase):
    def test_long_audio_ranges_have_one_second_overlap(self) -> None:
        self.assertEqual(
            worker.chunk_ranges(239.5),
            [(0.0, 120.0), (119.0, 239.0), (238.0, 239.5)],
        )

    def test_short_and_empty_audio_have_a_single_range(self) -> None:
        self.assertEqual(worker.chunk_ranges(12.5), [(0.0, 12.5)])
        self.assertEqual(worker.chunk_ranges(0.0), [(0.0, 0.0)])

    def test_invalid_chunk_parameters_are_rejected(self) -> None:
        with self.assertRaises(ValueError):
            worker.chunk_ranges(-1.0)
        with self.assertRaises(ValueError):
            worker.chunk_ranges(2.0, chunk_seconds=1.0, overlap_seconds=1.0)

    def test_midpoint_dedup_chooses_the_next_chunk_in_overlap(self) -> None:
        merged = worker.deduplicate_chunk_words(
            [
                (
                    0.0,
                    120.0,
                    [
                        {"word": "before", "start": 118.0, "end": 118.8},
                        {"word": "overlap", "start": 119.2, "end": 119.8},
                    ],
                ),
                (
                    119.0,
                    239.0,
                    [
                        {"word": "overlap", "start": 0.2, "end": 0.8},
                        {"word": "after", "start": 1.0, "end": 1.5},
                    ],
                ),
            ]
        )
        self.assertEqual([word["word"] for word in merged], ["before", "overlap", "after"])
        self.assertEqual(merged[1]["start"], 119.2)

    def test_invalid_words_are_skipped_and_timestamps_are_normalized(self) -> None:
        merged = worker.deduplicate_words(
            [(0.0, 10.0, [{"word": "", "start": 0, "end": 1}, {"text": "ok", "start": 2, "end": 1}])]
        )
        self.assertEqual(merged, [{"word": "ok", "start": 2.0, "end": 2.0}])

    def test_chunk_text_preserves_untimestamped_part(self) -> None:
        ranges = [(0.0, 120.0), (119.0, 200.0)]
        words = [{"word": "first", "start": 5.0, "end": 5.5}]
        self.assertEqual(
            worker.assemble_chunk_text(ranges, ["first", "second sentence"], words),
            "first second sentence",
        )

    def test_chunk_text_prefers_owned_words_to_duplicate_overlap(self) -> None:
        ranges = [(0.0, 120.0), (119.0, 200.0)]
        words = [
            {"word": "before", "start": 118.0, "end": 118.5},
            {"word": "after", "start": 119.2, "end": 119.8},
        ]
        self.assertEqual(
            worker.assemble_chunk_text(ranges, ["before repeated", "repeated after"], words),
            "before after",
        )


class ProtocolTests(unittest.TestCase):
    def test_emit_is_one_line_json_and_flushes(self) -> None:
        output = io.StringIO()
        worker._emit(output, {"type": "progress", "id": "a", "completed": 1, "total": 2})
        self.assertEqual(
            json.loads(output.getvalue()),
            {"type": "progress", "id": "a", "completed": 1, "total": 2},
        )

    def test_error_payload_has_no_exception_text(self) -> None:
        output = io.StringIO()
        exception = RuntimeError("secret transcript that must stay out of JSON")
        worker._emit(output, {"type": "error", "id": "a", "error": worker._error_code(exception)})
        self.assertNotIn(str(exception), output.getvalue())
        self.assertEqual(json.loads(output.getvalue())["error"], "transcription_failed")


if __name__ == "__main__":
    unittest.main()
