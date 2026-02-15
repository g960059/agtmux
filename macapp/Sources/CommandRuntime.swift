import Foundation

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

struct AGTMUXCLIClient {
    let socketPath: String
    let appBinaryPath: String

    init(socketPath: String) throws {
        self.socketPath = socketPath
        self.appBinaryPath = try BinaryResolver.resolve(binary: "agtmux-app", envKey: "AGTMUX_APP_BIN")
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

    func sendText(target: String, paneID: String, text: String, requestRef: String, enter: Bool, paste: Bool) async throws -> ActionResponse {
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

    func kill(target: String, paneID: String, requestRef: String, mode: String, signal: String) async throws -> ActionResponse {
        let args = [
            "action", "kill",
            "--request-ref", requestRef,
            "--target", target,
            "--pane", paneID,
            "--mode", mode,
            "--signal", signal,
            "--json",
        ]
        let out = try await runAppCommand(args)
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
        return try await Task.detached(priority: .userInitiated) {
            try runCapture(appBinaryPath, allArgs)
        }.value
    }
}

final class DaemonManager {
    let socketPath: String
    let dbPath: String
    let logPath: String

    private let daemonBinaryPath: String
    private var ownedProcess: Process?
    private var logHandle: FileHandle?
    private var lastBackupAt: Date?
    private var lastBackedUpDBModificationAt: Date?
    private let backupIntervalSeconds: TimeInterval = 300
    private let maxBackupFiles = 12

    init(socketPath: String, dbPath: String, logPath: String) throws {
        self.socketPath = socketPath
        self.dbPath = dbPath
        self.logPath = logPath
        self.daemonBinaryPath = try BinaryResolver.resolve(binary: "agtmuxd", envKey: "AGTMUX_DAEMON_BIN")
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

        let process = Process()
        process.executableURL = URL(fileURLWithPath: daemonBinaryPath)
        process.arguments = ["--socket", socketPath, "--db", dbPath]
        process.standardInput = nil
        let logHandle = try FileHandle(forWritingTo: logURL)
        logHandle.seekToEndOfFile()
        process.standardOutput = logHandle
        process.standardError = logHandle

        try process.run()
        ownedProcess = process
        self.logHandle = logHandle

        for _ in 0..<50 {
            if await client.healthcheck() {
                return
            }
            try await Task.sleep(for: .milliseconds(100))
        }

        stopIfOwned()
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
}

private struct CommandResult {
    let stdout: String
    let stderr: String
    let status: Int32
}

private func runCapture(_ executable: String, _ args: [String]) throws -> String {
    let result = try runCommand(executable, args)
    guard result.status == 0 else {
        let cmd = ([executable] + args).joined(separator: " ")
        throw RuntimeError.commandFailed(cmd, result.status, result.stderr)
    }
    return result.stdout
}

private func runCommand(_ executable: String, _ args: [String]) throws -> CommandResult {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = args

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
