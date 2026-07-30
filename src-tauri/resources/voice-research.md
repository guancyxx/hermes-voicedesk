# JARVIS Voice Research

> Research date: 2026-07-30 | Platform: macOS (M4 Max) | Goal: Paul Bettany-like British AI butler voice for Hermes VoiceDesk TTS

---

## Summary of Findings

| # | Approach | Quality | Latency | Offline | JARVIS Match | Complexity |
|---|----------|---------|---------|---------|-------------|------------|
| 1 | **Edge-TTS** (`en-GB-RyanNeural`) | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ❌ (needs internet) | ⭐⭐⭐⭐⭐ | Low |
| 2 | **Kokoro TTS** (82M ONNX, `bm_lewis`) | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ✅ | ⭐⭐⭐ | Medium |
| 3 | **macOS `say` + enhanced ffmpeg** | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ✅ | ⭐⭐ | Low |
| 4 | **Orpheus TTS** (3B LLM-based) | ⭐⭐⭐⭐⭐ | ⭐⭐ | ✅ (needs GPU) | ⭐⭐⭐⭐ | High |
| 5 | **Piper TTS** (lightweight) | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ✅ | ⭐⭐ | Low |
| 6 | **JARVIS-ChatGPT** (Tacotron+IBM Watson) | ⭐⭐⭐⭐ | ⭐⭐ | ❌ (API keys) | ⭐⭐⭐⭐ | High |

**Winner: Edge-TTS (`en-GB-RyanNeural`)** — Best voice quality for JARVIS-like British AI butler, free, simple Python CLI, used by many JARVIS projects. Kokoro TTS as offline fallback.

---

## Detailed Analysis

### 1. Edge-TTS (`edge-tts`) ⭐ WINNER

**GitHub**: https://github.com/rany2/edge-tts (7.5k+ stars)

**Description**: Python CLI/tool that uses Microsoft Edge's free TTS engine. No API key required. Microsoft's neural voices are among the best in the industry.

**JARVIS-relevant voices**:
- `en-GB-RyanNeural` — British male, warm butler-like tone, closest to Paul Bettany's JARVIS
- `en-GB-ThomasNeural` — British male, slightly deeper/rumbly
- `en-GB-SoniaNeural` — British female, similar to FRIDAY

**Pros**:
- Free, no API key, no account needed
- Production-grade neural TTS quality
- Very JARVIS-like British male voices (Ryan)
- Simple CLI: `edge-tts --voice en-GB-RyanNeural --text "..." --write-media output.mp3`
- Supports SSML for fine control (pitch, rate, pauses)
- Tiny Python dependency (~100KB)
- Used by many JARVIS/AI assistant projects as their TTS backend

**Cons**:
- Requires internet connection
- ~200-500ms latency per call (network roundtrip)
- May be rate-limited eventually (rare in practice)
- Relies on Microsoft's service staying free

**Installation**:
```bash
pip3 install edge-tts
```

**Usage**:
```bash
echo "At your service, sir." | edge-tts --voice en-GB-RyanNeural --write-media /tmp/jarvis.mp3
afplay /tmp/jarvis.mp3
```

**JARVIS-specific SSML preset**:
```xml
<speak version="1.0" xmlns="http://www.w3.org/2001/10/synthesis">
  <voice name="en-GB-RyanNeural">
    <prosody rate="-5%" pitch="-2%">
      At your service, sir.
    </prosody>
  </voice>
</speak>
```
This slightly lowers rate and pitch for a more measured JARVIS delivery.

---

### 2. Kokoro TTS — Best Local/Offline Option

**GitHub**: https://github.com/nazdridoy/kokoro-tts (1,735 stars)
**Model**: Kokoro-82M (hexgrad/Kokoro-82M on HuggingFace)
**Engine**: ONNX Runtime — great Apple Silicon support

**Description**: Fast, local TTS using a compact 82M-parameter model. Produces surprisingly natural speech for its size. ONNX runtime runs efficiently on Apple Silicon (MPS/CoreML backend).

**British male voices**: `bm_lewis`, `bm_george`

**Pros**:
- Completely offline/local
- Fast inference (~50ms on M4 Max)
- Small model (~300MB)
- Voice blending for custom voices
- Streaming support
- Free and open-source (MIT)

**Cons**:
- Voice quality good but not quite Edge-TTS level
- British male voices limited (2 available)
- Requires Python + ONNX runtime + model download
- First-run model download ~300MB

**Installation**:
```bash
pip3 install kokoro-tts
kokoro-tts --help  # downloads model on first run
```

**Usage**:
```bash
echo "At your service, sir." | kokoro-tts - --stream --voice bm_lewis --lang en-gb
```

---

### 3. macOS `say` + Enhanced FFmpeg — Simplest Fallback

**Current approach** in player.rs: `say -v Daniel` → ffmpeg filters → afplay

**Available en_GB voices on macOS**:
- `Daniel` — Currently used, British male, decent for JARVIS
- `Oliver` — British male (may be available on newer macOS)
- `Eddy` — British male (multi-language)
- `Flo`, `Reed`, `Rocko`, `Sandy`, `Shelley` — other options

**Enhanced ffmpeg filter chain for JARVIS effect**:
```bash
ffmpeg -i raw.aiff -af \
  "asetrate=44100*0.95,atempo=1/0.95,\
   aecho=0.8:0.7:20:0.3,aecho=0.8:0.6:50:0.2,\
   highpass=f=100,lowpass=f=8000,\
   compand=attacks=0.003:decays=0.1:points=-90/-90|-30/-15|0/-3:gain=3" \
  processed.aiff
```

Changes from current filter: deeper pitch shift (5% vs 3%), tighter reverb delay for smaller room presence, added lowpass for warmer tone.

---

### 4. Orpheus TTS — Best Quality (Heavy)

**GitHub**: https://github.com/canopyai/Orpheus-TTS (6,266 stars)

**Description**: SOTA LLM-based TTS (Llama-3B backbone). Natural, emotional speech with zero-shot voice cloning. Can be finetuned on Paul Bettany's voice samples.

**Pros**:
- Most natural-sounding open-source TTS
- Zero-shot voice cloning — clone Paul Bettany from movie clips
- Emotion tags: `<laugh>`, `<sigh>`, `<chuckle>`
- Can be finetuned (300 samples for good results)

**Cons**:
- 3B parameters — needs ~6GB VRAM
- Slower inference (~1-3s on M4 Max)
- Complex Python setup (vLLM or transformers)
- No streaming support built-in
- Overkill for a desktop TTS assistant

**Verdict**: Use for finetuning a JARVIS voice model offline for later use, not as the real-time engine.

---

### 5. Piper TTS — Lightweight Local

**GitHub**: https://github.com/rhasspy/piper

**Description**: Fast, lightweight TTS optimized for embedded/Raspberry Pi. VITS-based.

**British voices**: `en_GB-alan` (male, medium), `en_GB-cori` (female), etc.

**Pros**:
- Very fast, very lightweight
- Good for low-resource environments

**Cons**:
- British male voices limited and quality is mediocre
- Not as natural as Kokoro or Edge-TTS
- Voice quality sounds robotic compared to neural TTS

---

### 6. JARVIS-ChatGPT (gia-guar) — Reference Implementation

**GitHub**: https://github.com/gia-guar/JARVIS-ChatGPT (456 stars)

**Description**: The most popular JARVIS-specific voice assistant on GitHub. Uses Tacotron model + IBM Watson for JARVIS voice synthesis.

**Pros**:
- Purpose-built for JARVIS voice
- Multiple synthetic voice options
- Complete voice assistant pipeline

**Cons**:
- Requires IBM Watson API (deprecated/discontinued?)
- Tacotron model is old (2020-era)
- Complex setup, many dependencies
- Not maintained recently

---

## Implementation Decision

### Chosen: **Edge-TTS as primary, macOS `say` + enhanced ffmpeg as fallback**

Edge-TTS `en-GB-RyanNeural` provides the best JARVIS-like British butler voice available today — free, simple, and high quality. The existing `say` + ffmpeg pipeline serves as a zero-dependency offline fallback.

### Implementation Architecture

```
speak(text)
  ├── if edge-tts available:
  │     edge-tts --voice en-GB-RyanNeural → mp3 → afplay
  ├── elif JARVIS mode + ffmpeg:
  │     say -v Daniel -o raw.aiff → ffmpeg filters → afplay
  └── else:
        say -v Daniel (direct)
```

### Future Improvements
1. **Kokoro TTS offline fallback**: When edge-tts is down, use Kokoro `bm_lewis` instead of say
2. **Orpheus finetuning**: Finetune Orpheus on Paul Bettany audio clips for a true JARVIS voice model
3. **Voice cloning**: Use 5-10 second JARVIS clips with XTTS-v2 or F5-TTS for zero-shot cloning

### Edge-TTS Voice Samples (en-GB-RyanNeural)
- "At your service, sir." — Warm, professional, British male
- "I've taken the liberty of adjusting your schedule." — Natural butler intonation
- The voice naturally sounds like an AI assistant, similar to JARVIS
