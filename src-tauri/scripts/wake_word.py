#!/usr/bin/env python3
"""
Wake word detector using Picovoice Porcupine.

Listens for a built-in wake word and prints JSON events to stdout.
Designed to run as a subprocess controlled by Hermes VoiceDesk.

Usage:
    python3 wake_word.py --keyword picovoice
    python3 wake_word.py --keyword "hey siri"

Output (stdout, one line per event):
    {"event": "ready", "keyword": "picovoice", "sample_rate": 16000}
    {"event": "wake_word", "keyword": "picovoice", "index": 0}
    {"event": "stopped"}

Stop: send SIGTERM, or write "stop\n" to stdin.
"""

import sys
import json
import argparse
import signal
import struct
import pvporcupine
import pyaudio

running = True


def signal_handler(sig, frame):
    global running
    running = False


signal.signal(signal.SIGTERM, signal_handler)
signal.signal(signal.SIGINT, signal_handler)


def check_stdin():
    """Non-blocking check if stdin has 'stop' command."""
    import select
    if select.select([sys.stdin], [], [], 0)[0]:
        line = sys.stdin.readline()
        if line.strip() == "stop":
            return True
    return False


def main():
    parser = argparse.ArgumentParser(description="Porcupine Wake Word Detector")
    parser.add_argument(
        "--keyword",
        default="picovoice",
        choices=list(pvporcupine.KEYWORDS),
        help="Wake word to detect",
    )
    parser.add_argument(
        "--sensitivity",
        type=float,
        default=0.5,
        help="Detection sensitivity 0.0-1.0",
    )
    parser.add_argument(
        "--access-key",
        default=None,
        help="Picovoice AccessKey (optional for built-in keywords)",
    )
    args = parser.parse_args()

    keywords = [args.keyword]
    pa = None      # type: pyaudio.PyAudio | None
    audio_stream = None
    porcupine = None

    try:
        porcupine = pvporcupine.create(
            access_key=args.access_key,
            keywords=keywords,
            sensitivities=[args.sensitivity],
        )

        pa = pyaudio.PyAudio()
        audio_stream = pa.open(
            rate=porcupine.sample_rate,
            channels=1,
            format=pyaudio.paInt16,
            input=True,
            frames_per_buffer=porcupine.frame_length,
        )

        print(
            json.dumps({
                "event": "ready",
                "keyword": args.keyword,
                "sample_rate": porcupine.sample_rate,
            }),
            flush=True,
        )

        while running:
            if check_stdin():
                break

            pcm = audio_stream.read(
                porcupine.frame_length, exception_on_overflow=False
            )
            pcm = struct.unpack_from("h" * porcupine.frame_length, pcm)

            keyword_index = porcupine.process(pcm)
            if keyword_index >= 0:
                print(
                    json.dumps({
                        "event": "wake_word",
                        "keyword": keywords[keyword_index],
                        "index": keyword_index,
                    }),
                    flush=True,
                )

    except Exception as e:
        print(json.dumps({"event": "error", "message": str(e)}), flush=True)
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

    print(json.dumps({"event": "stopped"}), flush=True)


if __name__ == "__main__":
    main()
