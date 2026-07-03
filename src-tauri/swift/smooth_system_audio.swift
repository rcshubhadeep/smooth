import Darwin
import AVFoundation
import CoreMedia
import Foundation
import ScreenCaptureKit

@main
struct SmoothSystemAudio {
    static func main() async {
        let command = CommandLine.arguments.dropFirst().first ?? "check"

        switch command {
        case "check":
            await checkPermission()
        case "capture":
            await captureSystemAudio()
        default:
            writeJSON([
                "type": "error",
                "granted": false,
                "message": "Unknown system audio helper command: \(command)",
                "displays": 0,
                "error": "unknown_command"
            ])
            exit(64)
        }
    }

    private static func captureSystemAudio() async {
        guard #available(macOS 13.0, *) else {
            writeJSON([
                "type": "error",
                "message": "System audio capture requires macOS 13.0 or newer.",
                "error": "unsupported_macos_version"
            ])
            exit(2)
        }

        guard let outputPath = argumentValue("--output") else {
            writeJSON([
                "type": "error",
                "message": "Missing required --output path.",
                "error": "missing_output"
            ])
            exit(64)
        }

        let capture = SystemAudioCapture(outputURL: URL(fileURLWithPath: outputPath))

        do {
            try await capture.start()
            writeJSON([
                "type": "started",
                "path": outputPath,
                "sample_rate": capture.sampleRate,
                "channels": capture.channels
            ])

            installStopReader(for: capture)
            await capture.waitUntilFinished()
            exit(capture.exitCode)
        } catch {
            writeJSON([
                "type": "error",
                "message": "Failed to start system audio capture.",
                "error": String(describing: error)
            ])
            exit(1)
        }
    }

    private static func checkPermission() async {
        if #available(macOS 12.3, *) {
            do {
                let content = try await SCShareableContent.current
                writeJSON([
                    "granted": true,
                    "message": "Screen and system audio recording permission is available.",
                    "displays": content.displays.count
                ])
                exit(0)
            } catch {
                writeJSON([
                    "granted": false,
                    "message": "Screen and system audio recording permission is not available.",
                    "displays": 0,
                    "error": String(describing: error)
                ])
                exit(1)
            }
        } else {
            writeJSON([
                "granted": false,
                "message": "ScreenCaptureKit requires macOS 12.3 or newer.",
                "displays": 0,
                "error": "unsupported_macos_version"
            ])
            exit(2)
        }
    }

    fileprivate static func writeJSON(_ object: [String: Any]) {
        do {
            let data = try JSONSerialization.data(withJSONObject: object)
            FileHandle.standardOutput.write(data)
            FileHandle.standardOutput.write(Data("\n".utf8))
        } catch {
            FileHandle.standardError.write(Data("Failed to encode helper response: \(error)\n".utf8))
        }
    }

    private static func argumentValue(_ name: String) -> String? {
        let arguments = Array(CommandLine.arguments.dropFirst())
        guard let index = arguments.firstIndex(of: name) else {
            return nil
        }
        let valueIndex = arguments.index(after: index)
        guard arguments.indices.contains(valueIndex) else {
            return nil
        }
        return arguments[valueIndex]
    }

    private static func installStopReader(for capture: SystemAudioCapture) {
        FileHandle.standardInput.readabilityHandler = { handle in
            let data = handle.availableData
            let message = String(data: data, encoding: .utf8) ?? ""
            if data.isEmpty || message.contains("stop") {
                FileHandle.standardInput.readabilityHandler = nil
                Task {
                    await capture.stop()
                }
            }
        }
    }
}

@available(macOS 13.0, *)
final class SystemAudioCapture: NSObject, SCStreamOutput, SCStreamDelegate {
    let outputURL: URL
    let sampleRate = 48_000
    let channels = 2

    private let stateLock = NSLock()
    private var stream: SCStream?
    private var audioFile: AVAudioFile?
    private var audioFormat: AVAudioFormat?
    private var framesWritten: AVAudioFramePosition = 0
    private var finishContinuation: CheckedContinuation<Void, Never>?
    private var finished = false
    private var stopping = false
    private var lastError: String?
    private let startedAt = Date()

    private(set) var exitCode: Int32 = 0

    init(outputURL: URL) {
        self.outputURL = outputURL
        super.init()
    }

    func start() async throws {
        let content = try await SCShareableContent.current
        guard let display = content.displays.first else {
            throw CaptureError.noDisplay
        }

        try FileManager.default.createDirectory(
            at: outputURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )

        let config = SCStreamConfiguration()
        config.width = 2
        config.height = 2
        config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        config.queueDepth = 3
        config.showsCursor = false
        config.capturesAudio = true
        config.excludesCurrentProcessAudio = true
        config.sampleRate = sampleRate
        config.channelCount = channels

        let filter = SCContentFilter(display: display, excludingWindows: [])
        let nextStream = SCStream(filter: filter, configuration: config, delegate: self)
        try nextStream.addStreamOutput(
            self,
            type: .audio,
            sampleHandlerQueue: DispatchQueue(label: "smooth.system-audio.samples")
        )

        stream = nextStream
        try await nextStream.startCapture()
    }

    func waitUntilFinished() async {
        var shouldResumeImmediately = false
        await withCheckedContinuation { continuation in
            stateLock.lock()
            if finished {
                shouldResumeImmediately = true
            } else {
                finishContinuation = continuation
            }
            stateLock.unlock()

            if shouldResumeImmediately {
                continuation.resume()
            }
        }
    }

    func stop() async {
        guard let activeStream = beginStopping() else {
            return
        }

        do {
            try await activeStream.stopCapture()
        } catch {
            finish(errorMessage: String(describing: error))
            return
        }

        finish(errorMessage: nil)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sampleBuffer.isValid else {
            return
        }

        do {
            try write(sampleBuffer: sampleBuffer)
        } catch {
            finish(errorMessage: String(describing: error))
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        finish(errorMessage: String(describing: error))
    }

    private func write(sampleBuffer: CMSampleBuffer) throws {
        guard let formatDescription = CMSampleBufferGetFormatDescription(sampleBuffer),
              let streamDescription = CMAudioFormatDescriptionGetStreamBasicDescription(formatDescription),
              let format = AVAudioFormat(streamDescription: streamDescription) else {
            throw CaptureError.invalidAudioFormat
        }

        let frameCount = AVAudioFrameCount(CMSampleBufferGetNumSamples(sampleBuffer))
        guard frameCount > 0,
              let pcmBuffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount) else {
            return
        }

        pcmBuffer.frameLength = frameCount
        let status = CMSampleBufferCopyPCMDataIntoAudioBufferList(
            sampleBuffer,
            at: 0,
            frameCount: Int32(frameCount),
            into: pcmBuffer.mutableAudioBufferList
        )
        guard status == noErr else {
            throw CaptureError.copyFailed(status)
        }

        stateLock.lock()
        defer { stateLock.unlock() }

        if audioFile == nil {
            audioFormat = format
            audioFile = try AVAudioFile(forWriting: outputURL, settings: format.settings)
        }

        try audioFile?.write(from: pcmBuffer)
        framesWritten += AVAudioFramePosition(pcmBuffer.frameLength)
    }

    private func finish(errorMessage: String?) {
        let continuation: CheckedContinuation<Void, Never>?
        let result: [String: Any]

        stateLock.lock()
        if finished {
            stateLock.unlock()
            return
        }

        finished = true
        lastError = errorMessage ?? lastError
        audioFile = nil

        let rate = Int(audioFormat?.sampleRate.rounded() ?? Double(sampleRate))
        let channelCount = Int(audioFormat?.channelCount ?? AVAudioChannelCount(channels))
        let frames = max(0, framesWritten)
        let durationMs = rate > 0 ? (Int64(frames) * 1000) / Int64(rate) : 0
        let samples = Int64(frames) * Int64(max(channelCount, 1))

        if let lastError {
            exitCode = framesWritten > 0 ? 0 : 1
            result = [
                "type": framesWritten > 0 ? "finished" : "error",
                "path": outputURL.path,
                "duration_ms": durationMs,
                "sample_rate": rate,
                "channels": channelCount,
                "samples": samples,
                "message": framesWritten > 0 ? "System audio capture stopped with a stream warning." : "System audio capture failed.",
                "error": lastError
            ]
        } else {
            exitCode = 0
            result = [
                "type": "finished",
                "path": outputURL.path,
                "duration_ms": durationMs,
                "sample_rate": rate,
                "channels": channelCount,
                "samples": samples,
                "message": "System audio capture finished."
            ]
        }

        continuation = finishContinuation
        finishContinuation = nil
        stateLock.unlock()

        SmoothSystemAudio.writeJSON(result)
        continuation?.resume()
    }

    private func beginStopping() -> SCStream? {
        stateLock.lock()
        defer { stateLock.unlock() }

        if stopping || finished {
            return nil
        }

        stopping = true
        return stream
    }
}

enum CaptureError: Error, CustomStringConvertible {
    case noDisplay
    case invalidAudioFormat
    case copyFailed(OSStatus)

    var description: String {
        switch self {
        case .noDisplay:
            return "No display is available for ScreenCaptureKit capture."
        case .invalidAudioFormat:
            return "ScreenCaptureKit returned an unsupported audio format."
        case .copyFailed(let status):
            return "Failed to copy ScreenCaptureKit audio buffer: \(status)."
        }
    }
}
