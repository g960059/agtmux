import Foundation

struct TargetsEnvelope: Decodable {
    let targets: [TargetItem]
}

struct TargetItem: Decodable, Identifiable, Hashable {
    let targetID: String
    let targetName: String
    let kind: String
    let connectionRef: String?
    let isDefault: Bool
    let health: String

    var id: String { targetID }

    enum CodingKeys: String, CodingKey {
        case targetID = "target_id"
        case targetName = "target_name"
        case kind
        case connectionRef = "connection_ref"
        case isDefault = "is_default"
        case health
    }
}

struct PaneEnvelope: Decodable {
    let items: [PaneItem]
}

struct WindowEnvelope: Decodable {
    let items: [WindowItem]
}

struct SessionEnvelope: Decodable {
    let items: [SessionItem]
}

struct PaneIdentity: Decodable, Hashable {
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

struct PaneItem: Decodable, Identifiable, Hashable {
    let identity: PaneIdentity
    let windowName: String?
    let currentCmd: String?
    let paneTitle: String?
    let state: String
    let reasonCode: String?
    let confidence: String?
    let runtimeID: String?
    let agentType: String?
    let agentPresence: String?
    let activityState: String?
    let displayCategory: String?
    let needsUserAction: Bool?
    let stateSource: String?
    let lastEventType: String?
    let lastEventAt: String?
    let awaitingResponseKind: String?
    let attentionState: String?
    let attentionReason: String?
    let attentionSince: String?
    let sessionLabel: String?
    let sessionLabelSource: String?
    let lastInteractionAt: String?
    let sessionLastActiveAt: String?
    let sessionTimeSource: String?
    let sessionTimeConfidence: Double?
    let stateEngineVersion: String?
    let providerV2: String?
    let providerConfidenceV2: Double?
    let activityStateV2: String?
    let activityConfidenceV2: Double?
    let activitySourceV2: String?
    let activityReasonsV2: [String]?
    let evidenceTraceID: String?
    let updatedAt: String

    var id: String {
        "\(identity.target)|\(identity.sessionName)|\(identity.windowID)|\(identity.paneID)"
    }

    enum CodingKeys: String, CodingKey {
        case identity
        case windowName = "window_name"
        case currentCmd = "current_cmd"
        case paneTitle = "pane_title"
        case state
        case reasonCode = "reason_code"
        case confidence
        case runtimeID = "runtime_id"
        case agentType = "agent_type"
        case agentPresence = "agent_presence"
        case activityState = "activity_state"
        case displayCategory = "display_category"
        case needsUserAction = "needs_user_action"
        case stateSource = "state_source"
        case lastEventType = "last_event_type"
        case lastEventAt = "last_event_at"
        case awaitingResponseKind = "awaiting_response_kind"
        case attentionState = "attention_state"
        case attentionReason = "attention_reason"
        case attentionSince = "attention_since"
        case sessionLabel = "session_label"
        case sessionLabelSource = "session_label_source"
        case lastInteractionAt = "last_interaction_at"
        case sessionLastActiveAt = "session_last_active_at"
        case sessionTimeSource = "session_time_source"
        case sessionTimeConfidence = "session_time_confidence"
        case stateEngineVersion = "state_engine_version"
        case providerV2 = "provider_v2"
        case providerConfidenceV2 = "provider_confidence_v2"
        case activityStateV2 = "activity_state_v2"
        case activityConfidenceV2 = "activity_confidence_v2"
        case activitySourceV2 = "activity_source_v2"
        case activityReasonsV2 = "activity_reasons_v2"
        case evidenceTraceID = "evidence_trace_id"
        case updatedAt = "updated_at"
    }

    init(
        identity: PaneIdentity,
        windowName: String?,
        currentCmd: String?,
        paneTitle: String?,
        state: String,
        reasonCode: String?,
        confidence: String?,
        runtimeID: String?,
        agentType: String?,
        agentPresence: String?,
        activityState: String?,
        displayCategory: String?,
        needsUserAction: Bool?,
        stateSource: String?,
        lastEventType: String?,
        lastEventAt: String?,
        awaitingResponseKind: String?,
        attentionState: String? = nil,
        attentionReason: String? = nil,
        attentionSince: String? = nil,
        sessionLabel: String?,
        sessionLabelSource: String?,
        lastInteractionAt: String?,
        sessionLastActiveAt: String? = nil,
        sessionTimeSource: String? = nil,
        sessionTimeConfidence: Double? = nil,
        stateEngineVersion: String? = nil,
        providerV2: String? = nil,
        providerConfidenceV2: Double? = nil,
        activityStateV2: String? = nil,
        activityConfidenceV2: Double? = nil,
        activitySourceV2: String? = nil,
        activityReasonsV2: [String]? = nil,
        evidenceTraceID: String? = nil,
        updatedAt: String
    ) {
        self.identity = identity
        self.windowName = windowName
        self.currentCmd = currentCmd
        self.paneTitle = paneTitle
        self.state = state
        self.reasonCode = reasonCode
        self.confidence = confidence
        self.runtimeID = runtimeID
        self.agentType = agentType
        self.agentPresence = agentPresence
        self.activityState = activityState
        self.displayCategory = displayCategory
        self.needsUserAction = needsUserAction
        self.stateSource = stateSource
        self.lastEventType = lastEventType
        self.lastEventAt = lastEventAt
        self.awaitingResponseKind = awaitingResponseKind
        self.attentionState = attentionState
        self.attentionReason = attentionReason
        self.attentionSince = attentionSince
        self.sessionLabel = sessionLabel
        self.sessionLabelSource = sessionLabelSource
        self.lastInteractionAt = lastInteractionAt
        self.sessionLastActiveAt = sessionLastActiveAt
        self.sessionTimeSource = sessionTimeSource
        self.sessionTimeConfidence = sessionTimeConfidence
        self.stateEngineVersion = stateEngineVersion
        self.providerV2 = providerV2
        self.providerConfidenceV2 = providerConfidenceV2
        self.activityStateV2 = activityStateV2
        self.activityConfidenceV2 = activityConfidenceV2
        self.activitySourceV2 = activitySourceV2
        self.activityReasonsV2 = activityReasonsV2
        self.evidenceTraceID = evidenceTraceID
        self.updatedAt = updatedAt
    }
}

struct WindowIdentity: Decodable, Hashable {
    let target: String
    let sessionName: String
    let windowID: String

    enum CodingKeys: String, CodingKey {
        case target
        case sessionName = "session_name"
        case windowID = "window_id"
    }
}

struct WindowItem: Decodable, Identifiable, Hashable {
    let identity: WindowIdentity
    let topState: String
    let topCategory: String?
    let byCategory: [String: Int]?
    let waitingCount: Int
    let runningCount: Int
    let totalPanes: Int

    var id: String { "\(identity.target)|\(identity.sessionName)|\(identity.windowID)" }

    enum CodingKeys: String, CodingKey {
        case identity
        case topState = "top_state"
        case topCategory = "top_category"
        case byCategory = "by_category"
        case waitingCount = "waiting_count"
        case runningCount = "running_count"
        case totalPanes = "total_panes"
    }
}

struct SessionIdentity: Decodable, Hashable {
    let target: String
    let sessionName: String

    enum CodingKeys: String, CodingKey {
        case target
        case sessionName = "session_name"
    }
}

struct SessionItem: Decodable, Identifiable, Hashable {
    let identity: SessionIdentity
    let topCategory: String?
    let totalPanes: Int
    let byState: [String: Int]
    let byAgent: [String: Int]
    let byCategory: [String: Int]?

    var id: String { "\(identity.target)|\(identity.sessionName)" }

    enum CodingKeys: String, CodingKey {
        case identity
        case topCategory = "top_category"
        case totalPanes = "total_panes"
        case byState = "by_state"
        case byAgent = "by_agent"
        case byCategory = "by_category"
    }
}

struct ActionResponse: Decodable {
    let actionID: String
    let resultCode: String
    let output: String?

    enum CodingKeys: String, CodingKey {
        case actionID = "action_id"
        case resultCode = "result_code"
        case output
    }
}

struct CapabilitiesEnvelope: Decodable {
    let capabilities: CapabilityFlags
}

struct CapabilityFlags: Decodable {
    let embeddedTerminal: Bool
    let terminalRead: Bool
    let terminalResize: Bool
    let terminalWriteViaActionSend: Bool
    let terminalAttach: Bool
    let terminalWrite: Bool
    let terminalStream: Bool
    let terminalProxyMode: String?
    let terminalFrameProtocol: String?

    enum CodingKeys: String, CodingKey {
        case embeddedTerminal = "embedded_terminal"
        case terminalRead = "terminal_read"
        case terminalResize = "terminal_resize"
        case terminalWriteViaActionSend = "terminal_write_via_action_send"
        case terminalAttach = "terminal_attach"
        case terminalWrite = "terminal_write"
        case terminalStream = "terminal_stream"
        case terminalProxyMode = "terminal_proxy_mode"
        case terminalFrameProtocol = "terminal_frame_protocol"
    }

    init(
        embeddedTerminal: Bool,
        terminalRead: Bool,
        terminalResize: Bool,
        terminalWriteViaActionSend: Bool,
        terminalAttach: Bool,
        terminalWrite: Bool,
        terminalStream: Bool,
        terminalProxyMode: String?,
        terminalFrameProtocol: String?
    ) {
        self.embeddedTerminal = embeddedTerminal
        self.terminalRead = terminalRead
        self.terminalResize = terminalResize
        self.terminalWriteViaActionSend = terminalWriteViaActionSend
        self.terminalAttach = terminalAttach
        self.terminalWrite = terminalWrite
        self.terminalStream = terminalStream
        self.terminalProxyMode = terminalProxyMode
        self.terminalFrameProtocol = terminalFrameProtocol
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        embeddedTerminal = try container.decodeIfPresent(Bool.self, forKey: .embeddedTerminal) ?? false
        terminalRead = try container.decodeIfPresent(Bool.self, forKey: .terminalRead) ?? false
        terminalResize = try container.decodeIfPresent(Bool.self, forKey: .terminalResize) ?? false
        terminalWriteViaActionSend = try container.decodeIfPresent(Bool.self, forKey: .terminalWriteViaActionSend) ?? false
        terminalAttach = try container.decodeIfPresent(Bool.self, forKey: .terminalAttach) ?? false
        terminalWrite = try container.decodeIfPresent(Bool.self, forKey: .terminalWrite) ?? false
        terminalStream = try container.decodeIfPresent(Bool.self, forKey: .terminalStream) ?? false
        terminalProxyMode = try container.decodeIfPresent(String.self, forKey: .terminalProxyMode)
        terminalFrameProtocol = try container.decodeIfPresent(String.self, forKey: .terminalFrameProtocol)
    }
}

struct TerminalReadEnvelope: Decodable {
    let frame: TerminalFrame
}

struct TerminalFrame: Decodable {
    let frameType: String
    let streamID: String
    let cursor: String
    let cursorX: Int?
    let cursorY: Int?
    let paneCols: Int?
    let paneRows: Int?
    let paneID: String
    let target: String
    let lines: Int
    let content: String?
    let resetReason: String?

    enum CodingKeys: String, CodingKey {
        case frameType = "frame_type"
        case streamID = "stream_id"
        case cursor
        case cursorX = "cursor_x"
        case cursorY = "cursor_y"
        case paneCols = "pane_cols"
        case paneRows = "pane_rows"
        case paneID = "pane_id"
        case target
        case lines
        case content
        case resetReason = "reset_reason"
    }
}

struct TerminalResizeResponse: Decodable {
    let target: String
    let paneID: String
    let cols: Int
    let rows: Int
    let resultCode: String
    let policy: String?
    let clientCount: Int?
    let reason: String?

    enum CodingKeys: String, CodingKey {
        case target
        case paneID = "pane_id"
        case cols
        case rows
        case resultCode = "result_code"
        case policy
        case clientCount = "client_count"
        case reason
    }
}

struct TerminalAttachResponse: Decodable {
    let sessionID: String
    let target: String
    let paneID: String
    let runtimeID: String?
    let stateVersion: Int64?
    let resultCode: String

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case target
        case paneID = "pane_id"
        case runtimeID = "runtime_id"
        case stateVersion = "state_version"
        case resultCode = "result_code"
    }
}

struct TerminalDetachResponse: Decodable {
    let sessionID: String
    let resultCode: String

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case resultCode = "result_code"
    }
}

struct TerminalWriteResponse: Decodable {
    let sessionID: String
    let resultCode: String
    let errorCode: String?

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case resultCode = "result_code"
        case errorCode = "error_code"
    }
}

struct TerminalStreamEnvelope: Decodable {
    let frame: TerminalStreamFrame
}

struct TerminalStreamFrame: Decodable {
    let frameType: String
    let streamID: String
    let cursor: String
    let cursorX: Int?
    let cursorY: Int?
    let paneCols: Int?
    let paneRows: Int?
    let sessionID: String
    let target: String
    let paneID: String
    let content: String?
    let resetReason: String?
    let errorCode: String?
    let message: String?

    enum CodingKeys: String, CodingKey {
        case frameType = "frame_type"
        case streamID = "stream_id"
        case cursor
        case cursorX = "cursor_x"
        case cursorY = "cursor_y"
        case paneCols = "pane_cols"
        case paneRows = "pane_rows"
        case sessionID = "session_id"
        case target
        case paneID = "pane_id"
        case content
        case resetReason = "reset_reason"
        case errorCode = "error_code"
        case message
    }
}

struct DashboardSnapshot {
    let targets: [TargetItem]
    let sessions: [SessionItem]
    let windows: [WindowItem]
    let panes: [PaneItem]
}

struct DashboardSnapshotEnvelope: Decodable {
    let targets: [TargetItem]
    let sessions: [SessionItem]
    let windows: [WindowItem]
    let panes: [PaneItem]
}
