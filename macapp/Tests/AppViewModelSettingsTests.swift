import Foundation
import XCTest
@testable import AGTMUXDesktop

@MainActor
final class AppViewModelSettingsTests: XCTestCase {
    func testComputeSnapshotPollIntervalUsesBaseWhenHighActivity() {
        let interval = AppViewModel.computeSnapshotPollInterval(
            baseInterval: 2,
            unchangedSnapshotStreak: 120,
            hasHighActivity: true,
            backoffStepCount: 3,
            maxExtraSeconds: 6
        )
        XCTAssertEqual(interval, 2, accuracy: 0.001)
    }

    func testComputeSnapshotPollIntervalBacksOffWhenStableAndIdle() {
        let base = AppViewModel.computeSnapshotPollInterval(
            baseInterval: 2,
            unchangedSnapshotStreak: 0,
            hasHighActivity: false,
            backoffStepCount: 3,
            maxExtraSeconds: 6
        )
        let step1 = AppViewModel.computeSnapshotPollInterval(
            baseInterval: 2,
            unchangedSnapshotStreak: 3,
            hasHighActivity: false,
            backoffStepCount: 3,
            maxExtraSeconds: 6
        )
        let step2 = AppViewModel.computeSnapshotPollInterval(
            baseInterval: 2,
            unchangedSnapshotStreak: 8,
            hasHighActivity: false,
            backoffStepCount: 3,
            maxExtraSeconds: 6
        )
        XCTAssertEqual(base, 2, accuracy: 0.001)
        XCTAssertEqual(step1, 3, accuracy: 0.001)
        XCTAssertEqual(step2, 4, accuracy: 0.001)
    }

    func testComputeSnapshotPollIntervalRespectsMaxBackoff() {
        let interval = AppViewModel.computeSnapshotPollInterval(
            baseInterval: 4,
            unchangedSnapshotStreak: 999,
            hasHighActivity: false,
            backoffStepCount: 3,
            maxExtraSeconds: 6
        )
        XCTAssertEqual(interval, 10, accuracy: 0.001)
    }

    func testTerminalPerformanceCollectsInputStreamAndFPSMetrics() throws {
        let model = try makeModel()
        let base = Date(timeIntervalSince1970: 100)

        model.noteTerminalInputDispatched(for: "pane-1", at: base)
        model.noteTerminalFrameApplied(for: "pane-1", at: base.addingTimeInterval(0.080))
        model.noteTerminalStreamRoundTrip(startedAt: base, completedAt: base.addingTimeInterval(0.120))
        model.noteTerminalFrameRendered(at: base)
        model.noteTerminalFrameRendered(at: base.addingTimeInterval(0.016))
        model.noteTerminalFrameRendered(at: base.addingTimeInterval(0.033))
        model.noteTerminalFrameRendered(at: base.addingTimeInterval(0.050))

        XCTAssertEqual(model.terminalPerformance.inputSampleCount, 1)
        XCTAssertEqual(model.terminalPerformance.streamSampleCount, 1)
        XCTAssertEqual(model.terminalPerformance.inputLatencyP50Ms ?? 0, 80, accuracy: 0.5)
        XCTAssertEqual(model.terminalPerformance.streamRTTP50Ms ?? 0, 120, accuracy: 0.5)
        XCTAssertGreaterThan(model.terminalPerformance.renderFPS, 45)
        XCTAssertTrue(model.terminalPerformanceSummary.contains("fps"))
    }

    func testTerminalPerformanceBudgetFailsWhenLatencyAndFPSArePoor() throws {
        let model = try makeModel()
        let base = Date(timeIntervalSince1970: 200)

        model.noteTerminalInputDispatched(for: "pane-1", at: base)
        model.noteTerminalFrameApplied(for: "pane-1", at: base.addingTimeInterval(0.400))
        model.noteTerminalStreamRoundTrip(startedAt: base, completedAt: base.addingTimeInterval(0.500))
        model.noteTerminalFrameRendered(at: base)
        model.noteTerminalFrameRendered(at: base.addingTimeInterval(0.200))

        XCTAssertFalse(model.terminalPerformanceWithinBudget)
    }

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

    func testPaneDisplayTitlePrefersRenamedOverride() async throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "claude",
            paneTitle: "✳ Claude Code",
            state: "idle",
            reasonCode: "idle",
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "claude",
            agentPresence: "managed",
            activityState: "idle",
            displayCategory: "idle",
            needsUserAction: nil,
            stateSource: "poller",
            lastEventType: "idle",
            lastEventAt: nil,
            awaitingResponseKind: nil,
            sessionLabel: "✳ Claude Code",
            sessionLabelSource: "pane_title",
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        model.panes = [pane]
        model.performRenamePane(pane, newName: "review lane")
        let ok = await waitUntil {
            model.paneDisplayTitle(for: pane) == "review lane"
        }
        XCTAssertTrue(ok)
        XCTAssertEqual(model.paneDisplayTitle(for: pane), "review lane")
    }

    func testReorderSessionSectionsPromotesSourceBeforeDestinationAndForcesStableMode() throws {
        let model = try makeModel()
        model.sessionSortMode = .recentActivity
        model.panes = [
            makePane(paneID: "%1", sessionName: "s1"),
            makePane(paneID: "%2", sessionName: "s2"),
            makePane(paneID: "%3", sessionName: "s3"),
        ]
        let sections = model.sessionSections
        let source = sections.first(where: { $0.sessionName == "s3" })!
        let destination = sections.first(where: { $0.sessionName == "s1" })!

        model.reorderSessionSections(sourceID: source.id, destinationID: destination.id)
        let reordered = model.sessionSections.map(\.sessionName)

        XCTAssertEqual(model.sessionSortMode, .stable)
        XCTAssertEqual(reordered.first, "s3")
    }

    func testLastActiveLabelDoesNotFallbackToUpdatedAt() throws {
        let model = try makeModel()
        let pane = makePane(paneID: "%1", displayCategory: "idle")
        XCTAssertEqual(model.lastActiveLabel(for: pane), "last active: -")
    }

    func testLastActiveShortLabelIgnoresAdministrativeEventTimestamp() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "codex",
            agentPresence: "managed",
            activityState: "idle",
            displayCategory: "idle",
            needsUserAction: nil,
            stateSource: "wrapper",
            lastEventType: "action.view-output",
            lastEventAt: ISO8601DateFormatter().string(from: Date()),
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertEqual(model.lastActiveShortLabel(for: pane), "-")
    }

    func testActivityStateDoesNotPromoteManagedIdleFromUpdatedAtOnly() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "codex",
            agentPresence: "managed",
            activityState: "idle",
            displayCategory: "idle",
            needsUserAction: nil,
            stateSource: "poller",
            lastEventType: nil,
            lastEventAt: nil,
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: ISO8601DateFormatter().string(from: Date())
        )
        XCTAssertEqual(model.activityState(for: pane), "idle")
    }

    func testActivityStateUsesExplicitDaemonActivityState() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "codex",
            agentPresence: "managed",
            activityState: "idle",
            displayCategory: "idle",
            needsUserAction: nil,
            stateSource: "hook",
            lastEventType: nil,
            lastEventAt: nil,
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(-5)),
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertEqual(model.activityState(for: pane), "idle")

        let runningPane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%2"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: "rt-2",
            agentType: "codex",
            agentPresence: "managed",
            activityState: "running",
            displayCategory: "running",
            needsUserAction: nil,
            stateSource: "hook",
            lastEventType: nil,
            lastEventAt: nil,
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertEqual(model.activityState(for: runningPane), "running")
    }

    func testActivityStatePrefersV2ShadowStateOverLegacyInference() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "claude",
            paneTitle: nil,
            state: "unknown",
            reasonCode: "waiting_input",
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "claude",
            agentPresence: "managed",
            activityState: "unknown",
            displayCategory: "unknown",
            needsUserAction: nil,
            stateSource: "poller",
            lastEventType: "agent.user_input_required",
            lastEventAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(-2)),
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            stateEngineVersion: "v2-shadow",
            providerV2: "claude",
            providerConfidenceV2: 0.99,
            activityStateV2: "idle",
            activityConfidenceV2: 0.85,
            activitySourceV2: "poller",
            activityReasonsV2: ["raw:managed_no_strong_signal"],
            evidenceTraceID: "trace-1",
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertEqual(model.activityState(for: pane), "idle")
        XCTAssertEqual(model.displayCategory(for: pane), "idle")
        XCTAssertEqual(model.stateReason(for: pane), "managed no strong signal")
    }

    func testActivityStatePromotesManagedUnknownToRunningWhenRecentClaudeRunningSignal() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "claude",
            paneTitle: nil,
            state: "unknown",
            reasonCode: "inconclusive",
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "claude",
            agentPresence: "managed",
            activityState: "unknown",
            displayCategory: "unknown",
            needsUserAction: nil,
            stateSource: "poller",
            lastEventType: "agent.turn.started",
            lastEventAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(-2)),
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertEqual(model.activityState(for: pane), "running")
    }

    func testActivityStateDemotesManagedUnknownToIdleWithoutRunningSignal() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "claude",
            paneTitle: nil,
            state: "unknown",
            reasonCode: "inconclusive",
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "claude",
            agentPresence: "managed",
            activityState: "unknown",
            displayCategory: "unknown",
            needsUserAction: nil,
            stateSource: "poller",
            lastEventType: "unknown",
            lastEventAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(-1)),
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(-1)),
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertEqual(model.activityState(for: pane), "idle")
    }

    func testActivityStateInfersWaitingInputFromExpandedEventVocabulary() throws {
        let model = try makeModel()
        let pane = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "claude",
            paneTitle: nil,
            state: "unknown",
            reasonCode: nil,
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "claude",
            agentPresence: "managed",
            activityState: "unknown",
            displayCategory: "unknown",
            needsUserAction: nil,
            stateSource: "hook",
            lastEventType: "agent.user_input_required",
            lastEventAt: ISO8601DateFormatter().string(from: Date()),
            awaitingResponseKind: nil,
            sessionLabel: nil,
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        XCTAssertEqual(model.activityState(for: pane), "waiting_input")
    }

    func testSessionStableOrderPersistsAcrossModelInstances() throws {
        let suiteName = "AGTMUXDesktopTests-SessionOrder-\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            XCTFail("Failed to create test defaults")
            throw NSError(domain: "test", code: 1)
        }
        defaults.removePersistentDomain(forName: suiteName)

        let firstModel = try makeModel(defaults: defaults)
        firstModel.sessionSortMode = .stable
        firstModel.panes = [
            makePane(paneID: "%1", displayCategory: "idle", sessionName: "z-session"),
            makePane(paneID: "%2", displayCategory: "idle", sessionName: "a-session"),
        ]
        let firstOrder = firstModel.sessionSections.map(\.sessionName)
        XCTAssertEqual(firstOrder, ["a-session", "z-session"])

        let secondModel = try makeModel(defaults: defaults)
        secondModel.sessionSortMode = .stable
        secondModel.panes = [
            makePane(paneID: "%3", displayCategory: "idle", sessionName: "z-session"),
            makePane(paneID: "%4", displayCategory: "idle", sessionName: "a-session"),
        ]
        let secondOrder = secondModel.sessionSections.map(\.sessionName)
        XCTAssertEqual(secondOrder, ["a-session", "z-session"])
    }

    func testPinnedSessionSortsBeforeUnpinnedRegardlessOfSortMode() throws {
        let model = try makeModel()
        model.sessionSortMode = .name
        model.panes = [
            makePane(paneID: "%1", displayCategory: "idle", sessionName: "a-session"),
            makePane(paneID: "%2", displayCategory: "idle", sessionName: "z-session"),
        ]
        model.setSessionPinned(target: "local", sessionName: "z-session", pinned: true)

        let order = model.sessionSections.map(\.sessionName)
        XCTAssertEqual(order, ["z-session", "a-session"])
    }

    func testPinnedSessionsPersistAcrossModelInstances() throws {
        let suiteName = "AGTMUXDesktopTests-PinnedSessions-\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            XCTFail("Failed to create test defaults")
            throw NSError(domain: "test", code: 1)
        }
        defaults.removePersistentDomain(forName: suiteName)

        let firstModel = try makeModel(defaults: defaults)
        firstModel.panes = [
            makePane(paneID: "%1", displayCategory: "idle", sessionName: "a-session"),
            makePane(paneID: "%2", displayCategory: "idle", sessionName: "z-session"),
        ]
        firstModel.setSessionPinned(target: "local", sessionName: "z-session", pinned: true)

        let secondModel = try makeModel(defaults: defaults)
        secondModel.panes = [
            makePane(paneID: "%3", displayCategory: "idle", sessionName: "a-session"),
            makePane(paneID: "%4", displayCategory: "idle", sessionName: "z-session"),
        ]
        XCTAssertTrue(secondModel.isSessionPinned(target: "local", sessionName: "z-session"))
        let secondOrder = secondModel.sessionSections.map(\.sessionName)
        XCTAssertEqual(secondOrder.first, "z-session")
    }

    func testShowPinnedOnlyFiltersSessionSections() throws {
        let model = try makeModel()
        model.panes = [
            makePane(paneID: "%1", displayCategory: "idle", sessionName: "a-session"),
            makePane(paneID: "%2", displayCategory: "idle", sessionName: "z-session"),
        ]
        model.setSessionPinned(target: "local", sessionName: "z-session", pinned: true)
        model.showPinnedOnly = true

        let visible = model.sessionSections.map(\.sessionName)
        XCTAssertEqual(visible, ["z-session"])
    }

    func testSessionSortPrefersDefaultTargetThenHealth() throws {
        let model = try makeModel()
        model.sessionSortMode = .name
        model.targets = [
            TargetItem(targetID: "local", targetName: "local", kind: "local", connectionRef: nil, isDefault: true, health: "ok"),
            TargetItem(targetID: "vm-down", targetName: "vm-down", kind: "ssh", connectionRef: "ssh://vm-down", isDefault: false, health: "down"),
            TargetItem(targetID: "vm-ok", targetName: "vm-ok", kind: "ssh", connectionRef: "ssh://vm-ok", isDefault: false, health: "ok"),
        ]
        model.panes = [
            makePane(paneID: "%1", displayCategory: "idle", target: "vm-down", sessionName: "a-vm-down"),
            makePane(paneID: "%2", displayCategory: "idle", target: "vm-ok", sessionName: "z-vm-ok"),
            makePane(paneID: "%3", displayCategory: "idle", target: "local", sessionName: "z-local-default"),
        ]

        let ordered = model.sessionSections.map { "\($0.target)/\($0.sessionName)" }
        XCTAssertEqual(ordered, ["local/z-local-default", "vm-ok/z-vm-ok", "vm-down/a-vm-down"])
    }

    func testAutoReconnectAttemptsDownSSHTargetAndMarksHealthOnSuccess() async throws {
        let capture = CLIRunCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { executable, args in
                capture.record(executable: executable, args: args)
                if args.count >= 5, args[2] == "target", args[3] == "connect", args[4] == "vm1" {
                    return "{\"targets\":[{\"target_id\":\"vm1\",\"target_name\":\"vm1\",\"kind\":\"ssh\",\"connection_ref\":\"ssh://vm1\",\"is_default\":false,\"health\":\"ok\"}]}"
                }
                return "{\"targets\":[]}"
            }
        )
        let model = try makeModel(client: client)
        model.targets = [
            TargetItem(targetID: "vm1", targetName: "vm1", kind: "ssh", connectionRef: "ssh://vm1", isDefault: false, health: "down")
        ]

        await model.autoReconnectTargetsIfNeeded(now: Date(timeIntervalSince1970: 100))

        let connectCalls = capture.records().filter { record in
            record.args.count >= 5 && record.args[2] == "target" && record.args[3] == "connect" && record.args[4] == "vm1"
        }
        XCTAssertEqual(connectCalls.count, 1)
        XCTAssertEqual(model.targetHealth(for: "vm1"), "ok")
    }

    func testAutoReconnectBackoffSkipsRapidRetriesAfterFailure() async throws {
        let capture = CLIRunCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { executable, args in
                capture.record(executable: executable, args: args)
                if args.count >= 5, args[2] == "target", args[3] == "connect" {
                    throw RuntimeError.commandFailed("agtmux-app target connect", 1, "connect failed")
                }
                return "{\"targets\":[]}"
            }
        )
        let model = try makeModel(client: client)
        model.targets = [
            TargetItem(targetID: "vm1", targetName: "vm1", kind: "ssh", connectionRef: "ssh://vm1", isDefault: false, health: "down")
        ]

        let t0 = Date(timeIntervalSince1970: 100)
        await model.autoReconnectTargetsIfNeeded(now: t0)
        await model.autoReconnectTargetsIfNeeded(now: t0.addingTimeInterval(1))
        await model.autoReconnectTargetsIfNeeded(now: t0.addingTimeInterval(5))

        let connectCalls = capture.records().filter { record in
            record.args.count >= 5 && record.args[2] == "target" && record.args[3] == "connect" && record.args[4] == "vm1"
        }
        XCTAssertEqual(connectCalls.count, 2)
    }

    func testPaneOrderWithinSessionIgnoresCategoryTransitions() throws {
        let model = try makeModel()
        let pane1 = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%1"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "running",
            reasonCode: nil,
            confidence: nil,
            runtimeID: "rt-1",
            agentType: "codex",
            agentPresence: "managed",
            activityState: "running",
            displayCategory: "running",
            needsUserAction: nil,
            stateSource: nil,
            lastEventType: nil,
            lastEventAt: nil,
            awaitingResponseKind: nil,
            sessionLabel: "one",
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        let pane2 = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: "%2"),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: "rt-2",
            agentType: "codex",
            agentPresence: "managed",
            activityState: "idle",
            displayCategory: "idle",
            needsUserAction: nil,
            stateSource: nil,
            lastEventType: nil,
            lastEventAt: nil,
            awaitingResponseKind: nil,
            sessionLabel: "two",
            sessionLabelSource: nil,
            lastInteractionAt: nil,
            updatedAt: "2026-02-15T00:00:00Z"
        )
        model.panes = [pane2, pane1]
        guard let section = model.sessionSections.first else {
            XCTFail("missing session section")
            return
        }
        XCTAssertEqual(section.panes.map(\.identity.paneID), ["%1", "%2"])
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

    func testOpenSelectedPaneInExternalTerminalBuildsLocalTmuxJumpCommand() throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })
        let pane = makePane(paneID: "%1", displayCategory: "idle")
        model.panes = [pane]
        model.selectedPane = pane

        model.openSelectedPaneInExternalTerminal()

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "/usr/bin/osascript")
        let joined = captured.args.joined(separator: " ")
        XCTAssertTrue(joined.contains("tell application \"Terminal\" to do script"))
        XCTAssertTrue(joined.contains("tmux select-window -t '@1'"))
        XCTAssertTrue(joined.contains("tmux select-pane -t '%1'"))
        XCTAssertTrue(joined.contains("&&"))
        assertSubsequenceOrder(
            joined,
            expected: [
                "tmux select-window -t",
                "tmux select-pane -t",
                "tmux attach-session -t",
            ]
        )
        XCTAssertEqual(model.errorMessage, "")
    }

    func testOpenSelectedPaneInExternalTerminalBuildsSSHCommand() throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })
        model.targets = [
            TargetItem(
                targetID: "vm1",
                targetName: "vm1",
                kind: "ssh",
                connectionRef: "ssh://devvm",
                isDefault: false,
                health: "ok"
            )
        ]
        let pane = makePane(paneID: "%3", displayCategory: "idle", target: "vm1")
        model.panes = [pane]
        model.selectedPane = pane

        model.openSelectedPaneInExternalTerminal()

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "/usr/bin/osascript")
        let joined = captured.args.joined(separator: " ")
        XCTAssertTrue(joined.contains("ssh -t"))
        XCTAssertTrue(joined.contains("devvm"))
        XCTAssertTrue(joined.contains("&&"))
        assertSubsequenceOrder(
            joined,
            expected: [
                "tmux select-window -t",
                "tmux select-pane -t",
                "tmux attach-session -t",
            ]
        )
        XCTAssertEqual(model.errorMessage, "")
    }

    func testOpenSelectedPaneInExternalTerminalFailsWhenTargetIsUnavailable() throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })
        let pane = makePane(paneID: "%9", displayCategory: "idle", target: "missing-target")
        model.panes = [pane]
        model.selectedPane = pane

        model.openSelectedPaneInExternalTerminal()

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "")
        XCTAssertTrue(captured.args.isEmpty)
        XCTAssertTrue(model.errorMessage.contains("target is unavailable"))
    }

    func testOpenSelectedPaneInExternalTerminalFailsWhenTargetKindIsUnsupported() throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })
        model.targets = [
            TargetItem(
                targetID: "vm2",
                targetName: "vm2",
                kind: "container",
                connectionRef: nil,
                isDefault: false,
                health: "ok"
            )
        ]
        let pane = makePane(paneID: "%10", displayCategory: "idle", target: "vm2")
        model.panes = [pane]
        model.selectedPane = pane

        model.openSelectedPaneInExternalTerminal()

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "")
        XCTAssertTrue(captured.args.isEmpty)
        XCTAssertTrue(model.errorMessage.contains("unsupported target kind"))
    }

    func testOpenSelectedPaneInExternalTerminalFailsWhenTargetKindIsUnavailable() throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })
        model.targets = [
            TargetItem(
                targetID: "vm3",
                targetName: "vm3",
                kind: "   ",
                connectionRef: nil,
                isDefault: false,
                health: "ok"
            )
        ]
        let pane = makePane(paneID: "%11", displayCategory: "idle", target: "vm3")
        model.panes = [pane]
        model.selectedPane = pane

        model.openSelectedPaneInExternalTerminal()

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "")
        XCTAssertTrue(captured.args.isEmpty)
        XCTAssertTrue(model.errorMessage.contains("target kind is unavailable"))
    }

    func testOpenSelectedPaneInExternalTerminalFailsWhenSSHConnectionRefMissing() throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })
        model.targets = [
            TargetItem(
                targetID: "vm4",
                targetName: "vm4",
                kind: "ssh",
                connectionRef: nil,
                isDefault: false,
                health: "ok"
            )
        ]
        let pane = makePane(paneID: "%12", displayCategory: "idle", target: "vm4")
        model.panes = [pane]
        model.selectedPane = pane

        model.openSelectedPaneInExternalTerminal()

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "")
        XCTAssertTrue(captured.args.isEmpty)
        XCTAssertTrue(model.errorMessage.contains("connection_ref is unavailable"))
    }

    func testOpenSelectedPaneInExternalTerminalFailsWhenPaneIdentityIncomplete() throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })
        let incomplete = PaneItem(
            identity: PaneIdentity(target: "local", sessionName: "s1", windowID: "@1", paneID: ""),
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
        model.panes = [incomplete]
        model.selectedPane = incomplete

        model.openSelectedPaneInExternalTerminal()

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "")
        XCTAssertTrue(captured.args.isEmpty)
        XCTAssertTrue(model.errorMessage.contains("pane identity is incomplete"))
    }

    func testOpenSelectedPaneInExternalTerminalRequiresSelection() throws {
        let model = try makeModel()
        model.infoMessage = "opened in external terminal"
        model.selectedPane = nil

        model.openSelectedPaneInExternalTerminal()

        XCTAssertEqual(model.infoMessage, "")
        XCTAssertEqual(model.errorMessage, "Pane を選択してください。")
    }

    func testOpenSelectedPaneInExternalTerminalClearsInfoMessageOnRunnerFailure() throws {
        enum DummyError: LocalizedError {
            case failed
            var errorDescription: String? { "external terminal failed" }
        }

        let model = try makeModel(externalTerminalCommandRunner: { _, _ in
            throw DummyError.failed
        })
        let pane = makePane(paneID: "%1", displayCategory: "idle")
        model.panes = [pane]
        model.selectedPane = pane
        model.infoMessage = "opened in external terminal"

        model.openSelectedPaneInExternalTerminal()

        XCTAssertTrue(model.errorMessage.contains("external terminal failed"))
        XCTAssertEqual(model.infoMessage, "")
    }

    func testPerformKillSessionBuildsLocalTmuxKillCommand() async throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })

        model.performKillSession(target: "local", sessionName: "s1")
        let ok = await waitUntil {
            capture.snapshot().executable == "/bin/zsh"
        }
        XCTAssertTrue(ok)

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "/bin/zsh")
        XCTAssertEqual(captured.args.count, 2)
        XCTAssertEqual(captured.args[0], "-lc")
        XCTAssertTrue(captured.args[1].contains("tmux kill-session -t 's1'"))
        XCTAssertEqual(model.errorMessage, "")
        XCTAssertTrue(model.infoMessage.contains("session killed"))
    }

    func testPerformKillSessionBuildsSSHCommand() async throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })
        model.targets = [
            TargetItem(
                targetID: "vm1",
                targetName: "vm1",
                kind: "ssh",
                connectionRef: "ssh://devvm",
                isDefault: false,
                health: "ok"
            )
        ]

        model.performKillSession(target: "vm1", sessionName: "sprint")
        let ok = await waitUntil {
            capture.snapshot().executable == "/bin/zsh"
        }
        XCTAssertTrue(ok)

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "/bin/zsh")
        XCTAssertEqual(captured.args.count, 2)
        XCTAssertEqual(captured.args[0], "-lc")
        XCTAssertTrue(captured.args[1].contains("ssh 'devvm'"))
        XCTAssertTrue(captured.args[1].contains("tmux kill-session -t"))
        XCTAssertTrue(captured.args[1].contains("sprint"))
        XCTAssertEqual(model.errorMessage, "")
    }

    func testPerformKillSessionFailsWhenTargetIsUnavailable() async throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return ""
        })

        model.performKillSession(target: "missing-target", sessionName: "s1")
        let ok = await waitUntil {
            !model.errorMessage.isEmpty
        }
        XCTAssertTrue(ok)

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "")
        XCTAssertTrue(captured.args.isEmpty)
        XCTAssertTrue(model.errorMessage.contains("target is unavailable"))
    }

    func testPerformKillSessionRemovesSessionFromLocalStateImmediately() async throws {
        let model = try makeModel(externalTerminalCommandRunner: { _, _ in "" })
        let removed = makePane(paneID: "%1", displayCategory: "idle", target: "local", sessionName: "remove-me")
        let remaining = makePane(paneID: "%2", displayCategory: "idle", target: "local", sessionName: "keep-me")
        model.panes = [removed, remaining]
        model.selectedPane = removed

        model.performKillSession(target: "local", sessionName: "remove-me")
        let ok = await waitUntil {
            !model.panes.contains(where: { $0.identity.sessionName == "remove-me" })
        }
        XCTAssertTrue(ok)
        XCTAssertNil(model.selectedPane)
        XCTAssertTrue(model.panes.contains(where: { $0.identity.sessionName == "keep-me" }))
    }

    func testPerformKillPaneOptimisticallyRemovesPaneAndMarksInFlight() async throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            Thread.sleep(forTimeInterval: 0.15)
            return ""
        })
        let removed = makePane(paneID: "%1", displayCategory: "idle", target: "local", sessionName: "proj")
        let remaining = makePane(paneID: "%2", displayCategory: "idle", target: "local", sessionName: "proj")
        model.panes = [removed, remaining]
        model.selectedPane = removed

        model.performKillPane(removed)

        XCTAssertTrue(model.isPaneKillInFlight(removed.id))
        XCTAssertFalse(model.panes.contains(where: { $0.id == removed.id }))
        XCTAssertEqual(model.selectedPane?.id, remaining.id)
        let invoked = await waitUntil {
            capture.snapshot().executable == "/bin/zsh"
        }
        XCTAssertTrue(invoked)

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "/bin/zsh")
        XCTAssertEqual(captured.args.count, 2)
        XCTAssertEqual(captured.args[0], "-lc")
        XCTAssertTrue(captured.args[1].contains("tmux kill-pane -t '%1'"))
    }

    func testPerformKillPaneTreatsMissingPaneAsSuccess() async throws {
        let model = try makeModel(externalTerminalCommandRunner: { _, _ in
            throw RuntimeError.commandFailed("kill pane", 1, "can't find pane %1")
        })
        let removed = makePane(paneID: "%1", displayCategory: "idle", target: "local", sessionName: "proj")
        let remaining = makePane(paneID: "%2", displayCategory: "idle", target: "local", sessionName: "proj")
        model.panes = [removed, remaining]
        model.selectedPane = removed

        model.performKillPane(removed)

        let invoked = await waitUntil {
            !model.errorMessage.isEmpty || !model.infoMessage.isEmpty
        }
        XCTAssertTrue(invoked)
        XCTAssertEqual(model.errorMessage, "")
        XCTAssertTrue(model.isPaneKillInFlight(removed.id))
        XCTAssertFalse(model.panes.contains(where: { $0.id == removed.id }))
    }

    func testPerformCreatePaneUsesAnchorCurrentPathAndSelectsCreatedPane() async throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return "__AGTMUX_NEW_PANE__%2\n"
        })
        let basePane = makePane(paneID: "%1", displayCategory: "idle", target: "local", sessionName: "proj")
        let newPane = makePane(paneID: "%2", displayCategory: "idle", target: "local", sessionName: "proj")
        model.panes = [basePane, newPane]
        model.selectedPane = basePane

        model.performCreatePane(target: "local", sessionName: "proj", anchorPaneID: "%1")
        let created = await waitUntil {
            model.selectedPane?.identity.paneID == "%2"
        }
        XCTAssertTrue(created)

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "/bin/zsh")
        XCTAssertEqual(captured.args.count, 2)
        XCTAssertEqual(captured.args[0], "-lc")
        XCTAssertTrue(captured.args[1].contains("tmux split-window -P -F '__AGTMUX_NEW_PANE__#{pane_id}' -t '%1'"))
        XCTAssertTrue(captured.args[1].contains("-c '#{pane_current_path}'"))
    }

    func testPerformCreatePaneDoesNotSwitchToExistingPaneWhenPaneIDIsMissing() async throws {
        let capture = ExternalTerminalRunCapture()
        let model = try makeModel(externalTerminalCommandRunner: { executable, args in
            capture.set(executable: executable, args: args)
            return "__AGTMUX_NEW_PANE__\n"
        })
        let basePane = makePane(paneID: "%1", displayCategory: "idle", target: "local", sessionName: "s1")
        let otherPane = makePane(paneID: "%2", displayCategory: "idle", target: "local", sessionName: "s2")
        model.panes = [basePane, otherPane]
        model.selectedPane = otherPane

        model.performCreatePane(target: "local", sessionName: "s1", anchorPaneID: "%1")
        let completed = await waitUntil(timeout: 8.0) {
            !model.isPaneCreationInFlight(target: "local", sessionName: "s1")
        }
        XCTAssertTrue(completed)
        let reported = await waitUntil {
            model.errorMessage.contains("selection unchanged")
        }
        XCTAssertTrue(reported)
        XCTAssertEqual(model.selectedPane?.id, otherPane.id)

        let captured = capture.snapshot()
        XCTAssertEqual(captured.executable, "/bin/zsh")
        XCTAssertEqual(captured.args.count, 2)
        XCTAssertEqual(captured.args[0], "-lc")
        XCTAssertTrue(captured.args[1].contains("__AGTMUX_NEW_PANE__#{pane_id}"))
    }

    func testInteractiveTerminalInputPreferenceDefaultsToTrueAndPersists() throws {
        let suiteName = "AGTMUXDesktopTests-InteractiveInput-\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            XCTFail("Failed to create test defaults")
            throw NSError(domain: "test", code: 1)
        }
        defaults.removePersistentDomain(forName: suiteName)

        let model = try makeModel(defaults: defaults)
        XCTAssertTrue(model.interactiveTerminalInputEnabled)
        model.interactiveTerminalInputEnabled = false

        let restored = try makeModel(defaults: defaults)
        XCTAssertFalse(restored.interactiveTerminalInputEnabled)
    }

    private func makeModel(
        defaults providedDefaults: UserDefaults? = nil,
        client providedClient: AGTMUXCLIClient? = nil,
        externalTerminalCommandRunner: @escaping AppViewModel.ExternalTerminalCommandRunner = { _, _ in "" }
    ) throws -> AppViewModel {
        let daemon = try DaemonManager(
            socketPath: "/tmp/agtmux-test.sock",
            dbPath: "/tmp/agtmux-test.db",
            logPath: "/tmp/agtmux-test.log",
            daemonBinaryPath: "/usr/bin/true"
        )
        let client = providedClient ?? AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true"
        )

        let defaults: UserDefaults
        if let providedDefaults {
            defaults = providedDefaults
        } else {
            let suiteName = "AGTMUXDesktopTests-\(UUID().uuidString)"
            guard let isolated = UserDefaults(suiteName: suiteName) else {
                XCTFail("Failed to create test defaults")
                throw NSError(domain: "test", code: 1)
            }
            isolated.removePersistentDomain(forName: suiteName)
            defaults = isolated
        }
        return AppViewModel(
            daemon: daemon,
            client: client,
            defaults: defaults,
            externalTerminalCommandRunner: externalTerminalCommandRunner
        )
    }

    private func makePane(
        paneID: String,
        displayCategory: String = "idle",
        target: String = "local",
        sessionName: String = "s1"
    ) -> PaneItem {
        PaneItem(
            identity: PaneIdentity(
                target: target,
                sessionName: sessionName,
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

private final class ExternalTerminalRunCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var executable: String = ""
    private var args: [String] = []

    func set(executable: String, args: [String]) {
        lock.lock()
        self.executable = executable
        self.args = args
        lock.unlock()
    }

    func snapshot() -> (executable: String, args: [String]) {
        lock.lock()
        defer { lock.unlock() }
        return (executable, args)
    }
}

private final class CLIRunCapture: @unchecked Sendable {
    struct Record {
        let executable: String
        let args: [String]
    }

    private let lock = NSLock()
    private var captured: [Record] = []

    func record(executable: String, args: [String]) {
        lock.lock()
        captured.append(Record(executable: executable, args: args))
        lock.unlock()
    }

    func records() -> [Record] {
        lock.lock()
        defer { lock.unlock() }
        return captured
    }
}

private func assertSubsequenceOrder(_ text: String, expected parts: [String], file: StaticString = #filePath, line: UInt = #line) {
    var lowerBound = text.startIndex
    for part in parts {
        guard let found = text.range(of: part, range: lowerBound..<text.endIndex) else {
            XCTFail("expected token not found: \(part) in: \(text)", file: file, line: line)
            return
        }
        lowerBound = found.upperBound
    }
}

@MainActor
private func waitUntil(
    timeout: TimeInterval = 1.0,
    pollIntervalNanoseconds: UInt64 = 20_000_000,
    condition: @escaping () -> Bool
) async -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if condition() {
            return true
        }
        try? await Task.sleep(nanoseconds: pollIntervalNanoseconds)
    }
    return condition()
}
