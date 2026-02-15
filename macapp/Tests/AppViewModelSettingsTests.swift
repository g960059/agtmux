import Foundation
import XCTest
@testable import AGTMUXDesktop

@MainActor
final class AppViewModelSettingsTests: XCTestCase {
    func testHideUnmanagedCategoryRemovesUnmanagedGroup() throws {
        let model = try makeModel()
        model.panes = [
            makePane(paneID: "%1", displayCategory: "idle"),
            makePane(paneID: "%2", displayCategory: "unmanaged"),
        ]
        model.hideUnmanagedCategory = true
        model.showUnknownCategory = true

        let keys = model.statusGroups.map(\.0)
        XCTAssertFalse(keys.contains("unmanaged"))
    }

    func testUnknownCategoryFollowsShowUnknownToggle() throws {
        let model = try makeModel()
        model.panes = [
            makePane(paneID: "%1", displayCategory: "unknown"),
            makePane(paneID: "%2", displayCategory: "idle"),
        ]

        model.showUnknownCategory = false
        var keys = model.statusGroups.map(\.0)
        XCTAssertFalse(keys.contains("unknown"))

        model.showUnknownCategory = true
        keys = model.statusGroups.map(\.0)
        XCTAssertTrue(keys.contains("unknown"))
    }

    func testPaneDisplayTitlePrefersPaneTitleOverCurrentCommand() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: "Review spec draft",
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: nil,
            agentType: nil,
            agentPresence: nil,
            activityState: nil,
            displayCategory: "idle",
            needsUserAction: nil,
            stateSource: nil,
            lastEventType: nil,
            lastEventAt: nil,
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertEqual(model.paneDisplayTitle(for: pane), "Review spec draft")
    }

    func testLastActiveLabelDoesNotFallbackToUpdatedAt() throws {
        let model = try makeModel()
        let pane = makePane(paneID: "%1", displayCategory: "idle")
        XCTAssertEqual(model.lastActiveLabel(for: pane), "last active: -")
    }

    private func makeModel() throws -> AppViewModel {
        setenv("AGTMUX_DAEMON_BIN", "/usr/bin/true", 1)
        setenv("AGTMUX_APP_BIN", "/usr/bin/true", 1)

        let daemon = try DaemonManager(
            socketPath: "/tmp/agtmux-test.sock",
            dbPath: "/tmp/agtmux-test.db",
            logPath: "/tmp/agtmux-test.log"
        )
        let client = try AGTMUXCLIClient(socketPath: "/tmp/agtmux-test.sock")

        let suiteName = "AGTMUXDesktopTests-\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            XCTFail("Failed to create test defaults")
            throw NSError(domain: "test", code: 1)
        }
        defaults.removePersistentDomain(forName: suiteName)
        return AppViewModel(daemon: daemon, client: client, defaults: defaults)
    }

    private func makePane(paneID: String, displayCategory: String) -> PaneItem {
        PaneItem(
            identity: PaneIdentity(
                target: "local",
                sessionName: "s1",
                windowID: "@1",
                paneID: paneID
            ),
            windowName: nil,
            currentCmd: nil,
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: nil,
            agentType: nil,
            agentPresence: nil,
            activityState: nil,
            displayCategory: displayCategory,
            needsUserAction: nil,
            stateSource: nil,
            lastEventType: nil,
            lastEventAt: nil,
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
    }
}
