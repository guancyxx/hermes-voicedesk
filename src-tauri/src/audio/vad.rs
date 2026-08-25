//! Voice Activity Detection engine.
//!
//! Primary path: Silero VAD v5 ONNX model (576-sample frames @ 16kHz) with
//! LSTM hidden state carried between frames. Falls back to the legacy
//! RMS energy threshold if the ONNX model cannot be loaded (model missing,
//! ort runtime failure) — the app stays functional either way.

use std::sync::OnceLock;

/// Probability threshold for classifying a frame as speech (Silero).
const SPEECH_PROB_THRESHOLD: f32 = 0.5;

/// RMS energy threshold for the fallback path (normalized f32 in [0,1]).
const RMS_FALLBACK_THRESHOLD: f32 = 0.01;

/// Consecutive speech frames (576 @ 16kHz = 36ms each) to confirm start (~110ms).
const START_CONFIRM_FRAMES: u32 = 3;

/// Consecutive silence frames to confirm end. 22 frames ≈ 800ms.
const END_CONFIRM_FRAMES: u32 = 22;

/// Maximum speech duration in Silero frames (30s).
const MAX_SPEECH_FRAMES: u32 = 833;

/// Pre-speech padding in frames (~500ms of 16kHz audio kept before trigger).
const PRE_SPEECH_FRAMES: usize = 14;

/// Silero v5 frame size at 16kHz.
const SILERO_FRAME: usize = 576;

/// Silero v5 expects 16kHz input.
const SILERO_SR: u32 = 16_000;

#[derive(Debug, Clone)]
pub enum VadEvent {
    SpeechStart,
    SpeechEnd { audio_data: Vec<i16> },
}

/// Shared ONNX session + input/output names. Initialized once per process.
struct SileroSession {
    /// rc.13's `Session::run` needs `&mut self`; we share the session
    /// process-wide, so guard it with a Mutex.
    session: std::sync::Mutex<ort::session::Session>,
    input_name: String,
    state_name: String,
    sr_name: String,
    output_name: String,
    /// Name of the second model output (the new LSTM state tensor).
    state_out_name: String,
}

static SILERO: OnceLock<Option<SileroSession>> = OnceLock::new();

/// Locate silero_vad.onnx: next to the executable (bundled resource), then in
/// the project tree (development: src-tauri/resources/).
fn find_model_path() -> Option<std::path::PathBuf> {
    // 1. Next to executable / in bundle Resources (tauri puts resources there)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidates = [
                dir.join("silero_vad.onnx"),
                dir.join("resources").join("silero_vad.onnx"),
                // Packaged .app: exe is in Contents/MacOS, resources in
                // Contents/Resources/resources/
                dir.join("../Resources/resources/silero_vad.onnx"),
            ];
            for c in candidates {
                if c.exists() {
                    return Some(c);
                }
            }
        }
    }

    // 2. Project tree (development)
    let mut current = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    loop {
        let candidate = current
            .join("src-tauri")
            .join("resources")
            .join("silero_vad.onnx");
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }

    None
}

fn load_silero() -> Option<SileroSession> {
    let path = match find_model_path() {
        Some(p) => p,
        None => {
            log::warn!("Silero VAD: model silero_vad.onnx not found — falling back to RMS VAD");
            return None;
        }
    };

    let start = std::time::Instant::now();
    let session = match ort::session::Session::builder().and_then(|mut b| b.commit_from_file(&path))
    {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "Silero VAD: failed to load ONNX model ({:?}): {} — falling back to RMS VAD",
                path,
                e
            );
            return None;
        }
    };

    let input_name = session
        .inputs()
        .first()
        .map(|i| i.name().to_string())
        .unwrap_or_else(|| "input".to_string());
    let state_name = session
        .inputs()
        .get(1)
        .map(|i| i.name().to_string())
        .unwrap_or_else(|| "state".to_string());
    let sr_name = session
        .inputs()
        .get(2)
        .map(|i| i.name().to_string())
        .unwrap_or_else(|| "sr".to_string());
    let output_name = session
        .outputs()
        .first()
        .map(|o| o.name().to_string())
        .unwrap_or_else(|| "output".to_string());
    let state_out_name = session
        .outputs()
        .get(1)
        .map(|o| o.name().to_string())
        .unwrap_or_else(|| "stateN".to_string());

    log::info!(
        "Silero VAD v5 model loaded from {:?} in {:?} (inputs: {}, {}, {})",
        path,
        start.elapsed(),
        input_name,
        state_name,
        sr_name
    );

    Some(SileroSession {
        session: std::sync::Mutex::new(session),
        input_name,
        state_name,
        sr_name,
        output_name,
        state_out_name,
    })
}

fn silero() -> Option<&'static SileroSession> {
    SILERO.get_or_init(load_silero).as_ref()
}

/// Simple linear-phase FIR-free decimation: 48kHz → 16kHz is exactly 3:1, so
/// plain third-sample picking is adequate for VAD purposes (anti-aliasing is
/// not critical for a speech/silence classifier).
struct Downsampler3to1 {
    /// Fractional phase accumulator for non-integer ratios (unused when exact 3:1).
    ratio: f32,
    pos: f32,
    out: Vec<i16>,
}

impl Downsampler3to1 {
    fn new(input_rate: u32, output_rate: u32) -> Self {
        Self {
            ratio: input_rate as f32 / output_rate as f32,
            pos: 0.0,
            out: Vec::with_capacity(1024),
        }
    }

    fn process(&mut self, samples: &[i16]) -> &[i16] {
        self.out.clear();
        // For 3:1 this picks every 3rd sample; rounding keeps drift < 1 sample.
        while (self.pos as usize) < samples.len() {
            self.out.push(samples[self.pos as usize]);
            self.pos += self.ratio;
        }
        self.pos -= samples.len() as f32;
        &self.out
    }
}

pub struct VadEngine {
    /// Input sample rate of the mic stream (e.g. 48kHz).
    input_rate: u32,
    downsampler: Downsampler3to1,
    /// 16kHz samples pending the next 576-sample Silero frame.
    frame_buf: Vec<f32>,
    /// Silero v5 LSTM state: [2, 1, 128].
    state: Vec<f32>,
    /// Pending 16kHz i16 samples for buffering / speech segments.
    pcm_buf: std::collections::VecDeque<i16>,
    speech_segment: Vec<i16>,
    speech_frames: u32,
    silence_frames: u32,
    total_frames: u32,
    is_speaking: bool,
}

impl VadEngine {
    pub fn new(sample_rate: u32) -> Self {
        let silero_active = silero().is_some();
        if silero_active {
            log::info!(
                "VadEngine: using Silero VAD v5 (input {}Hz → 16kHz)",
                sample_rate
            );
        } else {
            log::warn!("VadEngine: Silero unavailable — RMS fallback active");
        }

        Self {
            input_rate: sample_rate,
            downsampler: Downsampler3to1::new(sample_rate, SILERO_SR),
            frame_buf: Vec::with_capacity(SILERO_FRAME),
            state: vec![0.0; 2 * 1 * 128],
            pcm_buf: std::collections::VecDeque::with_capacity(SILERO_FRAME * PRE_SPEECH_FRAMES),
            speech_segment: Vec::new(),
            speech_frames: 0,
            silence_frames: 0,
            total_frames: 0,
            is_speaking: false,
        }
    }

    /// True when the Silero model is loaded and active.
    pub fn is_silero_active(&self) -> bool {
        silero().is_some()
    }

    /// Feed a chunk of mic samples (any length, at `input_rate`). Returns every
    /// event produced by the chunk. Audio in `SpeechEnd` is 16kHz i16.
    pub fn process_samples(&mut self, samples: &[i16]) -> Vec<VadEvent> {
        if samples.is_empty() {
            return Vec::new();
        }

        let downsampled = self.downsampler.process(samples).to_vec();
        for &s in downsampled.iter() {
            self.pcm_buf.push_back(s);
        }
        let max_buf = SILERO_FRAME * PRE_SPEECH_FRAMES;
        while self.pcm_buf.len() > max_buf {
            self.pcm_buf.pop_front();
        }

        // Convert buffered tail into frames
        let mut events = Vec::new();
        for &s in downsampled.iter() {
            self.frame_buf.push(s as f32 / 32768.0);
        }

        while self.frame_buf.len() >= SILERO_FRAME {
            let frame: Vec<f32> = self.frame_buf.drain(..SILERO_FRAME).collect();
            let is_speech = match self.classify_frame(&frame) {
                Some(p) => p > SPEECH_PROB_THRESHOLD,
                // Classification failed mid-run → degrade to RMS for this frame.
                None => {
                    let sum: f64 = frame.iter().map(|&x| (x as f64).powi(2)).sum();
                    let rms = (sum / frame.len() as f64).sqrt() as f32;
                    rms > RMS_FALLBACK_THRESHOLD
                }
            };

            if let Some(ev) = self.handle_decision(is_speech) {
                events.push(ev);
            }
        }

        events
    }

    /// Legacy 30ms-frame entry point kept for API compatibility. If capture.rs
    /// still calls process_frame with input-rate frames, we just route through
    /// the new pipeline.
    pub fn process_frame(&mut self, frame: &[i16]) -> Vec<VadEvent> {
        self.process_samples(frame)
    }

    /// Return raw Silero probabilities for complete frames in `samples`.
    /// Intended for a separate, strict barge-in detector instance.
    pub fn process_probabilities(&mut self, samples: &[i16]) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }
        let downsampled = self.downsampler.process(samples).to_vec();
        self.frame_buf.extend(
            downsampled
                .into_iter()
                .map(|sample| sample as f32 / 32768.0),
        );
        let mut probabilities = Vec::new();
        while self.frame_buf.len() >= SILERO_FRAME {
            let frame: Vec<f32> = self.frame_buf.drain(..SILERO_FRAME).collect();
            let probability = self.classify_frame(&frame).unwrap_or_else(|| {
                let sum: f64 = frame.iter().map(|&x| (x as f64).powi(2)).sum();
                let rms = (sum / frame.len() as f64).sqrt() as f32;
                if rms > RMS_FALLBACK_THRESHOLD {
                    1.0
                } else {
                    0.0
                }
            });
            probabilities.push(probability);
        }
        probabilities
    }

    /// Run one 576-sample frame through the ONNX model. Returns speech
    /// probability, or None if inference failed.
    fn classify_frame(&mut self, frame: &[f32]) -> Option<f32> {
        use ort::value::Tensor;

        let sil = silero()?;
        let input = Tensor::from_array((vec![1usize, SILERO_FRAME], frame.to_vec())).ok()?;
        let state =
            Tensor::from_array((vec![2usize, 1usize, 128usize], self.state.clone())).ok()?;
        let sr = Tensor::from_array((vec![1usize], vec![SILERO_SR as i64])).ok()?;

        // ort rc.10's SessionOutputs borrows the session, so extract both the
        // probability and the new LSTM state while the lock is held.
        let (prob, new_state) = {
            let mut session = sil.session.lock().ok()?;
            let outputs = session
                .run(ort::inputs![
                    sil.input_name.as_str() => input,
                    sil.state_name.as_str() => state,
                    sil.sr_name.as_str() => sr,
                ])
                .ok()?;

            // Speech probability
            let (_, prob_data) = outputs[sil.output_name.as_str()]
                .try_extract_tensor::<f32>()
                .ok()?;
            let prob = *prob_data.first()?;

            // New LSTM state (second model output). We can't call
            // session.outputs() while `outputs` borrows the session, so take
            // the output name from the model metadata captured at load time.
            let state_name = sil.state_out_name.clone();
            let new_state = outputs[state_name.as_str()]
                .try_extract_tensor::<f32>()
                .ok()
                .and_then(|(_, s)| Some(s.to_vec()));

            (prob, new_state)
        };

        if let Some(s) = new_state {
            self.state = s;
        }
        Some(prob)
    }

    /// Apply speech/silence hysteresis and manage segment buffering.
    fn handle_decision(&mut self, is_speech: bool) -> Option<VadEvent> {
        if !self.is_speaking {
            if is_speech {
                self.speech_frames += 1;
                self.silence_frames = 0;
                if self.speech_frames >= START_CONFIRM_FRAMES {
                    self.is_speaking = true;
                    self.speech_frames = 0;
                    self.total_frames = 0;
                    self.silence_frames = 0;
                    self.speech_segment.clear();
                    self.speech_segment.extend(self.pcm_buf.iter().copied());
                    return Some(VadEvent::SpeechStart);
                }
            } else {
                self.speech_frames = 0;
            }
        } else {
            self.total_frames += 1;
            self.speech_segment.extend(self.pcm_buf.drain(..));

            if !is_speech {
                self.silence_frames += 1;
                if self.silence_frames >= END_CONFIRM_FRAMES
                    || self.total_frames >= MAX_SPEECH_FRAMES
                {
                    self.is_speaking = false;
                    self.silence_frames = 0;
                    let audio_data = std::mem::take(&mut self.speech_segment);
                    return Some(VadEvent::SpeechEnd { audio_data });
                }
            } else {
                self.silence_frames = 0;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_confirmation_is_about_eight_hundred_milliseconds() {
        assert_eq!(END_CONFIRM_FRAMES, 22);
        assert_eq!(END_CONFIRM_FRAMES as usize * SILERO_FRAME, 12_672);
    }

    #[test]
    fn empty_chunks_produce_no_events_or_probabilities() {
        let mut vad = VadEngine::new(48_000);
        assert!(vad.process_samples(&[]).is_empty());
        assert!(vad.process_probabilities(&[]).is_empty());
    }
}

/// Input sample rate this engine was constructed with (kept for callers).
impl VadEngine {
    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }
}
