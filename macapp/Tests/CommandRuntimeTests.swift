import Foundation
import XCTest
@testable import AGTMUXDesktop

final class CommandRuntimeTests: XCTestCase {
    func testAddTargetIncludesSSHConnectionAndDefaultFlag() async throws {
        let capture = ArgsCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { _, args in
                capture.set(args)
                return """
                {"targets":[{"target_id":"vm-1","target_name":"vm-1","kind":"ssh","connection_ref":"ssh://dev-vm","is_default":true,"health":"ok"}]}
                """
            }
        )

        let targets = try await client.addTarget(
            name: "vm-1",
            kind: "ssh",
            connectionRef: "ssh://dev-vm",
            isDefault: true
        )

        let joined = capture.snapshot().joined(separator: " ")
        XCTAssertTrue(joined.contains("target add vm-1"))
        XCTAssertTrue(joined.contains("--kind ssh"))
        XCTAssertTrue(joined.contains("--connection-ref ssh://dev-vm"))
        XCTAssertTrue(joined.contains("--default"))
        XCTAssertTrue(joined.contains("--json"))
        XCTAssertEqual(targets.first?.targetName, "vm-1")
        XCTAssertEqual(targets.first?.kind, "ssh")
    }

    func testAddTargetLocalOmitsConnectionRef() async throws {
        let capture = ArgsCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { _, args in
                capture.set(args)
                return """
                {"targets":[{"target_id":"local2","target_name":"local2","kind":"local","connection_ref":"","is_default":false,"health":"ok"}]}
                """
            }
        )

        _ = try await client.addTarget(
            name: "local2",
            kind: "local",
            connectionRef: nil,
            isDefault: false
        )

        let joined = capture.snapshot().joined(separator: " ")
        XCTAssertTrue(joined.contains("target add local2"))
        XCTAssertTrue(joined.contains("--kind local"))
        XCTAssertFalse(joined.contains("--connection-ref"))
        XCTAssertFalse(joined.contains("--default"))
    }

    func testConnectTargetBuildsArgs() async throws {
        let capture = ArgsCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { _, args in
                capture.set(args)
                return """
                {"targets":[{"target_id":"vm-2","target_name":"vm-2","kind":"ssh","connection_ref":"ssh://vm-2","is_default":false,"health":"ok"}]}
                """
            }
        )

        let targets = try await client.connectTarget(name: "vm-2")
        let joined = capture.snapshot().joined(separator: " ")
        XCTAssertTrue(joined.contains("target connect vm-2"))
        XCTAssertTrue(joined.contains("--json"))
        XCTAssertEqual(targets.first?.targetName, "vm-2")
    }

    func testFetchSnapshotUsesDaemonTransportWhenAvailable() async throws {
        var transport = StubTerminalTransport()
        transport.dashboardSnapshotHandler = { _ in
            DashboardSnapshotEnvelope(
                targets: [
                    TargetItem(
                        targetID: "local",
                        targetName: "local",
                        kind: "local",
                        connectionRef: "",
                        isDefault: true,
                        health: "ok"
                    )
                ],
                sessions: [],
                windows: [],
                panes: []
            )
        }
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            daemonTransport: transport,
            commandRunner: { _, _ in
                XCTFail("snapshot should not fallback to CLI when daemon transport is healthy")
                return "{}"
            }
        )

        let snapshot = try await client.fetchSnapshot()
        XCTAssertEqual(snapshot.targets.count, 1)
        XCTAssertEqual(snapshot.targets.first?.targetName, "local")
        XCTAssertTrue(snapshot.sessions.isEmpty)
        XCTAssertTrue(snapshot.windows.isEmpty)
        XCTAssertTrue(snapshot.panes.isEmpty)
    }

    func testFetchSnapshotFallsBackToCLIWhenDaemonTransportUnavailable() async throws {
        var transport = StubTerminalTransport()
        transport.dashboardSnapshotHandler = { _ in
            throw DaemonUnixClientError.unavailable("snapshot unavailable")
        }
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            daemonTransport: transport,
            commandRunner: { _, args in
                if args.contains("targets") {
                    return """
                    {"targets":[{"target_id":"local","target_name":"local","kind":"local","connection_ref":"","is_default":true,"health":"ok"}]}
                    """
                }
                if args.contains("sessions") {
                    return """
                    {"items":[{"identity":{"target":"local","session_name":"s1"},"total_panes":1,"by_state":{"idle":1},"by_agent":{"none":1}}]}
                    """
                }
                if args.contains("windows") {
                    return """
                    {"items":[{"identity":{"target":"local","session_name":"s1","window_id":"@1"},"top_state":"idle","waiting_count":0,"running_count":0,"total_panes":1}]}
                    """
                }
                if args.contains("panes") {
                    return """
                    {"items":[{"identity":{"target":"local","session_name":"s1","window_id":"@1","pane_id":"%1"},"state":"idle","updated_at":"2026-02-18T00:00:00Z"}]}
                    """
                }
                XCTFail("unexpected args: \(args)")
                return "{}"
            }
        )

        let snapshot = try await client.fetchSnapshot()
        XCTAssertEqual(snapshot.targets.count, 1)
        XCTAssertEqual(snapshot.sessions.count, 1)
        XCTAssertEqual(snapshot.windows.count, 1)
        XCTAssertEqual(snapshot.panes.count, 1)
        XCTAssertEqual(snapshot.panes.first?.identity.paneID, "%1")
    }

    func testAddTargetRejectsSSHWithoutConnectionRef() async throws {
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { _, _ in
                XCTFail("command runner should not be called")
                return "{}"
            }
        )

        do {
            _ = try await client.addTarget(
                name: "vm-3",
                kind: "ssh",
                connectionRef: nil,
                isDefault: false
            )
            XCTFail("expected validation error")
        } catch RuntimeError.commandFailed(let command, _, let message) {
            XCTAssertTrue(command.contains("target add"))
            XCTAssertTrue(message.contains("connection_ref"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }

    func testTerminalWriteDoesNotFallbackWhenDaemonTransportUnavailable() async throws {
        var transport = StubTerminalTransport()
        transport.terminalWriteHandler = { _, _, _, _, _, _ in
            throw DaemonUnixClientError.unavailable("transport down")
        }
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            daemonTransport: transport,
            commandRunner: { _, _ in
                XCTFail("terminalWrite fallback must not run for non-idempotent operation")
                return "{\"session_id\":\"term-1\",\"result_code\":\"completed\"}"
            }
        )

        do {
            _ = try await client.terminalWrite(
                sessionID: "term-1",
                text: "echo hello",
                key: nil,
                bytes: nil,
                enter: true,
                paste: false
            )
            XCTFail("expected terminalWrite to fail when daemon transport is unavailable")
        } catch RuntimeError.commandFailed(let command, _, _) {
            XCTAssertTrue(command.contains("daemon /v1/terminal/write"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }

    func testTerminalStreamFallsBackWhenDaemonTransportUnavailable() async throws {
        var transport = StubTerminalTransport()
        transport.terminalStreamHandler = { _, _, _ in
            throw DaemonUnixClientError.unavailable("transport down")
        }
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            daemonTransport: transport,
            commandRunner: { _, args in
                XCTAssertTrue(args.contains("terminal"))
                XCTAssertTrue(args.contains("stream"))
                return """
                {"frame":{"frame_type":"output","stream_id":"st-1","cursor":"c1","session_id":"term-1","target":"local","pane_id":"%1","content":"fallback-content"}}
                """
            }
        )

        let resp = try await client.terminalStream(sessionID: "term-1", cursor: nil, lines: 120)
        XCTAssertEqual(resp.frame.frameType, "output")
        XCTAssertEqual(resp.frame.cursor, "c1")
        XCTAssertEqual(resp.frame.content, "fallback-content")
    }

    func testSendTextIncludesStaleGuardFlagsWhenProvided() async throws {
        let capture = ArgsCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { _, args in
                capture.set(args)
                return #"{"action_id":"a-send","result_code":"completed"}"#
            }
        )

        _ = try await client.sendText(
            target: "local",
            paneID: "%1",
            text: "echo hi",
            requestRef: "req-1",
            enter: true,
            paste: false,
            ifRuntime: "rt-1",
            ifState: "idle",
            ifUpdatedWithin: "30s",
            forceStale: true
        )

        let capturedArgs = capture.snapshot()
        let joined = capturedArgs.joined(separator: " ")
        XCTAssertTrue(joined.contains("--if-runtime rt-1"))
        XCTAssertTrue(joined.contains("--if-state idle"))
        XCTAssertTrue(joined.contains("--if-updated-within 30s"))
        XCTAssertTrue(capturedArgs.contains("--force-stale"))
    }

    func testKillIncludesStaleGuardFlagsWhenProvided() async throws {
        let capture = ArgsCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { _, args in
                capture.set(args)
                return #"{"action_id":"a-kill","result_code":"completed"}"#
            }
        )

        _ = try await client.kill(
            target: "local",
            paneID: "%1",
            requestRef: "req-2",
            mode: "key",
            signal: "INT",
            ifRuntime: "rt-2",
            ifState: "running",
            ifUpdatedWithin: "45s",
            forceStale: true
        )

        let capturedArgs = capture.snapshot()
        let joined = capturedArgs.joined(separator: " ")
        XCTAssertTrue(joined.contains("--if-runtime rt-2"))
        XCTAssertTrue(joined.contains("--if-state running"))
        XCTAssertTrue(joined.contains("--if-updated-within 45s"))
        XCTAssertTrue(capturedArgs.contains("--force-stale"))
    }

    func testSendTextOmitsStaleGuardFlagsWhenNotProvided() async throws {
        let capture = ArgsCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { _, args in
                capture.set(args)
                return #"{"action_id":"a-send","result_code":"completed"}"#
            }
        )

        _ = try await client.sendText(
            target: "local",
            paneID: "%1",
            text: "echo hi",
            requestRef: "req-3",
            enter: true,
            paste: false
        )

        let joined = capture.snapshot().joined(separator: " ")
        XCTAssertFalse(joined.contains("--if-runtime"))
        XCTAssertFalse(joined.contains("--if-state"))
        XCTAssertFalse(joined.contains("--if-updated-within"))
        XCTAssertFalse(joined.contains("--force-stale"))
    }

    func testKillOmitsStaleGuardFlagsWhenNotProvided() async throws {
        let capture = ArgsCapture()
        let client = AGTMUXCLIClient(
            socketPath: "/tmp/agtmux-test.sock",
            appBinaryPath: "/usr/bin/true",
            commandRunner: { _, args in
                capture.set(args)
                return #"{"action_id":"a-kill","result_code":"completed"}"#
            }
        )

        _ = try await client.kill(
            target: "local",
            paneID: "%1",
            requestRef: "req-4",
            mode: "signal",
            signal: "TERM"
        )

        let joined = capture.snapshot().joined(separator: " ")
        XCTAssertFalse(joined.contains("--if-runtime"))
        XCTAssertFalse(joined.contains("--if-state"))
        XCTAssertFalse(joined.contains("--if-updated-within"))
        XCTAssertFalse(joined.contains("--force-stale"))
    }
}

private final class ArgsCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var value: [String] = []

    func set(_ args: [String]) {
        lock.lock()
        value = args
        lock.unlock()
    }

    func snapshot() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

private struct StubTerminalTransport: TerminalDaemonTransport {
    enum StubError: Error {
        case missingHandler(String)
    }

    var dashboardSnapshotHandler: ((String?) async throws -> DashboardSnapshotEnvelope)?
    var listCapabilitiesHandler: (() async throws -> CapabilitiesEnvelope)?
    var terminalReadHandler: ((String, String, String?, Int) async throws -> TerminalReadEnvelope)?
    var terminalResizeHandler: ((String, String, Int, Int) async throws -> TerminalResizeResponse)?
    var terminalAttachHandler: ((String, String, String?, String?, String?, Bool) async throws -> TerminalAttachResponse)?
    var terminalDetachHandler: ((String) async throws -> TerminalDetachResponse)?
    var terminalWriteHandler: ((String, String?, String?, [UInt8]?, Bool, Bool) async throws -> TerminalWriteResponse)?
    var terminalStreamHandler: ((String, String?, Int) async throws -> TerminalStreamEnvelope)?

    func dashboardSnapshot(target: String?) async throws -> DashboardSnapshotEnvelope {
        guard let dashboardSnapshotHandler else {
            throw StubError.missingHandler("dashboardSnapshot")
        }
        return try await dashboardSnapshotHandler(target)
    }

    func listCapabilities() async throws -> CapabilitiesEnvelope {
        guard let listCapabilitiesHandler else {
            throw StubError.missingHandler("listCapabilities")
        }
        return try await listCapabilitiesHandler()
    }

    func terminalRead(target: String, paneID: String, cursor: String?, lines: Int) async throws -> TerminalReadEnvelope {
        guard let terminalReadHandler else {
            throw StubError.missingHandler("terminalRead")
        }
        return try await terminalReadHandler(target, paneID, cursor, lines)
    }

    func terminalResize(target: String, paneID: String, cols: Int, rows: Int) async throws -> TerminalResizeResponse {
        guard let terminalResizeHandler else {
            throw StubError.missingHandler("terminalResize")
        }
        return try await terminalResizeHandler(target, paneID, cols, rows)
    }

    func terminalAttach(
        target: String,
        paneID: String,
        ifRuntime: String?,
        ifState: String?,
        ifUpdatedWithin: String?,
        forceStale: Bool
    ) async throws -> TerminalAttachResponse {
        guard let terminalAttachHandler else {
            throw StubError.missingHandler("terminalAttach")
        }
        return try await terminalAttachHandler(target, paneID, ifRuntime, ifState, ifUpdatedWithin, forceStale)
    }

    func terminalDetach(sessionID: String) async throws -> TerminalDetachResponse {
        guard let terminalDetachHandler else {
            throw StubError.missingHandler("terminalDetach")
        }
        return try await terminalDetachHandler(sessionID)
    }

    func terminalWrite(
        sessionID: String,
        text: String?,
        key: String?,
        bytes: [UInt8]?,
        enter: Bool,
        paste: Bool
    ) async throws -> TerminalWriteResponse {
        guard let terminalWriteHandler else {
            throw StubError.missingHandler("terminalWrite")
        }
        return try await terminalWriteHandler(sessionID, text, key, bytes, enter, paste)
    }

    func terminalStream(sessionID: String, cursor: String?, lines: Int) async throws -> TerminalStreamEnvelope {
        guard let terminalStreamHandler else {
            throw StubError.missingHandler("terminalStream")
        }
        return try await terminalStreamHandler(sessionID, cursor, lines)
    }
}
