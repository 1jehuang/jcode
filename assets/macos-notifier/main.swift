// Jcode macOS notification helper.
//
// Why this exists: a notification posted with `osascript -e 'display
// notification ...'` is owned by Script Editor, so clicking the banner
// activates Script Editor instead of the terminal running the jcode session.
// Wrapping it in `tell application id "<terminal>"` does not help for
// terminals without an AppleScript dictionary (Ghostty, kitty, ...), because
// osascript still posts it. The only reliable fix is a real app bundle that
// owns its notifications and handles the click itself.
//
// Modes:
//   --post ...   post a notification, recording the click target in userInfo,
//                then stay alive briefly so the click can be delivered.
//   (relaunch)   when the user clicks a banner, macOS launches this bundle and
//                delivers the response to the delegate, which activates the
//                recorded terminal and runs the optional focus command.
import AppKit
import UserNotifications

/// Seconds to stay resident after posting so a click can still be delivered to
/// this process rather than requiring a cold relaunch.
private let residentSecondsAfterPost: TimeInterval = 600

/// Grace period after handling a click before exiting, so the activation and
/// focus command have time to take effect.
private let exitDelayAfterClick: TimeInterval = 0.7

struct PostRequest {
    var title = ""
    var subtitle: String?
    var body = ""
    var sound: String?
    var targetBundleID: String?
    var focusCommand: String?
    var probeFile: String?
}

final class Delegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    private let post: PostRequest?

    init(post: PostRequest?) {
        self.post = post
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        guard let post else { return }
        center.requestAuthorization(options: [.alert, .sound]) { granted, error in
            if let error {
                log("authorization error: \(error.localizedDescription)")
            }
            guard granted else {
                log("authorization denied; enable Jcode in System Settings > Notifications")
                DispatchQueue.main.async { exit(2) }
                return
            }
            self.deliver(post, via: center)
        }
    }

    private func deliver(_ post: PostRequest, via center: UNUserNotificationCenter) {
        let content = UNMutableNotificationContent()
        content.title = post.title
        if let subtitle = post.subtitle, !subtitle.isEmpty { content.subtitle = subtitle }
        content.body = post.body
        if let sound = post.sound, !sound.isEmpty {
            content.sound = UNNotificationSound(named: UNNotificationSoundName(sound))
        }
        var info: [String: String] = [:]
        if let value = post.targetBundleID { info["targetBundleID"] = value }
        if let value = post.focusCommand { info["focusCommand"] = value }
        if let value = post.probeFile { info["probeFile"] = value }
        content.userInfo = info

        let request = UNNotificationRequest(
            identifier: UUID().uuidString, content: content, trigger: nil)
        center.add(request) { error in
            if let error {
                log("post failed: \(error.localizedDescription)")
                DispatchQueue.main.async { exit(3) }
                return
            }
            log("posted: \(post.title)")
            // Stay alive so a click is delivered in-process. Exit eventually so
            // we never leak a resident helper.
            DispatchQueue.main.asyncAfter(deadline: .now() + residentSecondsAfterPost) {
                exit(0)
            }
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let info = response.notification.request.content.userInfo
        // Record that the click was handled, so automated validation can assert
        // on it rather than relying on a human watching the screen.
        if let probe = info["probeFile"] as? String, !probe.isEmpty {
            append("clicked target=\(info["targetBundleID"] as? String ?? "none")\n", to: probe)
        }
        if let focus = info["focusCommand"] as? String, !focus.isEmpty {
            runShell(focus)
        }
        if let bundleID = info["targetBundleID"] as? String, !bundleID.isEmpty {
            activate(bundleID: bundleID)
        }
        completionHandler()
        DispatchQueue.main.asyncAfter(deadline: .now() + exitDelayAfterClick) { exit(0) }
    }

    private func activate(bundleID: String) {
        if let app = NSRunningApplication
            .runningApplications(withBundleIdentifier: bundleID).first
        {
            app.activate(options: [.activateAllWindows])
            log("activated running app \(bundleID)")
            return
        }
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) else {
            log("cannot resolve app for \(bundleID)")
            return
        }
        let config = NSWorkspace.OpenConfiguration()
        config.activates = true
        NSWorkspace.shared.openApplication(at: url, configuration: config)
        log("launched \(bundleID)")
    }

    private func runShell(_ command: String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", command]
        do { try process.run() } catch { log("focus command failed: \(error)") }
    }
}

func log(_ message: String) {
    append("[\(ISO8601DateFormatter().string(from: Date()))] \(message)\n",
           to: NSString(string: "~/.jcode/logs/macos-notifier.log").expandingTildeInPath)
}

func append(_ line: String, to path: String) {
    guard let data = line.data(using: .utf8) else { return }
    let manager = FileManager.default
    if !manager.fileExists(atPath: path) {
        manager.createFile(atPath: path, contents: nil)
    }
    guard let handle = FileHandle(forWritingAtPath: path) else { return }
    handle.seekToEndOfFile()
    handle.write(data)
    try? handle.close()
}

func parsePost() -> PostRequest? {
    let args = Array(CommandLine.arguments.dropFirst())
    guard args.contains("--post") else { return nil }
    var request = PostRequest()
    var index = 0
    while index < args.count {
        let value: String? = index + 1 < args.count ? args[index + 1] : nil
        switch args[index] {
        case "--title": request.title = value ?? ""; index += 2
        case "--subtitle": request.subtitle = value; index += 2
        case "--body": request.body = value ?? ""; index += 2
        case "--sound": request.sound = value; index += 2
        case "--target-bundle-id": request.targetBundleID = value; index += 2
        case "--focus-command": request.focusCommand = value; index += 2
        case "--probe-file": request.probeFile = value; index += 2
        default: index += 1
        }
    }
    return request
}

let app = NSApplication.shared
let delegate = Delegate(post: parsePost())
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()
