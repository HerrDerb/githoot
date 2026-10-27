#!/usr/bin/env python3
"""Synthesize the hoot, `assets/hoot.mp3`, from scratch.

The clip is this project's own work: every sample comes out of the numbers below, so it is covered by
the Unlicense like the rest of the repository. This script is the record of that, and running it
again reproduces the shipped clip.

    python3 scripts/make-hoot.py

Needs numpy and scipy (`pip install --user numpy scipy`), and `ffmpeg` on `PATH` for the MP3 encode.

## What it is

Five legato hoots, short-long-long-short-long, gliding between roughly 250 and 380 Hz. The voice is a
near-pure tone with faint 2nd and 3rd harmonics that fade as the pitch climbs, which is what keeps it
soft rather than buzzy. Distance comes from a short room, a filter on the highs and one quiet, darker
echo, not from a long tail: the hoot is a notification, so it should end quickly.

The first second is silence, on purpose. A Bluetooth headset wakes its audio link only when a stream
starts and swallows whatever plays first. See `HOOT` in `src/sound.rs`.
"""

import subprocess
import tempfile
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from scipy.signal import butter, fftconvolve, sosfilt

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "hoot.mp3"

SR = 44100

# One pitch curve per hoot: (position within the hoot 0..1, Hz), eased between points.
# Slot lengths in seconds; the hoots run into each other, the loudness envelope makes the gaps.
HOOTS = [
    (0.101, [(0, 285), (0.6, 385), (1, 355)]),
    (0.203, [(0, 345), (0.25, 330), (0.45, 368), (1, 280)]),
    (0.240, [(0, 296), (0.35, 352), (1, 258)]),
    (0.077, [(0, 283), (0.8, 392), (1, 370)]),
    (0.342, [(0, 385), (0.25, 302), (0.5, 343), (1, 242)]),
]
PITCH = 0.968  # whole hoot, so the voice centres near 334 Hz

# Loudness over the whole call: (ms from onset, dB). Each hoot swells quickly to a pointed peak and
# falls into a quieter tail, which is what makes them sound clear and short rather than held.
O2, O3, O4, O5 = 101, 304, 544, 621
ENVELOPE = [
    (0, -60), (12, -12), (24, -4), (36, 0), (46, -3), (58, -6), (75, -12), (O2, -17),
    (O2 + 35, -14), (O2 + 52, -6), (O2 + 62, -4), (O2 + 74, 0), (O2 + 88, -4), (O2 + 140, -6),
    (O2 + 175, -12), (O3, -22),
    (O3 + 20, -14), (O3 + 32, -5), (O3 + 45, -1), (O3 + 105, -2.5), (O3 + 155, -6), (O3 + 185, -12),
    (O4, -24),
    (O4 + 22, -12), (O4 + 32, -6), (O4 + 46, 0), (O4 + 58, -6), (O4 + 70, -12), (O5, -13),
    (O5 + 12, -6), (O5 + 35, -4), (O5 + 60, -6), (O5 + 80, -9), (O5 + 97, -6), (O5 + 108, -2),
    (O5 + 114, 0), (O5 + 122, -2), (O5 + 168, -6), (O5 + 200, -12), (O5 + 255, -21), (O5 + 300, -36),
    (O5 + 315, -60),
]

VIBRATO_HZ, VIBRATO_CENTS = 5.5, 8
H2_DB, H3_DB = -40, -60
H_TILT_DB = 9  # overtones lose this much per 50 Hz above 330 Hz: a damp voice, not a bright one
AIR_LOWPASS_HZ = 850  # distance: air and foliage swallow the highs first
ROOM_RT60, ROOM_PREDELAY, ROOM_WET_DB = 0.12, 0.02, -10
ECHO_DELAY, ECHO_DB, ECHO_LOWPASS_HZ = 0.14, -19, 1200
PEAK_DBFS = -4.2  # lands the clip at about -12 LUFS, the loudness the previous clip had


def ease(u: np.ndarray) -> np.ndarray:
    u = np.clip(u, 0, 1)
    return u * u * (3 - 2 * u)


def through(points, x: np.ndarray) -> np.ndarray:
    """Eases `x` through `(x, y)` points, holding the last value past the end."""
    y = np.full(len(x), float(points[-1][1]))
    for (x0, y0), (x1, y1) in zip(points, points[1:]):
        m = (x >= x0) & (x < x1)
        y[m] = y0 + (y1 - y0) * ease((x[m] - x0) / (x1 - x0))
    return y


def voice() -> np.ndarray:
    """The dry hoot, mono, without the lead-in."""
    pitch = np.concatenate(
        [through(points, np.arange(int(slot * SR)) / (slot * SR)) for slot, points in HOOTS]
    )
    t = np.arange(len(pitch)) / SR
    pitch = pitch * PITCH * 2 ** (VIBRATO_CENTS / 1200 * np.sin(2 * np.pi * VIBRATO_HZ * t))
    # One continuous phase, so nothing clicks where one hoot hands over to the next.
    phase = 2 * np.pi * np.cumsum(pitch) / SR
    tilt = 10 ** (-H_TILT_DB * np.clip(pitch - 330, -60, None) / 50 / 20)
    y = (
        np.sin(phase)
        + tilt * 10 ** (H2_DB / 20) * np.sin(2 * phase + 0.3)
        + tilt * 10 ** (H3_DB / 20) * np.sin(3 * phase + 1.1)
    )
    ms = t * 1000
    loudness = 10 ** (through(ENVELOPE, ms) / 20)
    loudness[ms >= ENVELOPE[-1][0]] = 0
    y *= loudness
    return sosfilt(butter(2, AIR_LOWPASS_HZ, "lowpass", fs=SR, output="sos"), y)


def room(seed: int) -> np.ndarray:
    """A short, soft room: decaying noise that swells in after the pre-delay, so it thickens the
    voice instead of slapping back at it."""
    n = int((ROOM_RT60 * 1.2 + ROOM_PREDELAY) * SR)
    t = np.arange(n) / SR
    r = np.random.default_rng(seed).standard_normal(n) * np.exp(-6.91 * (t - ROOM_PREDELAY) / ROOM_RT60)
    r[t < ROOM_PREDELAY] = 0
    r *= ease((t - ROOM_PREDELAY) / 0.04)
    r = sosfilt(butter(1, 2500, "lowpass", fs=SR, output="sos"), r)
    return r / np.sqrt((r**2).sum())


def hoot() -> np.ndarray:
    dry = np.concatenate([np.zeros(SR), voice(), np.zeros(SR)])
    dry_rms = np.sqrt((dry**2).mean())
    dark = sosfilt(butter(2, ECHO_LOWPASS_HZ, "lowpass", fs=SR, output="sos"), dry)
    channels = []
    # Fixed seeds, so a rerun is the same clip. Two of them, so left and right tails differ and give
    # width without moving the owl; the echo arrives 7 ms later on the right for the same reason.
    for seed, echo_shift in ((1, 0.0), (2, 0.007)):
        wet = fftconvolve(dry, room(seed))[: len(dry)]
        wet *= 10 ** (ROOM_WET_DB / 20) * dry_rms / np.sqrt((wet**2).mean())
        out = dry + wet
        k = int((ECHO_DELAY + echo_shift) * SR)
        out[k:] += 10 ** (ECHO_DB / 20) * dark[:-k]
        channels.append(out)
    stereo = np.stack(channels, 1)
    return (stereo * 10 ** (PEAK_DBFS / 20) / np.abs(stereo).max()).astype(np.float32)


def main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        wav = Path(tmp) / "hoot.wav"
        wavfile.write(wav, SR, hoot())
        # The same format the players in `src/sound.rs` were chosen for: MP3, 44.1 kHz stereo.
        subprocess.run(
            ["ffmpeg", "-v", "error", "-y", "-i", str(wav), "-c:a", "libmp3lame", "-b:a", "128k",
             "-map_metadata", "-1", str(OUT)],
            check=True,
        )
    print(f"wrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
