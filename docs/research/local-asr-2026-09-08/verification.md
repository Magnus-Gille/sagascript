# Verification record — 2026-09-08 checkpoint

The owner requested a suitable stop for resumption in cmux. This is a checked
research draft, not final readiness or an issue closure.

- `python3 docs/research/local-asr-2026-09-08/verify.py`: PASS. 37 repositories,
  143 selected publisher artifacts, 21 source baselines, six runtime archives,
  20 families (five shortlisted), 27 comparisons and 17 quality rows. Hash syntax,
  source-registry identity at its pinned revision, effect arithmetic, censored
  values, shortlist blockers, tensor subtotals and local Markdown links checked.
- `python3 -m unittest discover -s scripts/dictation_eval -p 'test_*.py'`:
  PASS, 127 existing tests. No scorer or application behavior changed.
- Source and publisher metadata were inspected; models, runtime binaries and
  fresh corpus were not downloaded or executed. Published scores are not a shared
  benchmark. Artifact SHA metadata was not locally hash-verified.
- Rust/Svelte builds are not applicable to the documentation/data-only changes.
- Independent cross-model review: **pending**, stopped before invocation at the
  owner's request. Conductor checked arithmetic and evidence qualifications.
- M5 `qwen3-coder-next-80b` bounded arithmetic leaf timed out with no usable output.
  `m5 --profile codex doctor` found public/private transport reachable but did not
  establish inference health. Conductor fallback; usefulness `redo`.

Resume with the worktree's `STATUS.md`. Review source attribution, source/runtime
qualification, batch-versus-dictation consistency and #222 acceptance before
calling the spike ready. No permission to push, open a PR, post an issue comment,
install models or deploy a remote inference host was granted.
