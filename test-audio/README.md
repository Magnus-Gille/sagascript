# Test Audio Files for FlowDictate

## Norwegian (NPSC — Norwegian Parliamentary Speech Corpus)
Source: NbAiLab/NPSC (CC0 license, Norwegian National Library)

### norwegian-short-3s.mp3
- **Duration:** ~3.4s
- **Ground truth:** "Stortingets møte er lovlig satt"
- **Speaker:** Marit Nybakk

### norwegian-medium-8s.mp3
- **Duration:** ~8s
- **Ground truth:** "representantene Fredric Helen Fredric Holen Bjørdal og Trond Giske som har vært permitterte har igjen tatt sete"

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
1. Run `npm run tauri dev` from the flowdictate-tauri directory
2. Open Settings > Transcribe tab
3. Drag one of these files onto the drop zone, or click "Open File..."
4. Compare the transcription result against the ground truth above
