"""Headless progress acceptance using a caller-selected >16 kHz audio fixture.

Usage: python3 scripts/check-transcribe-progress.py CLI FIXTURE
Uses Base EN already provisioned by the caller. Never downloads models.
Checks captured stderr (no TTY) and prints timings, never transcript content.
"""
import json
import subprocess
import sys


def main():
    run = subprocess.run(
        [sys.argv[1], "transcribe", sys.argv[2], "--language", "en",
         "--model", "base.en", "--beam", "0", "--no-vad", "--json", "--progress-json"],
        capture_output=True, text=True, timeout=600,
    )
    if run.returncode:
        raise RuntimeError(f"transcribe exited {run.returncode}; inspect stderr locally")
    json.loads(run.stdout)
    events = []
    for line in run.stderr.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue  # Existing human diagnostics also use stderr.
        if isinstance(event, dict) and event.get("event") == "transcription_progress":
            events.append(event)
    assert events, "no machine-readable progress when stderr is piped"
    assert all(a["elapsed_ms"] <= b["elapsed_ms"] for a, b in zip(events, events[1:]))
    phases = list(dict.fromkeys(e["phase"] for e in events))
    expected = ["decoding", "resampling", "loading", "language_detection",
                "preparing", "encoding", "transcribing", "finalizing", "completed"]
    assert phases == expected, phases
    for event in events:
        expected_step = 1 if event["phase"] == "decoding" else 3 if event["phase"] in (
            "transcribing", "finalizing", "completed"
        ) else 2
        assert event["step"] == expected_step and event["steps_total"] == 3, event
    for phase in ("decoding", "resampling"):
        stage = [e for e in events if e["phase"] == phase]
        values = [e["percent"] for e in stage]
        assert values[0] == 0 and values[-1] == 100, (phase, values)
        assert len(values) > 20 and any(20 < p < 80 for p in values), (phase, values)
        assert all(a < b for a, b in zip(values, values[1:])), (phase, values)
    for phase in phases:
        stage = [e for e in events if e["phase"] == phase]
        print(json.dumps({"phase": phase, "events": len(stage),
                          "start_ms": stage[0]["elapsed_ms"],
                          "last_ms": stage[-1]["elapsed_ms"],
                          "first_percent": stage[0]["percent"],
                          "last_percent": stage[-1]["percent"]}))
    print("PASS: separate measured decode/conversion progress with redirected stderr")


if __name__ == "__main__":
    main()
