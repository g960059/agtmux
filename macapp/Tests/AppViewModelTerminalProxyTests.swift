import Foundation
import XCTest
@testable import AGTMUXDesktop

@MainActor
final class AppViewModelTerminalProxyTests: XCTestCase {
    func testPerformViewOutputAttachedFrameFetchesFollowupFrame() async throws {
        let fixture = try makeModelFixture(
            streamMode: "attached_then_output",
            streamContent: "hello-from-follow",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]
        model.selectedPane = pane

        model.performViewOutput(lines: 80)

        await waitUntil("view output info message", timeout: 2.0) {
            model.infoMessage == "terminal-output: c2"
        }
        XCTAssertEqual(model.outputPreview, "hello-from-follow")
        XCTAssertEqual(model.errorMessage, "")
        let log = readLog(at: fixture.logURL)
        XCTAssertEqual(occurrenceCount(in: log, token: "terminal stream"), 2)
    }

    func testPerformSendAttachedFrameFetchesFollowupFrame() async throws {
        let fixture = try makeModelFixture(
            streamMode: "attached_then_output",
            streamContent: "hello-from-send",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]
        model.selectedPane = pane
        model.sendText = "echo test"
        model.sendEnter = true
        model.sendPaste = false

        model.performSend()

        await waitUntil("send completed", timeout: 2.0) {
            model.infoMessage == "terminal-write: completed"
        }
        XCTAssertEqual(model.outputPreview, "hello-from-send")
        XCTAssertEqual(model.errorMessage, "")
        let log = readLog(at: fixture.logURL)
        XCTAssertEqual(occurrenceCount(in: log, token: "terminal write"), 1)
        XCTAssertEqual(occurrenceCount(in: log, token: "terminal stream"), 2)
    }

    func testPerformViewOutputGenerationMismatchDetachesStaleSession() async throws {
        let fixture = try makeModelFixture(
            attachDelaySeconds: "0.20",
            streamMode: "output_only",
            streamContent: "unused",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let paneA = makePane(sessionName: "s1", paneID: "%1")
        let paneB = makePane(sessionName: "s1", paneID: "%2")
        model.panes = [paneA, paneB]
        model.selectedPane = paneA

        model.performViewOutput(lines: 80)
        await waitUntil("attach started for view output", timeout: 2.0) {
            self.logContainsAttach(for: "%1", in: self.readLog(at: fixture.logURL))
        }
        model.selectedPane = paneB
        model.selectedPane = paneA
        XCTAssertEqual(model.selectedPane?.id, paneA.id)

        await waitUntil("detach on generation mismatch", timeout: 2.0) {
            self.readLog(at: fixture.logURL).contains("terminal detach --session term-1")
        }
        let log = readLog(at: fixture.logURL)
        XCTAssertFalse(log.contains("terminal stream"))
        XCTAssertEqual(model.errorMessage, "")
        XCTAssertEqual(model.outputPreview, "")
    }

    func testPerformSendGenerationMismatchCancelsBeforeWrite() async throws {
        let fixture = try makeModelFixture(
            attachDelaySeconds: "0.20",
            streamMode: "output_only",
            streamContent: "unused",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let paneA = makePane(sessionName: "s1", paneID: "%1")
        let paneB = makePane(sessionName: "s1", paneID: "%2")
        model.panes = [paneA, paneB]
        model.selectedPane = paneA
        model.sendText = "echo hello"
        model.sendEnter = true
        model.sendPaste = false

        model.performSend()
        await waitUntil("attach started for send", timeout: 2.0) {
            self.logContainsAttach(for: "%1", in: self.readLog(at: fixture.logURL))
        }
        model.selectedPane = paneB
        model.selectedPane = paneA
        XCTAssertEqual(model.selectedPane?.id, paneA.id)

        await waitUntil("detach after cancelled send", timeout: 2.0) {
            let log = self.readLog(at: fixture.logURL)
            return log.contains("terminal detach --session term-1")
        }
        let log = readLog(at: fixture.logURL)
        XCTAssertFalse(log.contains("terminal write"))
        XCTAssertEqual(model.errorMessage, "")
        XCTAssertEqual(model.sendText, "echo hello")
    }

    func testSelectedPaneStartsStreamWhenAutoStreamEnabled() async throws {
        let fixture = try makeModelFixture(
            streamMode: "output_only",
            streamContent: "stream-started",
            autoStreamOnSelection: true
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]

        model.selectedPane = pane

        await waitUntil("auto stream starts on pane selection", timeout: 2.0) {
            self.logContainsAttach(for: "%1", in: self.readLog(at: fixture.logURL))
        }
        await waitUntil("stream frame after auto attach", timeout: 2.0) {
            self.readLog(at: fixture.logURL).contains("terminal stream")
        }
    }

    func testSelectedPaneStartsStreamWithDefaultInitializerFlag() async throws {
        let fixture = try makeModelFixtureWithDefaultAuto(
            streamMode: "output_only",
            streamContent: "default-auto-stream"
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]

        model.selectedPane = pane

        await waitUntil("auto stream with default init", timeout: 2.0) {
            let log = self.readLog(at: fixture.logURL)
            return self.logContainsAttach(for: "%1", in: log) && log.contains("terminal stream")
        }
    }

    func testSelectedPaneStartsStreamWhenCapabilitiesCommandFails() async throws {
        let fixture = try makeModelFixture(
            streamMode: "attached_then_output",
            streamContent: "caps-failed-but-streamed",
            autoStreamOnSelection: true,
            capabilitiesAvailable: false
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]

        model.selectedPane = pane

        await waitUntil("stream still starts when capabilities fail", timeout: 2.0) {
            self.logContainsAttach(for: "%1", in: self.readLog(at: fixture.logURL))
        }
        await waitUntil("follow-up stream frame renders content", timeout: 2.0) {
            model.outputPreview == "caps-failed-but-streamed"
        }
        XCTAssertNotEqual(model.infoMessage, "terminal stream waiting for daemon capabilities...")
    }

    func testAutoAttachIncludesRuntimeAndStateGuards() async throws {
        let fixture = try makeModelFixture(
            streamMode: "output_only",
            streamContent: "guard-check",
            autoStreamOnSelection: true
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]

        model.selectedPane = pane

        await waitUntil("attach log with guard args", timeout: 4.0) {
            let lines = self.logLines(at: fixture.logURL)
            return lines.contains(where: { line in
                line.contains("terminal attach") &&
                    line.contains("--pane %1") &&
                    line.contains("--if-runtime rt-%1") &&
                    line.contains("--if-state idle")
            })
        }
    }

    func testPaneReselectClearsCursorBeforeFirstStreamRequest() async throws {
        let fixture = try makeModelFixture(
            streamMode: "output_only",
            streamContent: "cursor-reset",
            autoStreamOnSelection: true
        )
        let model = fixture.model
        let paneA = makePane(sessionName: "s1", paneID: "%1")
        let paneB = makePane(sessionName: "s1", paneID: "%2")
        model.panes = [paneA, paneB]

        model.selectedPane = paneB
        await waitUntil("initial stream for paneB", timeout: 4.0) {
            self.occurrenceCount(
                in: self.readLog(at: fixture.logURL),
                token: "terminal stream --session term-2"
            ) >= 1
        }

        model.selectedPane = paneA
        await waitUntil("stream for paneA", timeout: 4.0) {
            self.occurrenceCount(
                in: self.readLog(at: fixture.logURL),
                token: "terminal stream --session term-1"
            ) >= 1
        }

        let beforeReselectLines = streamLines(forSession: "term-2", at: fixture.logURL)
        let beforeReselectCount = beforeReselectLines.count
        model.selectedPane = paneB
        await waitUntil("stream after reselect paneB", timeout: 4.0) {
            self.streamLines(forSession: "term-2", at: fixture.logURL).count > beforeReselectCount
        }
        model.selectedPane = nil

        let afterReselectLines = streamLines(forSession: "term-2", at: fixture.logURL)
        XCTAssertGreaterThan(afterReselectLines.count, beforeReselectCount)
        let firstReselectLine = afterReselectLines[beforeReselectCount]
        XCTAssertFalse(
            firstReselectLine.contains("--cursor"),
            "reselect直後の最初のstreamはstale cursorを送らないべき"
        )
    }

    func testSelectedPaneDoesNotStartStreamWhenAutoStreamDisabled() async throws {
        let fixture = try makeModelFixture(
            streamMode: "output_only",
            streamContent: "stream-should-not-start",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]

        model.selectedPane = pane

        await assertNever("auto stream should stay disabled", duration: 1.2) {
            let log = self.readLog(at: fixture.logURL)
            return self.logContainsAttach(for: "%1", in: log) || log.contains("terminal stream")
        }
        let finalLog = readLog(at: fixture.logURL)
        XCTAssertFalse(logContainsAttach(for: "%1", in: finalLog))
        XCTAssertFalse(finalLog.contains("terminal stream"))
    }

    func testPerformViewOutputDegradesToTerminalReadAfterProxyFailures() async throws {
        let fixture = try makeModelFixture(
            streamMode: "stream_always_fail",
            streamContent: "read-fallback-content",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]
        model.selectedPane = pane

        for _ in 0..<3 {
            model.performViewOutput(lines: 80)
            await waitUntil("proxy failure surfaced", timeout: 2.0) {
                !model.errorMessage.isEmpty
            }
            model.errorMessage = ""
        }

        model.performViewOutput(lines: 80)

        await waitUntil("fallback read succeeds", timeout: 2.0) {
            model.outputPreview == "read-fallback-content"
        }
        XCTAssertEqual(model.errorMessage, "")
        let log = readLog(at: fixture.logURL)
        XCTAssertGreaterThanOrEqual(occurrenceCount(in: log, token: "terminal stream"), 3)
        XCTAssertGreaterThanOrEqual(occurrenceCount(in: log, token: "terminal read"), 1)
    }

    func testPerformInteractiveInputSendsTerminalKey() async throws {
        let fixture = try makeModelFixture(
            streamMode: "output_only",
            streamContent: "interactive-key-ok",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]
        model.selectedPane = pane

        model.performInteractiveInput(key: "Up")

        await waitUntil("interactive key write", timeout: 2.0) {
            let log = self.readLog(at: fixture.logURL)
            return log.contains("terminal write --session term-1 --json --key Up")
        }
        await waitUntil("interactive key stream update", timeout: 2.0) {
            model.outputPreview == "interactive-key-ok"
        }
        XCTAssertEqual(model.errorMessage, "")
    }

    func testPerformInteractiveInputSendsTerminalTextChunk() async throws {
        let fixture = try makeModelFixture(
            streamMode: "output_only",
            streamContent: "interactive-text-ok",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]
        model.selectedPane = pane

        model.performInteractiveInput(text: "/status")

        await waitUntil("interactive text write", timeout: 2.0) {
            let log = self.readLog(at: fixture.logURL)
            return log.contains("terminal write --session term-1 --json --text /status")
        }
        await waitUntil("interactive text stream update", timeout: 2.0) {
            model.outputPreview == "interactive-text-ok"
        }
        XCTAssertEqual(model.errorMessage, "")
    }

    func testPerformInteractiveInputBytesSendsTerminalTextChunk() async throws {
        let fixture = try makeModelFixture(
            streamMode: "output_only",
            streamContent: "interactive-bytes-ok",
            autoStreamOnSelection: false
        )
        let model = fixture.model
        let pane = makePane(sessionName: "s1", paneID: "%1")
        model.panes = [pane]
        model.selectedPane = pane

        model.performInteractiveInput(bytes: Array("/status".utf8))

        await waitUntil("interactive bytes write", timeout: 2.0) {
            let log = self.readLog(at: fixture.logURL)
            return log.contains("terminal write --session term-1 --json --bytes-b64")
        }
        await waitUntil("interactive bytes stream update", timeout: 2.0) {
            model.outputPreview == "interactive-bytes-ok"
        }
        XCTAssertEqual(model.errorMessage, "")
    }

    private struct ModelFixture {
        let model: AppViewModel
        let logURL: URL
    }

    private enum AutoStreamSelectionConfig {
        case explicit(Bool)
        case useDefaultInitializer
    }

    private func makeModelFixture(
        attachDelaySeconds: String? = nil,
        streamMode: String,
        streamContent: String,
        autoStreamOnSelection: Bool,
        capabilitiesAvailable: Bool = true
    ) throws -> ModelFixture {
        try makeModelFixture(
            attachDelaySeconds: attachDelaySeconds,
            streamMode: streamMode,
            streamContent: streamContent,
            capabilitiesAvailable: capabilitiesAvailable,
            autoStreamConfig: .explicit(autoStreamOnSelection)
        )
    }

    private func makeModelFixtureWithDefaultAuto(
        attachDelaySeconds: String? = nil,
        streamMode: String,
        streamContent: String,
        capabilitiesAvailable: Bool = true
    ) throws -> ModelFixture {
        try makeModelFixture(
            attachDelaySeconds: attachDelaySeconds,
            streamMode: streamMode,
            streamContent: streamContent,
            capabilitiesAvailable: capabilitiesAvailable,
            autoStreamConfig: .useDefaultInitializer
        )
    }

    private func makeModelFixture(
        attachDelaySeconds: String? = nil,
        streamMode: String,
        streamContent: String,
        capabilitiesAvailable: Bool = true,
        autoStreamConfig: AutoStreamSelectionConfig
    ) throws -> ModelFixture {
        let fm = FileManager.default
        let tempRoot = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("agtmux-swift-tests-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: tempRoot, withIntermediateDirectories: true)

        let scriptURL = tempRoot.appendingPathComponent("agtmux-app-stub.sh")
        let logURL = tempRoot.appendingPathComponent("stub.log")
        let stateURL = tempRoot.appendingPathComponent("stream-state.txt")
        try Data().write(to: logURL)
        try Data().write(to: stateURL)

        let attachDelay = attachDelaySeconds ?? "0"
        let capabilitiesFlag = capabilitiesAvailable ? "1" : "0"
        let script = """
        #!/usr/bin/env bash
        set -euo pipefail

        LOG_FILE='\(logURL.path)'
        STATE_FILE='\(stateURL.path)'
        ATTACH_DELAY='\(attachDelay)'
        STREAM_MODE='\(streamMode)'
        STREAM_CONTENT='\(streamContent)'
        CAPABILITIES_AVAILABLE='\(capabilitiesFlag)'
        DEFAULT_SESSION_PREFIX='term'
        args=("$@")
        if [[ "${args[0]:-}" == "--socket" ]]; then
          args=("${args[@]:2}")
        fi
        printf '%s\\n' "${args[*]}" >> "$LOG_FILE"

        cmd="${args[0]:-}"
        sub="${args[1]:-}"
        target="local"
        pane="%1"
        session=""
        for ((i=0; i<${#args[@]}; i++)); do
          if [[ "${args[$i]}" == "--target" && $((i+1)) -lt ${#args[@]} ]]; then
            target="${args[$((i+1))]}"
          fi
          if [[ "${args[$i]}" == "--pane" && $((i+1)) -lt ${#args[@]} ]]; then
            pane="${args[$((i+1))]}"
          fi
          if [[ "${args[$i]}" == "--session" && $((i+1)) -lt ${#args[@]} ]]; then
            session="${args[$((i+1))]}"
          fi
        done
        if [[ -z "$session" ]]; then
          pane_suffix="${pane#%}"
          session="${DEFAULT_SESSION_PREFIX}-${pane_suffix}"
        fi

        if [[ "$cmd" == "terminal" && "$sub" == "capabilities" ]]; then
          if [[ "$CAPABILITIES_AVAILABLE" != "1" ]]; then
            echo "capabilities unavailable" >&2
            exit 2
          fi
          cat <<'JSON'
        {"capabilities":{"embedded_terminal":true,"terminal_read":true,"terminal_resize":true,"terminal_write_via_action_send":true,"terminal_attach":true,"terminal_write":true,"terminal_stream":true,"terminal_proxy_mode":"daemon-proxy-pty-poc","terminal_frame_protocol":"terminal-stream-v1"}}
        JSON
          exit 0
        fi

        if [[ "$cmd" == "terminal" && "$sub" == "attach" ]]; then
          if [[ "$ATTACH_DELAY" != "0" ]]; then
            sleep "$ATTACH_DELAY"
          fi
          cat <<JSON
        {"session_id":"$session","target":"$target","pane_id":"$pane","runtime_id":"rt-1","state_version":1,"result_code":"completed"}
        JSON
          exit 0
        fi

        if [[ "$cmd" == "terminal" && "$sub" == "write" ]]; then
          cat <<JSON
        {"session_id":"$session","result_code":"completed"}
        JSON
          exit 0
        fi

        if [[ "$cmd" == "terminal" && "$sub" == "read" ]]; then
          cat <<JSON
        {"frame":{"frame_type":"reset","stream_id":"st-read","cursor":"r1","pane_id":"$pane","target":"$target","lines":200,"content":"$STREAM_CONTENT"}}
        JSON
          exit 0
        fi

        if [[ "$cmd" == "terminal" && "$sub" == "stream" ]]; then
          if [[ "$STREAM_MODE" == "stream_always_fail" ]]; then
            echo "E_RUNTIME_STALE: forced stream failure" >&2
            exit 1
          fi
          count=0
          if [[ -f "$STATE_FILE" ]]; then
            count="$(cat "$STATE_FILE")"
          fi
          count=$((count+1))
          printf '%s' "$count" > "$STATE_FILE"

          if [[ "$STREAM_MODE" == "attached_then_output" && "$count" -eq 1 ]]; then
            cat <<JSON
        {"frame":{"frame_type":"attached","stream_id":"st-1","cursor":"c1","session_id":"$session","target":"$target","pane_id":"$pane","content":null}}
        JSON
          else
            cat <<JSON
        {"frame":{"frame_type":"output","stream_id":"st-1","cursor":"c$count","session_id":"$session","target":"$target","pane_id":"$pane","content":"$STREAM_CONTENT"}}
        JSON
          fi
          exit 0
        fi

        if [[ "$cmd" == "terminal" && "$sub" == "detach" ]]; then
          cat <<JSON
        {"session_id":"$session","result_code":"completed"}
        JSON
          exit 0
        fi

        echo "unsupported args: ${args[*]}" >&2
        exit 2
        """
        try script.write(to: scriptURL, atomically: true, encoding: .utf8)
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: scriptURL.path)

        addTeardownBlock {
            try? fm.removeItem(at: tempRoot)
        }

        let daemon = try DaemonManager(
            socketPath: tempRoot.appendingPathComponent("agtmux-test.sock").path,
            dbPath: tempRoot.appendingPathComponent("state.db").path,
            logPath: tempRoot.appendingPathComponent("daemon.log").path,
            daemonBinaryPath: "/usr/bin/true"
        )
        let client = AGTMUXCLIClient(
            socketPath: tempRoot.appendingPathComponent("agtmux-test.sock").path,
            appBinaryPath: scriptURL.path
        )

        let suiteName = "AGTMUXDesktopProxyTests-\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            XCTFail("Failed to create test defaults")
            throw NSError(domain: "test", code: 1)
        }
        defaults.removePersistentDomain(forName: suiteName)
        addTeardownBlock {
            defaults.removePersistentDomain(forName: suiteName)
        }

        let model: AppViewModel
        switch autoStreamConfig {
        case .explicit(let autoStreamOnSelection):
            model = AppViewModel(
                daemon: daemon,
                client: client,
                defaults: defaults,
                autoStreamOnSelection: autoStreamOnSelection
            )
        case .useDefaultInitializer:
            model = AppViewModel(
                daemon: daemon,
                client: client,
                defaults: defaults
            )
        }
        return ModelFixture(model: model, logURL: logURL)
    }

    private func makePane(sessionName: String, paneID: String) -> PaneItem {
        PaneItem(
            identity: PaneIdentity(
                target: "local",
                sessionName: sessionName,
                windowID: "@1",
                paneID: paneID
            ),
            windowName: nil,
            currentCmd: "codex",
            paneTitle: nil,
            state: "idle",
            reasonCode: nil,
            confidence: nil,
            runtimeID: "rt-\(paneID)",
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
    }

    private func waitUntil(
        _ label: String,
        timeout: TimeInterval,
        condition: @escaping @MainActor () -> Bool
    ) async {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() <= deadline {
            if condition() {
                return
            }
            try? await Task.sleep(for: .milliseconds(20))
        }
        XCTFail("Timed out waiting for \(label)")
    }

    private func assertNever(
        _ label: String,
        duration: TimeInterval,
        condition: @escaping @MainActor () -> Bool
    ) async {
        let deadline = Date().addingTimeInterval(duration)
        while Date() <= deadline {
            if condition() {
                XCTFail("Unexpected condition met: \(label)")
                return
            }
            try? await Task.sleep(for: .milliseconds(20))
        }
    }

    private func readLog(at url: URL) -> String {
        (try? String(contentsOf: url, encoding: .utf8)) ?? ""
    }

    private func logLines(at url: URL) -> [String] {
        readLog(at: url)
            .split(separator: "\n")
            .map(String.init)
    }

    private func streamLines(forSession sessionID: String, at url: URL) -> [String] {
        logLines(at: url).filter { line in
            line.contains("terminal stream") && line.contains("--session \(sessionID)")
        }
    }

    private func logContainsAttach(for paneID: String, in log: String) -> Bool {
        log
            .split(separator: "\n")
            .contains { line in
                line.contains("terminal attach") && line.contains("--pane \(paneID)")
            }
    }

    private func occurrenceCount(in text: String, token: String) -> Int {
        guard !token.isEmpty else {
            return 0
        }
        return text.components(separatedBy: token).count - 1
    }
}
