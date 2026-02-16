import Foundation
import SwiftUI

@MainActor
final class AppViewModel: ObservableObject {
    enum DaemonState: String {
        case starting
        case running
        case error
    }

    enum ViewMode: String, CaseIterable, Identifiable {
        case bySession
        case byStatus
        case byChronological

        var id: String { rawValue }

        var title: String {
            switch self {
            case .bySession:
                return "By Session"
            case .byStatus:
                return "By Status"
            case .byChronological:
                return "By Chronological"
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
    }

    private struct PaneObservation {
        let state: String
        let category: String
        let lastEventType: String
        let lastEventAt: String
        let awaitingKind: String
    }

    private enum PreferenceKey {
        static let uiPrefsVersion = "ui.prefs_version"
        static let viewMode = "ui.view_mode"
        static let windowGrouping = "ui.window_grouping"
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
    @Published var selectedPane: PaneItem? {
        didSet {
            if oldValue?.id != selectedPane?.id {
                terminalStreamTask?.cancel()
                if let oldPaneID = oldValue?.id {
                    if let oldSessionID = terminalProxySessionByPaneID.removeValue(forKey: oldPaneID) {
                        terminalCursorByPaneID.removeValue(forKey: oldPaneID)
                        Task { [weak self] in
                            await self?.detachTerminalProxySession(sessionID: oldSessionID)
                        }
                    } else {
                        terminalCursorByPaneID.removeValue(forKey: oldPaneID)
                    }
                }
                if let paneID = selectedPane?.id {
                    // Force a fresh snapshot/stream sync on pane switch.
                    terminalCursorByPaneID.removeValue(forKey: paneID)
                }
                outputPreview = ""
                restartTerminalStreamForSelectedPane()
            }
        }
    }
    @Published var searchQuery: String = ""
    @Published var sendText: String = ""
    @Published var sendEnter: Bool = true
    @Published var sendPaste: Bool = false
    @Published var outputPreview: String = ""
    @Published var refreshInFlight: Bool = false
    @Published var viewMode: ViewMode = .bySession {
        didSet {
            defaults.set(viewMode.rawValue, forKey: PreferenceKey.viewMode)
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

    private let daemon: DaemonManager
    private let client: AGTMUXCLIClient
    private let defaults: UserDefaults
    private var pollingTask: Task<Void, Never>?
    private var paneObservations: [String: PaneObservation] = [:]
    private var queueLastEmitByKey: [String: Date] = [:]
    private var terminalCapabilities: CapabilityFlags?
    private var terminalCapabilitiesFetchedAt: Date?
    private var terminalCursorByPaneID: [String: String] = [:]
    private var terminalProxySessionByPaneID: [String: String] = [:]
    private var terminalStreamTask: Task<Void, Never>?
    private var terminalStreamGeneration: Int = 0
    private var didBootstrap = false
    private var recoveryInFlight = false
    private var lastRecoveryAttemptAt: Date?
    private let queueDedupeWindowSeconds: TimeInterval = 30
    private let recoveryCooldownSeconds: TimeInterval = 6
    private let queueLimit = 250
    private let currentUIPrefsVersion = 4
    private let terminalCapabilitiesCacheTTLSeconds: TimeInterval = 60
    private let terminalStreamPollIntervalSeconds: TimeInterval = 1.0
    private let terminalOutputMaxChars = 120_000

    init(daemon: DaemonManager, client: AGTMUXCLIClient, defaults: UserDefaults = .standard) {
        self.daemon = daemon
        self.client = client
        self.defaults = defaults
        loadPreferences()
    }

    deinit {
        pollingTask?.cancel()
        terminalStreamTask?.cancel()
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

    var hasSelectedPane: Bool {
        selectedPane != nil
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
                if await shouldUseTerminalProxy() {
                    let sessionID = try await ensureTerminalProxySession(for: pane)
                    guard selectedPane?.id == pane.id else {
                        return
                    }
                    let resp = try await client.terminalWrite(
                        sessionID: sessionID,
                        text: sendText,
                        key: nil,
                        enter: sendEnter,
                        paste: sendPaste
                    )
                    if resp.resultCode != "completed" {
                        let reason = resp.errorCode ?? "unknown"
                        errorMessage = "terminal-write failed: \(reason)"
                        return
                    }
                    let streamResp = try await client.terminalStream(
                        sessionID: sessionID,
                        cursor: terminalCursorByPaneID[pane.id],
                        lines: 200
                    )
                    terminalCursorByPaneID[pane.id] = streamResp.frame.cursor
                    applyTerminalStreamFrame(streamResp.frame, paneID: pane.id)
                    if streamResp.frame.frameType == "attached" {
                        let followResp = try await client.terminalStream(
                            sessionID: sessionID,
                            cursor: streamResp.frame.cursor,
                            lines: 200
                        )
                        terminalCursorByPaneID[pane.id] = followResp.frame.cursor
                        applyTerminalStreamFrame(followResp.frame, paneID: pane.id)
                    }
                    infoMessage = "terminal-write: \(resp.resultCode)"
                } else {
                    let requestRef = "macapp-send-\(UUID().uuidString)"
                    let resp = try await client.sendText(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        text: sendText,
                        requestRef: requestRef,
                        enter: sendEnter,
                        paste: sendPaste
                    )
                    infoMessage = "send: \(resp.resultCode) (\(resp.actionID))"
                    await refresh()
                }
                errorMessage = ""
                sendText = ""
            } catch {
                errorMessage = error.localizedDescription
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
                if await shouldUseTerminalProxy() {
                    if forceSnapshot {
                        terminalCursorByPaneID.removeValue(forKey: pane.id)
                    }
                    let cursor = forceSnapshot ? nil : terminalCursorByPaneID[pane.id]
                    let sessionID = try await ensureTerminalProxySession(for: pane)
                    guard selectedPane?.id == pane.id else {
                        return
                    }
                    let resp = try await client.terminalStream(sessionID: sessionID, cursor: cursor, lines: lines)
                    terminalCursorByPaneID[pane.id] = resp.frame.cursor
                    applyTerminalStreamFrame(resp.frame, paneID: pane.id)
                    if resp.frame.frameType == "attached" {
                        let followResp = try await client.terminalStream(
                            sessionID: sessionID,
                            cursor: resp.frame.cursor,
                            lines: lines
                        )
                        terminalCursorByPaneID[pane.id] = followResp.frame.cursor
                        applyTerminalStreamFrame(followResp.frame, paneID: pane.id)
                    }
                    infoMessage = "terminal-\(resp.frame.frameType): \(resp.frame.cursor)"
                } else if await shouldUseTerminalRead() {
                    if forceSnapshot {
                        terminalCursorByPaneID.removeValue(forKey: pane.id)
                    }
                    let cursor = forceSnapshot ? nil : terminalCursorByPaneID[pane.id]
                    let resp = try await client.terminalRead(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        cursor: cursor,
                        lines: lines
                    )
                    terminalCursorByPaneID[pane.id] = resp.frame.cursor
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
        let grouped = Dictionary(grouping: filteredPanes, by: { paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName) })
        let sessionMeta = Dictionary(uniqueKeysWithValues: sessions.map { (paneSessionKey(target: $0.identity.target, sessionName: $0.identity.sessionName), $0) })
        var out: [SessionSection] = []
        for (key, paneList) in grouped {
            guard let first = paneList.first else {
                continue
            }
            let sorted = sortedPanes(paneList)
            let counts = countByCategory(in: sorted)
            let topCategory = sessionMeta[key]?.topCategory ?? topCategory(from: counts)
            let windows = shouldGroupByWindow(sorted) ? buildWindowSections(sorted, key: key) : []
            out.append(SessionSection(
                id: key,
                target: first.identity.target,
                sessionName: first.identity.sessionName,
                topCategory: topCategory,
                byCategory: counts,
                panes: sorted,
                windows: windows
            ))
        }
        out.sort { lhs, rhs in
            let lp = categoryPrecedence(lhs.topCategory)
            let rp = categoryPrecedence(rhs.topCategory)
            if lp != rp {
                return lp < rp
            }
            if lhs.target != rhs.target {
                return lhs.target < rhs.target
            }
            return lhs.sessionName < rhs.sessionName
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
        if let lastInteraction = parseTimestamp(pane.lastInteractionAt ?? "") {
            return lastInteraction
        }
        if let lastEvent = parseTimestamp(pane.lastEventAt ?? "") {
            return lastEvent
        }
        return parseTimestamp(pane.updatedAt)
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
        guard let index = reviewQueue.firstIndex(where: { $0.id == item.id }) else {
            return
        }
        reviewQueue[index].acknowledgedAt = Date()
        reviewQueue[index].unread = false
    }

    func acknowledgeAllQueueItems() {
        let now = Date()
        for idx in reviewQueue.indices {
            if reviewQueue[idx].acknowledgedAt == nil {
                reviewQueue[idx].acknowledgedAt = now
                reviewQueue[idx].unread = false
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
        if let cat = normalizedToken(pane.displayCategory) {
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

    func activityState(for pane: PaneItem) -> String {
        if let state = normalizedToken(pane.activityState) {
            return state
        }
        switch normalizedState(pane.state) {
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
            return "unknown"
        }
    }

    func stateReason(for pane: PaneItem) -> String {
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
        let anchor = parseTimestamp(pane.lastInteractionAt ?? "")
        guard let updated = anchor else {
            return "last active: -"
        }
        return "last active: \(compactRelativeTimestamp(since: updated, now: Date()))"
    }

    func lastActiveShortLabel(for pane: PaneItem) -> String {
        let anchor = parseTimestamp(pane.lastInteractionAt ?? "")
        guard let updated = anchor else {
            return "-"
        }
        return compactRelativeTimestamp(since: updated, now: Date())
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
                let resp = try await client.kill(
                    target: pane.identity.target,
                    paneID: pane.identity.paneID,
                    requestRef: requestRef,
                    mode: mode,
                    signal: signal
                )
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
                try? await Task.sleep(for: .seconds(2))
            }
        }
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
        observeTransitions(newPanes: snapshot.panes, now: Date())
        targets = snapshot.targets
        sessions = snapshot.sessions
        windows = snapshot.windows
        panes = snapshot.panes
        let paneIDs = Set(panes.map(\.id))
        let staleSessions = terminalProxySessionByPaneID.filter { !paneIDs.contains($0.key) }
        terminalCursorByPaneID = terminalCursorByPaneID.filter { paneIDs.contains($0.key) }
        terminalProxySessionByPaneID = terminalProxySessionByPaneID.filter { paneIDs.contains($0.key) }
        if !staleSessions.isEmpty {
            for (_, sessionID) in staleSessions {
                Task { [weak self] in
                    await self?.detachTerminalProxySession(sessionID: sessionID)
                }
            }
        }
        if let current = selectedPane {
            selectedPane = panes.first(where: { $0.id == current.id })
            if selectedPane == nil {
                infoMessage = "選択中 pane が消えました。再選択してください。"
            }
        }
    }

    private func observeTransitions(newPanes: [PaneItem], now: Date) {
        var next: [String: PaneObservation] = [:]
        for pane in newPanes {
            let category = displayCategory(for: pane)
            let state = normalizedState(pane.state)
            let lastEventType = normalizedToken(pane.lastEventType) ?? ""
            let lastEventAt = pane.lastEventAt ?? ""
            let awaitingKind = awaitingResponseKind(for: pane) ?? ""
            if let prev = paneObservations[pane.id] {
                if category == "attention" && prev.category != "attention" {
                    switch awaitingKind {
                    case "input":
                        enqueueReview(kind: .needsInput, pane: pane, now: now)
                    case "approval":
                        enqueueReview(kind: .needsApproval, pane: pane, now: now)
                    default:
                        enqueueReview(kind: .error, pane: pane, now: now)
                    }
                }
                if isCompletionEventType(lastEventType) &&
                    (prev.lastEventType != lastEventType || prev.lastEventAt != lastEventAt) {
                    enqueueReview(kind: .taskCompleted, pane: pane, now: now)
                }
            }
            next[pane.id] = PaneObservation(
                state: state,
                category: category,
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
            let lcat = displayCategory(for: lhs)
            let rcat = displayCategory(for: rhs)
            let lp = categoryPrecedence(lcat)
            let rp = categoryPrecedence(rcat)
            if lp != rp {
                return lp < rp
            }
            if lhs.identity.windowID != rhs.identity.windowID {
                return lhs.identity.windowID < rhs.identity.windowID
            }
            return lhs.identity.paneID < rhs.identity.paneID
        }
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
        if let raw = defaults.string(forKey: PreferenceKey.windowGrouping), let restored = WindowGrouping(rawValue: raw) {
            windowGrouping = restored
        }
        showWindowMetadata = readBoolPreference(PreferenceKey.showWindowMetadata, fallback: false)
        showWindowGroupBackground = readBoolPreference(PreferenceKey.showWindowGroupBackground, fallback: true)
        showSessionMetadataInStatusView = readBoolPreference(PreferenceKey.showSessionMetadataInStatusView, fallback: false)
        showEmptyStatusColumns = readBoolPreference(PreferenceKey.showEmptyStatusColumns, fallback: false)
        showTechnicalDetails = readBoolPreference(PreferenceKey.showTechnicalDetails, fallback: false)
        hideUnmanagedCategory = readBoolPreference(PreferenceKey.hideUnmanagedCategory, fallback: false)
        showUnknownCategory = readBoolPreference(PreferenceKey.showUnknownCategory, fallback: false)
        reviewUnreadOnly = readBoolPreference(PreferenceKey.reviewUnreadOnly, fallback: true)

        let storedVersion = defaults.integer(forKey: PreferenceKey.uiPrefsVersion)
        if storedVersion < currentUIPrefsVersion {
            // v4: keep cleaner cards and enable by-session window group highlight by default.
            showWindowMetadata = false
            showSessionMetadataInStatusView = false
            showWindowGroupBackground = true
            defaults.set(currentUIPrefsVersion, forKey: PreferenceKey.uiPrefsVersion)
        }
    }

    private func readBoolPreference(_ key: String, fallback: Bool) -> Bool {
        guard defaults.object(forKey: key) != nil else {
            return fallback
        }
        return defaults.bool(forKey: key)
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
        var cursor = terminalCursorByPaneID[paneID]
        var consecutiveFailures = 0
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
                if await shouldUseTerminalProxy() {
                    let sessionID = try await ensureTerminalProxySession(for: pane, generation: generation)
                    let resp = try await client.terminalStream(
                        sessionID: sessionID,
                        cursor: cursor,
                        lines: 200
                    )
                    guard generation == terminalStreamGeneration, selectedPane?.id == paneID else {
                        return
                    }
                    cursor = resp.frame.cursor
                    terminalCursorByPaneID[paneID] = resp.frame.cursor
                    applyTerminalStreamFrame(resp.frame, paneID: paneID)
                } else if await shouldUseTerminalRead() {
                    let resp = try await client.terminalRead(
                        target: pane.identity.target,
                        paneID: pane.identity.paneID,
                        cursor: cursor,
                        lines: 200
                    )
                    guard generation == terminalStreamGeneration, selectedPane?.id == paneID else {
                        return
                    }
                    cursor = resp.frame.cursor
                    terminalCursorByPaneID[paneID] = resp.frame.cursor
                    applyTerminalFrame(resp.frame, paneID: paneID)
                } else {
                    consecutiveFailures += 1
                    let delayMillis = terminalStreamRetryDelayMillis(for: consecutiveFailures)
                    if selectedPane?.id == paneID, consecutiveFailures >= 3 {
                        infoMessage = "terminal stream waiting for daemon capabilities..."
                    }
                    try? await Task.sleep(for: .milliseconds(delayMillis))
                    continue
                }
                consecutiveFailures = 0
                try await Task.sleep(for: .milliseconds(Int(terminalStreamPollIntervalSeconds * 1000)))
            } catch {
                if error is CancellationError || Task.isCancelled || generation != terminalStreamGeneration {
                    return
                }
                if shouldResetTerminalProxySession(for: error),
                   let staleSessionID = terminalProxySessionByPaneID[paneID] {
                    terminalProxySessionByPaneID.removeValue(forKey: paneID)
                    terminalCursorByPaneID.removeValue(forKey: paneID)
                    cursor = nil
                    await detachTerminalProxySession(sessionID: staleSessionID)
                }
                consecutiveFailures += 1
                let delayMillis = terminalStreamRetryDelayMillis(for: consecutiveFailures)
                if selectedPane?.id == paneID, consecutiveFailures >= 3 {
                    infoMessage = "terminal stream reconnecting..."
                }
                try? await Task.sleep(for: .milliseconds(delayMillis))
            }
        }
    }

    private func terminalStreamRetryDelayMillis(for consecutiveFailures: Int) -> Int {
        let cappedFailure = min(max(consecutiveFailures, 1), 5)
        return min(4000, Int(Double(250) * pow(2, Double(cappedFailure - 1))))
    }

    private func applyTerminalFrame(_ frame: TerminalFrame, paneID: String) {
        guard selectedPane?.id == paneID else {
            return
        }
        let content = frame.content ?? ""
        if frame.frameType == "delta" {
            if !content.isEmpty {
                outputPreview += content
            }
        } else {
            outputPreview = content
        }
        trimOutputPreviewIfNeeded()
    }

    private func applyTerminalStreamFrame(_ frame: TerminalStreamFrame, paneID: String) {
        guard selectedPane?.id == paneID else {
            return
        }
        switch frame.frameType {
        case "attached":
            return
        case "output":
            let content = frame.content ?? ""
            if !content.isEmpty {
                outputPreview += content
            }
        case "reset":
            outputPreview = frame.content ?? ""
        case "error":
            let reason = frame.errorCode ?? frame.message ?? "unknown"
            errorMessage = "terminal-stream error: \(reason)"
            if let sessionID = terminalProxySessionByPaneID[paneID] {
                terminalProxySessionByPaneID.removeValue(forKey: paneID)
                terminalCursorByPaneID.removeValue(forKey: paneID)
                Task { [weak self] in
                    await self?.detachTerminalProxySession(sessionID: sessionID)
                }
            }
            return
        default:
            return
        }
        trimOutputPreviewIfNeeded()
    }

    private func trimOutputPreviewIfNeeded() {
        if outputPreview.count <= terminalOutputMaxChars {
            return
        }
        outputPreview = String(outputPreview.suffix(terminalOutputMaxChars))
    }

    private func ensureTerminalProxySession(for pane: PaneItem, generation: Int? = nil) async throws -> String {
        if let sessionID = terminalProxySessionByPaneID[pane.id], !sessionID.isEmpty {
            return sessionID
        }
        let response = try await client.terminalAttach(
            target: pane.identity.target,
            paneID: pane.identity.paneID,
            ifRuntime: trimmedNonEmpty(pane.runtimeID),
            ifState: nil,
            ifUpdatedWithin: nil,
            forceStale: false
        )
        let sessionID = response.sessionID.trimmingCharacters(in: .whitespacesAndNewlines)
        if response.resultCode != "completed" || sessionID.isEmpty {
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
        terminalProxySessionByPaneID[pane.id] = sessionID
        return sessionID
    }

    private func detachTerminalProxySession(sessionID: String) async {
        guard !sessionID.isEmpty else {
            return
        }
        _ = try? await client.terminalDetach(sessionID: sessionID)
    }

    private func shouldUseTerminalProxy() async -> Bool {
        guard let caps = await fetchTerminalCapabilities() else {
            return false
        }
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

    private func shouldUseTerminalRead() async -> Bool {
        guard let caps = await fetchTerminalCapabilities() else {
            return false
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

    private func shouldResetTerminalProxySession(for error: Error) -> Bool {
        guard case let RuntimeError.commandFailed(_, _, stderr) = error else {
            return false
        }
        let normalized = stderr.lowercased()
        return normalized.contains("e_ref_not_found") ||
            normalized.contains("session not found") ||
            normalized.contains("e_runtime_stale")
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

    private var filteredPanes: [PaneItem] {
        let q = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !q.isEmpty else {
            return panes
        }
        return panes.filter { pane in
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
