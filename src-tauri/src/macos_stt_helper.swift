#!/usr/bin/env swift
import Foundation
import Speech

// macos-stt-helper — macOS native speech recognition fallback
// Usage: macos-stt-helper <audio.wav>
// Outputs transcription to stdout, errors to stderr
// Exit codes: 0 = success, 1 = error/unauthorized, 2 = silence

let args = CommandLine.arguments
guard args.count == 2 else {
    fputs("[macos_stt_error: usage: macos-stt-helper <audio.wav>]\n", stderr)
    exit(1)
}

let audioPath = args[1]
let audioURL = URL(fileURLWithPath: audioPath)

// Check file exists and is readable
guard FileManager.default.isReadableFile(atPath: audioPath) else {
    fputs("[macos_stt_error: file not found or not readable: \(audioPath)]\n", stderr)
    exit(1)
}

// Check file is non-empty (at least 44 bytes = WAV header)
guard let attrs = try? FileManager.default.attributesOfItem(atPath: audioPath),
      let fileSize = attrs[.size] as? Int, fileSize > 44 else {
    print("[silence]")
    exit(2)
}

// Request speech recognition authorization
let authSemaphore = DispatchSemaphore(value: 0)
var authorized = false

SFSpeechRecognizer.requestAuthorization { status in
    authorized = (status == .authorized)
    authSemaphore.signal()
}
authSemaphore.wait()

guard authorized else {
    fputs("[macos_stt_error: speech recognition not authorized]\n", stderr)
    exit(1)
}

guard let recognizer = SFSpeechRecognizer(), recognizer.isAvailable else {
    fputs("[macos_stt_error: speech recognizer unavailable]\n", stderr)
    exit(1)
}

let request = SFSpeechURLRecognitionRequest(url: audioURL)
request.requiresOnDeviceRecognition = false
request.shouldReportPartialResults = false

var transcription: String?
var recognitionError: Error?
var finished = false

let task = recognizer.recognitionTask(with: request) { result, error in
    if let error = error {
        recognitionError = error
    }
    if let result = result, result.isFinal {
        transcription = result.bestTranscription.formattedString
    }
    if result?.isFinal == true || error != nil {
        finished = true
        CFRunLoopStop(CFRunLoopGetMain())
    }
}

// Run the main run loop until done or timeout — DispatchSemaphore.wait()
// blocks the thread and prevents SFSpeechRecognizer callbacks from being
// dispatched on macOS 26+ where callbacks are delivered via the run loop.
let deadline = Date().addingTimeInterval(15)
while !finished && Date() < deadline {
    RunLoop.main.run(mode: .default, before: Date(timeIntervalSinceNow: 0.1))
}

if !finished {
    task.cancel()
    fputs("[macos_stt_error: recognition timed out]\n", stderr)
    exit(1)
}

if let error = recognitionError {
    fputs("[macos_stt_error: \(error.localizedDescription)]\n", stderr)
    exit(1)
}

let text = (transcription ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
if text.isEmpty {
    print("[silence]")
    exit(2)
}
print(text)
exit(0)
