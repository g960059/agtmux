import Foundation

struct TerminalFailureOutcome: Equatable {
    let delayMillis: Int
    let didEnterDegradedMode: Bool
    let degradedUntil: Date?
}

actor TerminalSessionController {
    private let failureThreshold: Int
    private let degradeCooldownSeconds: TimeInterval

    private var cursorByPaneID: [String: String] = [:]
    private var proxySessionByPaneID: [String: String] = [:]
    private var failureCountByPaneID: [String: Int] = [:]
    private var degradedUntilByPaneID: [String: Date] = [:]

    init(
        failureThreshold: Int = 3,
        degradeCooldownSeconds: TimeInterval = 8
    ) {
        self.failureThreshold = max(1, failureThreshold)
        self.degradeCooldownSeconds = max(1, degradeCooldownSeconds)
    }

    func cursor(for paneID: String) -> String? {
        cursorByPaneID[paneID]
    }

    func setCursor(_ cursor: String?, for paneID: String) {
        if let cursor = normalizedNonEmpty(cursor) {
            cursorByPaneID[paneID] = cursor
            return
        }
        cursorByPaneID.removeValue(forKey: paneID)
    }

    func clearCursor(for paneID: String) {
        cursorByPaneID.removeValue(forKey: paneID)
    }

    func proxySession(for paneID: String) -> String? {
        proxySessionByPaneID[paneID]
    }

    func setProxySession(_ sessionID: String?, for paneID: String) {
        if let sessionID = normalizedNonEmpty(sessionID) {
            proxySessionByPaneID[paneID] = sessionID
            return
        }
        proxySessionByPaneID.removeValue(forKey: paneID)
    }

    func clearProxySession(for paneID: String) -> String? {
        proxySessionByPaneID.removeValue(forKey: paneID)
    }

    @discardableResult
    func resetPane(_ paneID: String) -> String? {
        cursorByPaneID.removeValue(forKey: paneID)
        failureCountByPaneID.removeValue(forKey: paneID)
        degradedUntilByPaneID.removeValue(forKey: paneID)
        return proxySessionByPaneID.removeValue(forKey: paneID)
    }

    func prune(keepingPaneIDs paneIDs: Set<String>) -> [String] {
        var staleSessions: [String] = []
        for (paneID, sessionID) in proxySessionByPaneID where !paneIDs.contains(paneID) {
            staleSessions.append(sessionID)
        }
        cursorByPaneID = cursorByPaneID.filter { paneIDs.contains($0.key) }
        proxySessionByPaneID = proxySessionByPaneID.filter { paneIDs.contains($0.key) }
        failureCountByPaneID = failureCountByPaneID.filter { paneIDs.contains($0.key) }
        degradedUntilByPaneID = degradedUntilByPaneID.filter { paneIDs.contains($0.key) }
        return staleSessions
    }

    func shouldUseProxy(for paneID: String, now: Date = Date()) -> Bool {
        guard let degradedUntil = degradedUntilByPaneID[paneID] else {
            return true
        }
        if now >= degradedUntil {
            degradedUntilByPaneID.removeValue(forKey: paneID)
            failureCountByPaneID.removeValue(forKey: paneID)
            return true
        }
        return false
    }

    func recordSuccess(for paneID: String) {
        failureCountByPaneID.removeValue(forKey: paneID)
        degradedUntilByPaneID.removeValue(forKey: paneID)
    }

    func recordFailure(for paneID: String, now: Date = Date()) -> TerminalFailureOutcome {
        let failures = (failureCountByPaneID[paneID] ?? 0) + 1
        failureCountByPaneID[paneID] = failures
        let delayMillis = retryDelayMillis(forConsecutiveFailures: failures)

        guard failures >= failureThreshold else {
            return TerminalFailureOutcome(
                delayMillis: delayMillis,
                didEnterDegradedMode: false,
                degradedUntil: degradedUntilByPaneID[paneID]
            )
        }

        let previous = degradedUntilByPaneID[paneID]
        let nextUntil = now.addingTimeInterval(degradeCooldownSeconds)
        let entered = previous == nil || now >= previous!
        if let previous, previous > nextUntil {
            degradedUntilByPaneID[paneID] = previous
        } else {
            degradedUntilByPaneID[paneID] = nextUntil
        }
        return TerminalFailureOutcome(
            delayMillis: delayMillis,
            didEnterDegradedMode: entered,
            degradedUntil: degradedUntilByPaneID[paneID]
        )
    }

    private func retryDelayMillis(forConsecutiveFailures count: Int) -> Int {
        let bounded = min(max(count, 1), 5)
        return min(4000, Int(Double(250) * pow(2, Double(bounded - 1))))
    }

    private func normalizedNonEmpty(_ raw: String?) -> String? {
        guard let raw else {
            return nil
        }
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
