"""Generate 24-bit / 48 kHz mono edge-case WAVs for the flacenc spike (SAGA-547 review follow-up).

480000 samples = 117 blocks of 4096 + 768, so every file ends on a short final frame.
Usage: python3 -I gen-edge-signals.py <output dir>
"""
import random
import struct
import sys
import wave

OUT = sys.argv[1]
N = 480_000
LO, HI = -(1 << 23), (1 << 23) - 1


def write(name, samples):
    assert len(samples) == N
    with wave.open(f"{OUT}/{name}.wav", "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(3)
        w.setframerate(48000)
        buf = bytearray()
        for s in samples:
            assert LO <= s <= HI
            buf += struct.pack("<i", s)[:3]
        w.writeframes(bytes(buf))


rng = random.Random(547)
write("silence", [0] * N)
write("dc_max", [HI] * N)
write("dc_min", [LO] * N)
write("square_fullscale", [HI if (i // 24) % 2 == 0 else LO for i in range(N)])
write("white_fullscale", [rng.randint(LO, HI) for _ in range(N)])
write("alternating_extremes", [HI if i % 2 == 0 else LO for i in range(N)])
write("single_impulse", [HI if i == N // 2 else 0 for i in range(N)])
print("ok", N, "samples each")
