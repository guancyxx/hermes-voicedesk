#!/usr/bin/env python3
"""
Wake word detector using Picovoice Porcupine (or simple RMS fallback).

Listens for a wake word and prints JSON events to stdout.
Designed to run as a subprocess controlled by Hermes VoiceDesk.

Usage:
    python3 wake_word.py --keyword jarvis
    python3 wake_word.py --keyword jarvis --access-key YOUR_KEY

Output (stdout, one line per event):
    {"event": "debug", "message": "Using RMS fallback mode"}
    {"event": "ready", "keyword": "jarvis", "sample_rate": 16000, "mode": "rms"}
    {"event": "wake_word", "keyword": "jarvis", "mode": "rms", "index": 0}
    {"event": "stopped"}

Stop: send SIGTERM, or write "stop\n" to stdin.
"""

import sys
import json
import argparse
import signal
import struct
import select

RUNNING = True


def _signal_handler(sig, frame):
    global RUNNING
    RUNNING = False


signal.signal(signal.SIGTERM, _signal_handler)
signal.signal(signal.SIGINT, _signal_handler)


def _check_stdin():
    """Non-blocking check if stdin has 'stop' command."""
    if select.select([sys.stdin], [], [], 0)[0]:
        line = sys.stdin.readline()
        if line.strip() == "stop":
            return True
    return False


def emit(event_type, **kwargs):
    """Print a JSON event to stdout, flushed."""
    payload = {"event": event_type, **kwargs}
    print(json.dumps(payload), flush=True)


# ─── Porcupine mode ─────────────────────────────────────────────────────

def run_porcupine(keyword, sensitivity, access_key):
    """Use Picovoice Porcupine for keyword spotting."""
    try:
        import pvporcupine  # noqa: F811
    except ImportError:
        emit("error", message="pvporcupine not installed. Install: pip install pvporcupine")
        sys.exit(1)

    try:
        import pyaudio  # noqa: F811
    except ImportError:
        emit("error", message="pyaudio not installed. Install: pip install pyaudio")
        sys.exit(1)

    keywords = [keyword]
    pa = None
    audio_stream = None
    porcupine = None

    try:
        porcupine = pvporcupine.create(
            keywords=keywords,
            sensitivities=[sensitivity],
            access_key=access_key or "",
        )

        pa = pyaudio.PyAudio()
        audio_stream = pa.open(
            rate=porcupine.sample_rate,
            channels=1,
            format=pyaudio.paInt16,
            input=True,
            frames_per_buffer=porcupine.frame_length,
        )

        emit("ready", keyword=keyword,
             sample_rate=porcupine.sample_rate, mode="porcupine")

        global RUNNING
        while RUNNING:
            if _check_stdin():
                break

            pcm = audio_stream.read(
                porcupine.frame_length, exception_on_overflow=False
            )
            pcm = struct.unpack_from("h" * porcupine.frame_length, pcm)

            keyword_index = porcupine.process(pcm)
            if keyword_index >= 0:
                emit("wake_word", keyword=keywords[keyword_index],
                     index=keyword_index)

    except Exception as e:
        emit("error", message=str(e))
        sys.exit(1)
    finally:
        if audio_stream is not None:
            try:
                audio_stream.stop_stream()
                audio_stream.close()
            except Exception:
                pass
        if pa is not None:
            try:
                pa.terminate()
            except Exception:
                pass
        if porcupine is not None:
            try:
                porcupine.delete()
            except Exception:
                pass

    emit("stopped")


# ─── Simple RMS fallback (no external deps) ─────────────────────────────

def run_rms_fallback(keyword, sensitivity):
    """
    Simple RMS energy-based wake word detection using sounddevice.
    Any sustained loud sound triggers activation.
    Falls back to pyaudio if sounddevice not available.
    """
    sample_rate = 16000
    chunk_duration = 0.03  # 30ms frames
    chunk_size = int(sample_rate * chunk_duration)

    # Threshold: RMS value for normalized float audio [-1, 1]
    # sensitivity 0.5 → threshold ~0.01; 1.0 → ~0.005; 0.0 → ~0.05
    rms_threshold = 0.05 - (sensitivity * 0.045)

    # Frames of sustained speech needed to trigger
    trigger_frames = 30  # ~0.9 seconds at 30ms frames
    silence_reset_frames = 60  # ~1.8 seconds to reset

    speech_frames = 0
    silence_frames = 0

    emit("ready", keyword=keyword, sample_rate=sample_rate,
         mode="rms", threshold=round(rms_threshold, 5),
         trigger_frames=trigger_frames)

    # Try sounddevice first, then pyaudio, then subprocess
    audio_ok = False

    # ── Attempt 1: sounddevice ──
    if not audio_ok:
        try:
            import sounddevice as sd  # type: ignore
            import numpy as np  # type: ignore

            def _callback(indata, _frames, _time_info, status):
                nonlocal speech_frames, silence_frames
                if status:
                    return  # Overflow, skip
                global RUNNING
                if not RUNNING:
                    raise sd.CallbackStop()

                rms = float(np.sqrt(np.mean(indata ** 2)))
                if rms > rms_threshold:
                    speech_frames += 1
                    silence_frames = 0
                    if speech_frames >= trigger_frames:
                        emit("wake_word", keyword=keyword,
                             mode="rms", rms=round(rms, 6))
                        RUNNING = False
                        raise sd.CallbackStop()
                else:
                    silence_frames += 1
                    if silence_frames > silence_reset_frames:
                        speech_frames = 0

            with sd.InputStream(
                samplerate=sample_rate,
                channels=1,
                callback=_callback,
                blocksize=chunk_size,
                dtype='float32',
            ):
                while RUNNING:
                    if _check_stdin():
                        break
                    sd.sleep(100)

            audio_ok = True
        except ImportError:
            pass

    # ── Attempt 2: pyaudio ──
    if not audio_ok:
        try:
            import pyaudio  # type: ignore

            p = pyaudio.PyAudio()
            stream = p.open(
                rate=sample_rate,
                channels=1,
                format=pyaudio.paInt16,
                input=True,
                frames_per_buffer=chunk_size,
            )

            while RUNNING:
                if _check_stdin():
                    break

                try:
                    data = stream.read(chunk_size, exception_on_overflow=False)
                except Exception:
                    continue

                samples = struct.unpack_from("h" * chunk_size, data)
                sum_sq = sum(s ** 2 for s in samples)
                rms = (sum_sq / chunk_size) ** 0.5 / 32768.0

                if rms > rms_threshold:
                    speech_frames += 1
                    silence_frames = 0
                    if speech_frames >= trigger_frames:
                        emit("wake_word", keyword=keyword,
                             mode="rms-pyaudio", rms=round(rms, 6))
                        break
                else:
                    silence_frames += 1
                    if silence_frames > silence_reset_frames:
                        speech_frames = 0

            stream.stop_stream()
            stream.close()
            p.terminate()
            audio_ok = True
        except ImportError:
            pass

    if not audio_ok:
        emit("error", message="No audio library. Install: pip install sounddevice")
        sys.exit(1)

    emit("stopped")


# ─── Main ───────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Wake Word Detector")
    parser.add_argument(
        "--keyword",
        default="jarvis",
        help="Wake word to detect (e.g., jarvis, alexa, hey siri)",
    )
    parser.add_argument(
        "--sensitivity",
        type=float,
        default=0.5,
        help="Detection sensitivity 0.0-1.0 (lower = less sensitive)",
    )
    parser.add_argument(
        "--access-key",
        default=None,
        help="Picovoice AccessKey (uses Porcupine if provided, else RMS fallback)",
    )
    parser.add_argument(
        "--mode",
        default="auto",
        choices=["auto", "porcupine", "rms"],
        help="Detection mode: auto (try Porcupine first), porcupine, or rms",
    )
    args = parser.parse_args()

    # Determine mode
    if args.mode == "porcupine":
        use_porcupine = True
    elif args.mode == "rms":
        use_porcupine = False
    elif args.access_key:
        use_porcupine = True
    else:
        # Auto: try Porcupine if installed, else fallback
        try:
            import pvporcupine  # noqa: F811,F401
            use_porcupine = True
        except ImportError:
            use_porcupine = False

    if use_porcupine:
        emit("debug", message="Using Porcupine mode")
        run_porcupine(args.keyword, args.sensitivity, args.access_key)
    else:
        emit("debug", message="Using RMS fallback mode")
        run_rms_fallback(args.keyword, args.sensitivity)


if __name__ == "__main__":
    main()
