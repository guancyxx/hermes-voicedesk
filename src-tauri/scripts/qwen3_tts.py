#!/usr/bin/env python3
"""Qwen3-TTS synthesis via mlx-audio (offline fallback for Edge-TTS).

Usage:
    python3 qwen3_tts.py --text "Hello" --out /tmp/out.wav

Model: mlx-community/Qwen3-TTS-12Hz-1.7B-CustomVoice-6bit
Speaker: Ryan (British male, per README: English speakers are Ryan / Aiden).

Exit codes:
    0  — success, WAV written to --out
    3  — mlx_audio not installed
    4  — synthesis failed (model download error, OOM, etc.)
"""

import argparse
import sys

MODEL_ID = "mlx-community/Qwen3-TTS-12Hz-1.7B-CustomVoice-6bit"
SPEAKER = "Ryan"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--text", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--speaker", default=SPEAKER)
    parser.add_argument("--model", default=MODEL_ID)
    args = parser.parse_args()

    try:
        import numpy as np
        from mlx_audio.tts.utils import load_model
    except ImportError:
        print("MLX_AUDIO_NOT_INSTALLED", file=sys.stderr)
        return 3

    try:
        import numpy as np
        from scipy.io import wavfile  # noqa: F401  (optional; see fallback below)
        HAVE_SCIPY = True
    except ImportError:
        HAVE_SCIPY = False

    try:
        model = load_model(args.model)
        results = list(
            model.generate_custom_voice(
                text=args.text,
                speaker=args.speaker,
                language="English",
                instruct="Calm, refined British butler tone.",
            )
        )
        if not results or results[0].audio is None:
            print("QWEN3_TTS_EMPTY_RESULT", file=sys.stderr)
            return 4

        audio = results[0].audio
        samples = np.asarray(audio, dtype=np.float32)
        if samples.ndim > 1:
            samples = samples.reshape(-1)
        # Normalize to avoid clipping
        peak = float(np.max(np.abs(samples))) if samples.size else 0.0
        if peak > 0:
            samples = samples / peak * 0.95

        sr = int(getattr(results[0], "sample_rate", 24000) or 24000)
        pcm16 = (samples * 32767.0).astype(np.int16)

        if HAVE_SCIPY:
            from scipy.io import wavfile
            wavfile.write(args.out, sr, pcm16)
        else:
            # Pure-python WAV writer
            import wave
            with wave.open(args.out, "wb") as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(sr)
                w.writeframes(pcm16.tobytes())

        print(f"OK {sr}")
        return 0
    except Exception as e:  # noqa: BLE001
        print(f"QWEN3_TTS_ERROR: {e}", file=sys.stderr)
        return 4


if __name__ == "__main__":
    sys.exit(main())
