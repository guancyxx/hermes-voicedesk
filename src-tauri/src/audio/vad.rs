//! Voice Activity Detection engine.
//!
//! Uses a simple RMS-based energy threshold for now (MVP).
//! Will be replaced with silero-vad ONNX in Phase 1.5.

/// RMS energy threshold for speech detection.
const SPEECH_THRESHOLD: f64 = 50.0;

/// Number of consecutive speech frames to confirm start.
const START_CONFIRM_FRAMES: u32 = 5;

/// Number of consecutive silence frames to confirm end.
const END_CONFIRM_FRAMES: u32 = 15;

/// Maximum speech duration in frames.
const MAX_SPEECH_FRAMES: u32 = 1000;

/// Pre-speech padding in frames (~500ms).
const PRE_SPEECH_PADDING: usize = 17;

#[derive(Debug, Clone)]
pub enum VadEvent {
    SpeechStart,
    SpeechEnd { audio_data: Vec<i16> },
}

pub struct VadEngine {
    speech_frames: u32,
    silence_frames: u32,
    total_frames: u32,
    is_speaking: bool,
    buffer: std::collections::VecDeque<i16>,
    speech_segment: Vec<i16>,
    frame_size: usize,
}

impl VadEngine {
    pub fn new(sample_rate: u32) -> Self {
        let frame_size = (sample_rate as f64 * 0.030) as usize;
        let pre_speech_samples = frame_size * PRE_SPEECH_PADDING;

        Self {
            speech_frames: 0,
            silence_frames: 0,
            total_frames: 0,
            is_speaking: false,
            buffer: std::collections::VecDeque::with_capacity(pre_speech_samples),
            speech_segment: Vec::new(),
            frame_size,
        }
    }

    pub fn process_frame(&mut self, frame: &[i16]) -> Option<VadEvent> {
        let sum: f64 = frame.iter().map(|&s| (s as f64).powi(2)).sum();
        let rms = (sum / frame.len() as f64).sqrt();
        let is_speech = rms > SPEECH_THRESHOLD;

        // Rolling buffer
        for &sample in frame {
            self.buffer.push_back(sample);
        }
        let max_buf = self.frame_size * PRE_SPEECH_PADDING;
        while self.buffer.len() > max_buf {
            self.buffer.pop_front();
        }

        if !self.is_speaking {
            if is_speech {
                self.speech_frames += 1;
                self.silence_frames = 0;
                if self.speech_frames >= START_CONFIRM_FRAMES {
                    self.is_speaking = true;
                    self.speech_frames = 0;
                    self.total_frames = 0;
                    self.speech_segment.clear();
                    self.speech_segment.extend(self.buffer.iter().copied());
                    return Some(VadEvent::SpeechStart);
                }
            } else {
                self.speech_frames = 0;
            }
        } else {
            self.total_frames += 1;
            self.speech_segment.extend_from_slice(frame);

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
