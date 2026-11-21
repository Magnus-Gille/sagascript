# Swedish one-hour demo file with exact reference

Builds `swedish-fleurs-60min.wav` (about 60 min, 16 kHz mono) from Google FLEURS
sv_se test (read speech, CC BY 4.0, exact reference text), plus the reference and a
manifest. It is the same recipe and clip selection as the README benchmark (307 clips, about 3,611 s); it is read speech, not a
conversation.

```sh
python3 scripts/demo-swedish-hour/build.py ~/sagascript-demo     # downloads ~0.5 GB once
```

Score a transcript, or do it all in one go:

```sh
python3 scripts/demo-swedish-hour/wer.py REFERENCE.txt HYPOTHESIS.txt
python3 scripts/demo-swedish-hour/run.py ~/sagascript-demo/swedish-fleurs-60min.wav \
        ~/sagascript-demo/swedish-fleurs-60min.reference.txt
```

## Demo prompt for an agent

> Transcribe `~/sagascript-demo/swedish-fleurs-60min.wav` with Sagascript, using
> `scripts/demo-swedish-hour/run.py` with the reference
> `swedish-fleurs-60min.reference.txt` next to it. Tell me the word error rate and
> how many seconds it took.

`run.py` shows each step live (files, engine start, transcription with a progress bar,
WER) and ends with a summary table: WER, errors, seconds (wall time of the whole
command), audio length and realtime factor. `--json` prints only one JSON object
(for agents); `NO_COLOR=1` turns colours off. Stdlib only.
Normalisation is lowercase, punctuation removed, no number normalisation.
