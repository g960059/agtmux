import Foundation
import Darwin

enum TTYV2SessionError: LocalizedError {
    case unavailable(String)
    case protocolError(String)
    case closed

    var errorDescription: String? {
        switch self {
        case .unavailable(let message):
            return "tty-v2 unavailable: \(message)"
        case .protocolError(let message):
            return "tty-v2 protocol error: \(message)"
        case .closed:
            return "tty-v2 session closed"
        }
    }
}

struct TTYV2PaneRef: Codable, Equatable, Sendable {
    let target: String
    let sessionName: String
    let windowID: String
    let paneID: String

    enum CodingKeys: String, CodingKey {
        case target
        case sessionName = "session_name"
        case windowID = "window_id"
        case paneID = "pane_id"
    }
}

struct TTYV2Frame: Sendable {
    let schemaVersion: String
    let type: String
    let frameSeq: UInt64
    let sentAt: String?
    let requestID: String?
    let payloadData: Data

    func decodePayload<T: Decodable>(_ type: T.Type) throws -> T {
        try JSONDecoder().decode(type, from: payloadData)
    }
}

struct TTYV2StatePayload: Decodable, Sendable {
    struct State: Decodable, Sendable {
        let activityState: String?
        let attentionState: String?
        let sessionLastActiveAt: String?

        enum CodingKeys: String, CodingKey {
            case activityState = "activity_state"
            case attentionState = "attention_state"
            case sessionLastActiveAt = "session_last_active_at"
        }
    }

    let paneRef: TTYV2PaneRef
    let state: State

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
        case state
    }
}

struct TTYV2AttachedPayload: Decodable, Sendable {
    let paneRef: TTYV2PaneRef
    let paneAlias: String?
    let outputSeq: UInt64?
    let initialSnapshotANSIBase64: String?
    let snapshotMode: String?
    let cursorX: Int?
    let cursorY: Int?
    let paneCols: Int?
    let paneRows: Int?
    let state: TTYV2StatePayload.State?

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
        case paneAlias = "pane_alias"
        case outputSeq = "output_seq"
        case initialSnapshotANSIBase64 = "initial_snapshot_ansi_base64"
        case snapshotMode = "snapshot_mode"
        case cursorX = "cursor_x"
        case cursorY = "cursor_y"
        case paneCols = "pane_cols"
        case paneRows = "pane_rows"
        case state
    }
}

struct TTYV2OutputPayload: Decodable, Sendable {
    let paneRef: TTYV2PaneRef?
    let paneAlias: String?
    let outputSeq: UInt64?
    let bytesBase64: String
    let source: String?
    let cursorX: Int?
    let cursorY: Int?
    let paneCols: Int?
    let paneRows: Int?
    let coalesced: Bool?
    let droppedChunks: Int?

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
        case paneAlias = "pane_alias"
        case outputSeq = "output_seq"
        case bytesBase64 = "bytes_base64"
        case source
        case cursorX = "cursor_x"
        case cursorY = "cursor_y"
        case paneCols = "pane_cols"
        case paneRows = "pane_rows"
        case coalesced
        case droppedChunks = "dropped_chunks"
    }
}

struct TTYV2AckPayload: Decodable, Sendable {
    let paneRef: TTYV2PaneRef?
    let ackKind: String
    let inputSeq: UInt64?
    let resizeSeq: UInt64?
    let resultCode: String

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
        case ackKind = "ack_kind"
        case inputSeq = "input_seq"
        case resizeSeq = "resize_seq"
        case resultCode = "result_code"
    }
}

struct TTYV2ErrorPayload: Decodable, Sendable {
    let code: String
    let message: String
    let recoverable: Bool
    let paneRef: TTYV2PaneRef?

    enum CodingKeys: String, CodingKey {
        case code
        case message
        case recoverable
        case paneRef = "pane_ref"
    }
}

private struct TTYV2HelloPayload: Encodable {
    let clientID: String
    let protocolVersions: [String]
    let capabilities: [String]

    enum CodingKeys: String, CodingKey {
        case clientID = "client_id"
        case protocolVersions = "protocol_versions"
        case capabilities
    }
}

private struct TTYV2AttachPayload: Encodable {
    let paneRef: TTYV2PaneRef
    let attachMode: String
    let wantInitialSnapshot: Bool
    let cols: Int?
    let rows: Int?

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
        case attachMode = "attach_mode"
        case wantInitialSnapshot = "want_initial_snapshot"
        case cols
        case rows
    }
}

private struct TTYV2WritePayload: Encodable {
    let paneRef: TTYV2PaneRef
    let inputSeq: UInt64
    let bytesBase64: String

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
        case inputSeq = "input_seq"
        case bytesBase64 = "bytes_base64"
    }
}

private struct TTYV2ResizePayload: Encodable {
    let paneRef: TTYV2PaneRef
    let resizeSeq: UInt64
    let cols: Int
    let rows: Int

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
        case resizeSeq = "resize_seq"
        case cols
        case rows
    }
}

private struct TTYV2FocusPayload: Encodable {
    let paneRef: TTYV2PaneRef

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
    }
}

private struct TTYV2DetachPayload: Encodable {
    let paneRef: TTYV2PaneRef

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
    }
}

private struct TTYV2ResyncPayload: Encodable {
    let paneRef: TTYV2PaneRef
    let reason: String

    enum CodingKeys: String, CodingKey {
        case paneRef = "pane_ref"
        case reason
    }
}

actor TTYV2TransportSession {
    private static let schemaVersion = "tty.v2.0"
    private static let upgradeToken = "agtmux-tty-v2"
    private static let maxFrameBytes = 1 << 20
    private static let iso8601: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    private let socketPath: String
    private let encoder = JSONEncoder()

    private var fd: Int32 = -1
    private var nextFrameSeq: UInt64 = 1
    private var nextInputSeq: UInt64 = 1
    private var nextResizeSeq: UInt64 = 1
    private var readTask: Task<Void, Never>?
    private var continuation: AsyncThrowingStream<TTYV2Frame, Error>.Continuation?
    private var isClosing = false

    init(socketPath: String) {
        self.socketPath = socketPath
    }

    func open(clientID: String = "AGTMUXDesktop") throws -> AsyncThrowingStream<TTYV2Frame, Error> {
        guard fd < 0 else {
            throw TTYV2SessionError.protocolError("session already open")
        }
        isClosing = false
        let connectedFD = try Self.openAndUpgrade(socketPath: socketPath)
        fd = connectedFD

        var capturedContinuation: AsyncThrowingStream<TTYV2Frame, Error>.Continuation?
        let stream = AsyncThrowingStream<TTYV2Frame, Error> { continuation in
            capturedContinuation = continuation
        }
        continuation = capturedContinuation

        readTask = Task.detached(priority: .userInitiated) { [fd = connectedFD] in
            do {
                while !Task.isCancelled {
                    let frame = try Self.readFrame(fd: fd)
                    await self.emit(frame: frame)
                }
            } catch {
                await self.finish(with: error)
            }
        }

        try send(
            frameType: "hello",
            requestID: UUID().uuidString,
            payload: TTYV2HelloPayload(
                clientID: clientID,
                protocolVersions: [Self.schemaVersion],
                capabilities: ["raw_output", "resync"]
            )
        )
        return stream
    }

    func close() {
        closeInternal(throwing: nil)
    }

    func attach(
        paneRef: TTYV2PaneRef,
        cols: Int? = nil,
        rows: Int? = nil,
        requestID: String = UUID().uuidString
    ) throws {
        try send(
            frameType: "attach",
            requestID: requestID,
            payload: TTYV2AttachPayload(
                paneRef: paneRef,
                attachMode: "exclusive",
                // Phase 27 stream-only path:
                // do not request capture-pane snapshot for selected pane.
                wantInitialSnapshot: false,
                cols: cols,
                rows: rows
            )
        )
    }

    func focus(paneRef: TTYV2PaneRef, requestID: String = UUID().uuidString) throws {
        try send(
            frameType: "focus",
            requestID: requestID,
            payload: TTYV2FocusPayload(paneRef: paneRef)
        )
    }

    func detach(paneRef: TTYV2PaneRef, requestID: String = UUID().uuidString) throws {
        try send(
            frameType: "detach",
            requestID: requestID,
            payload: TTYV2DetachPayload(paneRef: paneRef)
        )
    }

    func resync(paneRef: TTYV2PaneRef, reason: String = "manual", requestID: String = UUID().uuidString) throws {
        try send(
            frameType: "resync",
            requestID: requestID,
            payload: TTYV2ResyncPayload(paneRef: paneRef, reason: reason)
        )
    }

    @discardableResult
    func write(paneRef: TTYV2PaneRef, bytes: [UInt8], requestID: String = UUID().uuidString) throws -> UInt64 {
        guard !bytes.isEmpty else {
            throw TTYV2SessionError.protocolError("write bytes are empty")
        }
        let seq = nextInputSeq
        nextInputSeq += 1
        try send(
            frameType: "write",
            requestID: requestID,
            payload: TTYV2WritePayload(
                paneRef: paneRef,
                inputSeq: seq,
                bytesBase64: Data(bytes).base64EncodedString()
            )
        )
        return seq
    }

    @discardableResult
    func resize(paneRef: TTYV2PaneRef, cols: Int, rows: Int, requestID: String = UUID().uuidString) throws -> UInt64 {
        guard cols > 0, rows > 0 else {
            throw TTYV2SessionError.protocolError("resize cols/rows must be positive")
        }
        let seq = nextResizeSeq
        nextResizeSeq += 1
        try send(
            frameType: "resize",
            requestID: requestID,
            payload: TTYV2ResizePayload(
                paneRef: paneRef,
                resizeSeq: seq,
                cols: cols,
                rows: rows
            )
        )
        return seq
    }

    private func send<T: Encodable>(frameType: String, requestID: String, payload: T) throws {
        guard fd >= 0 else {
            throw TTYV2SessionError.closed
        }
        let frameBody = try encodeFrame(frameType: frameType, requestID: requestID, payload: payload)
        try Self.writeLengthPrefixed(fd: fd, body: frameBody)
    }

    private func encodeFrame<T: Encodable>(frameType: String, requestID: String, payload: T) throws -> Data {
        let payloadData = try encoder.encode(payload)
        let payloadJSON = try JSONSerialization.jsonObject(with: payloadData, options: [])
        var envelope: [String: Any] = [
            "schema_version": Self.schemaVersion,
            "type": frameType,
            "frame_seq": nextFrameSeq,
            "sent_at": Self.iso8601.string(from: Date()),
            "payload": payloadJSON,
        ]
        nextFrameSeq += 1
        let trimmedRequestID = requestID.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmedRequestID.isEmpty {
            envelope["request_id"] = trimmedRequestID
        }
        guard JSONSerialization.isValidJSONObject(envelope) else {
            throw TTYV2SessionError.protocolError("invalid tty-v2 envelope")
        }
        return try JSONSerialization.data(withJSONObject: envelope, options: [])
    }

    private func emit(frame: TTYV2Frame) {
        continuation?.yield(frame)
    }

    private func finish(with error: Error?) {
        let finalError: Error? = isClosing ? nil : error
        closeInternal(throwing: finalError)
    }

    private func closeInternal(throwing error: Error?) {
        if isClosing, error == nil {
            return
        }
        isClosing = true
        readTask?.cancel()
        readTask = nil
        if fd >= 0 {
            Darwin.close(fd)
            fd = -1
        }
        if let error {
            continuation?.finish(throwing: error)
        } else {
            continuation?.finish()
        }
        continuation = nil
    }

    private static func openAndUpgrade(socketPath: String) throws -> Int32 {
        let fd = try openAndConnect(socketPath: socketPath)
        do {
            try performUpgrade(fd: fd)
            return fd
        } catch {
            Darwin.close(fd)
            throw error
        }
    }

    private static func openAndConnect(socketPath: String) throws -> Int32 {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw TTYV2SessionError.unavailable("open unix socket failed: \(errnoDescription())")
        }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let maxPathCount = MemoryLayout.size(ofValue: addr.sun_path)
        var pathBytes = Array(socketPath.utf8)
        if pathBytes.count >= maxPathCount {
            Darwin.close(fd)
            throw TTYV2SessionError.unavailable("socket path too long")
        }
        pathBytes.append(0)
        withUnsafeMutableBytes(of: &addr.sun_path) { dest in
            dest.copyBytes(from: pathBytes)
        }

        let len = socklen_t(MemoryLayout.size(ofValue: addr))
        let result = withUnsafePointer(to: &addr) { ptr -> Int32 in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPtr in
                Darwin.connect(fd, sockaddrPtr, len)
            }
        }
        if result != 0 {
            let message = errnoDescription()
            Darwin.close(fd)
            throw TTYV2SessionError.unavailable("connect unix socket failed: \(message)")
        }
        return fd
    }

    private static func performUpgrade(fd: Int32) throws {
        var request = "GET /v2/tty/session HTTP/1.1\r\n"
        request += "Host: unix\r\n"
        request += "Connection: Upgrade\r\n"
        request += "Upgrade: \(upgradeToken)\r\n"
        request += "\r\n"
        try writeAll(fd: fd, data: Data(request.utf8))

        let header = try readHTTPHeader(fd: fd, maxBytes: 64 * 1024)
        guard header.hasPrefix("HTTP/1.1 101 ") || header.hasPrefix("HTTP/1.0 101 ") else {
            throw TTYV2SessionError.protocolError("upgrade failed: \(header.split(separator: "\r\n").first ?? "")")
        }
    }

    private static func readHTTPHeader(fd: Int32, maxBytes: Int) throws -> String {
        var data = Data()
        var byte = [UInt8](repeating: 0, count: 1)
        while data.count < maxBytes {
            let n = Darwin.read(fd, &byte, 1)
            if n == 0 {
                break
            }
            if n < 0 {
                if errno == EINTR {
                    continue
                }
                throw TTYV2SessionError.unavailable("read upgrade response failed: \(errnoDescription())")
            }
            data.append(byte[0])
            if data.count >= 4 && data.suffix(4) == Data([13, 10, 13, 10]) {
                break
            }
        }
        guard let text = String(data: data, encoding: .utf8), !text.isEmpty else {
            throw TTYV2SessionError.protocolError("empty upgrade response")
        }
        return text
    }

    private static func writeLengthPrefixed(fd: Int32, body: Data) throws {
        guard !body.isEmpty else {
            throw TTYV2SessionError.protocolError("frame body empty")
        }
        if body.count > maxFrameBytes {
            throw TTYV2SessionError.protocolError("frame too large")
        }
        var length = UInt32(body.count).bigEndian
        let lengthData = Data(bytes: &length, count: MemoryLayout<UInt32>.size)
        var payload = Data()
        payload.reserveCapacity(lengthData.count + body.count)
        payload.append(lengthData)
        payload.append(body)
        try writeAll(fd: fd, data: payload)
    }

    private static func readFrame(fd: Int32) throws -> TTYV2Frame {
        let lengthData = try readExact(fd: fd, count: 4)
        let size = Int(
            (UInt32(lengthData[0]) << 24) |
                (UInt32(lengthData[1]) << 16) |
                (UInt32(lengthData[2]) << 8) |
                UInt32(lengthData[3])
        )
        if size <= 0 || size > maxFrameBytes {
            throw TTYV2SessionError.protocolError("invalid frame length: \(size)")
        }
        let body = try readExact(fd: fd, count: size)
        return try decodeFrame(body)
    }

    private static func decodeFrame(_ body: Data) throws -> TTYV2Frame {
        guard let object = try JSONSerialization.jsonObject(with: body, options: []) as? [String: Any] else {
            throw TTYV2SessionError.protocolError("frame must be object")
        }
        guard let schemaVersion = object["schema_version"] as? String,
              let frameType = object["type"] as? String else {
            throw TTYV2SessionError.protocolError("frame missing schema/type")
        }
        guard schemaVersion == Self.schemaVersion else {
            throw TTYV2SessionError.protocolError("unsupported schema version: \(schemaVersion)")
        }
        let frameSeq: UInt64
        if let num = object["frame_seq"] as? NSNumber {
            frameSeq = num.uint64Value
        } else if let str = object["frame_seq"] as? String, let parsed = UInt64(str) {
            frameSeq = parsed
        } else {
            throw TTYV2SessionError.protocolError("frame_seq is required")
        }
        guard let payloadObject = object["payload"] else {
            throw TTYV2SessionError.protocolError("payload is required")
        }
        let payloadData = try JSONSerialization.data(withJSONObject: payloadObject, options: [])
        return TTYV2Frame(
            schemaVersion: schemaVersion,
            type: frameType,
            frameSeq: frameSeq,
            sentAt: object["sent_at"] as? String,
            requestID: object["request_id"] as? String,
            payloadData: payloadData
        )
    }

    private static func readExact(fd: Int32, count: Int) throws -> Data {
        var data = Data(count: count)
        let readBytes = data.withUnsafeMutableBytes { ptr -> Int in
            guard let base = ptr.bindMemory(to: UInt8.self).baseAddress else {
                return -1
            }
            var offset = 0
            while offset < count {
                let n = Darwin.read(fd, base.advanced(by: offset), count - offset)
                if n == 0 {
                    return offset
                }
                if n < 0 {
                    if errno == EINTR {
                        continue
                    }
                    return -1
                }
                offset += n
            }
            return offset
        }
        if readBytes < 0 {
            throw TTYV2SessionError.unavailable("read failed: \(errnoDescription())")
        }
        if readBytes != count {
            throw TTYV2SessionError.closed
        }
        return data
    }

    private static func writeAll(fd: Int32, data: Data) throws {
        try data.withUnsafeBytes { ptr in
            guard let base = ptr.bindMemory(to: UInt8.self).baseAddress else {
                throw TTYV2SessionError.protocolError("write buffer unavailable")
            }
            var offset = 0
            while offset < data.count {
                let n = Darwin.write(fd, base.advanced(by: offset), data.count - offset)
                if n < 0 {
                    if errno == EINTR {
                        continue
                    }
                    throw TTYV2SessionError.unavailable("write failed: \(errnoDescription())")
                }
                offset += n
            }
        }
    }

    private static func errnoDescription() -> String {
        String(cString: strerror(errno))
    }
}
