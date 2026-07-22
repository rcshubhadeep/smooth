import Darwin
import AppKit
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
        case "list-sources":
            await listVisualSources()
        case "snapshot":
            await captureVisualSnapshot()
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

    private static func listVisualSources() async {
        if #available(macOS 12.3, *) {
            do {
                // Ask ScreenCaptureKit for windows from every Space. Browsers
                // are commonly kept full-screen, behind another app, or in a
                // Stage Manager set, all of which can make `isOnScreen` false.
                let content = try await SCShareableContent.excludingDesktopWindows(
                    true,
                    onScreenWindowsOnly: false
                )
                writeJSON([
                    "type": "sources",
                    "sources": visualSources(from: content)
                ])
                exit(0)
            } catch {
                writeJSON([
                    "type": "error",
                    "message": "Unable to list ScreenCaptureKit sources.",
                    "error": String(describing: error),
                    "sources": []
                ])
                exit(1)
            }
        } else {
            writeJSON([
                "type": "error",
                "message": "ScreenCaptureKit requires macOS 12.3 or newer.",
                "error": "unsupported_macos_version",
                "sources": []
            ])
            exit(2)
        }
    }

    private static func captureVisualSnapshot() async {
        guard #available(macOS 14.0, *) else {
            writeJSON([
                "type": "error",
                "message": "Visual snapshots require macOS 14.0 or newer.",
                "error": "unsupported_macos_version"
            ])
            exit(2)
        }

        guard let sourceID = argumentValue("--source-id") else {
            writeJSON([
                "type": "error",
                "message": "Missing required --source-id value.",
                "error": "missing_source_id"
            ])
            exit(64)
        }

        guard let outputPath = argumentValue("--output") else {
            writeJSON([
                "type": "error",
                "message": "Missing required --output path.",
                "error": "missing_output"
            ])
            exit(64)
        }

        do {
            let result = try await SnapshotCapture.capture(sourceID: sourceID, outputPath: outputPath)
            writeJSON([
                "type": "snapshot",
                "path": outputPath,
                "source_id": sourceID,
                "width": result.width,
                "height": result.height
            ])
            exit(0)
        } catch {
            writeJSON([
                "type": "error",
                "message": "Failed to capture visual snapshot.",
                "source_id": sourceID,
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

    @available(macOS 12.3, *)
    private static func visualSources(from content: SCShareableContent) -> [[String: Any]] {
        var sources: [[String: Any]] = []

        for (index, display) in content.displays.enumerated() {
            sources.append([
                "id": "display:\(display.displayID)",
                "kind": "display",
                "name": "Display \(index + 1)",
                "display_id": Int(display.displayID),
                "window_id": NSNull(),
                "app_name": NSNull(),
                "width": display.width,
                "height": display.height
            ])
        }

        for window in content.windows {
            let title = (window.title ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let appName = (window.owningApplication?.applicationName ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let width = Int(window.frame.width.rounded())
            let height = Int(window.frame.height.rounded())

            // SCShareableContent has already limited this collection to
            // capturable windows. Keep normal application windows of a useful
            // size, but do not discard windows solely because they are on a
            // different Space or currently occluded.
            guard window.windowLayer == 0, width >= 80, height >= 60 else {
                continue
            }

            guard !title.isEmpty || !appName.isEmpty else {
                continue
            }

            let name: String
            if title.isEmpty {
                name = appName
            } else if appName.isEmpty || title == appName {
                name = title
            } else {
                name = "\(appName) - \(title)"
            }

            sources.append([
                "id": "window:\(window.windowID)",
                "kind": "window",
                "name": name,
                "display_id": NSNull(),
                "window_id": Int(window.windowID),
                "app_name": appName.isEmpty ? NSNull() : appName,
                "width": width,
                "height": height
            ])
        }

        return sources
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

@available(macOS 14.0, *)
private enum SnapshotCapture {
    struct Result {
        let width: Int
        let height: Int
    }

    private struct FilterCapture {
        let label: String
        let filter: SCContentFilter
        let sourceRect: CGRect?
    }

    static func capture(sourceID: String, outputPath: String) async throws -> Result {
        let content = try await SCShareableContent.excludingDesktopWindows(
            true,
            onScreenWindowsOnly: false
        )
        let target = try captureTarget(sourceID: sourceID, content: content)
        let image = try await captureImage(using: target.captures, fallbackRect: target.fallbackRect)

        let outputURL = URL(fileURLWithPath: outputPath)
        try FileManager.default.createDirectory(
            at: outputURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )

        let bitmap = NSBitmapImageRep(cgImage: image)
        guard let data = bitmap.representation(using: NSBitmapImageRep.FileType.png, properties: [:]) else {
            throw SnapshotError.pngEncodingFailed
        }
        try data.write(to: outputURL, options: Data.WritingOptions.atomic)

        return Result(width: image.width, height: image.height)
    }

    private static func captureImage(
        using captures: [FilterCapture],
        fallbackRect: CGRect?
    ) async throws -> CGImage {
        var lastError: Error?
        var failures: [String] = []

        for capture in captures {
            do {
                return try await captureImage(using: capture)
            } catch {
                lastError = error
                failures.append("\(capture.label): \(String(describing: error))")
            }
        }

        if let fallbackRect, #available(macOS 15.2, *) {
            do {
                return try await captureImage(in: fallbackRect)
            } catch {
                lastError = error
                failures.append("screen-rect: \(String(describing: error))")
            }
        }

        if !failures.isEmpty {
            throw SnapshotError.allAttemptsFailed(failures)
        }

        throw lastError ?? SnapshotError.emptyImage
    }

    @available(macOS 15.2, *)
    private static func captureImage(in rect: CGRect) async throws -> CGImage {
        let normalizedRect = CGRect(
            x: rect.origin.x.rounded(.down),
            y: rect.origin.y.rounded(.down),
            width: max(1, rect.width.rounded(.up)),
            height: max(1, rect.height.rounded(.up))
        )

        return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<CGImage, Error>) in
            SCScreenshotManager.captureImage(in: normalizedRect) { image, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                guard let image else {
                    continuation.resume(throwing: SnapshotError.emptyImage)
                    return
                }
                continuation.resume(returning: image)
            }
        }
    }

    private static func captureImage(using capture: FilterCapture) async throws -> CGImage {
        let filter = capture.filter
        let config = SCStreamConfiguration()
        let scale = CGFloat(max(filter.pointPixelScale, 1))
        let rect = capture.sourceRect ?? filter.contentRect
        config.width = max(1, Int((rect.width * scale).rounded(.up)))
        config.height = max(1, Int((rect.height * scale).rounded(.up)))
        if let sourceRect = capture.sourceRect {
            config.sourceRect = sourceRect
            config.ignoreShadowsDisplay = true
        } else {
            config.ignoreShadowsSingleWindow = true
        }
        config.queueDepth = 3
        config.showsCursor = true
        config.capturesAudio = false
        config.scalesToFit = true
        config.shouldBeOpaque = true

        return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<CGImage, Error>) in
            SCScreenshotManager.captureImage(contentFilter: filter, configuration: config) { image, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                guard let image else {
                    continuation.resume(throwing: SnapshotError.emptyImage)
                    return
                }
                continuation.resume(returning: image)
            }
        }
    }

    private static func captureTarget(
        sourceID: String,
        content: SCShareableContent
    ) throws -> (captures: [FilterCapture], fallbackRect: CGRect?) {
        if let displayID = idValue(sourceID, prefix: "display:") {
            guard let display = content.displays.first(where: { Int($0.displayID) == displayID }) else {
                throw SnapshotError.sourceNotFound(sourceID)
            }

            return ([
                FilterCapture(
                    label: "display",
                    filter: SCContentFilter(display: display, excludingWindows: []),
                    sourceRect: nil
                )
            ], nil)
        }

        if let windowID = idValue(sourceID, prefix: "window:") {
            guard let window = content.windows.first(where: { Int($0.windowID) == windowID }) else {
                throw SnapshotError.sourceNotFound(sourceID)
            }

            var captures: [FilterCapture] = []
            // Preserve the display-crop path for visible windows because it is
            // the most reliable way to include their exact on-screen pixels.
            // Off-screen windows must use a desktop-independent filter; a crop
            // would otherwise capture whatever occupies that rectangle on the
            // current Space.
            if window.isOnScreen,
               let display = displayContaining(window: window, displays: content.displays) {
                if let cropRect = sourceRect(for: window, on: display) {
                    captures.append(FilterCapture(
                        label: "display-crop",
                        filter: SCContentFilter(
                            display: display,
                            excludingApplications: [],
                            exceptingWindows: []
                        ),
                        sourceRect: cropRect
                    ))
                }
            }
            captures.append(FilterCapture(
                label: "desktop-independent-window",
                filter: SCContentFilter(desktopIndependentWindow: window),
                sourceRect: nil
            ))
            return (captures, window.isOnScreen ? window.frame : nil)
        }

        throw SnapshotError.invalidSourceID(sourceID)
    }

    private static func sourceRect(for window: SCWindow, on display: SCDisplay) -> CGRect? {
        let intersection = window.frame.intersection(display.frame)
        guard !intersection.isNull, intersection.width >= 1, intersection.height >= 1 else {
            return nil
        }

        return CGRect(
            x: max(0, intersection.origin.x - display.frame.origin.x),
            y: max(0, intersection.origin.y - display.frame.origin.y),
            width: intersection.width,
            height: intersection.height
        )
    }

    private static func displayContaining(window: SCWindow, displays: [SCDisplay]) -> SCDisplay? {
        let windowMidX = window.frame.midX
        let windowMidY = window.frame.midY
        if let display = displays.first(where: { $0.frame.contains(CGPoint(x: windowMidX, y: windowMidY)) }) {
            return display
        }

        return displays.first(where: { $0.frame.intersects(window.frame) })
    }

    private static func idValue(_ sourceID: String, prefix: String) -> Int? {
        guard sourceID.hasPrefix(prefix) else {
            return nil
        }
        return Int(sourceID.dropFirst(prefix.count))
    }
}

private enum SnapshotError: Error, CustomStringConvertible {
    case invalidSourceID(String)
    case sourceNotFound(String)
    case emptyImage
    case pngEncodingFailed
    case allAttemptsFailed([String])

    var description: String {
        switch self {
        case .invalidSourceID(let sourceID):
            return "Invalid visual source id: \(sourceID)."
        case .sourceNotFound(let sourceID):
            return "Visual source is no longer available: \(sourceID)."
        case .emptyImage:
            return "ScreenCaptureKit returned an empty image."
        case .pngEncodingFailed:
            return "Failed to encode snapshot as PNG."
        case .allAttemptsFailed(let failures):
            return "All snapshot attempts failed: \(failures.joined(separator: " | "))"
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
