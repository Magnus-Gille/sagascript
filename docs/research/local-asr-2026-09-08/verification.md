# Verification record — 2026-09-08

The research report satisfies #222 through its explicit per-candidate blocker
alternative. No new model accuracy or product performance was measured; this is
research completion, not model adoption or GitHub issue closure.

- `python3 docs/research/local-asr-2026-09-08/verify.py`: PASS after corrections.
  37 repositories, 143 selected publisher artifacts, 21 source baselines, six
  runtime archives, 20 families (five shortlisted), 27 comparisons and 17 quality
  rows. Hash syntax, source-registry identity at its pinned revision, effect
  arithmetic, censored values, shortlist blockers, tensor subtotals and local
  Markdown links checked.
- `python3 -m unittest discover -s scripts/dictation_eval -p 'test_*.py'`:
  PASS, 127 existing tests at checkpoint. No scorer or application behavior changed;
  subsequent corrections affect only research prose and evidence qualifications.
- `git diff --check`: PASS after corrections; exact staged diff inspected before
  committing.
- Source and publisher metadata were inspected; models, runtime binaries and
  fresh corpus were not downloaded or executed. Published scores are not a shared
  benchmark. Artifact SHA metadata was not locally hash-verified.
- Rust/Svelte builds are not applicable to the documentation/data-only changes.

## Independent review and dispositions

Actual reviewer: **Claude Sonnet 5** (`claude-sonnet-5`, resolved from `sonnet`),
through configured Claude Code in plan mode, without tools, browser, MCP or session
persistence. Review input was the scoped research files, issue acceptance and
prior Finnish report. It did not independently fetch sources or run commands.
The CLI also reported auxiliary Haiku usage; Sonnet produced the review.

The first invocation returned no useful review, only an attempt to write a report.
A retry with an explicit direct-text system prompt produced findings and assessed
all six acceptance bullets as met, with no blocking findings. Conductor verified
its hypotheses rather than accepting the verdict as factual verification:

- Corrected the supported minor finding: README's FI Base-to-Finnish-Tiny relative
  reduction is 6.94% when calculated from the displayed rounded WER, matching JSON.
  The prior report's 6.95% remains historical evidence and was not edited.
- Made the RASMUS limitation explicit: this snapshot extracts R2; exact R1/R2
  reproduction remains blocked by checkpoints and frozen evaluation manifests.
- Other numbered review observations were self-dismissed or informational; they
  did not identify further defects. Installed CLI/source equivalence remains
  explicitly unclaimed. Offline consistency is independently checked by the
  Python validator; M5's failure does not invalidate that executed check.

Conductor corrections before the completed review: NB paper citation now points
to tables 5–7; NB Medium Q5 belongs in the 250 MB–1 GB download tier; two prior FI
comparison rows now mark matched tensor/execution precision unknown instead of
true. Added explicit reconciliation against the six issue acceptance bullets.

## Delegation and remaining limits

M5 `qwen3-coder-next-80b` bounded arithmetic leaf timed out with no usable output.
`m5 --profile codex doctor` found public/private transport reachable but did not
establish inference health. Conductor fallback; usefulness `redo`.

Fresh held-out audio, platform/runtime adapters, full installed/RAM accounting and
local artifact execution remain the stated next experiment's prerequisites. No
permission to push, open a PR, post an issue comment, install models or deploy a
remote inference host was granted. The finished work is committed locally.
