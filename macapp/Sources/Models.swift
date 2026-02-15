import Foundation

struct TargetsEnvelope: Decodable {
    let targets: [TargetItem]
}

struct TargetItem: Decodable, Identifiable, Hashable {
    let targetID: String
    let targetName: String
    let kind: String
    let isDefault: Bool
    let health: String

    var id: String { targetID }

    enum CodingKeys: String, CodingKey {
        case targetID = "target_id"
        case targetName = "target_name"
        case kind
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
    let sessionLabel: String?
    let sessionLabelSource: String?
    let lastInteractionAt: String?
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
        case sessionLabel = "session_label"
        case sessionLabelSource = "session_label_source"
        case lastInteractionAt = "last_interaction_at"
        case updatedAt = "updated_at"
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

struct DashboardSnapshot {
    let targets: [TargetItem]
    let sessions: [SessionItem]
    let windows: [WindowItem]
    let panes: [PaneItem]
}
