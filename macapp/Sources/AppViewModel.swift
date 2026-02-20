import Foundation
import SwiftUI

@MainActor
final class AppViewModel: ObservableObject {
    typealias ExternalTerminalCommandRunner = (String, [String]) throws -> String

    enum DaemonState: String {
        case starting
        case running
        case error
    }

    enum ViewMode: String, CaseIterable, Identifiable {
        case bySession
        case byChronological

        var id: String { rawValue }

        var title: String {
            switch self {
            case .bySession:
                return "By Session"
            case .byChronological:
                return "By Chronological"
            }
        }
    }

    enum StatusFilter: String, CaseIterable, Identifiable {
        case all
        case managed
        case attention
        case pinned

        var id: String { rawValue }

        var title: String {
            switch self {
            case .all:
                return "All"
            case .managed:
                return "Managed"
            case .attention:
                return "Attention"
            case .pinned:
                return "Pinned"
            }
        }
    }

    enum SessionSortMode: String, CaseIterable, Identifiable {
        case stable
        case recentActivity
        case name

        var id: String { rawValue }

        var title: String {
            switch self {
            case .stable:
                return "Manual Order"
            case .recentActivity:
                return "Updated"
            case .name:
                return "Name"
            }
        }
    }

    enum WindowGrouping: String, CaseIterable, Identifiable {
        case off
        case auto
        case on

        var id: String { rawValue }

        var title: String {
            switch self {
            case .off:
                return "Off"
            case .auto:
                return "Auto"
            case .on:
                return "On"
            }
        }
    }

    enum ReviewKind: String, CaseIterable, Hashable {
        case taskCompleted = "task_completed"
        case needsInput = "needs_input"
        case needsApproval = "needs_approval"
        case error = "error"

        var title: String {
            switch self {
            case .taskCompleted:
                return "Task Completed"
            case .needsInput:
                return "Needs Input"
            case .needsApproval:
                return "Needs Approval"
            case .error:
                return "Error"
            }
        }
    }

    struct ReviewQueueItem: Identifiable, Hashable {
        let id: String
        let kind: ReviewKind
        let target: String
        let sessionName: String
        let paneID: String
        let windowID: String?
        let runtimeID: String?
        let createdAt: Date
        let summary: String
        var unread: Bool
        var acknowledgedAt: Date?
    }

    struct WindowSection: Identifiable, Hashable {
        let id: String
        let windowID: String
        let topCategory: String
        let byCategory: [String: Int]
        let panes: [PaneItem]
    }

    struct SessionSection: Identifiable, Hashable {
        let id: String
        let target: String
        let sessionName: String
        let topCategory: String
        let byCategory: [String: Int]
        let panes: [PaneItem]
        let windows: [WindowSection]
        let lastActiveAt: Date?
    }

    private struct PaneObservation {
        let state: String
        let category: String
        let attentionState: String
        let lastEventType: String
        let lastEventAt: String
        let awaitingKind: String
    }

    private struct TerminalRenderCache {
        let output: String
        let cursorX: Int?
        let cursorY: Int?
        let paneCols: Int?
        let paneRows: Int?
    }

    private struct InteractiveInputError: LocalizedError {
        let message: String

        var errorDescription: String? { message }
    }

    private struct TargetReconnectState {
        let nextAttemptAt: Date
        let nextBackoffSeconds: TimeInterval
    }

    struct TerminalPerformanceSnapshot: Equatable {
        let renderFPS: Double
        let inputLatencyP50Ms: Double?
        let streamRTTP50Ms: Double?
        let inputSampleCount: Int
        let streamSampleCount: Int

        static let empty = TerminalPerformanceSnapshot(
            renderFPS: 0,
            inputLatencyP50Ms: nil,
            streamRTTP50Ms: nil,
            inputSampleCount: 0,
            streamSampleCount: 0
        )
    }

    private enum PreferenceKey {
        static let uiPrefsVersion = "ui.prefs_version"
        static let viewMode = "ui.view_mode"
        static let statusFilter = "ui.status_filter"
        static let sessionSortMode = "ui.session_sort_mode"
        static let sessionStableOrder = "ui.session_stable_order"
        static let sessionStableOrderNext = "ui.session_stable_order_next"
        static let pinnedSessions = "ui.pinned_sessions"
        static let paneDisplayNameOverrides = "ui.pane_display_name_overrides"
        static let showPinnedOnly = "ui.show_pinned_only"
        static let windowGrouping = "ui.window_grouping"
        static let interactiveTerminalInputEnabled = "ui.interactive_terminal_input_enabled"
        static let showWindowMetadata = "ui.show_window_metadata"
        static let showWindowGroupBackground = "ui.show_window_group_background"
        static let showSessionMetadataInStatusView = "ui.show_session_metadata_in_status_view"
        static let showEmptyStatusColumns = "ui.show_empty_status_columns"
        static let showTechnicalDetails = "ui.show_technical_details"
        static let hideUnmanagedCategory = "ui.hide_unmanaged_category"
        static let showUnknownCategory = "ui.show_unknown_category"
        static let reviewUnreadOnly = "ui.review_unread_only"
    }

    @Published var daemonState: DaemonState = .starting
    @Published var errorMessage: String = ""
    @Published var infoMessage: String = ""
    @Published var targets: [TargetItem] = []
    @Published var sessions: [SessionItem] = []
    @Published var windows: [WindowItem] = []
    @Published var panes: [PaneItem] = []
    @Published private(set) var paneCreationInFlightSessionKeys: Set<String> = []
    @Published private(set) var paneKillInFlightPaneIDs: Set<String> = []
    let nativeTmuxTerminalEnabled: Bool
    @Published var selectedPane: PaneItem? {
        didSet {
            if oldValue?.id != selectedPane?.id {
                terminalStreamTask?.cancel()
                if let oldPaneID = oldValue?.id {
                    cancelBufferedInteractiveInput(for: oldPaneID)
                    Task { [weak self] in
                        guard let self else {
                            return
                        }
                        if let oldSessionID = await self.terminalSessionController.resetPane(oldPaneID) {
                            await self.detachTerminalProxySession(sessionID: oldSessionID)
                        }
                    }
                }
                let nextPaneID = selectedPane?.id
                if let nextPaneID {
                    if !applyCachedTerminalRender(for: nextPaneID) {
                        outputPreview = ""
                        terminalCursorX = nil
                        terminalCursorY = nil
                        terminalPaneCols = nil
                        terminalPaneRows = nil
                    }
                } else {
                    outputPreview = ""
                    terminalCursorX = nil
                    terminalCursorY = nil
                    terminalPaneCols = nil
                    terminalPaneRows = nil
                }
                if nextPaneID != nil {
                    syncSelectedPaneViewportIfKnown()
                }
                if autoStreamOnSelection {
                    Task { [weak self] in
                        guard let self else {
                            return
                        }
                        if let nextPaneID {
                            // Force a fresh snapshot/stream sync on pane switch.
                            await self.terminalSessionController.clearCursor(for: nextPaneID)
                        }
                        guard self.selectedPane?.id == nextPaneID else {
                            return
                        }
                        self.restartTerminalStreamForSelectedPane()
                    }
                } else {
                    if let nextPaneID {
                        Task { [weak self] in
                            guard let self else {
                                return
                            }
                            guard self.selectedPane?.id == nextPaneID else {
                                return
                            }
                            await self.terminalSessionController.clearCursor(for: nextPaneID)
                        }
                    }
                    if nextPaneID != nil {
                        // Keep generation semantics aligned with production even when auto stream is disabled for tests.
                        terminalStreamGeneration += 1
                    }
                }
            }
        }
    }
    @Published var searchQuery: String = ""
    @Published var sendText: String = ""
    @Published var sendEnter: Bool = true
    @Published var sendPaste: Bool = false
    @Published var interactiveTerminalInputEnabled: Bool = true {
        didSet {
            defaults.set(interactiveTerminalInputEnabled, forKey: PreferenceKey.interactiveTerminalInputEnabled)
        }
    }
    @Published var outputPreview: String = ""
    @Published var terminalCursorX: Int?
    @Published var terminalCursorY: Int?
    @Published var terminalPaneCols: Int?
    @Published var terminalPaneRows: Int?
    @Published private(set) var terminalPerformance: TerminalPerformanceSnapshot = .empty
    @Published var refreshInFlight: Bool = false
    @Published var viewMode: ViewMode = .bySession {
        didSet {
            defaults.set(viewMode.rawValue, forKey: PreferenceKey.viewMode)
        }
    }
    @Published var statusFilter: StatusFilter = .all {
        didSet {
            defaults.set(statusFilter.rawValue, forKey: PreferenceKey.statusFilter)
        }
    }
    @Published var sessionSortMode: SessionSortMode = .stable {
        didSet {
            defaults.set(sessionSortMode.rawValue, forKey: PreferenceKey.sessionSortMode)
        }
    }
    @Published var showPinnedOnly: Bool = false {
        didSet {
            defaults.set(showPinnedOnly, forKey: PreferenceKey.showPinnedOnly)
        }
    }
    @Published var windowGrouping: WindowGrouping = .auto {
        didSet {
            defaults.set(windowGrouping.rawValue, forKey: PreferenceKey.windowGrouping)
        }
    }
    @Published var showWindowMetadata: Bool = false {
        didSet {
            defaults.set(showWindowMetadata, forKey: PreferenceKey.showWindowMetadata)
        }
    }
    @Published var showWindowGroupBackground: Bool = true {
        didSet {
            defaults.set(showWindowGroupBackground, forKey: PreferenceKey.showWindowGroupBackground)
        }
    }
    @Published var showSessionMetadataInStatusView: Bool = false {
        didSet {
            defaults.set(showSessionMetadataInStatusView, forKey: PreferenceKey.showSessionMetadataInStatusView)
        }
    }
    @Published var showEmptyStatusColumns: Bool = false {
        didSet {
            defaults.set(showEmptyStatusColumns, forKey: PreferenceKey.showEmptyStatusColumns)
        }
    }
    @Published var showTechnicalDetails: Bool = false {
        didSet {
            defaults.set(showTechnicalDetails, forKey: PreferenceKey.showTechnicalDetails)
        }
    }
    @Published var hideUnmanagedCategory: Bool = false {
        didSet {
            defaults.set(hideUnmanagedCategory, forKey: PreferenceKey.hideUnmanagedCategory)
        }
    }
    @Published var showUnknownCategory: Bool = false {
        didSet {
            defaults.set(showUnknownCategory, forKey: PreferenceKey.showUnknownCategory)
        }
    }
    @Published var reviewUnreadOnly: Bool = true {
        didSet {
            defaults.set(reviewUnreadOnly, forKey: PreferenceKey.reviewUnreadOnly)
        }
    }
    @Published private(set) var reviewQueue: [ReviewQueueItem] = []
    @Published private(set) var informationalQueue: [ReviewQueueItem] = []

    private let daemon: DaemonManager
    private let client: AGTMUXCLIClient
    private let defaults: UserDefaults
    private let externalTerminalCommandRunner: ExternalTerminalCommandRunner
    // Keeps production behavior enabled by default while allowing deterministic unit tests.
    private let autoStreamOnSelection: Bool
    private var pollingTask: Task<Void, Never>?
    private var paneObservations: [String: PaneObservation] = [:]
    private var queueLastEmitByKey: [String: Date] = [:]
    private var sessionStableOrder: [String: Int] = [:]
    private var nextSessionStableOrder = 0
    private var pinnedSessionKeys: Set<String> = []
    private var paneDisplayNameOverridesByID: [String: String] = [:]
    private var targetReconnectStateByName: [String: TargetReconnectState] = [:]
    private var targetReconnectInFlightNames: Set<String> = []
    private var targetReconnectSweepInFlight = false
    private var frameRenderTimestamps: [Date] = []
    private var inputLatencySamplesMs: [Double] = []
    private var streamLatencySamplesMs: [Double] = []
    private var pendingInputLatencyStartByPaneID: [String: Date] = [:]
    private var terminalCapabilities: CapabilityFlags?
    private var terminalCapabilitiesFetchedAt: Date?
    private let terminalSessionController: TerminalSessionController
    private var terminalStreamTask: Task<Void, Never>?
    private var terminalResizeTask: Task<Void, Never>?
    private var terminalStreamGeneration: Int = 0
    private var didBootstrap = false
    private var recoveryInFlight = false
    private var lastRecoveryAttemptAt: Date?
    private var bufferedInteractiveBytesByPaneID: [String: [UInt8]] = [:]
    private var bufferedInteractiveFlushTaskByPaneID: [String: Task<Void, Never>] = [:]
    private var lastInteractiveInputAtByPaneID: [String: Date] = [:]
    private var terminalRenderCacheByPaneID: [String: TerminalRenderCache] = [:]
    private var lastKnownTerminalViewportCols: Int?
    private var lastKnownTerminalViewportRows: Int?
    private var unchangedSnapshotStreak: Int = 0
    private let queueDedupeWindowSeconds: TimeInterval = 30
    private let recoveryCooldownSeconds: TimeInterval = 6
    private let queueLimit = 250
    private let currentUIPrefsVersion = 10
    private let terminalCapabilitiesCacheTTLSeconds: TimeInterval = 60
    private let snapshotPollIntervalSeconds: TimeInterval = 2
    private let snapshotPollIntervalStreamingSeconds: TimeInterval = 4
    private let terminalStreamPollFastMillis = 45
    private let terminalStreamPollNormalMillis = 75
    private let terminalStreamPollIdleMillis = 140
    private let terminalStreamFastWindowSeconds: TimeInterval = 1.2
    private let terminalStreamDefaultLines = 240
    private let terminalStreamMinLines = 160
    private let terminalStreamMaxLines = 1200
    private let snapshotPollBackoffUnchangedStepCount = 3
    private let snapshotPollBackoffMaxExtraSeconds = 6
    private let snapshotPollUnchangedStreakCap = 120
    private let interactiveInputBatchWindowMillis = 12
    private let interactiveInputBatchChunkBytes = 320
    private let terminalOutputMaxChars = 60_000
    private let targetReconnectInitialBackoffSeconds: TimeInterval = 4
    private let targetReconnectMaxBackoffSeconds: TimeInterval = 90
    private let telemetryFPSWindowSeconds: TimeInterval = 3
    private let telemetryMaxSamples = 240
    private let telemetryBudgetInputP50Ms = 120.0
    private let telemetryBudgetStreamP50Ms = 220.0
    private let telemetryBudgetMinFPS = 24.0
    private let sessionTimeConfidenceThreshold = 0.65

    init(
        daemon: DaemonManager,
        client: AGTMUXCLIClient,
        defaults: UserDefaults = .standard,
        nativeTmuxTerminalEnabled: Bool = false,
        autoStreamOnSelection: Bool = true,
        terminalSessionController: TerminalSessionController = TerminalSessionController(),
        externalTerminalCommandRunner: @escaping ExternalTerminalCommandRunner = AppViewModel.runExternalTerminalCommand
    ) {
        self.daemon = daemon
        self.client = client
        self.defaults = defaults
        self.nativeTmuxTerminalEnabled = nativeTmuxTerminalEnabled
        self.externalTerminalCommandRunner = externalTerminalCommandRunner
        self.autoStreamOnSelection = autoStreamOnSelection
        self.terminalSessionController = terminalSessionController
        loadPreferences()
    }

    deinit {
        pollingTask?.cancel()
        terminalStreamTask?.cancel()
        terminalResizeTask?.cancel()
    }

    func bootstrap() {
        guard !didBootstrap else {
            return
        }
        didBootstrap = true
        Task {
            await startDaemonAndLoad()
        }
    }

    func manualRefresh() {
        Task {
            await refresh()
        }
    }

    func restartDaemon() {
        Task {
            daemonState = .starting
            errorMessage = ""
            do {
                try await daemon.restart(with: client)
                daemonState = .running
                await refresh()
            } catch {
                daemonState = .error
                errorMessage = error.localizedDescription
            }
        }
    }

    func openSelectedPaneInExternalTerminal() {
        guard let pane = selectedPane else {
            infoMessage = ""
            errorMessage = "Pane を選択してください。"
            return
        }
        do {
            let invocation = try buildExternalTerminalInvocation(for: pane)
            _ = try externalTerminalCommandRunner(invocation.executable, invocation.args)
            infoMessage = "opened in external terminal"
            errorMessage = ""
        } catch {
            infoMessage = ""
            errorMessage = error.localizedDescription
        }
    }

    func performKillSession(target: String, sessionName: String) {
        let targetToken = target.trimmingCharacters(in: .whitespacesAndNewlines)
        let sessionToken = sessionName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !targetToken.isEmpty, !sessionToken.isEmpty else {
            infoMessage = ""
            errorMessage = "session identity is incomplete"
            return
        }
        Task {
            do {
                let invocation = try buildSessionKillInvocation(target: targetToken, sessionName: sessionToken)
                _ = try externalTerminalCommandRunner(invocation.executable, invocation.args)
                if let selected = selectedPane,
                   selected.identity.target == targetToken,
                   selected.identity.sessionName == sessionToken {
                    selectedPane = nil
                }
                removeSessionFromLocalState(target: targetToken, sessionName: sessionToken)
                infoMessage = "session killed: \(sessionToken)"
                errorMessage = ""
            } catch {
                infoMessage = ""
                errorMessage = error.localizedDescription
            }
        }
    }

    func isSessionPinned(target: String, sessionName: String) -> Bool {
        pinnedSessionKeys.contains(paneSessionKey(target: target, sessionName: sessionName))
    }

    func isPaneCreationInFlight(target: String, sessionName: String) -> Bool {
        paneCreationInFlightSessionKeys.contains(paneSessionKey(target: target, sessionName: sessionName))
    }

    func isPaneKillInFlight(_ paneID: String) -> Bool {
        paneKillInFlightPaneIDs.contains(paneID.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    func setSessionPinned(target: String, sessionName: String, pinned: Bool) {
        let key = paneSessionKey(target: target, sessionName: sessionName)
        if pinned {
            pinnedSessionKeys.insert(key)
        } else {
            pinnedSessionKeys.remove(key)
        }
        persistPinnedSessions()
    }

    func reorderSessionSections(sourceID: String, destinationID: String) {
        let sourceKey = sourceID.trimmingCharacters(in: .whitespacesAndNewlines)
        let destinationKey = destinationID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !sourceKey.isEmpty, !destinationKey.isEmpty, sourceKey != destinationKey else {
            return
        }
        var ordered = sessionSections.map(\.id)
        guard let fromIndex = ordered.firstIndex(of: sourceKey),
              let toIndex = ordered.firstIndex(of: destinationKey) else {
            return
        }
        let moved = ordered.remove(at: fromIndex)
        ordered.insert(moved, at: toIndex)
        applySessionStableOrder(ordered)
        if sessionSortMode != .stable {
            sessionSortMode = .stable
        }
    }

    func performRenameSession(target: String, sessionName: String, newName: String) {
        let targetToken = target.trimmingCharacters(in: .whitespacesAndNewlines)
        let fromSession = sessionName.trimmingCharacters(in: .whitespacesAndNewlines)
        let toSession = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !targetToken.isEmpty, !fromSession.isEmpty else {
            infoMessage = ""
            errorMessage = "session identity is incomplete"
            return
        }
        guard !toSession.isEmpty else {
            infoMessage = ""
            errorMessage = "new session name is required"
            return
        }
        guard fromSession != toSession else {
            infoMessage = ""
            errorMessage = "session name is unchanged"
            return
        }
        Task {
            do {
                let command = "tmux rename-session -t \(shellQuote(fromSession)) \(shellQuote(toSession))"
                let invocation = try buildTmuxInvocation(target: targetToken, operation: "rename session", command: command)
                _ = try externalTerminalCommandRunner(invocation.executable, invocation.args)
                renameSessionInLocalState(target: targetToken, from: fromSession, to: toSession)
                infoMessage = "session renamed: \(fromSession) -> \(toSession)"
                errorMessage = ""
                await refresh()
            } catch {
                infoMessage = ""
                errorMessage = error.localizedDescription
            }
        }
    }

    func performCreatePane(target: String, sessionName: String, anchorPaneID: String?) {
        let targetToken = target.trimmingCharacters(in: .whitespacesAndNewlines)
        let sessionToken = sessionName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !targetToken.isEmpty, !sessionToken.isEmpty else {
            infoMessage = ""
            errorMessage = "session identity is incomplete"
            return
        }
        let sessionKey = paneSessionKey(target: targetToken, sessionName: sessionToken)
        let anchor = anchorPaneID?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let command: String
        if !anchor.isEmpty {
            command = "tmux split-window -P -F '__AGTMUX_NEW_PANE__#{pane_id}' -t \(shellQuote(anchor)) -c \(shellQuote("#{pane_current_path}"))"
        } else {
            command = "tmux split-window -P -F '__AGTMUX_NEW_PANE__#{pane_id}' -t \(shellQuote(sessionToken))"
        }
        let baselinePaneIDs = Set(
            panes
                .filter { pane in
                    normalizedTargetLookupKey(pane.identity.target) == normalizedTargetLookupKey(targetToken) &&
                        pane.identity.sessionName == sessionToken
                }
                .map(\.identity.paneID)
        )
        paneCreationInFlightSessionKeys.insert(sessionKey)
        let selectedPaneIDBeforeCreate = selectedPane?.id
        Task {
            defer {
                paneCreationInFlightSessionKeys.remove(sessionKey)
            }
            do {
                let invocation = try buildTmuxInvocation(target: targetToken, operation: "create pane", command: command)
                let output = try externalTerminalCommandRunner(invocation.executable, invocation.args)
                let preferredPaneID = parseCreatedPaneID(output)
                infoMessage = "pane created: \(sessionToken)"
                errorMessage = ""
                let resolvedPaneID = await resolveCreatedPaneIDAfterCreate(
                    target: targetToken,
                    sessionName: sessionToken,
                    preferredPaneID: preferredPaneID,
                    baselinePaneIDs: baselinePaneIDs
                )
                if let resolvedPaneID {
                    let selected = selectCreatedPane(
                        target: targetToken,
                        sessionName: sessionToken,
                        paneID: resolvedPaneID
                    )
                    if !selected {
                        restoreSelectionIfNeeded(paneID: selectedPaneIDBeforeCreate)
                        infoMessage = ""
                        errorMessage = "new pane created but selection failed"
                    }
                } else {
                    restoreSelectionIfNeeded(paneID: selectedPaneIDBeforeCreate)
                    infoMessage = ""
                    errorMessage = "new pane created but not visible yet; selection unchanged"
                }
            } catch {
                infoMessage = ""
                errorMessage = error.localizedDescription
            }
        }
    }

    func performKillPane(_ pane: PaneItem?) {
        guard let pane else {
            infoMessage = ""
            errorMessage = "Pane を選択してください。"
            return
        }
        let paneToken = pane.identity.paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !paneToken.isEmpty else {
            infoMessage = ""
            errorMessage = "pane identity is incomplete"
            return
        }
        let paneID = pane.id
        guard !paneKillInFlightPaneIDs.contains(paneID) else {
            return
        }
        paneKillInFlightPaneIDs.insert(paneID)
        removePaneFromLocalState(paneID)
        selectFallbackPaneAfterPaneRemoval(preferredBy: pane)
        syncSelectedPaneViewportIfKnown()
        restartTerminalStreamForSelectedPane()
        Task {
            do {
                let command = "tmux kill-pane -t \(shellQuote(paneToken))"
                let invocation = try buildTmuxInvocation(target: pane.identity.target, operation: "kill pane", command: command)
                _ = try externalTerminalCommandRunner(invocation.executable, invocation.args)
                let removed = await waitForPaneAbsenceInSnapshot(paneID)
                if removed {
                    infoMessage = "pane killed: \(paneToken)"
                    errorMessage = ""
                    syncSelectedPaneViewportIfKnown()
                    restartTerminalStreamForSelectedPane()
                } else {
                    paneKillInFlightPaneIDs.remove(paneID)
                    infoMessage = ""
                    errorMessage = "pane kill timed out; pane is still present"
                    syncSelectedPaneViewportIfKnown()
                    restartTerminalStreamForSelectedPane()
                }
            } catch {
                if isAlreadyMissingPaneError(error) {
                    infoMessage = "pane already closed: \(paneToken)"
                    errorMessage = ""
                    _ = await waitForPaneAbsenceInSnapshot(paneID)
                    syncSelectedPaneViewportIfKnown()
                    restartTerminalStreamForSelectedPane()
                    return
                }
                let removed = await waitForPaneAbsenceInSnapshot(paneID)
                if removed {
                    infoMessage = "pane already closed: \(paneToken)"
                    errorMessage = ""
                    syncSelectedPaneViewportIfKnown()
                    restartTerminalStreamForSelectedPane()
                } else {
                    paneKillInFlightPaneIDs.remove(paneID)
                    infoMessage = ""
                    errorMessage = error.localizedDescription
                    await refresh()
                    selectFallbackPaneAfterPaneRemoval(preferredBy: pane)
                    syncSelectedPaneViewportIfKnown()
                    restartTerminalStreamForSelectedPane()
                }
            }
        }
    }

    func performRenamePane(_ pane: PaneItem?, newName: String) {
        guard let pane else {
            infoMessage = ""
            errorMessage = "Pane を選択してください。"
            return
        }
        let paneToken = pane.identity.paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !paneToken.isEmpty else {
            infoMessage = ""
            errorMessage = "pane identity is incomplete"
            return
        }
        let name = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else {
            infoMessage = ""
            errorMessage = "pane name is required"
            return
        }
        Task {
            do {
                let command = "tmux select-pane -t \(shellQuote(paneToken)) -T \(shellQuote(name))"
                let invocation = try buildTmuxInvocation(target: pane.identity.target, operation: "rename pane", command: command)
                _ = try externalTerminalCommandRunner(invocation.executable, invocation.args)
                setPaneDisplayNameOverride(name: name, paneID: pane.id)
                infoMessage = "pane renamed: \(name)"
                errorMessage = ""
                await refresh()
            } catch {
                infoMessage = ""
                errorMessage = error.localizedDescription
            }
        }
    }

    func toggleSessionPinned(target: String, sessionName: String) {
        let key = paneSessionKey(target: target, sessionName: sessionName)
        if pinnedSessionKeys.contains(key) {
            pinnedSessionKeys.remove(key)
        } else {
            pinnedSessionKeys.insert(key)
        }
        persistPinnedSessions()
    }

    func targetHealth(for targetName: String) -> String {
        guard let target = targetRecord(for: targetName) else {
            return "unknown"
        }
        return normalizedTargetHealth(target.health)
    }

    func canReconnectTarget(named targetName: String) -> Bool {
        guard let target = targetRecord(for: targetName) else {
            return false
        }
        guard normalizedTargetKind(target.kind) == "ssh" else {
            return false
        }
        return trimmedNonEmpty(target.connectionRef) != nil
    }

    func reconnectTarget(named targetName: String) {
        Task { [weak self] in
            guard let self else {
                return
            }
            _ = await self.connectTargetNow(name: targetName, userInitiated: true)
        }
    }

    func noteTerminalFrameRendered(at: Date = Date()) {
        frameRenderTimestamps.append(at)
        trimFrameRenderWindow(now: at)
        recomputeTerminalPerformanceSnapshot(now: at)
    }

    func noteTerminalInputDispatched(for paneID: String, at: Date = Date()) {
        let paneToken = paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !paneToken.isEmpty else {
            return
        }
        pendingInputLatencyStartByPaneID[paneToken] = at
    }

    func noteTerminalFrameApplied(for paneID: String, at: Date = Date()) {
        let paneToken = paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !paneToken.isEmpty else {
            return
        }
        guard let startedAt = pendingInputLatencyStartByPaneID.removeValue(forKey: paneToken) else {
            return
        }
        let latencyMs = max(0, at.timeIntervalSince(startedAt) * 1000)
        appendLatencySample(latencyMs, into: &inputLatencySamplesMs)
        recomputeTerminalPerformanceSnapshot(now: at)
    }

    func noteTerminalStreamRoundTrip(startedAt: Date, completedAt: Date = Date()) {
        let latencyMs = max(0, completedAt.timeIntervalSince(startedAt) * 1000)
        appendLatencySample(latencyMs, into: &streamLatencySamplesMs)
        recomputeTerminalPerformanceSnapshot(now: completedAt)
    }

    var terminalPerformanceSummary: String {
        let fps = String(format: "%.1f", terminalPerformance.renderFPS)
        let input = terminalPerformance.inputLatencyP50Ms.map { String(format: "%.0fms", $0) } ?? "-"
        let stream = terminalPerformance.streamRTTP50Ms.map { String(format: "%.0fms", $0) } ?? "-"
        return "fps \(fps) | input \(input) | stream \(stream)"
    }

    var terminalPerformanceWithinBudget: Bool {
        if terminalPerformance.renderFPS > 0, terminalPerformance.renderFPS < telemetryBudgetMinFPS {
            return false
        }
        if let input = terminalPerformance.inputLatencyP50Ms, input > telemetryBudgetInputP50Ms {
            return false
        }
        if let stream = terminalPerformance.streamRTTP50Ms, stream > telemetryBudgetStreamP50Ms {
            return false
        }
        return true
    }

    func performAddTarget(
        name: String,
        kind: String,
        connectionRef: String,
        isDefault: Bool,
        connectAfterAdd: Bool
    ) {
        let targetName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !targetName.isEmpty else {
            infoMessage = ""
            errorMessage = "target name is required"
            return
        }

        let normalizedKind = normalizedToken(kind) ?? "ssh"
        if normalizedKind != "local" && normalizedKind != "ssh" {
            infoMessage = ""
            errorMessage = "target kind must be local or ssh"
            return
        }

        let normalizedConnectionRef = normalizedConnectionRefForTargetAdd(
            kind: normalizedKind,
            rawConnectionRef: connectionRef
        )
        if normalizedKind == "ssh", normalizedConnectionRef == nil {
            infoMessage = ""
            errorMessage = "ssh target requires connection reference"
            return
        }

        Task {
            var didAddTarget = false
            do {
                let created = try await client.addTarget(
                    name: targetName,
                    kind: normalizedKind,
                    connectionRef: normalizedConnectionRef,
                    isDefault: isDefault
                )
                didAddTarget = true
                let addedName = created.first?.targetName ?? targetName
                if connectAfterAdd {
                    _ = try await client.connectTarget(name: targetName)
                }
                await refresh()
                infoMessage = connectAfterAdd ? "target added and connected: \(addedName)" : "target added: \(addedName)"
                errorMessage = ""
            } catch {
                if didAddTarget {
                    await refresh()
                    infoMessage = "target added: \(targetName) (connect failed)"
                    errorMessage = error.localizedDescription
                } else {
                    infoMessage = ""
                    errorMessage = error.localizedDescription
                }
            }
        }
    }

    var hasSelectedPane: Bool {
        selectedPane != nil
    }

    func supportsNativeTmuxTerminal(for pane: PaneItem) -> Bool {
        normalizedToken(pane.identity.target) == "local"
    }

    var canSend: Bool {
        hasSelectedPane && !sendText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    func performSend() {
        guard let pane = selectedPane else {
            errorMessage = "Pane を選択してください。"
            return
        }
        let text = sendText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            errorMessage = "送信テキストが空です。"
            return
        }

        Task {
            do {
                guard selectedPane?.id == pane.id else {
                    return
                }
                let streamGeneration = terminalStreamGeneration
                let guardOptions = writeGuardOptions(for: pane)
                if await shouldUseTerminalProxy(for: pane.id) {
                    do {
                        let sessionID = try await ensureTerminalProxySession(for: pane, generation: streamGeneration)
                        guard selectedPane?.id == pane.id else {
                            return
                        }
                        markInteractiveInput(for: pane.id)
                        let resp = try await client.terminalWrite(
                            sessionID: sessionID,
                            text: sendText,
                            key: nil,
                            bytes: nil,
                            enter: sendEnter,
                            paste: sendPaste
                        )
                        if resp.resultCode != "completed" {
                            let reason = resp.errorCode ?? "unknown"
                            errorMessage = "terminal-write failed: \(reason)"
                            return
                        }
                        if shouldPerformImmediateWriteRefresh(for: pane.id, generation: streamGeneration) {
                            let currentCursor = await terminalSessionController.cursor(for: pane.id)
                            let lines = currentTerminalStreamLineBudget(for: pane.id)
                            let firstStreamStartedAt = Date()
                            let streamResp = try await client.terminalStream(
                                sessionID: sessionID,
                                cursor: currentCursor,
                                lines: lines
                            )
                            noteTerminalStreamRoundTrip(startedAt: firstStreamStartedAt)
                            await terminalSessionController.setCursor(streamResp.frame.cursor, for: pane.id)
                            applyTerminalStreamFrame(streamResp.frame, paneID: pane.id)
                            if streamResp.frame.frameType == "attached" {
                                let followStreamStartedAt = Date()
                                let followResp = try await client.terminalStream(
                                    sessionID: sessionID,
                                    cursor: streamResp.frame.cursor,
                                    lines: lines
                                )
                                noteTerminalStreamRoundTrip(startedAt: followStreamStartedAt)
                                await terminalSessionController.setCursor(followResp.frame.cursor, for: pane.id)
                                applyTerminalStreamFrame(followResp.frame, paneID: pane.id)
                            }
                        }
                        await terminalSessionController.recordSuccess(for: pane.id)
                        infoMessage = "terminal-write: \(resp.resultCode)"
                    } catch {
                        if error is CancellationError {
                            throw error
                        }
                        pendingInputLatencyStartByPaneID.removeValue(forKey: pane.id)
                        let outcome = await terminalSessionController.recordFailure(for: pane.id)
                        if shouldResetTerminalProxySession(error: error),
                           let staleSessionID = await terminalSessionController.clearProxySession(for: pane.id) {
                            await terminalSessionController.clearCursor(for: pane.id)
                            await detachTerminalProxySession(sessionID: staleSessionID)
                        }
                        if outcome.didEnterDegradedMode {
                            infoMessage = "interactive terminal degraded to snapshot mode"
                        }
                        throw error
                    }
                } else {
                    let requestRef = "macapp-send-\(UUID().uuidString)"
                    let resp = try await client.sendText(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        text: text,
                        requestRef: requestRef,
                        enter: sendEnter,
                        paste: sendPaste,
                        ifRuntime: guardOptions.ifRuntime,
                        ifState: guardOptions.ifState,
                        ifUpdatedWithin: guardOptions.ifUpdatedWithin,
                        forceStale: guardOptions.forceStale
                    )
                    infoMessage = "send: \(resp.resultCode) (\(resp.actionID))"
                    await refresh()
                }
                errorMessage = ""
                sendText = ""
            } catch is CancellationError {
                errorMessage = ""
                return
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    func performInteractiveInput(text: String? = nil, key: String? = nil, bytes: [UInt8]? = nil) {
        guard let pane = selectedPane else {
            errorMessage = "Pane を選択してください。"
            return
        }
        let inputText = text ?? ""
        let inputKey = (key ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let inputBytes = bytes ?? []
        let hasText = !inputText.isEmpty
        let hasKey = !inputKey.isEmpty
        let hasBytes = !inputBytes.isEmpty
        let modeCount = (hasText ? 1 : 0) + (hasKey ? 1 : 0) + (hasBytes ? 1 : 0)
        guard modeCount == 1 else {
            if modeCount > 1 {
                errorMessage = "text / key / bytes は同時指定できません。"
            }
            return
        }

        Task {
            do {
                guard selectedPane?.id == pane.id else {
                    return
                }
                let streamGeneration = terminalStreamGeneration
                try await performInteractiveInputCore(
                    pane: pane,
                    inputText: inputText,
                    inputKey: inputKey,
                    inputBytes: inputBytes,
                    streamGeneration: streamGeneration,
                    performImmediateRefresh: true
                )
                errorMessage = ""
            } catch is CancellationError {
                errorMessage = ""
                return
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    func performInteractiveInput(bytes: [UInt8]) {
        guard !bytes.isEmpty else {
            return
        }
        performInteractiveInput(text: nil, key: nil, bytes: bytes)
    }

    func enqueueInteractiveInput(bytes: [UInt8]) {
        guard !bytes.isEmpty else {
            return
        }
        guard let pane = selectedPane else {
            return
        }
        guard supportsNativeTmuxTerminal(for: pane) else {
            return
        }

        let paneID = pane.id
        markInteractiveInput(for: paneID)
        bufferedInteractiveBytesByPaneID[paneID, default: []].append(contentsOf: bytes)
        guard bufferedInteractiveFlushTaskByPaneID[paneID] == nil else {
            return
        }
        let streamGeneration = terminalStreamGeneration
        let delay = interactiveInputBatchWindowMillis
        bufferedInteractiveFlushTaskByPaneID[paneID] = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(delay))
            await self?.flushBufferedInteractiveInput(for: paneID, generation: streamGeneration)
        }
    }

    private func performInteractiveInputCore(
        pane: PaneItem,
        inputText: String,
        inputKey: String,
        inputBytes: [UInt8],
        streamGeneration: Int,
        performImmediateRefresh: Bool
    ) async throws {
        let hasText = !inputText.isEmpty
        let hasKey = !inputKey.isEmpty
        let hasBytes = !inputBytes.isEmpty
        let guardOptions = writeGuardOptions(for: pane)
        if await shouldUseTerminalProxy(for: pane.id) {
            do {
                let sessionID = try await ensureTerminalProxySession(for: pane, generation: streamGeneration)
                guard selectedPane?.id == pane.id else {
                    return
                }
                markInteractiveInput(for: pane.id)
                let resp = try await client.terminalWrite(
                    sessionID: sessionID,
                    text: hasText ? inputText : nil,
                    key: hasKey ? inputKey : nil,
                    bytes: hasBytes ? inputBytes : nil,
                    enter: false,
                    paste: false
                )
                if resp.resultCode != "completed" {
                    let reason = resp.errorCode ?? "unknown"
                    throw InteractiveInputError(message: "terminal-write failed: \(reason)")
                }
                if performImmediateRefresh && shouldPerformImmediateWriteRefresh(for: pane.id, generation: streamGeneration) {
                    let currentCursor = await terminalSessionController.cursor(for: pane.id)
                    let lines = currentTerminalStreamLineBudget(for: pane.id)
                    let firstStreamStartedAt = Date()
                    let streamResp = try await client.terminalStream(
                        sessionID: sessionID,
                        cursor: currentCursor,
                        lines: lines
                    )
                    noteTerminalStreamRoundTrip(startedAt: firstStreamStartedAt)
                    await terminalSessionController.setCursor(streamResp.frame.cursor, for: pane.id)
                    applyTerminalStreamFrame(streamResp.frame, paneID: pane.id)
                    if streamResp.frame.frameType == "attached" {
                        let followStreamStartedAt = Date()
                        let followResp = try await client.terminalStream(
                            sessionID: sessionID,
                            cursor: streamResp.frame.cursor,
                            lines: lines
                        )
                        noteTerminalStreamRoundTrip(startedAt: followStreamStartedAt)
                        await terminalSessionController.setCursor(followResp.frame.cursor, for: pane.id)
                        applyTerminalStreamFrame(followResp.frame, paneID: pane.id)
                    }
                }
                await terminalSessionController.recordSuccess(for: pane.id)
                return
            } catch {
                if error is CancellationError {
                    throw error
                }
                pendingInputLatencyStartByPaneID.removeValue(forKey: pane.id)
                let outcome = await terminalSessionController.recordFailure(for: pane.id)
                if shouldResetTerminalProxySession(error: error),
                   let staleSessionID = await terminalSessionController.clearProxySession(for: pane.id) {
                    await terminalSessionController.clearCursor(for: pane.id)
                    await detachTerminalProxySession(sessionID: staleSessionID)
                }
                if outcome.didEnterDegradedMode {
                    infoMessage = "interactive terminal degraded to snapshot mode"
                }
                throw error
            }
        }

        if hasText {
            let requestRef = "macapp-send-key-\(UUID().uuidString)"
            _ = try await client.sendText(
                target: pane.identity.target,
                paneID: pane.identity.paneID,
                text: inputText,
                requestRef: requestRef,
                enter: false,
                paste: false,
                ifRuntime: guardOptions.ifRuntime,
                ifState: guardOptions.ifState,
                ifUpdatedWithin: guardOptions.ifUpdatedWithin,
                forceStale: guardOptions.forceStale
            )
            await refresh()
            return
        }
        if hasBytes {
            throw InteractiveInputError(message: "interactive byte input requires terminal proxy support")
        }
        throw InteractiveInputError(message: "interactive key input requires terminal proxy support")
    }

    private func performInteractiveInputBytes(
        pane: PaneItem,
        bytes: [UInt8],
        generation: Int,
        performImmediateRefresh: Bool
    ) async throws {
        guard !bytes.isEmpty else {
            return
        }
        try await performInteractiveInputCore(
            pane: pane,
            inputText: "",
            inputKey: "",
            inputBytes: bytes,
            streamGeneration: generation,
            performImmediateRefresh: performImmediateRefresh
        )
    }

    func performTerminalResize(cols: Int, rows: Int) {
        guard cols > 0, rows > 0 else {
            return
        }
        lastKnownTerminalViewportCols = cols
        lastKnownTerminalViewportRows = rows
        guard let pane = selectedPane else {
            return
        }
        terminalResizeTask?.cancel()
        terminalResizeTask = Task { [weak self] in
            guard let self else {
                return
            }
            // Debounce rapid resize notifications while dragging window splitters.
            try? await Task.sleep(for: .milliseconds(80))
            guard !Task.isCancelled, self.selectedPane?.id == pane.id else {
                return
            }
            do {
                _ = try await self.client.terminalResize(
                    target: pane.identity.target,
                    paneID: pane.identity.paneID,
                    cols: cols,
                    rows: rows
                )
            } catch {
                // Keep resize errors non-blocking for typing interaction.
            }
        }
    }

    func performViewOutput(lines: Int = 80, forceSnapshot: Bool = false) {
        guard let pane = selectedPane else {
            errorMessage = "Pane を選択してください。"
            return
        }
        Task {
            do {
                guard selectedPane?.id == pane.id else {
                    return
                }
                let streamGeneration = terminalStreamGeneration
                if await shouldUseTerminalProxy(for: pane.id) {
                    do {
                        if forceSnapshot {
                            await terminalSessionController.clearCursor(for: pane.id)
                        }
                        let cursor = forceSnapshot ? nil : await terminalSessionController.cursor(for: pane.id)
                        let sessionID = try await ensureTerminalProxySession(for: pane, generation: streamGeneration)
                        guard selectedPane?.id == pane.id else {
                            return
                        }
                        let firstStreamStartedAt = Date()
                        let resp = try await client.terminalStream(sessionID: sessionID, cursor: cursor, lines: lines)
                        noteTerminalStreamRoundTrip(startedAt: firstStreamStartedAt)
                        var lastFrame = resp.frame
                        await terminalSessionController.setCursor(resp.frame.cursor, for: pane.id)
                        applyTerminalStreamFrame(resp.frame, paneID: pane.id)
                        if resp.frame.frameType == "attached" {
                            let followStreamStartedAt = Date()
                            let followResp = try await client.terminalStream(
                                sessionID: sessionID,
                                cursor: resp.frame.cursor,
                                lines: lines
                            )
                            noteTerminalStreamRoundTrip(startedAt: followStreamStartedAt)
                            lastFrame = followResp.frame
                            await terminalSessionController.setCursor(followResp.frame.cursor, for: pane.id)
                            applyTerminalStreamFrame(followResp.frame, paneID: pane.id)
                        }
                        await terminalSessionController.recordSuccess(for: pane.id)
                        infoMessage = "terminal-\(lastFrame.frameType): \(lastFrame.cursor)"
                    } catch {
                        if error is CancellationError {
                            throw error
                        }
                        let outcome = await terminalSessionController.recordFailure(for: pane.id)
                        if shouldResetTerminalProxySession(error: error),
                           let staleSessionID = await terminalSessionController.clearProxySession(for: pane.id) {
                            await terminalSessionController.clearCursor(for: pane.id)
                            await detachTerminalProxySession(sessionID: staleSessionID)
                        }
                        if outcome.didEnterDegradedMode {
                            infoMessage = "interactive terminal degraded to snapshot mode"
                        }
                        throw error
                    }
                } else if await shouldUseTerminalRead() {
                    if forceSnapshot {
                        await terminalSessionController.clearCursor(for: pane.id)
                    }
                    let cursor = forceSnapshot ? nil : await terminalSessionController.cursor(for: pane.id)
                    let readStartedAt = Date()
                    let resp = try await client.terminalRead(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        cursor: cursor,
                        lines: lines
                    )
                    noteTerminalStreamRoundTrip(startedAt: readStartedAt)
                    await terminalSessionController.setCursor(resp.frame.cursor, for: pane.id)
                    if resp.frame.frameType == "delta", let content = resp.frame.content {
                        outputPreview = outputPreview + content
                    } else {
                        outputPreview = resp.frame.content ?? ""
                    }
                    trimOutputPreviewIfNeeded()
                    infoMessage = "terminal-\(resp.frame.frameType): \(resp.frame.cursor)"
                } else {
                    let requestRef = "macapp-view-\(UUID().uuidString)"
                    let resp = try await client.viewOutput(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        requestRef: requestRef,
                        lines: lines
                    )
                    outputPreview = resp.output ?? ""
                    trimOutputPreviewIfNeeded()
                    infoMessage = "view-output: \(resp.resultCode) (\(resp.actionID))"
                }
                errorMessage = ""
            } catch is CancellationError {
                errorMessage = ""
                return
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    func performKillKeyINT(for pane: PaneItem? = nil) {
        kill(mode: "key", signal: "INT", pane: pane)
    }

    func performKillSignalTERM(for pane: PaneItem? = nil) {
        kill(mode: "signal", signal: "TERM", pane: pane)
    }

    var reviewUnreadCount: Int {
        reviewQueue.reduce(into: 0) { acc, item in
            if item.acknowledgedAt == nil && item.unread {
                acc += 1
            }
        }
    }

    var informationalUnreadCount: Int {
        informationalQueue.reduce(into: 0) { acc, item in
            if item.acknowledgedAt == nil && item.unread {
                acc += 1
            }
        }
    }

    var visibleReviewQueue: [ReviewQueueItem] {
        reviewQueue.filter { item in
            if item.acknowledgedAt != nil {
                return false
            }
            if reviewUnreadOnly && !item.unread {
                return false
            }
            return true
        }
    }

    var visibleInformationalQueue: [ReviewQueueItem] {
        informationalQueue.filter { item in
            if item.acknowledgedAt != nil {
                return false
            }
            if reviewUnreadOnly && !item.unread {
                return false
            }
            return true
        }
    }

    var statusGroups: [(String, [PaneItem])] {
        let hiddenCategories = hiddenStatusCategories()
        let order = ["attention", "running", "idle", "unmanaged", "unknown"]
            .filter { !hiddenCategories.contains($0) }
        let grouped = Dictionary(grouping: filteredPanes, by: { displayCategory(for: $0) })
        let primary = order.map { ($0, sortedPanes(grouped[$0, default: []])) }
        let extras = grouped.keys
            .filter { !order.contains($0) && !hiddenCategories.contains($0) }
            .sorted { lhs, rhs in
                let lp = categoryPrecedence(lhs)
                let rp = categoryPrecedence(rhs)
                if lp != rp {
                    return lp < rp
                }
                return lhs < rhs
            }
            .map { ($0, sortedPanes(grouped[$0, default: []])) }
        return primary + extras
    }

    var sessionSections: [SessionSection] {
        updateSessionStableOrder(with: filteredPanes)
        let grouped = Dictionary(grouping: filteredPanes, by: { paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName) })
        let sessionMeta = Dictionary(uniqueKeysWithValues: sessions.map { (paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName), $0) })
        let targetMeta = Dictionary(uniqueKeysWithValues: targets.map {
            (
                normalizedTargetLookupKey($0.targetName),
                (isDefault: $0.isDefault, healthRank: targetHealthRank($0.health))
            )
        })
        var out: [SessionSection] = []
        for (key, paneList) in grouped {
            guard let first = paneList.first else {
                continue
            }
            let sorted = sortedPanes(paneList)
            let counts = countByCategory(in: sorted)
            let topCategory = sessionMeta[key]?.topCategory ?? topCategory(from: counts)
            let windows = shouldGroupByWindow(sorted) ? buildWindowSections(sorted, key: key) : []
            let lastActiveAt = sorted.compactMap { paneRecencyDate(for: $0) }.max()
            out.append(SessionSection(
                id: key,
                target: first.identity.target,
                sessionName: first.identity.sessionName,
                topCategory: topCategory,
                byCategory: counts,
                panes: sorted,
                windows: windows,
                lastActiveAt: lastActiveAt
            ))
        }
        if showPinnedOnly {
            out = out.filter { pinnedSessionKeys.contains($0.id) }
        }
        out.sort { lhs, rhs in
            let lhsPinned = pinnedSessionKeys.contains(lhs.id)
            let rhsPinned = pinnedSessionKeys.contains(rhs.id)
            if lhsPinned != rhsPinned {
                return lhsPinned && !rhsPinned
            }
            let lhsMeta = targetMeta[normalizedTargetLookupKey(lhs.target)] ?? (isDefault: false, healthRank: targetHealthRank("unknown"))
            let rhsMeta = targetMeta[normalizedTargetLookupKey(rhs.target)] ?? (isDefault: false, healthRank: targetHealthRank("unknown"))
            if lhsMeta.isDefault != rhsMeta.isDefault {
                return lhsMeta.isDefault && !rhsMeta.isDefault
            }
            if lhsMeta.healthRank != rhsMeta.healthRank {
                return lhsMeta.healthRank < rhsMeta.healthRank
            }
            switch sessionSortMode {
            case .stable:
                let li = stableSessionOrder(for: lhs.id)
                let ri = stableSessionOrder(for: rhs.id)
                if li != ri {
                    return li < ri
                }
            case .recentActivity:
                let lDate = lhs.lastActiveAt ?? Date.distantPast
                let rDate = rhs.lastActiveAt ?? Date.distantPast
                if lDate != rDate {
                    return lDate > rDate
                }
            case .name:
                break
            }
            if lhs.target != rhs.target {
                return lhs.target < rhs.target
            }
            if lhs.sessionName != rhs.sessionName {
                return lhs.sessionName < rhs.sessionName
            }
            let li = stableSessionOrder(for: lhs.id)
            let ri = stableSessionOrder(for: rhs.id)
            return li < ri
        }
        return out
    }

    var chronologicalPanes: [PaneItem] {
        filteredPanes.sorted { lhs, rhs in
            let lDate = paneRecencyDate(for: lhs) ?? Date.distantPast
            let rDate = paneRecencyDate(for: rhs) ?? Date.distantPast
            if lDate != rDate {
                return lDate > rDate
            }
            if lhs.identity.target != rhs.identity.target {
                return lhs.identity.target < rhs.identity.target
            }
            if lhs.identity.sessionName != rhs.identity.sessionName {
                return lhs.identity.sessionName < rhs.identity.sessionName
            }
            if lhs.identity.windowID != rhs.identity.windowID {
                return lhs.identity.windowID < rhs.identity.windowID
            }
            return lhs.identity.paneID < rhs.identity.paneID
        }
    }

    func paneRecencyDate(for pane: PaneItem) -> Date? {
        guard agentPresence(for: pane) == "managed" else {
            return nil
        }
        if let confidence = pane.sessionTimeConfidence, confidence < sessionTimeConfidenceThreshold {
            return nil
        }
        if let sessionLastActive = parseTimestamp(pane.sessionLastActiveAt ?? "") {
            return sessionLastActive
        }
        return nil
    }

    var summaryCards: [(String, Int)] {
        let panes = filteredPanes
        let counts = countByCategory(in: panes)
        let sessions = Set(panes.map { paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName) }).count
        return [
            ("Sessions", sessions),
            ("Panes", panes.count),
            ("Attention", counts["attention", default: 0]),
            ("Running", counts["running", default: 0]),
            ("Idle", counts["idle", default: 0]),
        ]
    }

    func acknowledgeQueueItem(_ item: ReviewQueueItem) {
        if let index = reviewQueue.firstIndex(where: { $0.id == item.id }) {
            reviewQueue[index].acknowledgedAt = Date()
            reviewQueue[index].unread = false
            return
        }
        if let index = informationalQueue.firstIndex(where: { $0.id == item.id }) {
            informationalQueue[index].acknowledgedAt = Date()
            informationalQueue[index].unread = false
        }
    }

    func acknowledgeAllQueueItems() {
        let now = Date()
        for idx in reviewQueue.indices {
            if reviewQueue[idx].acknowledgedAt == nil {
                reviewQueue[idx].acknowledgedAt = now
                reviewQueue[idx].unread = false
            }
        }
        for idx in informationalQueue.indices {
            if informationalQueue[idx].acknowledgedAt == nil {
                informationalQueue[idx].acknowledgedAt = now
                informationalQueue[idx].unread = false
            }
        }
    }

    func openQueueItem(_ item: ReviewQueueItem) {
        selectedPane = panes.first(where: {
            $0.identity.target == item.target &&
                $0.identity.sessionName == item.sessionName &&
                $0.identity.paneID == item.paneID
        })
        if let index = reviewQueue.firstIndex(where: { $0.id == item.id }) {
            reviewQueue[index].unread = false
            return
        }
        if let index = informationalQueue.firstIndex(where: { $0.id == item.id }) {
            informationalQueue[index].unread = false
        }
    }

    func categoryLabel(_ category: String) -> String {
        switch category {
        case "attention":
            return "ATTENTION"
        case "running":
            return "RUNNING"
        case "idle":
            return "IDLE"
        case "unmanaged":
            return "UNMANAGED"
        default:
            return "UNKNOWN"
        }
    }

    func displayCategory(for pane: PaneItem) -> String {
        if isActionableAttentionState(attentionState(for: pane)) {
            return "attention"
        }
        if pane.stateEngineVersion == "v2-shadow" || normalizedToken(pane.activityStateV2) != nil {
            let presence = agentPresence(for: pane)
            let activity = activityState(for: pane)
            if presence == "none" {
                return "unmanaged"
            }
            switch activity {
            case "waiting_input", "waiting_approval", "error":
                return "attention"
            case "running":
                return "running"
            case "idle":
                return "idle"
            default:
                return "unknown"
            }
        }
        if let cat = normalizedToken(pane.displayCategory) {
            if cat == "unknown", agentPresence(for: pane) == "managed" {
                switch activityState(for: pane) {
                case "running":
                    return "running"
                case "waiting_input", "waiting_approval", "error":
                    return "attention"
                case "idle":
                    return "idle"
                default:
                    break
                }
            }
            return cat
        }
        let presence = agentPresence(for: pane)
        let activity = activityState(for: pane)
        if presence == "none" {
            return "unmanaged"
        }
        switch activity {
        case "waiting_input", "waiting_approval", "error":
            return "attention"
        case "running":
            return "running"
        case "idle":
            return "idle"
        default:
            return "unknown"
        }
    }

    func needsUserAction(for pane: PaneItem) -> Bool {
        if isActionableAttentionState(attentionState(for: pane)) {
            return true
        }
        if let explicit = pane.needsUserAction {
            return explicit
        }
        switch activityState(for: pane) {
        case "waiting_input", "waiting_approval", "error":
            return true
        default:
            return false
        }
    }

    func attentionState(for pane: PaneItem) -> String {
        if let explicit = normalizedToken(pane.attentionState) {
            return explicit
        }
        switch activityState(for: pane) {
        case "waiting_input":
            return "action_required_input"
        case "waiting_approval":
            return "action_required_approval"
        case "error":
            return "action_required_error"
        default:
            break
        }
        if isCompletionEventType(pane.lastEventType) {
            return "informational_completed"
        }
        return "none"
    }

    func activityState(for pane: PaneItem) -> String {
        if pane.stateEngineVersion == "v2-shadow",
           let stateV2 = normalizedToken(pane.activityStateV2) {
            if stateV2 == "completed" {
                return "idle"
            }
            return stateV2
        }
        if let stateV2 = normalizedToken(pane.activityStateV2) {
            if stateV2 == "completed" {
                return "idle"
            }
            return stateV2
        }

        if let inferred = inferAttentionStateFromReason(pane.reasonCode) {
            return inferred
        }
        if let inferred = inferAttentionStateFromEvent(pane.lastEventType) {
            return inferred
        }

        if let state = normalizedToken(pane.activityState) {
            if state == "completed" {
                return "idle"
            }
            if state == "unknown",
               shouldTreatManagedUnknownAsRunning(pane) {
                return "running"
            }
            if state == "unknown",
               shouldDemoteManagedUnknownToIdle(pane) {
                return "idle"
            }
            return state
        }

        let normalized = normalizedState(pane.state)
        switch normalized {
        case "running":
            return "running"
        case "waiting_input":
            return "waiting_input"
        case "waiting_approval":
            return "waiting_approval"
        case "error":
            return "error"
        case "idle", "completed":
            return "idle"
        default:
            if shouldTreatManagedUnknownAsRunning(pane) {
                return "running"
            }
            if shouldDemoteManagedUnknownToIdle(pane) {
                return "idle"
            }
            return "unknown"
        }
    }

    func stateReason(for pane: PaneItem) -> String {
        if pane.stateEngineVersion == "v2-shadow",
           let reasons = pane.activityReasonsV2,
           let first = reasons.first,
           !first.isEmpty {
            return formatReason(first)
        }
        switch activityState(for: pane) {
        case "waiting_input":
            return "waiting input"
        case "waiting_approval":
            return "waiting approval"
        case "error":
            return "runtime error"
        case "running":
            return "active"
        case "idle":
            if normalizedState(pane.state) == "completed" || isCompletionEventType(pane.lastEventType) {
                return "task completed"
            }
            return "idle"
        default:
            return pane.reasonCode ?? "unknown"
        }
    }

    private func formatReason(_ raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return "unknown"
        }
        if trimmed.hasPrefix("raw:") {
            return String(trimmed.dropFirst(4)).replacingOccurrences(of: "_", with: " ")
        }
        return trimmed.replacingOccurrences(of: "_", with: " ")
    }

    func paneDisplayTitle(for pane: PaneItem, among candidates: [PaneItem]? = nil) -> String {
        let base = basePaneDisplayTitle(for: pane)
        let source = candidates ?? panes
        let duplicates = source.filter {
            $0.identity.target == pane.identity.target &&
                $0.identity.sessionName == pane.identity.sessionName &&
                basePaneDisplayTitle(for: $0) == base
        }
        guard duplicates.count > 1 else {
            return base
        }
        let ordered = duplicates.sorted { lhs, rhs in
            lhs.identity.paneID < rhs.identity.paneID
        }
        guard let index = ordered.firstIndex(where: { $0.id == pane.id }) else {
            return base
        }
        return "\(base) \(index + 1)"
    }

    private func basePaneDisplayTitle(for pane: PaneItem) -> String {
        if let override = trimmedNonEmpty(paneDisplayNameOverridesByID[pane.id]) {
            return override
        }
        if let label = trimmedNonEmpty(pane.sessionLabel) {
            return label
        }
        if let paneTitle = trimmedNonEmpty(pane.paneTitle) {
            return paneTitle
        }
        let presence = agentPresence(for: pane)
        if presence == "managed" {
            if let agent = normalizedToken(pane.agentType), agent != "none", agent != "unknown" {
                return "\(agent) session"
            }
            return "agent session"
        }
        if let cmd = trimmedNonEmpty(pane.currentCmd) {
            return cmd
        }
        return "terminal pane"
    }

    func lastActiveLabel(for pane: PaneItem) -> String {
        let anchor = paneRecencyDate(for: pane)
        guard agentPresence(for: pane) == "managed", let updated = anchor else {
            return "last active: -"
        }
        return "last active: \(compactRelativeTimestamp(since: updated, now: Date()))"
    }

    func lastActiveShortLabel(for pane: PaneItem) -> String {
        guard agentPresence(for: pane) == "managed" else {
            return "-"
        }
        let anchor = paneRecencyDate(for: pane)
        guard let updated = anchor else {
            return "-"
        }
        return compactRelativeTimestamp(since: updated, now: Date())
    }

    func sessionLastActiveShortLabel(for section: SessionSection) -> String {
        guard let date = section.lastActiveAt else {
            return "-"
        }
        return compactRelativeTimestamp(since: date, now: Date())
    }

    func sessionCategorySummary(for section: SessionSection) -> String {
        let counts = section.byCategory
        var parts: [String] = []
        if counts["attention", default: 0] > 0 {
            parts.append("A\(counts["attention", default: 0])")
        }
        if counts["running", default: 0] > 0 {
            parts.append("R\(counts["running", default: 0])")
        }
        if counts["idle", default: 0] > 0 {
            parts.append("I\(counts["idle", default: 0])")
        }
        if parts.isEmpty {
            parts.append("0")
        }
        return parts.joined(separator: " ")
    }

    func actionableAttentionCount(target: String, sessionName: String) -> Int {
        panes.reduce(into: 0) { acc, pane in
            guard pane.identity.target == target, pane.identity.sessionName == sessionName else {
                return
            }
            if isActionableAttentionState(attentionState(for: pane)) {
                acc += 1
            }
        }
    }

    func isStateReasonRedundant(for pane: PaneItem, withinCategory category: String? = nil) -> Bool {
        let state = activityState(for: pane)
        let reason = normalizedToken(stateReason(for: pane).replacingOccurrences(of: " ", with: "_"))
        let cat = normalizedToken(category)
        switch state {
        case "running":
            if reason == "running" || reason == "active" {
                return true
            }
            if cat == "running" {
                return true
            }
        case "idle":
            if reason == "idle" {
                return true
            }
            if cat == "idle" {
                return true
            }
        case "waiting_input":
            return false
        case "waiting_approval":
            return false
        case "error":
            return false
        default:
            break
        }
        return false
    }

    func awaitingResponseKind(for pane: PaneItem) -> String? {
        if let explicit = normalizedToken(pane.awaitingResponseKind) {
            return explicit
        }
        switch activityState(for: pane) {
        case "waiting_input":
            return "input"
        case "waiting_approval":
            return "approval"
        default:
            return nil
        }
    }

    private func kill(mode: String, signal: String, pane: PaneItem? = nil) {
        guard let pane = pane ?? selectedPane else {
            errorMessage = "Pane を選択してください。"
            return
        }
        Task {
            do {
                let requestRef = "macapp-kill-\(UUID().uuidString)"
                let guardOptions = writeGuardOptions(for: pane)
                let resp: ActionResponse
                do {
                    resp = try await client.kill(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        requestRef: requestRef,
                        mode: mode,
                        signal: signal,
                        ifRuntime: guardOptions.ifRuntime,
                        ifState: guardOptions.ifState,
                        ifUpdatedWithin: guardOptions.ifUpdatedWithin,
                        forceStale: guardOptions.forceStale
                    )
                } catch {
                    guard isStaleGuardConflict(error) else {
                        throw error
                    }
                    resp = try await client.kill(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        requestRef: requestRef,
                        mode: mode,
                        signal: signal,
                        ifRuntime: guardOptions.ifRuntime,
                        ifState: guardOptions.ifState,
                        ifUpdatedWithin: guardOptions.ifUpdatedWithin,
                        forceStale: true
                    )
                }
                infoMessage = "kill: \(resp.resultCode) (\(resp.actionID))"
                errorMessage = ""
                await refresh()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    private func startDaemonAndLoad() async {
        daemonState = .starting
        clearBackendSurfaceMessages()
        do {
            try await daemon.ensureRunning(with: client)
            daemonState = .running
            await refresh()
            startPolling()
        } catch {
            daemonState = .error
            showBackgroundRecoveryNotice()
            _ = await autoRecoverFromDaemonError(triggeredBy: error)
            startPolling()
        }
    }

    private func startPolling() {
        pollingTask?.cancel()
        pollingTask = Task {
            while !Task.isCancelled {
                await refresh()
                let interval = currentSnapshotPollInterval()
                try? await Task.sleep(for: .milliseconds(Int(interval * 1000)))
            }
        }
    }

    private func currentSnapshotPollInterval() -> TimeInterval {
        let baseInterval: TimeInterval
        if autoStreamOnSelection, selectedPane != nil {
            baseInterval = snapshotPollIntervalStreamingSeconds
        } else {
            baseInterval = snapshotPollIntervalSeconds
        }
        return Self.computeSnapshotPollInterval(
            baseInterval: baseInterval,
            unchangedSnapshotStreak: unchangedSnapshotStreak,
            hasHighActivity: hasHighActivityPanes(),
            backoffStepCount: snapshotPollBackoffUnchangedStepCount,
            maxExtraSeconds: snapshotPollBackoffMaxExtraSeconds
        )
    }

    static func computeSnapshotPollInterval(
        baseInterval: TimeInterval,
        unchangedSnapshotStreak: Int,
        hasHighActivity: Bool,
        backoffStepCount: Int,
        maxExtraSeconds: Int
    ) -> TimeInterval {
        guard baseInterval > 0 else {
            return 1
        }
        guard !hasHighActivity else {
            return baseInterval
        }
        let safeStep = max(1, backoffStepCount)
        let safeStreak = max(0, unchangedSnapshotStreak)
        let safeMaxExtra = max(0, maxExtraSeconds)
        let extra = min(safeMaxExtra, safeStreak / safeStep)
        return baseInterval + TimeInterval(extra)
    }

    private func hasHighActivityPanes() -> Bool {
        panes.contains { pane in
            let category = displayCategory(for: pane)
            return category == "running" || category == "attention"
        }
    }

    private func flushBufferedInteractiveInput(for paneID: String, generation: Int) async {
        defer {
            bufferedInteractiveFlushTaskByPaneID[paneID] = nil
        }
        guard generation == terminalStreamGeneration else {
            bufferedInteractiveBytesByPaneID.removeValue(forKey: paneID)
            return
        }
        guard selectedPane?.id == paneID else {
            bufferedInteractiveBytesByPaneID.removeValue(forKey: paneID)
            return
        }
        guard let pane = panes.first(where: { $0.id == paneID }) else {
            bufferedInteractiveBytesByPaneID.removeValue(forKey: paneID)
            return
        }
        var payload = bufferedInteractiveBytesByPaneID.removeValue(forKey: paneID) ?? []
        guard !payload.isEmpty else {
            return
        }

        do {
            while !payload.isEmpty {
                guard generation == terminalStreamGeneration, selectedPane?.id == paneID else {
                    return
                }
                let chunkCount = min(payload.count, interactiveInputBatchChunkBytes)
                let chunk = Array(payload.prefix(chunkCount))
                payload.removeFirst(chunkCount)
                try await performInteractiveInputBytes(
                    pane: pane,
                    bytes: chunk,
                    generation: generation,
                    performImmediateRefresh: false
                )
            }
            errorMessage = ""
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func cancelBufferedInteractiveInput(for paneID: String) {
        bufferedInteractiveFlushTaskByPaneID[paneID]?.cancel()
        bufferedInteractiveFlushTaskByPaneID.removeValue(forKey: paneID)
        bufferedInteractiveBytesByPaneID.removeValue(forKey: paneID)
        pendingInputLatencyStartByPaneID.removeValue(forKey: paneID)
    }

    private func refresh() async {
        if refreshInFlight {
            return
        }
        refreshInFlight = true
        defer { refreshInFlight = false }
        do {
            let snapshot = try await client.fetchSnapshot()
            applySnapshot(snapshot)
            daemonState = .running
            clearBackendSurfaceMessages()
        } catch {
            daemonState = .error
            if shouldAttemptAutoRecover(for: error) {
                showBackgroundRecoveryNotice()
                _ = await autoRecoverFromDaemonError(triggeredBy: error)
            } else if isRuntimeTransportError(error) {
                showBackgroundRecoveryNotice()
            } else {
                errorMessage = error.localizedDescription
            }
        }
    }

    private func applySnapshot(_ snapshot: DashboardSnapshot) {
        let selectedBefore = selectedPane
        let previousWindowPaneIDs: Set<String> = {
            guard let selectedBefore else {
                return []
            }
            return Set(
                panes
                    .filter {
                        $0.identity.target == selectedBefore.identity.target &&
                            $0.identity.sessionName == selectedBefore.identity.sessionName &&
                            $0.identity.windowID == selectedBefore.identity.windowID
                    }
                    .map(\.id)
            )
        }()
        let snapshotChanged = targets != snapshot.targets ||
            sessions != snapshot.sessions ||
            windows != snapshot.windows ||
            panes != snapshot.panes
        if snapshotChanged {
            unchangedSnapshotStreak = 0
        } else {
            unchangedSnapshotStreak = min(unchangedSnapshotStreak + 1, snapshotPollUnchangedStreakCap)
        }
        observeTransitions(newPanes: snapshot.panes, now: Date())
        updateSessionStableOrder(with: snapshot.panes)
        if targets != snapshot.targets {
            targets = snapshot.targets
        }
        if sessions != snapshot.sessions {
            sessions = snapshot.sessions
        }
        if windows != snapshot.windows {
            windows = snapshot.windows
        }
        if panes != snapshot.panes {
            panes = snapshot.panes
        }
        paneKillInFlightPaneIDs = paneKillInFlightPaneIDs.intersection(Set(panes.map(\.id)))
        let liveTargetKeys = Set(targets.map { normalizedTargetLookupKey($0.targetName) })
        targetReconnectStateByName = targetReconnectStateByName.filter { liveTargetKeys.contains($0.key) }
        targetReconnectInFlightNames.formIntersection(liveTargetKeys)
        let paneIDs = Set(panes.map(\.id))
        let liveSessionKeys = Set(panes.map { paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName) })
        let removedPinned = pinnedSessionKeys.subtracting(liveSessionKeys)
        if !removedPinned.isEmpty {
            pinnedSessionKeys.subtract(removedPinned)
            persistPinnedSessions()
        }
        terminalRenderCacheByPaneID = terminalRenderCacheByPaneID.filter { paneIDs.contains($0.key) }
        lastInteractiveInputAtByPaneID = lastInteractiveInputAtByPaneID.filter { paneIDs.contains($0.key) }
        pendingInputLatencyStartByPaneID = pendingInputLatencyStartByPaneID.filter { paneIDs.contains($0.key) }
        if !paneIDs.isEmpty {
            let removedOverrides = Set(paneDisplayNameOverridesByID.keys).subtracting(paneIDs)
            if !removedOverrides.isEmpty {
                for key in removedOverrides {
                    paneDisplayNameOverridesByID.removeValue(forKey: key)
                }
                persistPaneDisplayNameOverrides()
            }
        }
        Task { [weak self] in
            guard let self else {
                return
            }
            let staleSessions = await self.terminalSessionController.prune(keepingPaneIDs: paneIDs)
            for sessionID in staleSessions {
                await self.detachTerminalProxySession(sessionID: sessionID)
            }
        }
        if let current = selectedPane {
            if let nextSelected = panes.first(where: { $0.id == current.id }) {
                if nextSelected != current {
                    selectedPane = nextSelected
                }
            } else {
                selectedPane = nil
                infoMessage = "選択中 pane が消えました。再選択してください。"
            }
        }
        if let selectedAfter = selectedPane,
           selectedBefore?.id == selectedAfter.id {
            let currentWindowPaneIDs = Set(
                panes
                    .filter {
                        $0.identity.target == selectedAfter.identity.target &&
                            $0.identity.sessionName == selectedAfter.identity.sessionName &&
                            $0.identity.windowID == selectedAfter.identity.windowID
                    }
                    .map(\.id)
            )
            if currentWindowPaneIDs != previousWindowPaneIDs {
                Task { [weak self] in
                    guard let self else {
                        return
                    }
                    let paneID = selectedAfter.id
                    await self.terminalSessionController.clearCursor(for: paneID)
                    guard self.selectedPane?.id == paneID else {
                        return
                    }
                    self.syncSelectedPaneViewportIfKnown()
                    self.restartTerminalStreamForSelectedPane()
                }
            }
        }
        Task { [weak self] in
            await self?.autoReconnectTargetsIfNeeded()
        }
    }

    private func observeTransitions(newPanes: [PaneItem], now: Date) {
        var next: [String: PaneObservation] = [:]
        for pane in newPanes {
            let category = displayCategory(for: pane)
            let state = normalizedState(pane.state)
            let currentAttentionState = attentionState(for: pane)
            let lastEventType = normalizedToken(pane.lastEventType) ?? ""
            let lastEventAt = pane.lastEventAt ?? ""
            let awaitingKind = awaitingResponseKind(for: pane) ?? ""
            if let prev = paneObservations[pane.id] {
                let nowActionable = isActionableAttentionState(currentAttentionState)
                let prevActionable = isActionableAttentionState(prev.attentionState)
                if nowActionable && !prevActionable {
                    switch awaitingKind {
                    case "input":
                        enqueueReview(kind: .needsInput, pane: pane, now: now)
                    case "approval":
                        enqueueReview(kind: .needsApproval, pane: pane, now: now)
                    default:
                        enqueueReview(kind: .error, pane: pane, now: now)
                    }
                }
                if currentAttentionState == "informational_completed" &&
                    (prev.lastEventType != lastEventType || prev.lastEventAt != lastEventAt) {
                    enqueueInformational(kind: .taskCompleted, pane: pane, now: now)
                }
            }
            next[pane.id] = PaneObservation(
                state: state,
                category: category,
                attentionState: currentAttentionState,
                lastEventType: lastEventType,
                lastEventAt: lastEventAt,
                awaitingKind: awaitingKind
            )
        }
        paneObservations = next
        queueLastEmitByKey = queueLastEmitByKey.filter { now.timeIntervalSince($0.value) < queueDedupeWindowSeconds * 4 }
        if reviewQueue.count > queueLimit {
            reviewQueue.removeLast(reviewQueue.count - queueLimit)
        }
        if informationalQueue.count > queueLimit {
            informationalQueue.removeLast(informationalQueue.count - queueLimit)
        }
    }

    private func enqueueReview(kind: ReviewKind, pane: PaneItem, now: Date) {
        let key = "\(pane.id)|\(kind.rawValue)"
        if let emittedAt = queueLastEmitByKey[key], now.timeIntervalSince(emittedAt) < queueDedupeWindowSeconds {
            return
        }
        if let existing = reviewQueue.firstIndex(where: {
            $0.kind == kind &&
                $0.target == pane.identity.target &&
                $0.sessionName == pane.identity.sessionName &&
                $0.paneID == pane.identity.paneID &&
                $0.acknowledgedAt == nil
        }) {
            reviewQueue[existing].unread = true
            queueLastEmitByKey[key] = now
            return
        }

        let summary: String
        switch kind {
        case .taskCompleted:
            summary = "Task completed in pane \(pane.identity.paneID)"
        case .needsInput:
            summary = "User input required in pane \(pane.identity.paneID)"
        case .needsApproval:
            summary = "Approval required in pane \(pane.identity.paneID)"
        case .error:
            summary = "Runtime error detected in pane \(pane.identity.paneID)"
        }

        let item = ReviewQueueItem(
            id: UUID().uuidString,
            kind: kind,
            target: pane.identity.target,
            sessionName: pane.identity.sessionName,
            paneID: pane.identity.paneID,
            windowID: pane.identity.windowID,
            runtimeID: pane.runtimeID,
            createdAt: now,
            summary: summary,
            unread: true,
            acknowledgedAt: nil
        )
        reviewQueue.insert(item, at: 0)
        queueLastEmitByKey[key] = now
    }

    private func enqueueInformational(kind: ReviewKind, pane: PaneItem, now: Date) {
        let key = "\(pane.id)|info|\(kind.rawValue)"
        if let emittedAt = queueLastEmitByKey[key], now.timeIntervalSince(emittedAt) < queueDedupeWindowSeconds {
            return
        }
        if let existing = informationalQueue.firstIndex(where: {
            $0.kind == kind &&
                $0.target == pane.identity.target &&
                $0.sessionName == pane.identity.sessionName &&
                $0.paneID == pane.identity.paneID &&
                $0.acknowledgedAt == nil
        }) {
            informationalQueue[existing].unread = true
            queueLastEmitByKey[key] = now
            return
        }
        let summary: String
        switch kind {
        case .taskCompleted:
            summary = "Task completed in pane \(pane.identity.paneID)"
        case .needsInput:
            summary = "Input checkpoint in pane \(pane.identity.paneID)"
        case .needsApproval:
            summary = "Approval checkpoint in pane \(pane.identity.paneID)"
        case .error:
            summary = "Runtime update in pane \(pane.identity.paneID)"
        }
        let item = ReviewQueueItem(
            id: UUID().uuidString,
            kind: kind,
            target: pane.identity.target,
            sessionName: pane.identity.sessionName,
            paneID: pane.identity.paneID,
            windowID: pane.identity.windowID,
            runtimeID: pane.runtimeID,
            createdAt: now,
            summary: summary,
            unread: true,
            acknowledgedAt: nil
        )
        informationalQueue.insert(item, at: 0)
        queueLastEmitByKey[key] = now
    }

    private func buildWindowSections(_ panes: [PaneItem], key: String) -> [WindowSection] {
        let grouped = Dictionary(grouping: panes, by: { $0.identity.windowID })
        var out: [WindowSection] = []
        for (windowID, paneList) in grouped {
            let sorted = sortedPanes(paneList)
            let counts = countByCategory(in: sorted)
            out.append(WindowSection(
                id: "\(key)|\(windowID)",
                windowID: windowID,
                topCategory: topCategory(from: counts),
                byCategory: counts,
                panes: sorted
            ))
        }
        out.sort { lhs, rhs in
            let lp = categoryPrecedence(lhs.topCategory)
            let rp = categoryPrecedence(rhs.topCategory)
            if lp != rp {
                return lp < rp
            }
            return lhs.windowID < rhs.windowID
        }
        return out
    }

    private func shouldGroupByWindow(_ panes: [PaneItem]) -> Bool {
        switch windowGrouping {
        case .off:
            return false
        case .on:
            return true
        case .auto:
            let windowCount = Set(panes.map { $0.identity.windowID }).count
            return panes.count >= 4 && windowCount > 1
        }
    }

    private func sortedPanes(_ panes: [PaneItem]) -> [PaneItem] {
        panes.sorted { lhs, rhs in
            let lw = sortableNumericSuffix(lhs.identity.windowID)
            let rw = sortableNumericSuffix(rhs.identity.windowID)
            if lw != rw {
                return lw < rw
            }
            if lhs.identity.windowID != rhs.identity.windowID {
                return lhs.identity.windowID < rhs.identity.windowID
            }
            let lp = sortableNumericSuffix(lhs.identity.paneID)
            let rp = sortableNumericSuffix(rhs.identity.paneID)
            if lp != rp {
                return lp < rp
            }
            return lhs.identity.paneID < rhs.identity.paneID
        }
    }

    private func sortableNumericSuffix(_ raw: String) -> Int {
        let digits = raw.filter(\.isNumber)
        return Int(digits) ?? .max
    }

    private func countByCategory(in panes: [PaneItem]) -> [String: Int] {
        var out: [String: Int] = [
            "attention": 0,
            "running": 0,
            "idle": 0,
            "unmanaged": 0,
            "unknown": 0,
        ]
        for pane in panes {
            out[displayCategory(for: pane), default: 0] += 1
        }
        return out
    }

    private func topCategory(from counts: [String: Int]) -> String {
        var best = "unknown"
        for (category, count) in counts where count > 0 {
            if categoryPrecedence(category) < categoryPrecedence(best) {
                best = category
            }
        }
        return best
    }

    private func paneSessionKey(target: String, sessionName: String) -> String {
        "\(target)|\(sessionName)"
    }

    private func hiddenStatusCategories() -> Set<String> {
        var hidden: Set<String> = []
        if hideUnmanagedCategory {
            hidden.insert("unmanaged")
        }
        if !showUnknownCategory {
            hidden.insert("unknown")
        }
        return hidden
    }

    private func loadPreferences() {
        if let raw = defaults.string(forKey: PreferenceKey.viewMode), let restored = ViewMode(rawValue: raw) {
            viewMode = restored
        } else {
            viewMode = .bySession
            defaults.set(ViewMode.bySession.rawValue, forKey: PreferenceKey.viewMode)
        }
        if let raw = defaults.string(forKey: PreferenceKey.statusFilter) {
            switch raw {
            case StatusFilter.all.rawValue:
                statusFilter = .all
            case StatusFilter.managed.rawValue, "running", "idle":
                statusFilter = .managed
            case StatusFilter.attention.rawValue:
                statusFilter = .attention
            case StatusFilter.pinned.rawValue:
                statusFilter = .pinned
            case "unmanaged", "unknown":
                statusFilter = .all
            default:
                statusFilter = .all
            }
        } else {
            statusFilter = .all
            defaults.set(StatusFilter.all.rawValue, forKey: PreferenceKey.statusFilter)
        }
        if let raw = defaults.string(forKey: PreferenceKey.sessionSortMode), let restored = SessionSortMode(rawValue: raw) {
            sessionSortMode = restored
        } else {
            sessionSortMode = .stable
            defaults.set(SessionSortMode.stable.rawValue, forKey: PreferenceKey.sessionSortMode)
        }
        if let raw = defaults.string(forKey: PreferenceKey.windowGrouping), let restored = WindowGrouping(rawValue: raw) {
            windowGrouping = restored
        }
        interactiveTerminalInputEnabled = readBoolPreference(PreferenceKey.interactiveTerminalInputEnabled, fallback: true)
        showWindowMetadata = readBoolPreference(PreferenceKey.showWindowMetadata, fallback: false)
        showWindowGroupBackground = readBoolPreference(PreferenceKey.showWindowGroupBackground, fallback: true)
        showPinnedOnly = readBoolPreference(PreferenceKey.showPinnedOnly, fallback: false)
        showSessionMetadataInStatusView = readBoolPreference(PreferenceKey.showSessionMetadataInStatusView, fallback: false)
        showEmptyStatusColumns = readBoolPreference(PreferenceKey.showEmptyStatusColumns, fallback: false)
        showTechnicalDetails = readBoolPreference(PreferenceKey.showTechnicalDetails, fallback: false)
        hideUnmanagedCategory = readBoolPreference(PreferenceKey.hideUnmanagedCategory, fallback: false)
        showUnknownCategory = readBoolPreference(PreferenceKey.showUnknownCategory, fallback: false)
        reviewUnreadOnly = readBoolPreference(PreferenceKey.reviewUnreadOnly, fallback: true)
        restoreSessionStableOrder()
        restorePinnedSessions()
        restorePaneDisplayNameOverrides()

        let storedVersion = defaults.integer(forKey: PreferenceKey.uiPrefsVersion)
        if storedVersion < currentUIPrefsVersion {
            // v10: keep tmux-first navigation defaults and reset filter/sort surface.
            viewMode = .bySession
            defaults.set(ViewMode.bySession.rawValue, forKey: PreferenceKey.viewMode)
            statusFilter = .all
            defaults.set(StatusFilter.all.rawValue, forKey: PreferenceKey.statusFilter)
            showWindowMetadata = false
            showSessionMetadataInStatusView = false
            showWindowGroupBackground = true
            sessionSortMode = .stable
            showPinnedOnly = false
            defaults.set(currentUIPrefsVersion, forKey: PreferenceKey.uiPrefsVersion)
        }
    }

    private func updateSessionStableOrder(with panes: [PaneItem]) {
        let keys = Set(panes.map { paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName) })
        if keys.isEmpty {
            return
        }
        let missing = keys.filter { sessionStableOrder[$0] == nil }.sorted()
        var changed = false
        for key in missing {
            sessionStableOrder[key] = nextSessionStableOrder
            nextSessionStableOrder += 1
            changed = true
        }
        if changed {
            persistSessionStableOrder()
        }
    }

    private func applySessionStableOrder(_ preferredOrder: [String]) {
        var next: [String: Int] = [:]
        var index = 0
        var seen: Set<String> = []
        for key in preferredOrder {
            let token = key.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !token.isEmpty, !seen.contains(token) else {
                continue
            }
            seen.insert(token)
            next[token] = index
            index += 1
        }

        let remaining = sessionStableOrder.keys
            .filter { !seen.contains($0) }
            .sorted { lhs, rhs in
                let li = sessionStableOrder[lhs] ?? .max
                let ri = sessionStableOrder[rhs] ?? .max
                if li != ri {
                    return li < ri
                }
                return lhs < rhs
            }
        for key in remaining {
            next[key] = index
            index += 1
        }
        sessionStableOrder = next
        nextSessionStableOrder = index
        persistSessionStableOrder()
    }

    private func stableSessionOrder(for key: String) -> Int {
        if let index = sessionStableOrder[key] {
            return index
        }
        let next = nextSessionStableOrder
        sessionStableOrder[key] = next
        nextSessionStableOrder += 1
        persistSessionStableOrder()
        return next
    }

    private func restoreSessionStableOrder() {
        if let data = defaults.data(forKey: PreferenceKey.sessionStableOrder),
           let restored = try? JSONDecoder().decode([String: Int].self, from: data) {
            sessionStableOrder = restored
        } else {
            sessionStableOrder = [:]
        }
        let restoredNext = defaults.integer(forKey: PreferenceKey.sessionStableOrderNext)
        if restoredNext > 0 {
            nextSessionStableOrder = restoredNext
            return
        }
        nextSessionStableOrder = (sessionStableOrder.values.max() ?? -1) + 1
    }

    private func restorePinnedSessions() {
        if let values = defaults.array(forKey: PreferenceKey.pinnedSessions) as? [String] {
            pinnedSessionKeys = Set(
                values
                    .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
            )
        } else {
            pinnedSessionKeys = []
        }
    }

    private func restorePaneDisplayNameOverrides() {
        if let raw = defaults.dictionary(forKey: PreferenceKey.paneDisplayNameOverrides) as? [String: String] {
            paneDisplayNameOverridesByID = raw.reduce(into: [String: String]()) { out, entry in
                let key = entry.key.trimmingCharacters(in: .whitespacesAndNewlines)
                let value = entry.value.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !key.isEmpty, !value.isEmpty else {
                    return
                }
                out[key] = value
            }
        } else {
            paneDisplayNameOverridesByID = [:]
        }
    }

    private func persistSessionStableOrder() {
        if let data = try? JSONEncoder().encode(sessionStableOrder) {
            defaults.set(data, forKey: PreferenceKey.sessionStableOrder)
        }
        defaults.set(nextSessionStableOrder, forKey: PreferenceKey.sessionStableOrderNext)
    }

    private func persistPinnedSessions() {
        let values = Array(pinnedSessionKeys).sorted()
        defaults.set(values, forKey: PreferenceKey.pinnedSessions)
    }

    private func persistPaneDisplayNameOverrides() {
        defaults.set(paneDisplayNameOverridesByID, forKey: PreferenceKey.paneDisplayNameOverrides)
    }

    private func readBoolPreference(_ key: String, fallback: Bool) -> Bool {
        guard defaults.object(forKey: key) != nil else {
            return fallback
        }
        return defaults.bool(forKey: key)
    }

    private func appendLatencySample(_ value: Double, into samples: inout [Double]) {
        guard value.isFinite else {
            return
        }
        samples.append(value)
        if samples.count > telemetryMaxSamples {
            samples.removeFirst(samples.count - telemetryMaxSamples)
        }
    }

    private func trimFrameRenderWindow(now: Date) {
        let threshold = now.addingTimeInterval(-telemetryFPSWindowSeconds)
        frameRenderTimestamps = frameRenderTimestamps.filter { $0 >= threshold }
        let maxFrames = max(4, Int(telemetryFPSWindowSeconds * 120))
        if frameRenderTimestamps.count > maxFrames {
            frameRenderTimestamps.removeFirst(frameRenderTimestamps.count - maxFrames)
        }
    }

    private func percentile(_ p: Double, from samples: [Double]) -> Double? {
        guard !samples.isEmpty else {
            return nil
        }
        let sorted = samples.sorted()
        let rank = min(max(p, 0), 1) * Double(sorted.count - 1)
        let low = Int(floor(rank))
        let high = Int(ceil(rank))
        if low == high {
            return sorted[low]
        }
        let weight = rank - Double(low)
        return sorted[low] * (1 - weight) + sorted[high] * weight
    }

    private func recomputeTerminalPerformanceSnapshot(now: Date) {
        trimFrameRenderWindow(now: now)
        let fps: Double
        if frameRenderTimestamps.count >= 2,
           let first = frameRenderTimestamps.first,
           let last = frameRenderTimestamps.last {
            let duration = max(last.timeIntervalSince(first), 0.001)
            fps = Double(frameRenderTimestamps.count - 1) / duration
        } else {
            fps = 0
        }
        terminalPerformance = TerminalPerformanceSnapshot(
            renderFPS: fps,
            inputLatencyP50Ms: percentile(0.5, from: inputLatencySamplesMs),
            streamRTTP50Ms: percentile(0.5, from: streamLatencySamplesMs),
            inputSampleCount: inputLatencySamplesMs.count,
            streamSampleCount: streamLatencySamplesMs.count
        )
    }

    private func shouldPerformImmediateWriteRefresh(for paneID: String, generation: Int) -> Bool {
        guard generation == terminalStreamGeneration else {
            return false
        }
        guard selectedPane?.id == paneID else {
            return false
        }
        if autoStreamOnSelection {
            return terminalStreamTask == nil
        }
        return true
    }

    private func currentTerminalStreamLineBudget(for _: String) -> Int {
        let baseRows = max(0, terminalPaneRows ?? 0)
        let baseline = baseRows > 0 ? baseRows + 200 : terminalStreamDefaultLines
        return min(max(baseline, terminalStreamMinLines), terminalStreamMaxLines)
    }

    private func markInteractiveInput(for paneID: String, now: Date = Date()) {
        lastInteractiveInputAtByPaneID[paneID] = now
        noteTerminalInputDispatched(for: paneID, at: now)
    }

    private func currentTerminalStreamPollIntervalMillis(for paneID: String, now: Date = Date()) -> Int {
        if let lastInput = lastInteractiveInputAtByPaneID[paneID],
           now.timeIntervalSince(lastInput) <= terminalStreamFastWindowSeconds {
            return terminalStreamPollFastMillis
        }
        if let pane = panes.first(where: { $0.id == paneID }) {
            let category = displayCategory(for: pane)
            if category == "running" || category == "attention" {
                return terminalStreamPollNormalMillis
            }
        }
        return terminalStreamPollIdleMillis
    }

    private func restartTerminalStreamForSelectedPane() {
        terminalStreamTask?.cancel()
        guard let pane = selectedPane else {
            return
        }
        terminalStreamGeneration += 1
        let generation = terminalStreamGeneration
        let paneID = pane.id
        terminalStreamTask = Task { [weak self] in
            await self?.terminalStreamLoop(for: paneID, generation: generation)
        }
    }

    private func terminalStreamLoop(for paneID: String, generation: Int) async {
        while !Task.isCancelled {
            guard generation == terminalStreamGeneration else {
                return
            }
            guard selectedPane?.id == paneID else {
                return
            }
            guard let pane = panes.first(where: { $0.id == paneID }) else {
                return
            }
            do {
                if await shouldUseTerminalProxy(for: paneID) {
                    let sessionID = try await ensureTerminalProxySession(for: pane, generation: generation)
                    let cursor = await terminalSessionController.cursor(for: paneID)
                    let lines = currentTerminalStreamLineBudget(for: paneID)
                    let firstStreamStartedAt = Date()
                    let firstResp = try await client.terminalStream(
                        sessionID: sessionID,
                        cursor: cursor,
                        lines: lines
                    )
                    noteTerminalStreamRoundTrip(startedAt: firstStreamStartedAt)
                    var frame = firstResp.frame
                    if frame.frameType == "attached" {
                        let followStreamStartedAt = Date()
                        let followResp = try await client.terminalStream(
                            sessionID: sessionID,
                            cursor: frame.cursor,
                            lines: lines
                        )
                        noteTerminalStreamRoundTrip(startedAt: followStreamStartedAt)
                        frame = followResp.frame
                    }
                    guard generation == terminalStreamGeneration, selectedPane?.id == paneID else {
                        return
                    }
                    await terminalSessionController.setCursor(frame.cursor, for: paneID)
                    applyTerminalStreamFrame(frame, paneID: paneID)
                    await terminalSessionController.recordSuccess(for: paneID)
                } else {
                    let cursor = await terminalSessionController.cursor(for: paneID)
                    let readStartedAt = Date()
                    let resp = try await client.terminalRead(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        cursor: cursor,
                        lines: currentTerminalStreamLineBudget(for: paneID)
                    )
                    noteTerminalStreamRoundTrip(startedAt: readStartedAt)
                    guard generation == terminalStreamGeneration, selectedPane?.id == paneID else {
                        return
                    }
                    await terminalSessionController.setCursor(resp.frame.cursor, for: paneID)
                    applyTerminalFrame(resp.frame, paneID: paneID)
                    await terminalSessionController.recordSuccess(for: paneID)
                }
                let waitMillis = currentTerminalStreamPollIntervalMillis(for: paneID)
                try await Task.sleep(for: .milliseconds(waitMillis))
            } catch {
                if error is CancellationError || Task.isCancelled || generation != terminalStreamGeneration {
                    return
                }
                if shouldResetTerminalProxySession(error: error),
                   let staleSessionID = await terminalSessionController.clearProxySession(for: paneID) {
                    await terminalSessionController.clearCursor(for: paneID)
                    await detachTerminalProxySession(sessionID: staleSessionID)
                }
                let outcome = await terminalSessionController.recordFailure(for: paneID)
                if selectedPane?.id == paneID {
                    if outcome.didEnterDegradedMode {
                        infoMessage = "interactive terminal degraded to snapshot mode"
                    } else {
                        infoMessage = "terminal stream reconnecting..."
                    }
                }
                try? await Task.sleep(for: .milliseconds(outcome.delayMillis))
            }
        }
    }

    private func applyTerminalFrame(_ frame: TerminalFrame, paneID: String) {
        guard selectedPane?.id == paneID else {
            return
        }
        noteTerminalFrameApplied(for: paneID)
        updateTerminalRenderMetadata(
            cursorX: frame.cursorX,
            cursorY: frame.cursorY,
            paneCols: frame.paneCols,
            paneRows: frame.paneRows,
            clearCursorIfMissing: true
        )
        let content = frame.content ?? ""
        if frame.frameType == "delta" {
            if !content.isEmpty {
                setOutputPreviewIfChanged(outputPreview + content)
            }
        } else {
            setOutputPreviewIfChanged(content)
        }
        trimOutputPreviewIfNeeded()
        updateTerminalRenderCache(for: paneID)
    }

    private func applyTerminalStreamFrame(_ frame: TerminalStreamFrame, paneID: String) {
        guard selectedPane?.id == paneID else {
            return
        }
        noteTerminalFrameApplied(for: paneID)
        updateTerminalRenderMetadata(
            cursorX: frame.cursorX,
            cursorY: frame.cursorY,
            paneCols: frame.paneCols,
            paneRows: frame.paneRows,
            clearCursorIfMissing: frame.frameType != "attached"
        )
        switch frame.frameType {
        case "attached":
            return
        case "output":
            setOutputPreviewIfChanged(frame.content ?? "")
        case "delta":
            if let delta = frame.content, !delta.isEmpty {
                setOutputPreviewIfChanged(outputPreview + delta)
            }
        case "reset":
            setOutputPreviewIfChanged(frame.content ?? "")
        case "error":
            let reason = frame.errorCode ?? frame.message ?? "unknown"
            errorMessage = "terminal-stream error: \(reason)"
            Task { [weak self] in
                guard let self else {
                    return
                }
                if let sessionID = await self.terminalSessionController.clearProxySession(for: paneID) {
                    await self.terminalSessionController.clearCursor(for: paneID)
                    await self.detachTerminalProxySession(sessionID: sessionID)
                }
            }
            return
        default:
            return
        }
        trimOutputPreviewIfNeeded()
        updateTerminalRenderCache(for: paneID)
    }

    private func updateTerminalRenderCache(for paneID: String) {
        terminalRenderCacheByPaneID[paneID] = TerminalRenderCache(
            output: outputPreview,
            cursorX: terminalCursorX,
            cursorY: terminalCursorY,
            paneCols: terminalPaneCols,
            paneRows: terminalPaneRows
        )
    }

    private func applyCachedTerminalRender(for paneID: String) -> Bool {
        guard let cached = terminalRenderCacheByPaneID[paneID] else {
            return false
        }
        setOutputPreviewIfChanged(cached.output)
        setTerminalCursorXIfChanged(cached.cursorX)
        setTerminalCursorYIfChanged(cached.cursorY)
        setTerminalPaneColsIfChanged(cached.paneCols)
        setTerminalPaneRowsIfChanged(cached.paneRows)
        return true
    }

    private func trimOutputPreviewIfNeeded() {
        if outputPreview.count <= terminalOutputMaxChars {
            return
        }
        setOutputPreviewIfChanged(String(outputPreview.suffix(terminalOutputMaxChars)))
    }

    private func updateTerminalRenderMetadata(
        cursorX: Int?,
        cursorY: Int?,
        paneCols: Int?,
        paneRows: Int?,
        clearCursorIfMissing: Bool = false
    ) {
        if let paneCols, paneCols > 0 {
            setTerminalPaneColsIfChanged(paneCols)
        }
        if let paneRows, paneRows > 0 {
            setTerminalPaneRowsIfChanged(paneRows)
        }
        if clearCursorIfMissing, (cursorX == nil || cursorY == nil) {
            setTerminalCursorXIfChanged(nil)
            setTerminalCursorYIfChanged(nil)
            return
        }
        guard let cursorX, let cursorY, cursorX >= 0, cursorY >= 0 else {
            return
        }
        setTerminalCursorXIfChanged(cursorX)
        setTerminalCursorYIfChanged(cursorY)
    }

    private func setOutputPreviewIfChanged(_ next: String) {
        if outputPreview != next {
            outputPreview = next
        }
    }

    private func setTerminalCursorXIfChanged(_ next: Int?) {
        if terminalCursorX != next {
            terminalCursorX = next
        }
    }

    private func setTerminalCursorYIfChanged(_ next: Int?) {
        if terminalCursorY != next {
            terminalCursorY = next
        }
    }

    private func setTerminalPaneColsIfChanged(_ next: Int?) {
        if terminalPaneCols != next {
            terminalPaneCols = next
        }
    }

    private func setTerminalPaneRowsIfChanged(_ next: Int?) {
        if terminalPaneRows != next {
            terminalPaneRows = next
        }
    }

    private func ensureTerminalProxySession(for pane: PaneItem, generation: Int? = nil) async throws -> String {
        if let sessionID = await terminalSessionController.proxySession(for: pane.id), !sessionID.isEmpty {
            return sessionID
        }
        let guardOptions = writeGuardOptions(for: pane)
        let response = try await client.terminalAttach(
            target: pane.identity.target,
            paneID: pane.identity.paneID,
            ifRuntime: guardOptions.ifRuntime,
            ifState: guardOptions.ifState,
            ifUpdatedWithin: guardOptions.ifUpdatedWithin,
            forceStale: guardOptions.forceStale
        )
        let sessionID = response.sessionID.trimmingCharacters(in: .whitespacesAndNewlines)
        if !shouldAcceptTerminalAttachResponse(response) {
            throw RuntimeError.commandFailed(
                "agtmux-app terminal attach",
                1,
                "terminal attach failed: \(response.resultCode)"
            )
        }
        if let generation {
            guard generation == terminalStreamGeneration, selectedPane?.id == pane.id else {
                await detachTerminalProxySession(sessionID: sessionID)
                throw CancellationError()
            }
        }
        await terminalSessionController.setProxySession(sessionID, for: pane.id)
        return sessionID
    }

    private func detachTerminalProxySession(sessionID: String) async {
        guard !sessionID.isEmpty else {
            return
        }
        _ = try? await client.terminalDetach(sessionID: sessionID)
    }

    private func shouldUseTerminalProxy(for paneID: String) async -> Bool {
        guard await terminalSessionController.shouldUseProxy(for: paneID) else {
            return false
        }
        guard let caps = await fetchTerminalCapabilities() else {
            // Capabilities fetch can transiently fail during daemon restarts.
            // Keep embedded terminal usable by optimistically attempting proxy.
            return true
        }
        return shouldUseTerminalProxy(caps: caps)
    }

    func shouldUseTerminalProxy(caps: CapabilityFlags) -> Bool {
        if !(caps.embeddedTerminal &&
            caps.terminalAttach &&
            caps.terminalWrite &&
            caps.terminalStream) {
            return false
        }
        guard normalizedToken(caps.terminalProxyMode) == "daemon-proxy-pty-poc" else {
            return false
        }
        guard normalizedToken(caps.terminalFrameProtocol) == "terminal-stream-v1" else {
            return false
        }
        return true
    }

    private struct WriteGuardOptions {
        let ifRuntime: String?
        let ifState: String?
        let ifUpdatedWithin: String?
        let forceStale: Bool
    }

    private func writeGuardOptions(for pane: PaneItem) -> WriteGuardOptions {
        let runtime = trimmedNonEmpty(pane.runtimeID)
        let normalized = normalizedState(pane.state)
        let state = normalized == "unknown" ? nil : normalized
        return WriteGuardOptions(
            ifRuntime: runtime,
            ifState: state,
            ifUpdatedWithin: nil,
            forceStale: false
        )
    }

    private func shouldUseTerminalRead() async -> Bool {
        guard let caps = await fetchTerminalCapabilities() else {
            // Optimistic fallback keeps terminal-read path alive when
            // capabilities endpoint is temporarily unavailable.
            return true
        }
        return caps.embeddedTerminal && caps.terminalRead
    }

    private func fetchTerminalCapabilities() async -> CapabilityFlags? {
        let now = Date()
        if let cached = terminalCapabilities,
           let fetchedAt = terminalCapabilitiesFetchedAt,
           now.timeIntervalSince(fetchedAt) <= terminalCapabilitiesCacheTTLSeconds {
            return cached
        }
        do {
            let env = try await client.fetchCapabilities()
            terminalCapabilities = env.capabilities
            terminalCapabilitiesFetchedAt = now
            return env.capabilities
        } catch {
            terminalCapabilities = nil
            terminalCapabilitiesFetchedAt = now
            return nil
        }
    }

    func shouldResetTerminalProxySession(error: Error) -> Bool {
        guard case let RuntimeError.commandFailed(_, _, stderr) = error else {
            return false
        }
        let normalized = stderr.lowercased()
        let resetTokens = ["e_ref_not_found", "session not found", "e_runtime_stale"]
        return resetTokens.contains { normalized.contains($0) }
    }

    private func isStaleGuardConflict(_ error: Error) -> Bool {
        guard case let RuntimeError.commandFailed(_, _, stderr) = error else {
            return false
        }
        let normalized = stderr.lowercased()
        return normalized.contains("e_runtime_stale") || normalized.contains("runtime stale")
    }

    func shouldAcceptTerminalAttachResponse(_ response: TerminalAttachResponse) -> Bool {
        if response.resultCode != "completed" {
            return false
        }
        return !response.sessionID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func categoryPrecedence(_ category: String) -> Int {
        switch category {
        case "attention":
            return 1
        case "running":
            return 2
        case "idle":
            return 3
        case "unmanaged":
            return 4
        case "unknown":
            return 5
        default:
            return 999
        }
    }

    private func agentPresence(for pane: PaneItem) -> String {
        if let presence = normalizedToken(pane.agentPresence) {
            return presence
        }
        guard let agent = normalizedToken(pane.agentType) else {
            return "unknown"
        }
        if agent == "none" {
            return "none"
        }
        if agent == "unknown" {
            return "unknown"
        }
        return "managed"
    }

    private func normalizedState(_ state: String) -> String {
        normalizedToken(state) ?? "unknown"
    }

    private func normalizedToken(_ value: String?) -> String? {
        guard let raw = value?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased(), !raw.isEmpty else {
            return nil
        }
        return raw
    }

    private func trimmedNonEmpty(_ value: String?) -> String? {
        guard let raw = value?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty else {
            return nil
        }
        return raw
    }

    private struct ExternalTerminalInvocation {
        let executable: String
        let args: [String]
    }

    private func buildExternalTerminalInvocation(for pane: PaneItem) throws -> ExternalTerminalInvocation {
        let sessionName = pane.identity.sessionName.trimmingCharacters(in: .whitespacesAndNewlines)
        let windowID = pane.identity.windowID.trimmingCharacters(in: .whitespacesAndNewlines)
        let paneID = pane.identity.paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        if sessionName.isEmpty || windowID.isEmpty || paneID.isEmpty {
            throw RuntimeError.commandFailed("external terminal", 1, "pane identity is incomplete")
        }

        let targetToken = pane.identity.target.trimmingCharacters(in: .whitespacesAndNewlines)
        let resolvedTarget = targetRecord(for: targetToken)
        let targetKind: String
        if let resolvedTarget {
            switch normalizedToken(resolvedTarget.kind) {
            case "local":
                targetKind = "local"
            case "ssh":
                targetKind = "ssh"
            case nil:
                throw RuntimeError.commandFailed("external terminal", 1, "target kind is unavailable")
            default:
                throw RuntimeError.commandFailed("external terminal", 1, "unsupported target kind")
            }
        } else if normalizedToken(targetToken) == "local" || targetToken.isEmpty {
            targetKind = "local"
        } else {
            throw RuntimeError.commandFailed("external terminal", 1, "target is unavailable")
        }

        let tmuxJump = [
            "tmux select-window -t \(shellQuote(windowID))",
            "tmux select-pane -t \(shellQuote(paneID))",
            "tmux attach-session -t \(shellQuote(sessionName))",
        ].joined(separator: " && ")

        var command = tmuxJump
        if targetKind == "ssh" {
            let connectionRef = normalizedSSHConnectionRef(resolvedTarget?.connectionRef)
            guard let connectionRef else {
                throw RuntimeError.commandFailed(
                    "external terminal",
                    1,
                    "ssh target connection_ref is unavailable"
                )
            }
            command = "ssh -t \(shellQuote(connectionRef)) \(shellQuote(tmuxJump))"
        }

        let escapedCommand = escapeForAppleScriptLiteral(command)
        return ExternalTerminalInvocation(
            executable: "/usr/bin/osascript",
            args: [
                "-e", "tell application \"Terminal\" to activate",
                "-e", "tell application \"Terminal\" to do script \"\(escapedCommand)\"",
            ]
        )
    }

    private func buildSessionKillInvocation(target targetToken: String, sessionName: String) throws -> ExternalTerminalInvocation {
        let target = targetToken.trimmingCharacters(in: .whitespacesAndNewlines)
        let session = sessionName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !target.isEmpty, !session.isEmpty else {
            throw RuntimeError.commandFailed("kill session", 1, "session identity is incomplete")
        }
        let command = "tmux kill-session -t \(shellQuote(session))"
        return try buildTmuxInvocation(target: target, operation: "kill session", command: command)
    }

    private func buildTmuxInvocation(target targetToken: String, operation: String, command: String) throws -> ExternalTerminalInvocation {
        let target = targetToken.trimmingCharacters(in: .whitespacesAndNewlines)
        let commandToken = command.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !target.isEmpty, !commandToken.isEmpty else {
            throw RuntimeError.commandFailed(operation, 1, "target/command is incomplete")
        }

        let resolvedTarget = targetRecord(for: target)
        let targetKind: String
        if let resolvedTarget {
            switch normalizedToken(resolvedTarget.kind) {
            case "local":
                targetKind = "local"
            case "ssh":
                targetKind = "ssh"
            case nil:
                throw RuntimeError.commandFailed(operation, 1, "target kind is unavailable")
            default:
                throw RuntimeError.commandFailed(operation, 1, "unsupported target kind")
            }
        } else if normalizedToken(target) == "local" {
            targetKind = "local"
        } else {
            throw RuntimeError.commandFailed(operation, 1, "target is unavailable")
        }

        if targetKind == "local" {
            return ExternalTerminalInvocation(
                executable: "/bin/zsh",
                args: [
                    "-lc",
                    commandToken,
                ]
            )
        }

        let connectionRef = normalizedSSHConnectionRef(resolvedTarget?.connectionRef)
        guard let connectionRef else {
            throw RuntimeError.commandFailed(operation, 1, "ssh target connection_ref is unavailable")
        }
        return ExternalTerminalInvocation(
            executable: "/bin/zsh",
            args: [
                "-lc",
                "ssh \(shellQuote(connectionRef)) \(shellQuote(commandToken))",
            ]
        )
    }

    private func setPaneDisplayNameOverride(name: String, paneID: String) {
        let key = paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        let value = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty, !value.isEmpty else {
            return
        }
        paneDisplayNameOverridesByID[key] = value
        persistPaneDisplayNameOverrides()
    }

    private func renameSessionInLocalState(target: String, from: String, to: String) {
        let targetToken = target.trimmingCharacters(in: .whitespacesAndNewlines)
        let fromToken = from.trimmingCharacters(in: .whitespacesAndNewlines)
        let toToken = to.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !targetToken.isEmpty, !fromToken.isEmpty, !toToken.isEmpty else {
            return
        }
        panes = panes.map { pane in
            guard pane.identity.target == targetToken, pane.identity.sessionName == fromToken else {
                return pane
            }
            let identity = PaneIdentity(
                target: pane.identity.target,
                sessionName: toToken,
                windowID: pane.identity.windowID,
                paneID: pane.identity.paneID
            )
            return PaneItem(
                identity: identity,
                windowName: pane.windowName,
                currentCmd: pane.currentCmd,
                paneTitle: pane.paneTitle,
                state: pane.state,
                reasonCode: pane.reasonCode,
                confidence: pane.confidence,
                runtimeID: pane.runtimeID,
                agentType: pane.agentType,
                agentPresence: pane.agentPresence,
                activityState: pane.activityState,
                displayCategory: pane.displayCategory,
                needsUserAction: pane.needsUserAction,
                stateSource: pane.stateSource,
                lastEventType: pane.lastEventType,
                lastEventAt: pane.lastEventAt,
                awaitingResponseKind: pane.awaitingResponseKind,
                sessionLabel: pane.sessionLabel,
                sessionLabelSource: pane.sessionLabelSource,
                lastInteractionAt: pane.lastInteractionAt,
                updatedAt: pane.updatedAt
            )
        }
        windows = windows.map { window in
            guard window.identity.target == targetToken, window.identity.sessionName == fromToken else {
                return window
            }
            let identity = WindowIdentity(
                target: window.identity.target,
                sessionName: toToken,
                windowID: window.identity.windowID
            )
            return WindowItem(
                identity: identity,
                topState: window.topState,
                topCategory: window.topCategory,
                byCategory: window.byCategory,
                waitingCount: window.waitingCount,
                runningCount: window.runningCount,
                totalPanes: window.totalPanes
            )
        }
        sessions = sessions.map { session in
            guard session.identity.target == targetToken, session.identity.sessionName == fromToken else {
                return session
            }
            let identity = SessionIdentity(target: session.identity.target, sessionName: toToken)
            return SessionItem(
                identity: identity,
                topCategory: session.topCategory,
                totalPanes: session.totalPanes,
                byState: session.byState,
                byAgent: session.byAgent,
                byCategory: session.byCategory
            )
        }
        reviewQueue = reviewQueue.map { item in
            guard item.target == targetToken, item.sessionName == fromToken else {
                return item
            }
            return ReviewQueueItem(
                id: item.id,
                kind: item.kind,
                target: item.target,
                sessionName: toToken,
                paneID: item.paneID,
                windowID: item.windowID,
                runtimeID: item.runtimeID,
                createdAt: item.createdAt,
                summary: item.summary,
                unread: item.unread,
                acknowledgedAt: item.acknowledgedAt
            )
        }
        informationalQueue = informationalQueue.map { item in
            guard item.target == targetToken, item.sessionName == fromToken else {
                return item
            }
            return ReviewQueueItem(
                id: item.id,
                kind: item.kind,
                target: item.target,
                sessionName: toToken,
                paneID: item.paneID,
                windowID: item.windowID,
                runtimeID: item.runtimeID,
                createdAt: item.createdAt,
                summary: item.summary,
                unread: item.unread,
                acknowledgedAt: item.acknowledgedAt
            )
        }
        let fromKey = paneSessionKey(target: targetToken, sessionName: fromToken)
        let toKey = paneSessionKey(target: targetToken, sessionName: toToken)
        if let order = sessionStableOrder.removeValue(forKey: fromKey) {
            sessionStableOrder[toKey] = order
            persistSessionStableOrder()
        }
        if pinnedSessionKeys.remove(fromKey) != nil {
            pinnedSessionKeys.insert(toKey)
            persistPinnedSessions()
        }
    }

    private func removePaneFromLocalState(_ paneID: String) {
        let token = paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else {
            return
        }
        panes = panes.filter { $0.id != token }
        let liveSessionKeys = Set(panes.map { paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName) })
        let liveWindowKeys = Set(panes.map { "\($0.identity.target)|\($0.identity.sessionName)|\($0.identity.windowID)" })
        sessions = sessions.filter { liveSessionKeys.contains(paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName)) }
        windows = windows.filter { liveWindowKeys.contains("\($0.identity.target)|\($0.identity.sessionName)|\($0.identity.windowID)") }
        reviewQueue = reviewQueue.filter { item in
            "\(item.target)|\(item.sessionName)|\(item.windowID ?? "")|\(item.paneID)" != token
        }
        informationalQueue = informationalQueue.filter { item in
            "\(item.target)|\(item.sessionName)|\(item.windowID ?? "")|\(item.paneID)" != token
        }
        paneObservations.removeValue(forKey: token)
        terminalRenderCacheByPaneID.removeValue(forKey: token)
        lastInteractiveInputAtByPaneID.removeValue(forKey: token)
        pendingInputLatencyStartByPaneID.removeValue(forKey: token)
        paneDisplayNameOverridesByID.removeValue(forKey: token)
        persistPaneDisplayNameOverrides()
        if selectedPane?.id == token {
            selectedPane = nil
        }
    }

    private func removeSessionFromLocalState(target: String, sessionName: String) {
        let targetToken = target.trimmingCharacters(in: .whitespacesAndNewlines)
        let sessionToken = sessionName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !targetToken.isEmpty, !sessionToken.isEmpty else {
            return
        }
        panes = panes.filter {
            !($0.identity.target == targetToken && $0.identity.sessionName == sessionToken)
        }
        windows = windows.filter {
            !($0.identity.target == targetToken && $0.identity.sessionName == sessionToken)
        }
        sessions = sessions.filter {
            !($0.identity.target == targetToken && $0.identity.sessionName == sessionToken)
        }
        reviewQueue = reviewQueue.filter {
            !($0.target == targetToken && $0.sessionName == sessionToken)
        }
        informationalQueue = informationalQueue.filter {
            !($0.target == targetToken && $0.sessionName == sessionToken)
        }
        let sessionKey = paneSessionKey(target: targetToken, sessionName: sessionToken)
        if sessionStableOrder.removeValue(forKey: sessionKey) != nil {
            persistSessionStableOrder()
        }
        if pinnedSessionKeys.remove(sessionKey) != nil {
            persistPinnedSessions()
        }
        paneObservations = paneObservations.filter { key, _ in
            !key.hasPrefix("\(targetToken)|\(sessionToken)|")
        }
        let removedPaneIDs = Set(paneDisplayNameOverridesByID.keys.filter { $0.hasPrefix("\(targetToken)|\(sessionToken)|") })
        if !removedPaneIDs.isEmpty {
            for key in removedPaneIDs {
                paneDisplayNameOverridesByID.removeValue(forKey: key)
            }
            persistPaneDisplayNameOverrides()
        }
    }

    func autoReconnectTargetsIfNeeded(now: Date = Date()) async {
        guard !targetReconnectSweepInFlight else {
            return
        }
        targetReconnectSweepInFlight = true
        defer { targetReconnectSweepInFlight = false }

        let reconnectTargets = targets.filter { shouldAttemptAutoReconnect($0, now: now) }
        guard !reconnectTargets.isEmpty else {
            return
        }
        for target in reconnectTargets {
            _ = await connectTargetNow(name: target.targetName, userInitiated: false, now: now)
        }
    }

    @discardableResult
    private func connectTargetNow(
        name: String,
        userInitiated: Bool,
        now: Date = Date()
    ) async -> Bool {
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedName.isEmpty else {
            if userInitiated {
                infoMessage = ""
                errorMessage = "target name is required"
            }
            return false
        }
        let targetKey = normalizedTargetLookupKey(trimmedName)
        if targetReconnectInFlightNames.contains(targetKey) {
            if userInitiated {
                infoMessage = "target connect already in progress: \(trimmedName)"
                errorMessage = ""
            }
            return false
        }
        targetReconnectInFlightNames.insert(targetKey)
        defer {
            targetReconnectInFlightNames.remove(targetKey)
        }

        do {
            let connectedTargets = try await client.connectTarget(name: trimmedName)
            mergeConnectedTargets(connectedTargets)
            clearTargetReconnectState(for: targetKey)
            setTargetHealth("ok", forTargetKey: targetKey)
            if userInitiated {
                infoMessage = "target connected: \(trimmedName)"
                errorMessage = ""
            }
            return true
        } catch {
            recordTargetReconnectFailure(for: targetKey, now: now)
            if userInitiated {
                infoMessage = ""
                errorMessage = error.localizedDescription
            }
            return false
        }
    }

    private func shouldAttemptAutoReconnect(_ target: TargetItem, now: Date) -> Bool {
        guard normalizedTargetKind(target.kind) == "ssh" else {
            return false
        }
        guard trimmedNonEmpty(target.connectionRef) != nil else {
            return false
        }
        let health = normalizedTargetHealth(target.health)
        if health == "ok" {
            clearTargetReconnectState(for: normalizedTargetLookupKey(target.targetName))
            return false
        }
        let targetKey = normalizedTargetLookupKey(target.targetName)
        if targetReconnectInFlightNames.contains(targetKey) {
            return false
        }
        if let state = targetReconnectStateByName[targetKey], now < state.nextAttemptAt {
            return false
        }
        return true
    }

    private func clearTargetReconnectState(for targetKey: String) {
        targetReconnectStateByName.removeValue(forKey: targetKey)
    }

    private func recordTargetReconnectFailure(for targetKey: String, now: Date) {
        let delay = targetReconnectStateByName[targetKey]?.nextBackoffSeconds ?? targetReconnectInitialBackoffSeconds
        let nextBackoff = min(delay * 2, targetReconnectMaxBackoffSeconds)
        targetReconnectStateByName[targetKey] = TargetReconnectState(
            nextAttemptAt: now.addingTimeInterval(delay),
            nextBackoffSeconds: nextBackoff
        )
    }

    private func mergeConnectedTargets(_ connectedTargets: [TargetItem]) {
        guard !connectedTargets.isEmpty else {
            return
        }
        var merged = targets
        for connected in connectedTargets {
            if let index = merged.firstIndex(where: { normalizedTargetLookupKey($0.targetName) == normalizedTargetLookupKey(connected.targetName) || normalizedTargetLookupKey($0.targetID) == normalizedTargetLookupKey(connected.targetID) }) {
                merged[index] = connected
            } else {
                merged.append(connected)
            }
        }
        targets = merged
    }

    private func setTargetHealth(_ health: String, forTargetKey targetKey: String) {
        let normalizedHealth = normalizedTargetHealth(health)
        targets = targets.map { target in
            guard normalizedTargetLookupKey(target.targetName) == targetKey else {
                return target
            }
            if normalizedTargetHealth(target.health) == normalizedHealth {
                return target
            }
            return TargetItem(
                targetID: target.targetID,
                targetName: target.targetName,
                kind: target.kind,
                connectionRef: target.connectionRef,
                isDefault: target.isDefault,
                health: normalizedHealth
            )
        }
    }

    private func normalizedTargetLookupKey(_ raw: String) -> String {
        if let token = normalizedToken(raw) {
            return token
        }
        return raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    private func normalizedTargetKind(_ raw: String) -> String {
        normalizedToken(raw) ?? "unknown"
    }

    private func normalizedTargetHealth(_ raw: String) -> String {
        switch normalizedToken(raw) {
        case "ok":
            return "ok"
        case "degraded":
            return "degraded"
        case "down":
            return "down"
        default:
            return "unknown"
        }
    }

    private func targetHealthRank(_ health: String) -> Int {
        switch normalizedTargetHealth(health) {
        case "ok":
            return 0
        case "degraded":
            return 1
        case "down":
            return 2
        default:
            return 3
        }
    }

    private func targetRecord(for targetToken: String) -> TargetItem? {
        let normalized = targetToken.trimmingCharacters(in: .whitespacesAndNewlines)
        if normalized.isEmpty {
            return nil
        }
        let lookupKey = normalizedTargetLookupKey(normalized)
        return targets.first {
            normalizedTargetLookupKey($0.targetName) == lookupKey ||
                normalizedTargetLookupKey($0.targetID) == lookupKey
        }
    }

    private func normalizedSSHConnectionRef(_ value: String?) -> String? {
        guard var ref = trimmedNonEmpty(value) else {
            return nil
        }
        if ref.hasPrefix("ssh://") {
            ref.removeFirst("ssh://".count)
        }
        return trimmedNonEmpty(ref)
    }

    private func normalizedConnectionRefForTargetAdd(kind: String, rawConnectionRef: String) -> String? {
        guard kind == "ssh" else {
            return nil
        }
        guard let trimmed = trimmedNonEmpty(rawConnectionRef) else {
            return nil
        }
        if trimmed.hasPrefix("ssh://") {
            return trimmed
        }
        return "ssh://\(trimmed)"
    }

    private func shellQuote(_ raw: String) -> String {
        "'" + raw.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
    }

    private func escapeForAppleScriptLiteral(_ raw: String) -> String {
        raw
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
    }

    nonisolated private static func runExternalTerminalCommand(_ executable: String, _ args: [String]) throws -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = args

        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        try process.run()
        process.waitUntilExit()

        let stdoutData = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
        let stderrData = stderrPipe.fileHandleForReading.readDataToEndOfFile()
        let stdout = String(data: stdoutData, encoding: .utf8) ?? ""
        let stderr = String(data: stderrData, encoding: .utf8) ?? ""

        if process.terminationStatus != 0 {
            let command = ([executable] + args).joined(separator: " ")
            let reason = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            throw RuntimeError.commandFailed(command, process.terminationStatus, reason)
        }
        return stdout
    }

    private func syncSelectedPaneViewportIfKnown() {
        guard let cols = lastKnownTerminalViewportCols,
              let rows = lastKnownTerminalViewportRows,
              cols > 0,
              rows > 0,
              selectedPane != nil else {
            return
        }
        performTerminalResize(cols: cols, rows: rows)
    }

    private func selectFallbackPaneAfterPaneRemoval(preferredBy removedPane: PaneItem) {
        guard selectedPane == nil else {
            return
        }
        let candidates = panes.filter { !paneKillInFlightPaneIDs.contains($0.id) }
        if let sameWindow = candidates.first(where: {
            $0.identity.target == removedPane.identity.target &&
                $0.identity.sessionName == removedPane.identity.sessionName &&
                $0.identity.windowID == removedPane.identity.windowID
        }) {
            selectedPane = sameWindow
            return
        }
        if let sameSession = candidates.first(where: {
            $0.identity.target == removedPane.identity.target &&
                $0.identity.sessionName == removedPane.identity.sessionName
        }) {
            selectedPane = sameSession
            return
        }
        if let sameTarget = candidates.first(where: { $0.identity.target == removedPane.identity.target }) {
            selectedPane = sameTarget
            return
        }
        selectedPane = candidates.first
    }

    private func isAlreadyMissingPaneError(_ error: Error) -> Bool {
        let lowered = error.localizedDescription.lowercased()
        return lowered.contains("can't find pane") ||
            lowered.contains("no such pane") ||
            lowered.contains("pane not found")
    }

    private var filteredPanes: [PaneItem] {
        var visiblePanes = panes.filter { !paneKillInFlightPaneIDs.contains($0.id) }
        let hiddenCategories = hiddenStatusCategories()
        visiblePanes = visiblePanes.filter { pane in
            let category = displayCategory(for: pane)
            guard !hiddenCategories.contains(category) else {
                return false
            }
            switch statusFilter {
            case .all:
                return true
            case .managed:
                return agentPresence(for: pane) == "managed"
            case .attention:
                return category == "attention"
            case .pinned:
                let key = paneSessionKey(target: pane.identity.target, sessionName: pane.identity.sessionName)
                return pinnedSessionKeys.contains(key)
            }
        }
        let q = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !q.isEmpty else {
            return visiblePanes
        }
        return visiblePanes.filter { pane in
            matchesSearch(q, in: pane)
        }
    }

    private func matchesSearch(_ query: String, in pane: PaneItem) -> Bool {
        let fields: [String] = [
            pane.identity.target,
            pane.identity.sessionName,
            pane.identity.windowID,
            pane.identity.paneID,
            pane.state,
            pane.reasonCode ?? "",
            pane.agentType ?? "",
            pane.runtimeID ?? "",
            pane.lastEventType ?? "",
            displayCategory(for: pane),
        ]
        return fields.contains { $0.lowercased().contains(query) }
    }

    private func isCompletionEventType(_ eventType: String?) -> Bool {
        guard let normalized = normalizedToken(eventType) else {
            return false
        }
        if normalized.contains("input") || normalized.contains("approval") {
            return false
        }
        return normalized.contains("complete") ||
            normalized.contains("finished") ||
            normalized.contains("exit")
    }

    private func isActionableAttentionState(_ state: String?) -> Bool {
        guard let normalized = normalizedToken(state) else {
            return false
        }
        switch normalized {
        case "action_required_input", "action_required_approval", "action_required_error":
            return true
        default:
            return false
        }
    }

    private func inferAttentionStateFromReason(_ reasonCode: String?) -> String? {
        guard let reason = normalizedToken(reasonCode) else {
            return nil
        }
        if reason.contains("waiting_approval") || reason.contains("approval_required") || reason == "approval" {
            return "waiting_approval"
        }
        if reason.contains("waiting_input") || reason.contains("needs_input") || reason == "input" {
            return "waiting_input"
        }
        if reason.contains("error") || reason.contains("failed") {
            return "error"
        }
        return nil
    }

    private func inferAttentionStateFromEvent(_ eventType: String?) -> String? {
        guard let event = normalizedToken(eventType) else {
            return nil
        }
        let canonical = event
            .replacingOccurrences(of: ".", with: "_")
            .replacingOccurrences(of: "-", with: "_")
            .replacingOccurrences(of: ":", with: "_")
        let approvalTokens = [
            "approval_requested",
            "waiting_approval",
            "needs_approval",
            "approval_required",
            "permission_required",
            "confirm_required",
        ]
        if approvalTokens.contains(where: { canonical.contains($0) }) {
            return "waiting_approval"
        }
        let inputTokens = [
            "input_requested",
            "waiting_input",
            "needs_input",
            "input_required",
            "awaiting_input",
            "user_input",
            "resume_required",
        ]
        if inputTokens.contains(where: { canonical.contains($0) }) {
            return "waiting_input"
        }
        if canonical.contains("error") || canonical.contains("failed") {
            return "error"
        }
        let runningTokens = [
            "running",
            "active",
            "working",
            "in_progress",
            "processing",
            "thinking",
            "streaming",
            "tool_call",
            "executing",
        ]
        if runningTokens.contains(where: { canonical.contains($0) }) {
            return "running"
        }
        let idleTokens = [
            "complete",
            "finished",
            "idle",
            "ready",
            "stopped",
            "quiescent",
        ]
        if idleTokens.contains(where: { canonical.contains($0) }) {
            return "idle"
        }
        return nil
    }

    private func shouldDemoteManagedUnknownToIdle(_ pane: PaneItem) -> Bool {
        if agentPresence(for: pane) != "managed" {
            return false
        }
        if let inferredFromReason = inferAttentionStateFromReason(pane.reasonCode),
           inferredFromReason != "idle" {
            return false
        }
        if let inferredFromEvent = inferAttentionStateFromEvent(pane.lastEventType),
           inferredFromEvent != "idle" {
            return false
        }
        let reason = normalizedToken(pane.reasonCode) ?? ""
        let state = normalizedToken(pane.state) ?? ""
        let event = normalizedToken(pane.lastEventType) ?? ""
        if reason == "inconclusive" || state == "unknown" || event == "unknown" {
            if let cmd = normalizedToken(pane.currentCmd), cmd == "claude" || cmd == "codex" {
                if shouldTreatManagedUnknownAsRunning(pane) {
                    return false
                }
                return true
            }
            if let agent = normalizedToken(pane.agentType), agent == "claude" || agent == "codex" {
                if shouldTreatManagedUnknownAsRunning(pane) {
                    return false
                }
                return true
            }
        }
        return false
    }

    private func shouldTreatManagedUnknownAsRunning(_ pane: PaneItem) -> Bool {
        if agentPresence(for: pane) != "managed" {
            return false
        }
        let reasonToken = canonicalSignalToken(pane.reasonCode)
        let eventToken = canonicalSignalToken(pane.lastEventType)
        if hasAttentionSignal(reasonToken) || hasAttentionSignal(eventToken) {
            return false
        }
        if hasIdleOrCompletionSignal(reasonToken) || hasIdleOrCompletionSignal(eventToken) {
            return false
        }
        guard hasRunningSignal(reasonToken) || hasRunningSignal(eventToken) else {
            return false
        }
        let now = Date()
        if let eventAt = parseTimestamp(pane.lastEventAt ?? ""),
           !isAdministrativeEventType(pane.lastEventType),
           now.timeIntervalSince(eventAt) <= 12 {
            return true
        }
        if let interactionAt = parseTimestamp(pane.lastInteractionAt ?? ""),
           now.timeIntervalSince(interactionAt) <= 12 {
            return true
        }
        return false
    }

    private func canonicalSignalToken(_ value: String?) -> String {
        guard let normalized = normalizedToken(value) else {
            return ""
        }
        return normalized
            .replacingOccurrences(of: ".", with: "_")
            .replacingOccurrences(of: "-", with: "_")
            .replacingOccurrences(of: ":", with: "_")
            .replacingOccurrences(of: " ", with: "_")
    }

    private func hasRunningSignal(_ token: String) -> Bool {
        if token.isEmpty {
            return false
        }
        for marker in [
            "running",
            "active",
            "working",
            "in_progress",
            "progress",
            "streaming",
            "task_started",
            "session_started",
            "agent_turn_started",
            "tool_call",
            "executing",
            "thinking",
        ] {
            if token.contains(marker) {
                return true
            }
        }
        return false
    }

    private func hasIdleOrCompletionSignal(_ token: String) -> Bool {
        if token.isEmpty {
            return false
        }
        for marker in [
            "idle",
            "complete",
            "completed",
            "finished",
            "exit",
            "stopped",
            "done",
            "ready",
            "quiescent",
        ] {
            if token.contains(marker) {
                return true
            }
        }
        return false
    }

    private func hasAttentionSignal(_ token: String) -> Bool {
        if token.isEmpty {
            return false
        }
        for marker in [
            "waiting_input",
            "needs_input",
            "input_requested",
            "awaiting_input",
            "waiting_approval",
            "approval_required",
            "approval_requested",
            "error",
            "failed",
            "panic",
        ] {
            if token.contains(marker) {
                return true
            }
        }
        return false
    }

    private func parseCreatedPaneID(_ output: String) -> String? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return nil
        }
        if let markerRegex = try? NSRegularExpression(pattern: "__AGTMUX_NEW_PANE__(%\\d+)", options: []) {
            let range = NSRange(location: 0, length: (trimmed as NSString).length)
            if let match = markerRegex.firstMatch(in: trimmed, options: [], range: range),
               match.numberOfRanges >= 2 {
                let ns = trimmed as NSString
                return ns.substring(with: match.range(at: 1))
            }
        }
        // Fallback: accept the last pane token from command output.
        if let fallback = trimmed
            .split(whereSeparator: \.isWhitespace)
            .last(where: { $0.hasPrefix("%") }) {
            return String(fallback)
        }
        return nil
    }

    private func selectCreatedPane(target: String, sessionName: String, paneID: String?) -> Bool {
        guard let paneID, !paneID.isEmpty else {
            return false
        }
        let targetToken = normalizedTargetLookupKey(target)
        let sessionToken = sessionName.trimmingCharacters(in: .whitespacesAndNewlines)
        let panesInSession = panes.filter { pane in
            normalizedTargetLookupKey(pane.identity.target) == targetToken &&
                pane.identity.sessionName == sessionToken
        }
        guard !panesInSession.isEmpty else {
            return false
        }
        if let exact = panesInSession.first(where: { $0.identity.paneID == paneID }) {
            selectedPane = exact
            return true
        }
        return false
    }

    private func resolveCreatedPaneIDAfterCreate(
        target: String,
        sessionName: String,
        preferredPaneID: String?,
        baselinePaneIDs: Set<String>
    ) async -> String? {
        let targetToken = normalizedTargetLookupKey(target)
        let sessionToken = sessionName.trimmingCharacters(in: .whitespacesAndNewlines)
        let preferred = preferredPaneID?.trimmingCharacters(in: .whitespacesAndNewlines)
        let attempts = 40
        for attempt in 0..<attempts {
            await refresh()
            let currentPaneIDs = Set(
                panes
                    .filter { pane in
                        normalizedTargetLookupKey(pane.identity.target) == targetToken &&
                            pane.identity.sessionName == sessionToken
                    }
                    .map(\.identity.paneID)
            )
            if let preferred, !preferred.isEmpty, currentPaneIDs.contains(preferred) {
                return preferred
            }
            let added = currentPaneIDs.subtracting(baselinePaneIDs)
            if added.count == 1, let only = added.first {
                return only
            }
            if added.count > 1 {
                return added.sorted(by: { lhs, rhs in
                    paneNumericID(lhs) > paneNumericID(rhs)
                }).first
            }
            if attempt + 1 < attempts {
                try? await Task.sleep(for: .milliseconds(150))
            }
        }
        return nil
    }

    private func waitForPaneAbsenceInSnapshot(_ paneID: String, attempts: Int = 10, intervalMs: Int = 120) async -> Bool {
        let token = paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else {
            return true
        }
        for attempt in 0..<attempts {
            await refresh()
            if !panes.contains(where: { $0.id == token }) {
                return true
            }
            if attempt + 1 < attempts {
                try? await Task.sleep(for: .milliseconds(intervalMs))
            }
        }
        return false
    }

    private func restoreSelectionIfNeeded(paneID: String?) {
        guard let paneID = paneID?.trimmingCharacters(in: .whitespacesAndNewlines), !paneID.isEmpty else {
            return
        }
        guard selectedPane?.id != paneID else {
            return
        }
        if let pane = panes.first(where: { $0.id == paneID }) {
            selectedPane = pane
        }
    }

    private func paneNumericID(_ paneID: String) -> Int {
        let token = paneID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard token.hasPrefix("%") else {
            return Int.min
        }
        let numeric = token.dropFirst()
        return Int(numeric) ?? Int.min
    }

    private func isAdministrativeEventType(_ eventType: String?) -> Bool {
        guard let normalized = normalizedToken(eventType) else {
            return false
        }
        let canonical = normalized
            .replacingOccurrences(of: ".", with: "_")
            .replacingOccurrences(of: "-", with: "_")
            .replacingOccurrences(of: ":", with: "_")
        if canonical.contains("wrapper_start") || canonical.contains("wrapper_exit") {
            return true
        }
        if canonical.contains("view_output") || canonical.contains("terminal_read") || canonical.contains("terminal_stream") {
            return true
        }
        if canonical.contains("action_attach") || canonical.contains("action_kill") {
            return true
        }
        return false
    }

    private func parseTimestamp(_ input: String) -> Date? {
        if let date = Self.isoFormatter.date(from: input) {
            return date
        }
        let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasSuffix("Z"), !trimmed.contains(".") {
            return Self.isoFormatterNoFractional.date(from: trimmed)
        }
        return nil
    }

    private static let isoFormatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    private static let isoFormatterNoFractional: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter
    }()

    private func compactRelativeTimestamp(since date: Date, now: Date) -> String {
        let elapsed = max(0, now.timeIntervalSince(date))
        if elapsed < 60 {
            return "\(Int(elapsed))s"
        }
        if elapsed < 3600 {
            return "\(Int(elapsed / 60))m"
        }
        if elapsed < 86_400 {
            return "\(Int(elapsed / 3600))h"
        }
        if elapsed < 2_592_000 {
            return "\(Int(elapsed / 86_400))d"
        }
        if elapsed < 31_536_000 {
            return "\(Int(elapsed / 2_592_000))mo"
        }
        return "\(Int(elapsed / 31_536_000))y"
    }

    private func autoRecoverFromDaemonError(triggeredBy error: Error) async -> Bool {
        guard shouldAttemptAutoRecover(for: error) else {
            return false
        }
        recoveryInFlight = true
        lastRecoveryAttemptAt = Date()
        defer { recoveryInFlight = false }
        do {
            try await daemon.ensureRunning(with: client)
            let snapshot = try await client.fetchSnapshot()
            applySnapshot(snapshot)
            daemonState = .running
            clearBackendSurfaceMessages()
            return true
        } catch {
            daemonState = .error
            showBackgroundRecoveryNotice()
            return false
        }
    }

    private func shouldAttemptAutoRecover(for error: Error) -> Bool {
        if recoveryInFlight {
            return false
        }
        if let last = lastRecoveryAttemptAt, Date().timeIntervalSince(last) < recoveryCooldownSeconds {
            return false
        }
        guard let runtimeError = error as? RuntimeError else {
            return false
        }
        switch runtimeError {
        case .commandFailed, .daemonStartTimeout:
            return true
        case .binaryNotFound, .invalidJSON:
            return false
        }
    }

    private func showBackgroundRecoveryNotice() {
        if infoMessage.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || isBackendSurfaceMessage(infoMessage) {
            infoMessage = "接続を自動復旧中です。しばらくお待ちください。"
        }
        if isBackendSurfaceMessage(errorMessage) {
            errorMessage = ""
        }
    }

    private func clearBackendSurfaceMessages() {
        if isBackendSurfaceMessage(errorMessage) {
            errorMessage = ""
        }
        if isBackendSurfaceMessage(infoMessage) {
            infoMessage = ""
        }
    }

    private func isRuntimeTransportError(_ error: Error) -> Bool {
        guard let runtimeError = error as? RuntimeError else {
            return false
        }
        switch runtimeError {
        case .commandFailed, .daemonStartTimeout, .invalidJSON:
            return true
        case .binaryNotFound:
            return false
        }
    }

    private func isBackendSurfaceMessage(_ text: String) -> Bool {
        let normalized = text.lowercased()
        if normalized.contains("daemon") {
            return true
        }
        if normalized.contains("backend") {
            return true
        }
        if normalized.contains("command failed") {
            return true
        }
        if normalized.contains("--socket") {
            return true
        }
        if normalized.contains("自動復旧中") {
            return true
        }
        return false
    }
}
