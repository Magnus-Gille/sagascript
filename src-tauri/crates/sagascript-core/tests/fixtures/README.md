# Transcription diagnostic fixtures

`ordinary-word-loop.json` is a synthetic, shareable representation of a
long-form ambient-audio hallucination. It contains 40 seconds of repeated
ordinary words with deliberately low `no_speech_prob` values, matching the
failure shape without including private source audio or transcript content.

The repetition diagnostic test loads this file directly. Keep its timestamps
long enough to exercise the sustained-loop threshold; short rhetorical
repetition belongs in the separate false-positive unit test.
