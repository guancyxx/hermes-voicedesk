use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

use super::vad::{VadEngine, VadEvent};

static IS_CAPTURING: AtomicBool = AtomicBool::new(false);

/// Start microphone capture with VAD detection.
pub async fn start_mic_capture(app: AppHandle) -> Result<(), String> {
    if IS_CAPTURING.load(Ordering::SeqCst) {
        return Ok(());
    }

    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("No microphone found")?;

    let supported_config = device
        .default_input_config()
        .map_err(|e| format!("Input config error: {}", e))?;

    let sample_rate: u32 = supported_config.sample_rate();
    log::info!("Mic sample rate: {}Hz", sample_rate);

    // Ring buffer: 5 seconds of audio
    let buffer_size = (sample_rate * 5) as usize;
    let rb = ringbuf::HeapRb::<i16>::new(buffer_size);
    let (mut prod, mut cons) = rb.split();

    let config = supported_config.config();

    IS_CAPTURING.store(true, Ordering::SeqCst);
    let app_handle = app.clone();

    let stream = device
        .build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                for &sample in data {
                    let s = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    let _ = prod.try_push(s);
                }
            },
            move |err| {
                log::error!("Audio capture error: {}", err);
            },
            None,
        )
        .map_err(|e| format!("Stream start error: {}", e))?;

    stream.play().map_err(|e| format!("Stream play error: {}", e))?;

    // Spawn VAD processing thread
    let mut vad = VadEngine::new(sample_rate);
    let frame_size = (sample_rate as f64 * 0.030) as usize;
    let mut frame = Vec::with_capacity(frame_size);

    std::thread::spawn(move || {
        while IS_CAPTURING.load(Ordering::SeqCst) {
            frame.clear();
            for _ in 0..frame_size {
                match cons.try_pop() {
                    Some(s) => frame.push(s),
                    None => break,
                }
            }

            if frame.len() < frame_size {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            }

            // Volume (RMS) for waveform
            let sum: f64 = frame.iter().map(|&s| (s as f64).powi(2)).sum();
            let volume = ((sum / frame.len() as f64).sqrt() / 32768.0) as f32;
            let _ = app_handle.emit("audio:volume", serde_json::json!({ "rms": volume }));

            // VAD
            if let Some(event) = vad.process_frame(&frame) {
                match event {
                    VadEvent::SpeechStart => {
                        let _ = app_handle.emit("audio:state", serde_json::json!({ "state": "listening" }));
                    }
                    VadEvent::SpeechEnd { audio_data } => {
                        let _ = app_handle.emit("audio:state", serde_json::json!({ "state": "thinking" }));
                        let tmp_path = save_audio_temp(&audio_data, sample_rate);
                        if let Ok(path) = tmp_path {
                            let _ = app_handle.emit("stt:audio_file", serde_json::json!({ "path": path }));
                        }
                    }
                }
            }
        }
    });

    // Keep stream alive
    std::mem::forget(stream);

    Ok(())
}

/// Stop microphone capture.
pub async fn stop_mic_capture() -> Result<(), String> {
    IS_CAPTURING.store(false, Ordering::SeqCst);
    Ok(())
}

/// Save raw audio to a temporary WAV file.
fn save_audio_temp(samples: &[i16], sample_rate: u32) -> Result<String, String> {
    use std::io::Write;
    let dir = std::env::temp_dir().join("hermes-voicedesk");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {}", e))?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros();
    let path = dir.join(format!("speech_{}.wav", ts));
    let path_str = path.to_string_lossy().to_string();

    let file = std::fs::File::create(&path).map_err(|e| format!("create: {}", e))?;
    let mut writer = std::io::BufWriter::new(file);

    let data_size = (samples.len() * 2) as u32;
    let file_size = 36u32 + data_size;

    // WAV header
    writer.write_all(b"RIFF").unwrap();
    writer.write_all(&file_size.to_le_bytes()).unwrap();
    writer.write_all(b"WAVE").unwrap();
    writer.write_all(b"fmt ").unwrap();
    writer.write_all(&16u32.to_le_bytes()).unwrap();
    writer.write_all(&1u16.to_le_bytes()).unwrap();   // PCM
    writer.write_all(&1u16.to_le_bytes()).unwrap();   // mono
    writer.write_all(&sample_rate.to_le_bytes()).unwrap();
    writer.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
    writer.write_all(&2u16.to_le_bytes()).unwrap();   // block align
    writer.write_all(&16u16.to_le_bytes()).unwrap();  // bits per sample
    writer.write_all(b"data").unwrap();
    writer.write_all(&data_size.to_le_bytes()).unwrap();

    for &sample in samples {
        writer.write_all(&sample.to_le_bytes()).unwrap();
    }
    writer.flush().ok();

    log::info!("Saved WAV: {} ({} samples)", path_str, samples.len());
    Ok(path_str)
}
