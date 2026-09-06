# Test Audio Files

## English (public US presidential speech)

`english-jfk.wav` is the 11-second JFK inaugural speech excerpt used by
[whisper.cpp](https://github.com/ggml-org/whisper.cpp/blob/52a939a2a762224e255d366c1182b2af4dd1a032/samples/jfk.wav).
It contains the public phrase "ask not what your country can do for you".
Source revision: `52a939a2a762224e255d366c1182b2af4dd1a032`.
SHA-256: `59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e`.
Use `--expect-word country` for the repeated English dictation gate. No private
dictation is stored in this fixture.

## Norwegian (NPSC -- Norwegian Parliamentary Speech Corpus)
Source: NbAiLab/NPSC (CC0 license, Norwegian National Library)

### norwegian-short-3s.mp3
- **Duration:** ~3.4s
- **Ground truth:** "Stortingets mote er lovlig satt"
- **Speaker:** Marit Nybakk

### norwegian-medium-8s.mp3
- **Duration:** ~8s
- **Ground truth:** "representantene Fredric Helen Fredric Holen Bjordal og Trond Giske som har vaert permitterte har igjen tatt sete"

## Swedish
To get Swedish test audio from Rixvox (KBLab/rixvox):
```python
from datasets import load_dataset
import soundfile as sf
ds = load_dataset("KBLab/rixvox", split="test", streaming=True)
for sample in ds:
    if 3 < sample["duration"] < 10:
        sf.write("swedish-test.wav", sample["audio"]["array"], sample["audio"]["sampling_rate"])
        print(sample["text"])
        break
```

## How to test
1. Run `cargo tauri dev` from the repo root
2. Open Settings > Transcribe tab
3. Drag one of these files onto the drop zone, or click "Open File..."
4. Compare the transcription result against the ground truth above
