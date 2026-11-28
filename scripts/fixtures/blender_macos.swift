import AppKit
import CoreGraphics
import Foundation

let arguments = CommandLine.arguments
if arguments[1] == "find" {
    let pid = Int(arguments[2])!
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements],
                                             kCGNullWindowID) as? [[String: Any]] ?? []
    for window in windows where window[kCGWindowOwnerPID as String] as? Int == pid
        && window[kCGWindowLayer as String] as? Int == 0 {
        if let id = window[kCGWindowNumber as String] as? Int { print(id) }
    }
    exit(0)
}

// LaunchServices starts a new foreground application instance. Retain that
// instance until EOF so even a failed benchmark cleans up only its own app.
let directory = URL(fileURLWithPath: arguments[2], isDirectory: true)
let bundle = URL(fileURLWithPath: arguments[3], isDirectory: true)
let configuration = NSWorkspace.OpenConfiguration()
configuration.activates = true
configuration.createsNewApplicationInstance = true
configuration.arguments = Array(arguments.dropFirst(4))
configuration.environment = ["DCC_CUA_BLENDER_BENCH": directory.path,
                             "BLENDER_USER_CONFIG": directory.appendingPathComponent("config").path]
NSWorkspace.shared.openApplication(at: bundle, configuration: configuration) { application, _ in
    guard let application else { exit(1) }
    do {
        try String(application.processIdentifier).write(to: directory.appendingPathComponent("pid"),
                                                        atomically: true, encoding: .utf8)
    } catch {
        _ = application.forceTerminate()
        exit(1)
    }
    DispatchQueue.global().async {
        _ = FileHandle.standardInput.readDataToEndOfFile()
        DispatchQueue.main.async {
            if !application.isTerminated { _ = application.forceTerminate() }
            let deadline = Date().addingTimeInterval(5)
            Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in
                if application.isTerminated { exit(0) }
                if Date() >= deadline { exit(2) }
            }
        }
    }
}
RunLoop.main.run()
