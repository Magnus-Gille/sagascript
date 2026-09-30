# chunk_merge golden fixtures

Recorded from the Python reference `parakeet_mlx/alignment.py` (Apache-2.0) plus the
window/merge loop in the bench runner `run_mlx_window.py`, using KlangAI's Pianissimo
MLX 8-bit model.

- `real_fleurs_sv_5min.json`: a 5-minute concatenation of Google FLEURS `sv_se` test
  clips (CC BY 4.0), run with (window, overlap) = (15 s, 2 s) and (30 s, 6 s). Per run:
  planned windows, offset-applied window tokens, every merge call (inputs, path,
  contiguous and LCS results) and the final tokens, sentences and text. The audio is not
  stored here.
- `synthetic.json`: hand-built edge cases (empty inputs, no overlap, identical tokens,
  forced LCS fallback, gap choice, cutoff branches) and a sentence/text case.

Tokens are `[id, text, start, duration]`; `end = start + duration`. Model start/duration
are rounded to 1e-6 before merging so recorded values are self-consistent. Merge inputs
omit the `a_prefix_len` leading tokens of `a` that end before the overlap region (they are
copied unchanged into the result).

Regenerate (uses the local model, GPU, under a minute; nothing is downloaded):

    ~/.cache/sagascript-bench/bench/.venv/bin/python \
      ~/.cache/sagascript-bench/bench/fixturegen/gen_merge_fixtures.py --out <this dir>

The generator is not part of the repo; a copy of its logic is described above.
