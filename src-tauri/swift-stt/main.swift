import Foundation
import Speech
import AVFoundation

guard CommandLine.arguments.count == 2 else {
    fputs("Usage: stt-helper <wav_file>\n", stderr)
    exit(1)
}

let filePath = CommandLine.arguments[1]
let fileURL = URL(fileURLWithPath: filePath)

guard FileManager.default.fileExists(atPath: filePath) else {
    fputs("File not found: \(filePath)\n", stderr)
    exit(1)
}

let semaphore = DispatchSemaphore(value: 0)
var recognitionResult: String? = nil
var recognitionError: String? = nil

SFSpeechRecognizer.requestAuthorization { status in
    guard status == .authorized else {
        recognitionError = "SFSpeechRecognizer not authorized: \(status.rawValue)"
        semaphore.signal()
        return
    }

    let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "zh-CN"))
                  ?? SFSpeechRecognizer()

    guard let recognizer = recognizer, recognizer.isAvailable else {
        recognitionError = "SFSpeechRecognizer unavailable"
        semaphore.signal()
        return
    }

    let request = SFSpeechURLRecognitionRequest(url: fileURL)
    request.shouldReportPartialResults = false

    recognizer.recognitionTask(with: request) { result, error in
        defer { semaphore.signal() }
        if let error = error {
            recognitionError = error.localizedDescription
        } else if let result = result, result.isFinal {
            recognitionResult = result.bestTranscription.formattedString
        }
    }
}

let timeout = semaphore.wait(timeout: .now() + 30)

if timeout == .timedOut {
    fputs("Recognition timed out\n", stderr)
    exit(1)
}

if let error = recognitionError {
    fputs("\(error)\n", stderr)
    exit(1)
}

if let text = recognitionResult, !text.isEmpty {
    print(text)
    exit(0)
} else {
    fputs("Empty result\n", stderr)
    exit(1)
}
