import ApplicationServices
import CoreGraphics
import Foundation
import XCTest

struct UITestPermissionReport {
    let accessibilityGranted: Bool
    let screenCaptureGranted: Bool

    var missingLabels: [String] {
        var labels: [String] = []
        if !accessibilityGranted {
            labels.append("Accessibility")
        }
        if !screenCaptureGranted {
            labels.append("Screen Recording")
        }
        return labels
    }

    var isGranted: Bool {
        missingLabels.isEmpty
    }

    static func current() -> UITestPermissionReport {
        UITestPermissionReport(
            accessibilityGranted: AXIsProcessTrusted(),
            screenCaptureGranted: preflightScreenCapture()
        )
    }

    private static func preflightScreenCapture() -> Bool {
        if #available(macOS 10.15, *) {
            return CGPreflightScreenCaptureAccess()
        }
        return true
    }
}

enum UITestGate {
    static var runEnabled: Bool {
        ProcessInfo.processInfo.environment["AGTMUX_RUN_UI_TESTS"] == "1"
    }

    static func requireEnabled() throws {
        if !runEnabled {
            throw XCTSkip(
                "AGTMUX_RUN_UI_TESTS=1 を付けた時のみ実行します。例: AGTMUX_RUN_UI_TESTS=1 swift test --filter AGTMUXDesktopUITests"
            )
        }
    }

    @discardableResult
    static func assertRequiredPermissions(file: StaticString = #filePath, line: UInt = #line) -> Bool {
        let report = UITestPermissionReport.current()
        if report.isGranted {
            return true
        }
        let details = report.missingLabels.joined(separator: ", ")
        XCTFail(
            """
            AGTMUXDesktopUITests の実行に必要な権限が不足しています: \(details)
            System Settings > Privacy & Security で権限付与後に再実行してください。
            """,
            file: file,
            line: line
        )
        return false
    }
}
