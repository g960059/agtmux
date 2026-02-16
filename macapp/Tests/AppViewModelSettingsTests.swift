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

    func testPaneDisplayTitleManagedFallbackUsesAgentSession() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: "frontend",
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: nil,
            agentType: "codex",
            agentPresence: "managed",
            activityState: "idle",
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
        XCTAssertEqual(model.paneDisplayTitle(for: pane), "codex session")
    }

    func testPaneDisplayTitleDisambiguatesDuplicatesWithinSession() throws {
        let model = try makeModel()
        let p1 = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: nil,
            agentType: "codex",
            agentPresence: "managed",
            activityState: "idle",
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
        let p2 = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%2"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: nil,
            agentType: "codex",
            agentPresence: "managed",
            activityState: "idle",
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
        model.panes = [p1, p2]

        XCTAssertEqual(model.paneDisplayTitle(for: p1), "codex session 1")
        XCTAssertEqual(model.paneDisplayTitle(for: p2), "codex session 2")
    }

    func testLastActiveLabelDoesNotFallbackToUpdatedAt() throws {
        let model = try makeModel()
        let pane = makePane(paneID: "%1", displayCategory: "idle")
        XCTAssertEqual(model.lastActiveLabel(for: pane), "last active: -")
    }

    func testStateReasonRedundantForIdle() throws {
        let model = try makeModel()
        let pane = makePane(paneID: "%1", displayCategory: "idle")
        XCTAssertTrue(model.isStateReasonRedundant(for: pane))
    }

    func testStateReasonNotRedundantForWaitingInput() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%3"),
            windowName: nil,
            currentCmd: nil,
            paneTitle: nil,
            state: "waiting_input",
            reasonCode: nil,
            confidence: nil,
            runtimeID: nil,
            agentType: "codex",
            agentPresence: "managed",
            activityState: "waiting_input",
            displayCategory: "attention",
            needsUserAction: true,
            stateSource: nil,
            lastEventType: nil,
            lastEventAt: nil,
            awaitingResponseKind: "input",
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertFalse(model.isStateReasonRedundant(for: pane, withinCategory: "attention"))
    }

    func testSummaryCardsDoNotIncludeQueueCard() throws {
        let model = try makeModel()
        model.panes = [makePane(paneID: "%1", displayCategory: "idle")]
        let labels = model.summaryCards.map(\.0)
        XCTAssertFalse(labels.contains("Queue"))
    }

    func testShouldUseTerminalProxyRequiresCanonicalValuesAfterNormalization() throws {
        let model = try makeModel()
        let valid = CapabilityFlags(
            embeddedTerminal: true,
            terminalRead: true,
            terminalResize: true,
            terminalWriteViaActionSend: true,
            terminalAttach: true,
            terminalWrite: true,
            terminalStream: true,
            terminalProxyMode: "daemon-proxy-pty-poc",
            terminalFrameProtocol: "terminal-stream-v1"
        )
        XCTAssertTrue(model.shouldUseTerminalProxy(caps: valid))

        let normalizedValid = CapabilityFlags(
            embeddedTerminal: true,
            terminalRead: true,
            terminalResize: true,
            terminalWriteViaActionSend: true,
            terminalAttach: true,
            terminalWrite: true,
            terminalStream: true,
            terminalProxyMode: " DAEMON-PROXY-PTY-POC ",
            terminalFrameProtocol: " TERMINAL-STREAM-V1 "
        )
        XCTAssertTrue(model.shouldUseTerminalProxy(caps: normalizedValid))

        let missingMode = CapabilityFlags(
            embeddedTerminal: true,
            terminalRead: true,
            terminalResize: true,
            terminalWriteViaActionSend: true,
            terminalAttach: true,
            terminalWrite: true,
            terminalStream: true,
            terminalProxyMode: nil,
            terminalFrameProtocol: "terminal-stream-v1"
        )
        XCTAssertFalse(model.shouldUseTerminalProxy(caps: missingMode))

        let wrongProtocol = CapabilityFlags(
            embeddedTerminal: true,
            terminalRead: true,
            terminalResize: true,
            terminalWriteViaActionSend: true,
            terminalAttach: true,
            terminalWrite: true,
            terminalStream: true,
            terminalProxyMode: "daemon-proxy-pty-poc",
            terminalFrameProtocol: "snapshot-delta-reset"
        )
        XCTAssertFalse(model.shouldUseTerminalProxy(caps: wrongProtocol))
    }

    func testShouldAcceptTerminalAttachResponseRequiresCompletedAndSessionID() throws {
        let model = try makeModel()
        let ok = TerminalAttachResponse(
            sessionID: "term-s1",
            target: "local",
            paneID: "%1",
            runtimeID: "rt-1",
            stateVersion: 1,
            resultCode: "completed"
        )
        XCTAssertTrue(model.shouldAcceptTerminalAttachResponse(ok))

        let failed = TerminalAttachResponse(
            sessionID: "term-s1",
            target: "local",
            paneID: "%1",
            runtimeID: "rt-1",
            stateVersion: 1,
            resultCode: "failed"
        )
        XCTAssertFalse(model.shouldAcceptTerminalAttachResponse(failed))

        let noSession = TerminalAttachResponse(
            sessionID: "   ",
            target: "local",
            paneID: "%1",
            runtimeID: nil,
            stateVersion: nil,
            resultCode: "completed"
        )
        XCTAssertFalse(model.shouldAcceptTerminalAttachResponse(noSession))
    }

    func testShouldResetTerminalProxySessionMatchesKnownErrors() throws {
        let model = try makeModel()
        XCTAssertTrue(model.shouldResetTerminalProxySession(
            error: RuntimeError.commandFailed("agtmux-app terminal stream", 1, "E_RUNTIME_STALE: runtime guard mismatch")
        ))
        XCTAssertTrue(model.shouldResetTerminalProxySession(
            error: RuntimeError.commandFailed("agtmux-app terminal write", 1, "terminal session not found")
        ))
        XCTAssertFalse(model.shouldResetTerminalProxySession(
            error: RuntimeError.commandFailed("agtmux-app terminal stream", 1, "E_TARGET_UNREACHABLE")
        ))
        XCTAssertFalse(model.shouldResetTerminalProxySession(
            error: RuntimeError.invalidJSON("broken payload")
        ))
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
