# Model sources and integrity manifest

Sagascript downloads model artifacts only from immutable Hugging Face commit
revisions and verifies the exact git-LFS SHA-256 and byte size before any
artifact is renamed into the model directory. Existing artifacts are verified
again before they are handed to whisper.cpp or ONNX Runtime. CoreML ZIPs are
verified before `ditto` extracts them.

GGML and ONNX verification results are cached only in process memory and are
bound to the artifact's canonical path, expected digest, size, modification
time, device, inode, and change time. Consequently, each app or CLI process
performs one full SHA-256 pass before that artifact's first native parse; later
uses in the same process avoid repeating a multi-gigabyte hash. No persisted
hash-cache sidecar is trusted. CoreML directories contain an archive-provenance
marker written only after the pinned ZIP passes verification; an older
unmarked directory is quarantined before whisper.cpp can discover it and a
verified copy is backfilled.

This protects the model download supply chain and detects ordinary local file
corruption. It does not attempt to defend against a malicious account that can
modify both Sagascript's model directory and its running process.

The metadata below was read from the official Hugging Face model API with
`blobs=true` on 2026-07-10. `SHA-256` is the repository's git-LFS object ID,
not a checksum inferred from a filename or mutable branch.

## Whisper transcription models

| Source | Revision | License declared by repository |
|---|---|---|
| `ggerganov/whisper.cpp` | `5359861c739e955e79d9a303bcbc70fb988958b1` | MIT |
| `KBLab/kb-whisper-tiny` | `76d796af43a50fa34321efa562c9b9887a187463` | Apache-2.0 |
| `KBLab/kb-whisper-base` | `1499d2d2f0c7ed545bd6f2eec85287cf8d8c8b38` | Apache-2.0 |
| `KBLab/kb-whisper-small` | `3564d61a42fc210ceaa55a22a96dd64478959c78` | Apache-2.0 |
| `KBLab/kb-whisper-medium` | `0abe10b9d7f75d0902656e5c06c5c4d549604dc5` | Apache-2.0 |
| `KBLab/kb-whisper-large` | `d5d5984b4d8f7c4847a8ea203f1976285fb28300` | Apache-2.0 |
| `NbAiLab/nb-whisper-tiny` | `8b38492d0e4111d5d6ad825e979cb082a2da013a` | Apache-2.0 |
| `NbAiLab/nb-whisper-base` | `2ab372b6baa181a22f54f18030cae3703402c59e` | Apache-2.0 |
| `NbAiLab/nb-whisper-small` | `e9bb5cb83cb74c96239fd506163aa97cff2fce4c` | Apache-2.0 |
| `NbAiLab/nb-whisper-medium` | `0ed074d5985bd56ca4140159a9dbffbc3fb5117e` | Apache-2.0 |
| `NbAiLab/nb-whisper-large` | `8c6249fdeeb4dcd05e5735a4c39640607eb6e4ac` | Apache-2.0 |

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `ggml-tiny.en.bin` | 77,704,715 | `921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f` |
| `ggml-tiny.bin` | 77,691,713 | `be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21` |
| `ggml-base.en.bin` | 147,964,211 | `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002` |
| `ggml-base.bin` | 147,951,465 | `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` |
| `ggml-small.en.bin` | 487,614,201 | `c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d` |
| `ggml-small.bin` | 487,601,967 | `1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b` |
| `ggml-medium.en.bin` | 1,533,774,781 | `cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356` |
| `ggml-medium.bin` | 1,533,763,059 | `6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208` |
| `ggml-large-v3-turbo.bin` | 1,624,555,275 | `1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69` |
| `ggml-large-v3-turbo-q8_0.bin` | 874,188,075 | `317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1` |
| KB tiny `ggml-model-q5_0.bin` | 29,875,738 | `98d46b7d23e5528d006e8a42e29eb0cb39b44bed94e1329f10f57d1fd15c658b` |
| KB base `ggml-model-q5_0.bin` | 55,295,450 | `aead29b356bca8840e72a8dc2286e2d69e6702639751a1e60cb3c8eacefec546` |
| KB small `ggml-model-q5_0.bin` | 175,209,680 | `6768836a51abc902e420c613153e6d418c90ea2774e913274d02ab23170225b7` |
| KB medium `ggml-model-q5_0.bin` | 539,212,484 | `7f8762e0ade9e0073674c0d5acae942a0b1ea98add9baa008ee89c94eaba43d0` |
| KB large `ggml-model-q5_0.bin` | 1,081,140,203 | `6d2863812d7410322bb7d8647a5c7260761300fa946714c9ed66d22bb30bcb19` |
| NB tiny `ggml-model-q5_0.bin` | 29,875,738 | `e5fb42192cdf31bea624a524d035e8895030b2bb4b31d4ea2a1ebf0ea8f57237` |
| NB base `ggml-model-q5_0.bin` | 55,295,450 | `dcb9f3ab963cd288974c826c1519ff73b78b2372e80d388a6ce94f29c6a5b40f` |
| NB small `ggml-model-q5_0.bin` | 175,209,680 | `2a9025afb6e825fc4ae6a46671e0cb2f43e62f1dec87270deea6fe61b5285a20` |
| NB medium `ggml-model-q5_0.bin` | 539,212,484 | `18733de634af639a43b0f8c5f5a2ea0920de4c5b32a5570ec130981581c0e5e7` |
| NB large `ggml-model-q5_0.bin` | 1,081,140,203 | `feb5951ae694a62cfeb81fb501f6cfa8cc50d96bcddb1e4e8215f7006bac23a2` |

## VAD and diarization models

| Artifact/source | Revision | License | Bytes | SHA-256 |
|---|---|---|---:|---|
| `ggml-org/whisper-vad` / `ggml-silero-v5.1.2.bin` | `9ffd54a1e1ee413ddf265af9913beaf518d1639b` | MIT | 885,098 | `29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf` |
| `csukuangfj/sherpa-onnx-pyannote-segmentation-3-0` / `model.onnx` | `9403a6902bb58e3d5ae8c7e77c3422de279db2e0` | Mirror has no license field; its model card identifies `pyannote/segmentation-3.0` as the source, whose model card declares MIT | 5,992,913 | `220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079` |
| `Wespeaker/wespeaker-voxceleb-resnet34-LM` / `voxceleb_resnet34_LM.onnx` | `f0c48c298fd835726c27956a5d617bad7115627e` | CC-BY-4.0 | 26,530,309 | `7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068` |

## CoreML encoder archives

All archives come from `ggerganov/whisper.cpp` revision
`5359861c739e955e79d9a303bcbc70fb988958b1` (MIT).

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `ggml-tiny.en-encoder.mlmodelc.zip` | 15,034,655 | `82b32eef73c94bb0c432a776a047b757d9525c26d84038a15d8798d7c8d1ee58` |
| `ggml-tiny-encoder.mlmodelc.zip` | 15,037,446 | `c88cbd2648e1f5415092bcf5256add463a0f19943e6938f46e8d4ffdebd47739` |
| `ggml-base.en-encoder.mlmodelc.zip` | 37,950,917 | `8cf860309e2449e2bdc8be834cf838ab2565747ecc8c0ef914ef5975115e192b` |
| `ggml-base-encoder.mlmodelc.zip` | 37,922,638 | `7e6ab77041942572f239b5b602f8aaa1c3ed29d73e3d8f20abea03a773541089` |
| `ggml-small.en-encoder.mlmodelc.zip` | 162,952,446 | `b2ef1c506378b825b4b4341979a93e1656b5d6c129f17114cfb8fb78aabc2f89` |
| `ggml-small-encoder.mlmodelc.zip` | 163,083,239 | `de43fb9fed471e95c19e60ae67575c2bf09e8fb607016da171b06ddad313988b` |
| `ggml-medium.en-encoder.mlmodelc.zip` | 566,993,085 | `cdc44fee3c62b5743913e3147ed75f4e8ecfb52dd7a0f0f7387094b406ff0ee6` |
| `ggml-medium-encoder.mlmodelc.zip` | 567,829,413 | `79b0b8d436d47d3f24dd3afc91f19447dd686a4f37521b2f6d9c30a642133fbd` |
| `ggml-large-v3-turbo-encoder.mlmodelc.zip` | 1,173,393,014 | `84bedfe895bd7b5de6e8e89a0803dfc5addf8c0c5bc4c937451716bf7cf7988a` |

The Q8 turbo GGML model intentionally reuses the same FP16 CoreML encoder as
the non-quantized turbo model.
