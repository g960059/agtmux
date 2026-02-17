import Foundation
import XCTest
@testable import AGTMUXDesktop

final class TerminalSessionControllerTests: XCTestCase {
    func testFailureThresholdEntersDegradedModeAndRecoversAfterCooldown() async {
        let sut = TerminalSessionController(failureThreshold: 3, degradeCooldownSeconds: 10)
        let paneID = "pane-1"
        let base = Date(timeIntervalSince1970: 1_700_000_000)

        let initialCanUseProxy = await sut.shouldUseProxy(for: paneID, now: base)
        XCTAssertTrue(initialCanUseProxy)

        let first = await sut.recordFailure(for: paneID, now: base)
        XCTAssertEqual(first.delayMillis, 250)
        XCTAssertFalse(first.didEnterDegradedMode)

        let second = await sut.recordFailure(for: paneID, now: base.addingTimeInterval(1))
        XCTAssertEqual(second.delayMillis, 500)
        XCTAssertFalse(second.didEnterDegradedMode)

        let third = await sut.recordFailure(for: paneID, now: base.addingTimeInterval(2))
        XCTAssertEqual(third.delayMillis, 1000)
        XCTAssertTrue(third.didEnterDegradedMode)
        XCTAssertNotNil(third.degradedUntil)

        let shouldNotUseProxyDuringCooldown = await sut.shouldUseProxy(for: paneID, now: base.addingTimeInterval(3))
        XCTAssertFalse(shouldNotUseProxyDuringCooldown)
        let shouldRecoverAfterCooldown = await sut.shouldUseProxy(for: paneID, now: base.addingTimeInterval(13))
        XCTAssertTrue(shouldRecoverAfterCooldown)
    }

    func testRecordSuccessClearsFailuresAndDegradedState() async {
        let sut = TerminalSessionController(failureThreshold: 2, degradeCooldownSeconds: 30)
        let paneID = "pane-2"
        let now = Date(timeIntervalSince1970: 1_700_000_100)

        _ = await sut.recordFailure(for: paneID, now: now)
        _ = await sut.recordFailure(for: paneID, now: now.addingTimeInterval(1))
        let degradedProxyState = await sut.shouldUseProxy(for: paneID, now: now.addingTimeInterval(2))
        XCTAssertFalse(degradedProxyState)

        await sut.recordSuccess(for: paneID)

        let recoveredProxyState = await sut.shouldUseProxy(for: paneID, now: now.addingTimeInterval(3))
        XCTAssertTrue(recoveredProxyState)
        let afterSuccess = await sut.recordFailure(for: paneID, now: now.addingTimeInterval(4))
        XCTAssertEqual(afterSuccess.delayMillis, 250)
        XCTAssertFalse(afterSuccess.didEnterDegradedMode)
    }

    func testPaneSessionAndCursorLifecycle() async {
        let sut = TerminalSessionController()
        let paneID = "pane-3"

        await sut.setProxySession("term-123", for: paneID)
        await sut.setCursor("c-10", for: paneID)

        let currentSession = await sut.proxySession(for: paneID)
        let currentCursor = await sut.cursor(for: paneID)
        XCTAssertEqual(currentSession, "term-123")
        XCTAssertEqual(currentCursor, "c-10")

        let removed = await sut.resetPane(paneID)
        XCTAssertEqual(removed, "term-123")
        let clearedSession = await sut.proxySession(for: paneID)
        let clearedCursor = await sut.cursor(for: paneID)
        XCTAssertNil(clearedSession)
        XCTAssertNil(clearedCursor)
    }

    func testRetryDelayIsExponentiallyBackedOffAndCapped() async {
        let sut = TerminalSessionController(failureThreshold: 99, degradeCooldownSeconds: 10)
        let paneID = "pane-4"
        let now = Date(timeIntervalSince1970: 1_700_000_200)

        var delays: [Int] = []
        for step in 0..<7 {
            let outcome = await sut.recordFailure(for: paneID, now: now.addingTimeInterval(Double(step)))
            delays.append(outcome.delayMillis)
        }

        XCTAssertEqual(delays, [250, 500, 1000, 2000, 4000, 4000, 4000])
    }
}
