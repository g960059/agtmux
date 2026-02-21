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
    }

    private func launchAppAndResolvePID() throws -> pid_t {
        let appBundleURL = try appBundleURL()

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

        let bundleID = "com.g960059.agtmux.desktop"
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
        guard
            let raw = CGWindowListCopyWindowInfo(
                [.optionOnScreenOnly, .excludeDesktopElements],
                kCGNullWindowID
            ) as? [[String: Any]]
        else {
            return false
        }

        for window in raw {
            guard let ownerPID = window[kCGWindowOwnerPID as String] as? Int32, ownerPID == pid else {
                continue
            }
            let alpha = window[kCGWindowAlpha as String] as? Double ?? 1.0
            if alpha <= 0 {
                continue
            }
            if
                let bounds = window[kCGWindowBounds as String] as? [String: Any],
                let width = bounds["Width"] as? Double,
                let height = bounds["Height"] as? Double,
                width > 40,
                height > 40
            {
                return true
            }
        }
        return false
    }
}
