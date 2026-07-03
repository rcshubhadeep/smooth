import Darwin
import Foundation
import ScreenCaptureKit

@main
struct SmoothSystemAudio {
    static func main() async {
        let command = CommandLine.arguments.dropFirst().first ?? "check"

        switch command {
        case "check":
            await checkPermission()
        default:
            writeJSON([
                "granted": false,
                "message": "Unknown system audio helper command: \(command)",
                "displays": 0,
                "error": "unknown_command"
            ])
            exit(64)
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

    private static func writeJSON(_ object: [String: Any]) {
        do {
            let data = try JSONSerialization.data(withJSONObject: object)
            FileHandle.standardOutput.write(data)
            FileHandle.standardOutput.write(Data("\n".utf8))
        } catch {
            FileHandle.standardError.write(Data("Failed to encode helper response: \(error)\n".utf8))
        }
    }
}
