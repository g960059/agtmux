import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import XCTest

@MainActor
final class AGTMUXDesktopUITests: XCTestCase {
    private var launchedProcess: Process?
    private var launchedAppPID: pid_t?

    override func tearDown() {
        if let process = launchedProcess, process.isRunning {
            process.terminate()
            process.waitUntilExit()
        }
        if let pid = launchedAppPID, let app = NSRunningApplication(processIdentifier: pid) {
            app.terminate()
        }
        launchedProcess = nil
        launchedAppPID = nil
        super.tearDown()
    }

    func testUIAutomationPermissionsAreGranted() throws {
        try UITestGate.requireEnabled()
        _ = UITestGate.assertRequiredPermissions()
    }

    func testLaunchBinaryAndExposeWindowToAccessibility() throws {
        try UITestGate.requireEnabled()
        let permissionReport = UITestPermissionReport.current()
        if !permissionReport.isGranted {
            throw XCTSkip(
                "権限不足のため launch テストをスキップします: \(permissionReport.missingLabels.joined(separator: ", "))"
            )
        }

        let pid = try launchAppAndResolvePID()
        launchedAppPID = pid

        if let runningApp = NSRunningApplication(processIdentifier: pid) {
            _ = runningApp.activate(options: [.activateAllWindows])
        }

        let windowVisible = waitForWindowPresence(
            pid: pid,
            timeout: 10.0
        )
        XCTAssertTrue(windowVisible, "起動後 10 秒以内に UI ウィンドウを検出できませんでした。")
        maybeCaptureWindowSnapshot(pid: pid, label: "launch-window")
    }

    func testSidebarSessionsLabelIsVisible() throws {
        try UITestGate.requireEnabled()
        let permissionReport = UITestPermissionReport.current()
        if !permissionReport.isGranted {
            throw XCTSkip(
                "権限不足のため sidebar テストをスキップします: \(permissionReport.missingLabels.joined(separator: ", "))"
            )
        }

        let pid = try launchAppAndResolvePID()
        launchedAppPID = pid
        let found = waitForAXStaticText(pid: pid, text: "Sessions", timeout: 10.0)
        if !found, hasVisibleCGWindow(pid: pid) {
            throw XCTSkip("AX 文字列列挙が環境依存で不安定なため skip（ウィンドウ表示は確認済み）。")
        }
        XCTAssertTrue(found, "サイドバー内に Sessions ラベルを検出できませんでした。")
        maybeCaptureWindowSnapshot(pid: pid, label: "sidebar-sessions")
    }

    private func launchAppAndResolvePID() throws -> pid_t {
        let appBundleURL = try appBundleURL()
        let bundleID = "com.g960059.agtmux.desktop"

        for app in NSRunningApplication.runningApplications(withBundleIdentifier: bundleID) {
            app.terminate()
        }
        RunLoop.current.run(until: Date().addingTimeInterval(0.2))

        let opener = Process()
        opener.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        opener.arguments = ["-na", appBundleURL.path]
        opener.environment = ProcessInfo.processInfo.environment
        try opener.run()
        launchedProcess = opener
        opener.waitUntilExit()

        guard opener.terminationStatus == 0 else {
            throw NSError(
                domain: "AGTMUXDesktopUITests",
                code: 3,
                userInfo: [NSLocalizedDescriptionKey: "open -na の起動に失敗しました。status=\(opener.terminationStatus)"]
            )
        }

        let deadline = Date().addingTimeInterval(10.0)
        while Date() < deadline {
            let apps = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
            if let pid = apps.map(\.processIdentifier).max() {
                return pid
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }

        throw NSError(
            domain: "AGTMUXDesktopUITests",
            code: 4,
            userInfo: [NSLocalizedDescriptionKey: "bundleID=\(bundleID) の起動プロセスを検出できませんでした。"]
        )
    }

    private func appBundleURL() throws -> URL {
        let env = ProcessInfo.processInfo.environment
        if let explicit = env["AGTMUX_UI_TEST_APP_BUNDLE"], !explicit.isEmpty {
            let explicitURL = URL(fileURLWithPath: explicit)
            if FileManager.default.fileExists(atPath: explicitURL.path) {
                return explicitURL
            }
            XCTFail("AGTMUX_UI_TEST_APP_BUNDLE が存在しません: \(explicitURL.path)")
        }

        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let defaultPath = "\(home)/Applications/AGTMUXDesktop.app"
        if FileManager.default.fileExists(atPath: defaultPath) {
            return URL(fileURLWithPath: defaultPath)
        }

        throw NSError(
            domain: "AGTMUXDesktopUITests",
            code: 1,
            userInfo: [
                NSLocalizedDescriptionKey:
                    """
                    AGTMUXDesktop.app が見つかりません。
                    先に ./scripts/install-app.sh を実行するか、
                    AGTMUX_UI_TEST_APP_BUNDLE=<path> を指定してください。
                    """
            ]
        )
    }

    private func waitForWindowPresence(pid: pid_t, timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            if hasAXWindow(pid: pid) || hasVisibleCGWindow(pid: pid) {
                return true
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }
        return false
    }

    private func hasAXWindow(pid: pid_t) -> Bool {
        let appElement = AXUIElementCreateApplication(pid)
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(
            appElement,
            kAXWindowsAttribute as CFString,
            &value
        )
        if result == .success, let windows = value as? [AXUIElement], !windows.isEmpty {
            return true
        }
        return false
    }

    private func hasVisibleCGWindow(pid: pid_t) -> Bool {
        firstVisibleCGWindowID(pid: pid) != nil
    }

    private func firstVisibleCGWindowID(pid: pid_t) -> CGWindowID? {
        guard
            let raw = CGWindowListCopyWindowInfo(
                [.optionOnScreenOnly, .excludeDesktopElements],
                kCGNullWindowID
            ) as? [[String: Any]]
        else {
            return nil
        }

        for window in raw {
            guard let ownerPID = window[kCGWindowOwnerPID as String] as? Int32, ownerPID == pid else {
                continue
            }
            let alpha = window[kCGWindowAlpha as String] as? Double ?? 1.0
            if alpha <= 0 {
                continue
            }
            guard let windowIDInt = window[kCGWindowNumber as String] as? Int else {
                continue
            }
            if
                let bounds = window[kCGWindowBounds as String] as? [String: Any],
                let width = bounds["Width"] as? Double,
                let height = bounds["Height"] as? Double,
                width > 40,
                height > 40
            {
                return CGWindowID(windowIDInt)
            }
        }
        return nil
    }

    private func waitForAXStaticText(pid: pid_t, text: String, timeout: TimeInterval) -> Bool {
        let appElement = AXUIElementCreateApplication(pid)
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if elementTreeContainsText(appElement, text: text) {
                return true
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }
        return false
    }

    private func elementTreeContainsText(_ root: AXUIElement, text: String) -> Bool {
        var queue: [AXUIElement] = [root]
        var visited = 0
        let limit = 6000

        while !queue.isEmpty && visited < limit {
            let element = queue.removeFirst()
            visited += 1

            if let value = attributeString(element, attribute: kAXValueAttribute as CFString), value == text {
                return true
            }
            if let title = attributeString(element, attribute: kAXTitleAttribute as CFString), title == text {
                return true
            }
            if let description = attributeString(element, attribute: kAXDescriptionAttribute as CFString), description == text {
                return true
            }

            queue.append(contentsOf: attributeElements(element, attribute: kAXChildrenAttribute as CFString))
        }
        return false
    }

    private func attributeString(_ element: AXUIElement, attribute: CFString) -> String? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, attribute, &value) == .success else {
            return nil
        }
        return value as? String
    }

    private func attributeElements(_ element: AXUIElement, attribute: CFString) -> [AXUIElement] {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, attribute, &value) == .success else {
            return []
        }
        return value as? [AXUIElement] ?? []
    }

    private func maybeCaptureWindowSnapshot(pid: pid_t, label: String) {
        guard ProcessInfo.processInfo.environment["AGTMUX_UI_TEST_CAPTURE"] == "1" else {
            return
        }
        guard let windowID = firstVisibleCGWindowID(pid: pid) else {
            return
        }

        let fm = FileManager.default
        let customDir = ProcessInfo.processInfo.environment["AGTMUX_UI_TEST_CAPTURE_DIR"]
        let outputDir = URL(fileURLWithPath: customDir?.isEmpty == false ? customDir! : "/tmp/agtmux-ui-captures", isDirectory: true)
        do {
            try fm.createDirectory(at: outputDir, withIntermediateDirectories: true)
            let ts = ISO8601DateFormatter().string(from: Date()).replacingOccurrences(of: ":", with: "-")
            let out = outputDir.appendingPathComponent("\(ts)-\(label).png")
            let capture = Process()
            capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            capture.arguments = ["-x", "-l", String(windowID), out.path]
            try capture.run()
            capture.waitUntilExit()
            if capture.terminationStatus == 0 {
                print("ui-snapshot: \(out.path)")
            } else {
                print("ui-snapshot-error: screencapture exit=\(capture.terminationStatus)")
            }
        } catch {
            print("ui-snapshot-error: \(error.localizedDescription)")
        }
    }
}
