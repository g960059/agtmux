import Foundation
import Darwin

enum RuntimeError: LocalizedError {
    case binaryNotFound(String)
    case commandFailed(String, Int32, String)
    case invalidJSON(String)
    case daemonStartTimeout(String)

    var errorDescription: String? {
        switch self {
        case .binaryNotFound(let name):
            return "Binary not found: \(name)"
        case .commandFailed(let cmd, let code, let stderr):
            return "Command failed (\(code)): \(cmd)\n\(stderr)"
        case .invalidJSON(let message):
            return "JSON decode error: \(message)"
        case .daemonStartTimeout(let socket):
            return "Backend start timed out: \(socket)"
        }
    }
}

struct BinaryResolver {
    static func resolve(binary name: String, envKey: String) throws -> String {
        if let override = ProcessInfo.processInfo.environment[envKey], !override.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            let path = override.trimmingCharacters(in: .whitespacesAndNewlines)
            if isExecutable(path) {
                return path
            }
        }

        for candidate in candidatePaths(for: name) {
            if isExecutable(candidate) {
                return candidate
            }
        }

        if let path = try? runCapture("/usr/bin/env", ["which", name]).trimmingCharacters(in: .whitespacesAndNewlines),
           !path.isEmpty,
           isExecutable(path) {
            return path
        }

        throw RuntimeError.binaryNotFound(name)
    }

    private static func candidatePaths(for name: String) -> [String] {
        var paths: [String] = []
        let fm = FileManager.default

        if let resourceURL = Bundle.main.resourceURL {
            paths.append(resourceURL.appendingPathComponent("bin/\(name)").path)
            paths.append(resourceURL.appendingPathComponent(name).path)
        }
        if let executableURL = Bundle.main.executableURL {
            let dir = executableURL.deletingLastPathComponent()
            paths.append(dir.appendingPathComponent(name).path)
            paths.append(dir.appendingPathComponent("bin/\(name)").path)
        }

        let cwd = fm.currentDirectoryPath
        paths.append("\(cwd)/bin/\(name)")
        paths.append("\(cwd)/\(name)")

        paths.append("/opt/homebrew/bin/\(name)")
        paths.append("/usr/local/bin/\(name)")
        paths.append("/usr/bin/\(name)")

        if let pathEnv = ProcessInfo.processInfo.environment["PATH"] {
            for dir in pathEnv.split(separator: ":") where !dir.isEmpty {
                paths.append("\(dir)/\(name)")
            }
        }

        return deduplicated(paths)
    }

    private static func deduplicated(_ paths: [String]) -> [String] {
        var seen: Set<String> = []
        var out: [String] = []
        for path in paths {
            if seen.insert(path).inserted {
                out.append(path)
            }
        }
        return out
    }

    private static func isExecutable(_ path: String) -> Bool {
        let fm = FileManager.default
        if fm.isExecutableFile(atPath: path) {
            return true
        }
        guard fm.fileExists(atPath: path) else {
            return false
        }

        // Bundled resources can lose +x bit; try a safe chmod once.
        _ = try? fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: path)
        return fm.isExecutableFile(atPath: path)
    }
}

protocol TerminalDaemonTransport {
    func listCapabilities() async throws -> CapabilitiesEnvelope
    func terminalRead(target: String, paneID: String, cursor: String?, lines: Int) async throws -> TerminalReadEnvelope
    func terminalResize(target: String, paneID: String, cols: Int, rows: Int) async throws -> TerminalResizeResponse
    func terminalAttach(
        target: String,
        paneID: String,
        ifRuntime: String?,
        ifState: String?,
        ifUpdatedWithin: String?,
        forceStale: Bool
    ) async throws -> TerminalAttachResponse
    func terminalDetach(sessionID: String) async throws -> TerminalDetachResponse
    func terminalWrite(
        sessionID: String,
        text: String?,
        key: String?,
        bytes: [UInt8]?,
        enter: Bool,
        paste: Bool
    ) async throws -> TerminalWriteResponse
    func terminalStream(sessionID: String, cursor: String?, lines: Int) async throws -> TerminalStreamEnvelope
}

enum DaemonUnixClientError: Error {
    case unavailable(String)
    case invalidResponse(String)
    case status(path: String, statusCode: Int, code: String, message: String)

    var canFallbackToCLI: Bool {
        switch self {
        case .unavailable, .invalidResponse:
            return true
        case .status:
            return false
        }
    }

    func asRuntimeError(command: String) -> RuntimeError {
        switch self {
        case .unavailable(let message):
            return .commandFailed("daemon \(command)", 1, message)
        case .invalidResponse(let message):
            return .invalidJSON(message)
        case .status(_, let statusCode, let code, let message):
            let stderr = [code, message]
                .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
                .joined(separator: ": ")
            return .commandFailed("daemon \(command)", Int32(statusCode), stderr)
        }
    }
}

private struct DaemonAPIErrorEnvelope: Decodable {
    struct Item: Decodable {
        let code: String
        let message: String
    }

    let error: Item
}

private struct DaemonTerminalReadRequest: Encodable {
    let target: String
    let paneID: String
    let cursor: String?
    let lines: Int

    enum CodingKeys: String, CodingKey {
        case target
        case paneID = "pane_id"
        case cursor
        case lines
    }
}

private struct DaemonTerminalResizeRequest: Encodable {
    let target: String
    let paneID: String
    let cols: Int
    let rows: Int

    enum CodingKeys: String, CodingKey {
        case target
        case paneID = "pane_id"
        case cols
        case rows
    }
}

private struct DaemonTerminalAttachRequest: Encodable {
    let target: String
    let paneID: String
    let ifRuntime: String?
    let ifState: String?
    let ifUpdatedWithin: String?
    let forceStale: Bool

    enum CodingKeys: String, CodingKey {
        case target
        case paneID = "pane_id"
        case ifRuntime = "if_runtime"
        case ifState = "if_state"
        case ifUpdatedWithin = "if_updated_within"
        case forceStale = "force_stale"
    }
}

private struct DaemonTerminalDetachRequest: Encodable {
    let sessionID: String

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
    }
}

private struct DaemonTerminalWriteRequest: Encodable {
    let sessionID: String
    let text: String?
    let key: String?
    let bytesB64: String?
    let enter: Bool
    let paste: Bool

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case text
        case key
        case bytesB64 = "bytes_b64"
        case enter
        case paste
    }
}

actor DaemonUnixTerminalTransport: TerminalDaemonTransport {
    private let socketPath: String
    private let decoder = JSONDecoder()
    private let encoder = JSONEncoder()

    init(socketPath: String) {
        self.socketPath = socketPath
    }

    func listCapabilities() async throws -> CapabilitiesEnvelope {
        let body = try await request(method: "GET", path: "/v1/capabilities", queryItems: [], requestBody: Optional<Data>.none)
        return try decode(body, as: CapabilitiesEnvelope.self, context: "capabilities")
    }

    func terminalRead(target: String, paneID: String, cursor: String?, lines: Int) async throws -> TerminalReadEnvelope {
        let req = DaemonTerminalReadRequest(target: target, paneID: paneID, cursor: cursor, lines: lines)
        let body = try await request(method: "POST", path: "/v1/terminal/read", queryItems: [], requestBody: req)
        return try decode(body, as: TerminalReadEnvelope.self, context: "terminal/read")
    }

    func terminalResize(target: String, paneID: String, cols: Int, rows: Int) async throws -> TerminalResizeResponse {
        let req = DaemonTerminalResizeRequest(target: target, paneID: paneID, cols: cols, rows: rows)
        let body = try await request(method: "POST", path: "/v1/terminal/resize", queryItems: [], requestBody: req)
        return try decode(body, as: TerminalResizeResponse.self, context: "terminal/resize")
    }

    func terminalAttach(
        target: String,
        paneID: String,
        ifRuntime: String?,
        ifState: String?,
        ifUpdatedWithin: String?,
        forceStale: Bool
    ) async throws -> TerminalAttachResponse {
        let req = DaemonTerminalAttachRequest(
            target: target,
            paneID: paneID,
            ifRuntime: ifRuntime,
            ifState: ifState,
            ifUpdatedWithin: ifUpdatedWithin,
            forceStale: forceStale
        )
        let body = try await request(method: "POST", path: "/v1/terminal/attach", queryItems: [], requestBody: req)
        return try decode(body, as: TerminalAttachResponse.self, context: "terminal/attach")
    }

    func terminalDetach(sessionID: String) async throws -> TerminalDetachResponse {
        let req = DaemonTerminalDetachRequest(sessionID: sessionID)
        let body = try await request(method: "POST", path: "/v1/terminal/detach", queryItems: [], requestBody: req)
        return try decode(body, as: TerminalDetachResponse.self, context: "terminal/detach")
    }

    func terminalWrite(
        sessionID: String,
        text: String?,
        key: String?,
        bytes: [UInt8]?,
        enter: Bool,
        paste: Bool
    ) async throws -> TerminalWriteResponse {
        let bytesB64: String?
        if let bytes, !bytes.isEmpty {
            bytesB64 = Data(bytes).base64EncodedString()
        } else {
            bytesB64 = nil
        }
        let req = DaemonTerminalWriteRequest(
            sessionID: sessionID,
            text: text,
            key: key,
            bytesB64: bytesB64,
            enter: enter,
            paste: paste
        )
        let body = try await request(method: "POST", path: "/v1/terminal/write", queryItems: [], requestBody: req)
        return try decode(body, as: TerminalWriteResponse.self, context: "terminal/write")
    }

    func terminalStream(sessionID: String, cursor: String?, lines: Int) async throws -> TerminalStreamEnvelope {
        var items: [URLQueryItem] = [URLQueryItem(name: "session_id", value: sessionID)]
        if let cursor, !cursor.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            items.append(URLQueryItem(name: "cursor", value: cursor))
        }
        if lines > 0 {
            items.append(URLQueryItem(name: "lines", value: String(lines)))
        }
        let body = try await request(method: "GET", path: "/v1/terminal/stream", queryItems: items, requestBody: Optional<Data>.none)
        return try decode(body, as: TerminalStreamEnvelope.self, context: "terminal/stream")
    }

    private func decode<T: Decodable>(_ data: Data, as type: T.Type, context: String) throws -> T {
        do {
            return try decoder.decode(type, from: data)
        } catch {
            throw DaemonUnixClientError.invalidResponse("decode \(context) failed: \(error.localizedDescription)")
        }
    }

    private func request<Body: Encodable>(
        method: String,
        path: String,
        queryItems: [URLQueryItem],
        requestBody: Body?
    ) async throws -> Data {
        let bodyData: Data?
        if let requestBody {
            bodyData = try encoder.encode(requestBody)
        } else {
            bodyData = nil
        }
        let socketPath = self.socketPath
        return try await Task.detached(priority: .userInitiated) {
            try Self.requestBlocking(
                socketPath: socketPath,
                method: method,
                path: path,
                queryItems: queryItems,
                body: bodyData
            )
        }.value
    }

    private static func requestBlocking(
        socketPath: String,
        method: String,
        path: String,
        queryItems: [URLQueryItem],
        body: Data?
    ) throws -> Data {
        let fd = try openAndConnect(socketPath: socketPath)
        defer {
            Darwin.close(fd)
        }

        var target = path
        if !queryItems.isEmpty {
            var components = URLComponents()
            components.queryItems = queryItems
            if let encoded = components.percentEncodedQuery, !encoded.isEmpty {
                target += "?\(encoded)"
            }
        }

        var requestText = "\(method) \(target) HTTP/1.0\r\n"
        requestText += "Host: unix\r\n"
        requestText += "Accept: application/json\r\n"
        requestText += "Connection: close\r\n"
        if let body {
            requestText += "Content-Type: application/json\r\n"
            requestText += "Content-Length: \(body.count)\r\n"
        }
        requestText += "\r\n"

        var payload = Data(requestText.utf8)
        if let body {
            payload.append(body)
        }
        try writeAll(fd: fd, data: payload)
        let raw = try readAll(fd: fd)
        return try parseResponse(raw: raw, path: path)
    }

    private static func openAndConnect(socketPath: String) throws -> Int32 {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw DaemonUnixClientError.unavailable("open unix socket failed: \(errnoDescription())")
        }

        // Prevent SIGPIPE from terminating the app when daemon side closes
        // the socket during startup races. We handle EPIPE as a normal error.
        var noSigPipe: Int32 = 1
        _ = withUnsafePointer(to: &noSigPipe) { ptr in
            Darwin.setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, ptr, socklen_t(MemoryLayout<Int32>.size))
        }

        var addr = sockaddr_un()
        memset(&addr, 0, MemoryLayout<sockaddr_un>.size)
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = Array(socketPath.utf8CString)
        let maxPathBytes = MemoryLayout.size(ofValue: addr.sun_path)
        guard pathBytes.count <= maxPathBytes else {
            Darwin.close(fd)
            throw DaemonUnixClientError.unavailable("socket path too long")
        }

        let copyResult = withUnsafeMutablePointer(to: &addr.sun_path) { pathPtr in
            pathBytes.withUnsafeBufferPointer { pathBuffer in
                memcpy(pathPtr, pathBuffer.baseAddress, pathBytes.count)
            }
        }
        _ = copyResult

        let sockLen = socklen_t(MemoryLayout<sa_family_t>.size + pathBytes.count)
        let connectResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                Darwin.connect(fd, sockPtr, sockLen)
            }
        }
        guard connectResult == 0 else {
            let message = errnoDescription()
            Darwin.close(fd)
            throw DaemonUnixClientError.unavailable("connect unix socket failed: \(message)")
        }
        return fd
    }

    private static func writeAll(fd: Int32, data: Data) throws {
        try data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else {
                return
            }
            var sent = 0
            while sent < raw.count {
                let wrote = Darwin.write(fd, base.advanced(by: sent), raw.count - sent)
                if wrote < 0 {
                    if errno == EINTR {
                        continue
                    }
                    throw DaemonUnixClientError.unavailable("write failed: \(errnoDescription())")
                }
                sent += wrote
            }
        }
    }

    private static func readAll(fd: Int32) throws -> Data {
        var data = Data()
        var buf = [UInt8](repeating: 0, count: 8192)
        while true {
            let n = Darwin.read(fd, &buf, buf.count)
            if n == 0 {
                break
            }
            if n < 0 {
                if errno == EINTR {
                    continue
                }
                throw DaemonUnixClientError.unavailable("read failed: \(errnoDescription())")
            }
            data.append(buf, count: Int(n))
        }
        return data
    }

    private static func parseResponse(raw: Data, path: String) throws -> Data {
        let separator = Data("\r\n\r\n".utf8)
        guard let headerRange = raw.range(of: separator) else {
            throw DaemonUnixClientError.invalidResponse("response header delimiter missing")
        }
        let headerData = raw.subdata(in: raw.startIndex..<headerRange.lowerBound)
        let body = raw.subdata(in: headerRange.upperBound..<raw.endIndex)

        guard let headerText = String(data: headerData, encoding: .utf8) else {
            throw DaemonUnixClientError.invalidResponse("response header is not utf8")
        }
        let lines = headerText.components(separatedBy: "\r\n")
        guard let statusLine = lines.first else {
            throw DaemonUnixClientError.invalidResponse("missing status line")
        }
        let parts = statusLine.split(separator: " ", omittingEmptySubsequences: true)
        guard parts.count >= 2, let statusCode = Int(parts[1]) else {
            throw DaemonUnixClientError.invalidResponse("invalid status line: \(statusLine)")
        }

        if statusCode >= 400 {
            var errorCode = "http_error"
            var errorMessage = String(data: body, encoding: .utf8) ?? "request failed"
            if let parsed = try? JSONDecoder().decode(DaemonAPIErrorEnvelope.self, from: body) {
                if !parsed.error.code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    errorCode = parsed.error.code
                }
                if !parsed.error.message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    errorMessage = parsed.error.message
                }
            }
            throw DaemonUnixClientError.status(
                path: path,
                statusCode: statusCode,
                code: errorCode,
                message: errorMessage
            )
        }
        return body
    }

    private static func errnoDescription() -> String {
        String(cString: strerror(errno))
    }
}

struct AGTMUXCLIClient {
    private enum FallbackPolicy {
        case allow
        case deny
    }

    let socketPath: String
    let appBinaryPath: String
    private let daemonTransport: (any TerminalDaemonTransport)?
    private let commandRunner: @Sendable (String, [String]) throws -> String

    init(
        socketPath: String,
        appBinaryPath: String,
        daemonTransport: (any TerminalDaemonTransport)? = nil,
        commandRunner: @escaping @Sendable (String, [String]) throws -> String = { executable, args in
            try runCapture(executable, args)
        }
    ) {
        self.socketPath = socketPath
        self.appBinaryPath = appBinaryPath.trimmingCharacters(in: .whitespacesAndNewlines)
        self.daemonTransport = daemonTransport
        self.commandRunner = commandRunner
    }

    init(socketPath: String) throws {
        let appBinaryPath = try BinaryResolver.resolve(binary: "agtmux-app", envKey: "AGTMUX_APP_BIN")
        self.init(
            socketPath: socketPath,
            appBinaryPath: appBinaryPath,
            daemonTransport: DaemonUnixTerminalTransport(socketPath: socketPath)
        )
    }

    func fetchSnapshot() async throws -> DashboardSnapshot {
        async let targets = fetchTargets()
        async let sessions = fetchSessions()
        async let windows = fetchWindows()
        async let panes = fetchPanes()
        return try await DashboardSnapshot(
            targets: targets,
            sessions: sessions,
            windows: windows,
            panes: panes
        )
    }

    func fetchTargets() async throws -> [TargetItem] {
        let out = try await runAppCommand(["view", "targets", "--json"])
        return try decodeSingleJSONLine(out, as: TargetsEnvelope.self).targets
    }

    func fetchSessions() async throws -> [SessionItem] {
        let out = try await runAppCommand(["view", "sessions", "--json"])
        return try decodeSingleJSONLine(out, as: SessionEnvelope.self).items
    }

    func fetchWindows() async throws -> [WindowItem] {
        let out = try await runAppCommand(["view", "windows", "--json"])
        return try decodeSingleJSONLine(out, as: WindowEnvelope.self).items
    }

    func fetchPanes() async throws -> [PaneItem] {
        let out = try await runAppCommand(["view", "panes", "--json"])
        return try decodeSingleJSONLine(out, as: PaneEnvelope.self).items
    }

    func fetchCapabilities() async throws -> CapabilitiesEnvelope {
        try await withDaemonFallback(
            daemonCommand: "/v1/capabilities",
            daemonOperation: { transport in
                try await transport.listCapabilities()
            },
            fallback: {
                let out = try await runAppCommand(["terminal", "capabilities", "--json"])
                return try decodeSingleJSONLine(out, as: CapabilitiesEnvelope.self)
            }
        )
    }

    func terminalRead(target: String, paneID: String, cursor: String?, lines: Int) async throws -> TerminalReadEnvelope {
        try await withDaemonFallback(
            daemonCommand: "/v1/terminal/read",
            daemonOperation: { transport in
                try await transport.terminalRead(target: target, paneID: paneID, cursor: cursor, lines: lines)
            },
            fallback: {
                var args = [
                    "terminal", "read",
                    "--target", target,
                    "--pane", paneID,
                    "--lines", String(lines),
                    "--json",
                ]
                if let cursor, !cursor.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    args += ["--cursor", cursor.trimmingCharacters(in: .whitespacesAndNewlines)]
                }
                let out = try await runAppCommand(args)
                return try decodeSingleJSONLine(out, as: TerminalReadEnvelope.self)
            }
        )
    }

    func terminalResize(target: String, paneID: String, cols: Int, rows: Int) async throws -> TerminalResizeResponse {
        try await withDaemonFallback(
            daemonCommand: "/v1/terminal/resize",
            daemonOperation: { transport in
                try await transport.terminalResize(target: target, paneID: paneID, cols: cols, rows: rows)
            },
            fallback: {
                let args = [
                    "terminal", "resize",
                    "--target", target,
                    "--pane", paneID,
                    "--cols", String(cols),
                    "--rows", String(rows),
                    "--json",
                ]
                let out = try await runAppCommand(args)
                return try decodeSingleJSONLine(out, as: TerminalResizeResponse.self)
            }
        )
    }

    func terminalAttach(
        target: String,
        paneID: String,
        ifRuntime: String? = nil,
        ifState: String? = nil,
        ifUpdatedWithin: String? = nil,
        forceStale: Bool = false
    ) async throws -> TerminalAttachResponse {
        try await withDaemonFallback(
            daemonCommand: "/v1/terminal/attach",
            fallbackPolicy: .deny,
            daemonOperation: { transport in
                try await transport.terminalAttach(
                    target: target,
                    paneID: paneID,
                    ifRuntime: ifRuntime,
                    ifState: ifState,
                    ifUpdatedWithin: ifUpdatedWithin,
                    forceStale: forceStale
                )
            },
            fallback: {
                var args = [
                    "terminal", "attach",
                    "--target", target,
                    "--pane", paneID,
                    "--json",
                ]
                if let ifRuntime, !ifRuntime.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    args += ["--if-runtime", ifRuntime.trimmingCharacters(in: .whitespacesAndNewlines)]
                }
                if let ifState, !ifState.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    args += ["--if-state", ifState.trimmingCharacters(in: .whitespacesAndNewlines)]
                }
                if let ifUpdatedWithin, !ifUpdatedWithin.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    args += ["--if-updated-within", ifUpdatedWithin.trimmingCharacters(in: .whitespacesAndNewlines)]
                }
                if forceStale {
                    args.append("--force-stale")
                }
                let out = try await runAppCommand(args)
                return try decodeSingleJSONLine(out, as: TerminalAttachResponse.self)
            }
        )
    }

    func terminalDetach(sessionID: String) async throws -> TerminalDetachResponse {
        try await withDaemonFallback(
            daemonCommand: "/v1/terminal/detach",
            fallbackPolicy: .deny,
            daemonOperation: { transport in
                try await transport.terminalDetach(sessionID: sessionID)
            },
            fallback: {
                let out = try await runAppCommand([
                    "terminal", "detach",
                    "--session", sessionID,
                    "--json",
                ])
                return try decodeSingleJSONLine(out, as: TerminalDetachResponse.self)
            }
        )
    }

    func terminalWrite(
        sessionID: String,
        text: String? = nil,
        key: String? = nil,
        bytes: [UInt8]? = nil,
        enter: Bool = false,
        paste: Bool = false
    ) async throws -> TerminalWriteResponse {
        let hasText = (text?.isEmpty == false)
        let normalizedKey = key?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let hasKey = !normalizedKey.isEmpty
        let hasBytes = (bytes?.isEmpty == false)
        let modeCount = (hasText ? 1 : 0) + (hasKey ? 1 : 0) + (hasBytes ? 1 : 0)
        if modeCount != 1 {
            throw RuntimeError.commandFailed(
                "agtmux-app terminal write",
                2,
                "exactly one of text, key, or bytes must be set"
            )
        }
        return try await withDaemonFallback(
            daemonCommand: "/v1/terminal/write",
            fallbackPolicy: .deny,
            daemonOperation: { transport in
                try await transport.terminalWrite(
                    sessionID: sessionID,
                    text: text,
                    key: hasKey ? normalizedKey : nil,
                    bytes: hasBytes ? bytes : nil,
                    enter: enter,
                    paste: paste
                )
            },
            fallback: {
                var args = [
                    "terminal", "write",
                    "--session", sessionID,
                    "--json",
                ]
                if let text, !text.isEmpty {
                    args += ["--text", text]
                } else if hasKey {
                    args += ["--key", normalizedKey]
                } else if let bytes, !bytes.isEmpty {
                    args += ["--bytes-b64", Data(bytes).base64EncodedString()]
                }
                if enter {
                    args.append("--enter")
                }
                if paste {
                    args.append("--paste")
                }
                let out = try await runAppCommand(args)
                return try decodeSingleJSONLine(out, as: TerminalWriteResponse.self)
            }
        )
    }

    func terminalStream(sessionID: String, cursor: String?, lines: Int) async throws -> TerminalStreamEnvelope {
        try await withDaemonFallback(
            daemonCommand: "/v1/terminal/stream",
            daemonOperation: { transport in
                try await transport.terminalStream(sessionID: sessionID, cursor: cursor, lines: lines)
            },
            fallback: {
                var args = [
                    "terminal", "stream",
                    "--session", sessionID,
                    "--lines", String(lines),
                    "--json",
                ]
                if let cursor, !cursor.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    args += ["--cursor", cursor.trimmingCharacters(in: .whitespacesAndNewlines)]
                }
                let out = try await runAppCommand(args)
                return try decodeSingleJSONLine(out, as: TerminalStreamEnvelope.self)
            }
        )
    }

    func sendText(
        target: String,
        paneID: String,
        text: String,
        requestRef: String,
        enter: Bool,
        paste: Bool,
        ifRuntime: String? = nil,
        ifState: String? = nil,
        ifUpdatedWithin: String? = nil,
        forceStale: Bool = false
    ) async throws -> ActionResponse {
        var args = [
            "action", "send",
            "--request-ref", requestRef,
            "--target", target,
            "--pane", paneID,
            "--text", text,
            "--json",
        ]
        if enter {
            args.append("--enter")
        }
        if paste {
            args.append("--paste")
        }
        if let ifRuntime, !ifRuntime.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            args += ["--if-runtime", ifRuntime.trimmingCharacters(in: .whitespacesAndNewlines)]
        }
        if let ifState, !ifState.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            args += ["--if-state", ifState.trimmingCharacters(in: .whitespacesAndNewlines)]
        }
        if let ifUpdatedWithin, !ifUpdatedWithin.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            args += ["--if-updated-within", ifUpdatedWithin.trimmingCharacters(in: .whitespacesAndNewlines)]
        }
        if forceStale {
            args.append("--force-stale")
        }
        let out = try await runAppCommand(args)
        return try decodeSingleJSONLine(out, as: ActionResponse.self)
    }

    func viewOutput(target: String, paneID: String, requestRef: String, lines: Int) async throws -> ActionResponse {
        let args = [
            "action", "view-output",
            "--request-ref", requestRef,
            "--target", target,
            "--pane", paneID,
            "--lines", String(lines),
            "--json",
        ]
        let out = try await runAppCommand(args)
        return try decodeSingleJSONLine(out, as: ActionResponse.self)
    }

    func kill(
        target: String,
        paneID: String,
        requestRef: String,
        mode: String,
        signal: String,
        ifRuntime: String? = nil,
        ifState: String? = nil,
        ifUpdatedWithin: String? = nil,
        forceStale: Bool = false
    ) async throws -> ActionResponse {
        let args = [
            "action", "kill",
            "--request-ref", requestRef,
            "--target", target,
            "--pane", paneID,
            "--mode", mode,
            "--signal", signal,
            "--json",
        ]
        var finalArgs = args
        if let ifRuntime, !ifRuntime.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            finalArgs += ["--if-runtime", ifRuntime.trimmingCharacters(in: .whitespacesAndNewlines)]
        }
        if let ifState, !ifState.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            finalArgs += ["--if-state", ifState.trimmingCharacters(in: .whitespacesAndNewlines)]
        }
        if let ifUpdatedWithin, !ifUpdatedWithin.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            finalArgs += ["--if-updated-within", ifUpdatedWithin.trimmingCharacters(in: .whitespacesAndNewlines)]
        }
        if forceStale {
            finalArgs.append("--force-stale")
        }
        let out = try await runAppCommand(finalArgs)
        return try decodeSingleJSONLine(out, as: ActionResponse.self)
    }

    func healthcheck() async -> Bool {
        do {
            _ = try await runAppCommand(["view", "targets", "--json"])
            return true
        } catch {
            return false
        }
    }

    private func runAppCommand(_ args: [String]) async throws -> String {
        let allArgs = ["--socket", socketPath] + args
        let appBinaryPath = self.appBinaryPath
        let commandRunner = self.commandRunner
        return try await Task.detached(priority: .userInitiated) {
            try commandRunner(appBinaryPath, allArgs)
        }.value
    }

    private func withDaemonFallback<T>(
        daemonCommand: String,
        fallbackPolicy: FallbackPolicy = .allow,
        daemonOperation: @escaping (any TerminalDaemonTransport) async throws -> T,
        fallback: @escaping () async throws -> T
    ) async throws -> T {
        guard let daemonTransport else {
            return try await fallback()
        }
        do {
            return try await daemonOperation(daemonTransport)
        } catch let daemonError as DaemonUnixClientError {
            if fallbackPolicy == .allow && daemonError.canFallbackToCLI {
                return try await fallback()
            }
            throw daemonError.asRuntimeError(command: daemonCommand)
        } catch {
            if fallbackPolicy == .allow {
                return try await fallback()
            }
            throw error
        }
    }
}

final class DaemonManager {
    let socketPath: String
    let dbPath: String
    let logPath: String

    private let daemonBinaryPath: String
    private let launcherLogPath: String
    private var ownedProcess: Process?
    private var logHandle: FileHandle?
    private var lastBackupAt: Date?
    private var lastBackedUpDBModificationAt: Date?
    private let backupIntervalSeconds: TimeInterval = 300
    private let maxBackupFiles = 12

    init(
        socketPath: String,
        dbPath: String,
        logPath: String,
        daemonBinaryPath: String? = nil
    ) throws {
        self.socketPath = socketPath
        self.dbPath = dbPath
        self.logPath = logPath
        if let daemonBinaryPath, !daemonBinaryPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            self.daemonBinaryPath = daemonBinaryPath.trimmingCharacters(in: .whitespacesAndNewlines)
        } else {
            self.daemonBinaryPath = try BinaryResolver.resolve(binary: "agtmuxd", envKey: "AGTMUX_DAEMON_BIN")
        }
        let supportDir = URL(fileURLWithPath: logPath).deletingLastPathComponent()
        self.launcherLogPath = supportDir.appendingPathComponent("launcher.log", isDirectory: false).path
    }

    deinit {
        stopIfOwned()
    }

    func ensureRunning(with client: AGTMUXCLIClient) async throws {
        if await client.healthcheck() {
            return
        }

        if let process = ownedProcess, !process.isRunning {
            ownedProcess = nil
            closeLogHandle()
        }

        let fm = FileManager.default
        let socketURL = URL(fileURLWithPath: socketPath)
        let dbURL = URL(fileURLWithPath: dbPath)
        let logURL = URL(fileURLWithPath: logPath)

        try fm.createDirectory(at: dbURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try fm.createDirectory(at: logURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        backupStateDBIfNeeded(fileManager: fm, dbURL: dbURL)
        _ = try? fm.removeItem(at: socketURL)
        if fm.fileExists(atPath: logURL.path) == false {
            _ = fm.createFile(atPath: logURL.path, contents: Data())
        }
        if fm.fileExists(atPath: launcherLogPath) == false {
            _ = fm.createFile(atPath: launcherLogPath, contents: Data())
        }

        appendLauncherLog("ensureRunning: start requested socket=\(socketPath)")
        do {
            try startOwnedDaemonProcess(logURL: logURL)
        } catch {
            appendLauncherLog("ensureRunning: owned start failed: \(error.localizedDescription)")
        }

        if await waitForHealthcheck(client: client, maxAttempts: 50, intervalMillis: 100) {
            appendLauncherLog("ensureRunning: healthy via owned process")
            return
        }

        stopIfOwned()
        appendLauncherLog("ensureRunning: owned process did not become healthy, trying detached fallback")
        do {
            try startDetachedDaemonProcess(logURL: logURL)
        } catch {
            appendLauncherLog("ensureRunning: detached fallback failed: \(error.localizedDescription)")
            throw error
        }

        if await waitForHealthcheck(client: client, maxAttempts: 60, intervalMillis: 100) {
            appendLauncherLog("ensureRunning: healthy via detached process")
            return
        }
        appendLauncherLog("ensureRunning: timeout waiting healthcheck")
        throw RuntimeError.daemonStartTimeout(socketPath)
    }

    func restart(with client: AGTMUXCLIClient) async throws {
        stopIfOwned()
        try await ensureRunning(with: client)
    }

    func stopIfOwned() {
        guard let process = ownedProcess else {
            return
        }
        if process.isRunning {
            process.terminate()
            process.waitUntilExit()
        }
        ownedProcess = nil
        closeLogHandle()
    }

    private func closeLogHandle() {
        logHandle?.closeFile()
        logHandle = nil
    }

    private func startOwnedDaemonProcess(logURL: URL) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: daemonBinaryPath)
        process.arguments = ["--socket", socketPath, "--db", dbPath]
        process.environment = daemonLaunchEnvironment()
        process.standardInput = nil
        let handle = try FileHandle(forWritingTo: logURL)
        handle.seekToEndOfFile()
        process.standardOutput = handle
        process.standardError = handle
        try process.run()
        ownedProcess = process
        logHandle = handle
        appendLauncherLog("startOwned: pid=\(process.processIdentifier)")
    }

    private func startDetachedDaemonProcess(logURL: URL) throws {
        let escapedBin = shellEscape(daemonBinaryPath)
        let escapedSocket = shellEscape(socketPath)
        let escapedDB = shellEscape(dbPath)
        let escapedLog = shellEscape(logURL.path)
        let command = "nohup \(escapedBin) --socket \(escapedSocket) --db \(escapedDB) >> \(escapedLog) 2>&1 &"
        let result = try runCommand("/bin/sh", ["-lc", command], environment: daemonLaunchEnvironment())
        guard result.status == 0 else {
            throw RuntimeError.commandFailed("/bin/sh -lc \(command)", result.status, result.stderr)
        }
        appendLauncherLog("startDetached: dispatched command")
    }

    private func waitForHealthcheck(client: AGTMUXCLIClient, maxAttempts: Int, intervalMillis: Int) async -> Bool {
        guard maxAttempts > 0 else {
            return await client.healthcheck()
        }
        for _ in 0..<maxAttempts {
            if await client.healthcheck() {
                return true
            }
            try? await Task.sleep(for: .milliseconds(intervalMillis))
        }
        return false
    }

    private func appendLauncherLog(_ message: String) {
        let line = "\(Self.launcherTimestampFormatter.string(from: Date())) \(message)\n"
        if let data = line.data(using: .utf8) {
            if FileManager.default.fileExists(atPath: launcherLogPath) == false {
                _ = FileManager.default.createFile(atPath: launcherLogPath, contents: Data())
            }
            if let handle = try? FileHandle(forWritingTo: URL(fileURLWithPath: launcherLogPath)) {
                handle.seekToEndOfFile()
                handle.write(data)
                try? handle.close()
            }
        }
    }

    private func backupStateDBIfNeeded(fileManager fm: FileManager, dbURL: URL) {
        guard fm.fileExists(atPath: dbURL.path) else {
            return
        }
        let now = Date()
        if let last = lastBackupAt, now.timeIntervalSince(last) < backupIntervalSeconds {
            return
        }

        let attrs = (try? fm.attributesOfItem(atPath: dbURL.path)) ?? [:]
        let mtime = attrs[.modificationDate] as? Date
        if let previous = lastBackedUpDBModificationAt, let current = mtime, current <= previous {
            return
        }

        let backupDir = dbURL.deletingLastPathComponent().appendingPathComponent("backups", isDirectory: true)
        do {
            try fm.createDirectory(at: backupDir, withIntermediateDirectories: true)
        } catch {
            return
        }

        let stamp = Self.backupTimestampFormatter.string(from: now)
        var backupURL = backupDir.appendingPathComponent("state-\(stamp).db", isDirectory: false)
        if fm.fileExists(atPath: backupURL.path) {
            backupURL = backupDir.appendingPathComponent("state-\(stamp)-\(UUID().uuidString.prefix(6)).db", isDirectory: false)
        }

        do {
            try fm.copyItem(at: dbURL, to: backupURL)
            lastBackupAt = now
            lastBackedUpDBModificationAt = mtime
            pruneBackups(fileManager: fm, backupDir: backupDir)
        } catch {
            // Backup should never block daemon recovery.
        }
    }

    private func pruneBackups(fileManager fm: FileManager, backupDir: URL) {
        guard let entries = try? fm.contentsOfDirectory(at: backupDir, includingPropertiesForKeys: [.creationDateKey], options: [.skipsHiddenFiles]) else {
            return
        }
        let backups = entries.filter { $0.lastPathComponent.hasPrefix("state-") && $0.pathExtension == "db" }
        if backups.count <= maxBackupFiles {
            return
        }
        let sorted = backups.sorted { lhs, rhs in
            let la = (try? lhs.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
            let ra = (try? rhs.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
            if la != ra {
                return la > ra
            }
            return lhs.lastPathComponent > rhs.lastPathComponent
        }
        for stale in sorted.dropFirst(maxBackupFiles) {
            try? fm.removeItem(at: stale)
        }
    }

    private static let backupTimestampFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        return formatter
    }()

    private static let launcherTimestampFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "yyyy-MM-dd'T'HH:mm:ss.SSS'Z'"
        return formatter
    }()

    private func daemonLaunchEnvironment() -> [String: String] {
        var environment = ProcessInfo.processInfo.environment
        let fm = FileManager.default
        let home = fm.homeDirectoryForCurrentUser.path
        if (environment["HOME"] ?? "").isEmpty {
            environment["HOME"] = home
        }

        var pathEntries: [String] = []
        if let current = environment["PATH"] {
            pathEntries += current
                .split(separator: ":")
                .map { String($0) }
                .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        }
        pathEntries += [
            "\(home)/.local/bin",
            "\(home)/.nix-profile/bin",
            "\(home)/.cargo/bin",
            "\(home)/go/bin",
            "/opt/homebrew/bin",
            "/opt/homebrew/sbin",
            "/usr/local/bin",
            "/usr/local/sbin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
            "/Library/Apple/usr/bin",
        ]

        var deduped: [String] = []
        var seen: Set<String> = []
        for raw in pathEntries {
            let path = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            if path.isEmpty {
                continue
            }
            if seen.insert(path).inserted {
                deduped.append(path)
            }
        }
        environment["PATH"] = deduped.joined(separator: ":")
        return environment
    }
}

private struct CommandResult {
    let stdout: String
    let stderr: String
    let status: Int32
}

private func runCapture(_ executable: String, _ args: [String], environment: [String: String]? = nil) throws -> String {
    let result = try runCommand(executable, args, environment: environment)
    guard result.status == 0 else {
        let cmd = ([executable] + args).joined(separator: " ")
        throw RuntimeError.commandFailed(cmd, result.status, result.stderr)
    }
    return result.stdout
}

private func runCommand(_ executable: String, _ args: [String], environment: [String: String]? = nil) throws -> CommandResult {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = args
    process.environment = environment

    let outPipe = Pipe()
    let errPipe = Pipe()
    process.standardOutput = outPipe
    process.standardError = errPipe

    try process.run()
    process.waitUntilExit()

    let stdoutData = outPipe.fileHandleForReading.readDataToEndOfFile()
    let stderrData = errPipe.fileHandleForReading.readDataToEndOfFile()
    let stdout = String(data: stdoutData, encoding: .utf8) ?? ""
    let stderr = String(data: stderrData, encoding: .utf8) ?? ""

    return CommandResult(stdout: stdout, stderr: stderr, status: process.terminationStatus)
}

private func shellEscape(_ raw: String) -> String {
    let escaped = raw.replacingOccurrences(of: "'", with: "'\"'\"'")
    return "'\(escaped)'"
}

private func decodeSingleJSONLine<T: Decodable>(_ raw: String, as type: T.Type) throws -> T {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else {
        throw RuntimeError.invalidJSON("empty output")
    }

    let decoder = JSONDecoder()
    if let data = trimmed.data(using: .utf8), let value = try? decoder.decode(type, from: data) {
        return value
    }

    var lastError: String = ""
    for line in trimmed.split(separator: "\n") {
        let candidate = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !candidate.isEmpty else {
            continue
        }
        guard candidate.first == "{" || candidate.first == "[" else {
            continue
        }
        guard let data = candidate.data(using: .utf8) else {
            continue
        }
        do {
            return try decoder.decode(type, from: data)
        } catch {
            lastError = error.localizedDescription
        }
    }

    if lastError.isEmpty {
        lastError = "no decodable JSON line found"
    }
    throw RuntimeError.invalidJSON(lastError)
}
