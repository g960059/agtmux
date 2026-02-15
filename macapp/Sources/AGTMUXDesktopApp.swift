import SwiftUI

@main
struct AGTMUXDesktopApp: App {
    private let model: AppViewModel?
    private let launchError: String?

    init() {
        do {
            let paths = AppPaths.resolve()
            let daemon = try DaemonManager(
                socketPath: paths.socketPath,
                dbPath: paths.dbPath,
                logPath: paths.logPath
            )
            let client = try AGTMUXCLIClient(socketPath: paths.socketPath)
            model = AppViewModel(daemon: daemon, client: client)
            launchError = nil
        } catch {
            model = nil
            launchError = error.localizedDescription
        }
    }

    var body: some Scene {
        WindowGroup {
            if let model {
                CockpitView()
                    .environmentObject(model)
                    .frame(minWidth: 1180, minHeight: 760)
                    .task {
                        model.bootstrap()
                    }
            } else {
                LaunchErrorView(message: launchError ?? "Unknown startup error")
                    .frame(minWidth: 980, minHeight: 560)
            }
        }
        .windowStyle(.hiddenTitleBar)
    }
}

private struct LaunchErrorView: View {
    let message: String

    var body: some View {
        ZStack {
            LinearGradient(
                colors: [Color(red: 0.16, green: 0.06, blue: 0.08), Color(red: 0.22, green: 0.08, blue: 0.10)],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            .ignoresSafeArea()

            VStack(alignment: .leading, spacing: 12) {
                Text("AGTMUXDesktop startup failed")
                    .font(.system(size: 24, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                Text(message)
                    .font(.system(size: 13, weight: .regular, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.9))
                    .textSelection(.enabled)
                Text("Check AGTMUX installation and bundled runtime files.")
                    .font(.system(size: 12, weight: .regular, design: .rounded))
                    .foregroundStyle(.white.opacity(0.8))
            }
            .padding(20)
            .background(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(Color.black.opacity(0.35))
            )
            .padding(20)
        }
    }
}

private struct CockpitView: View {
    @EnvironmentObject private var model: AppViewModel
    @State private var showAckAllConfirmation = false
    @State private var pendingKillAction: KillAction?
    @State private var reviewQueueExpandedWhenEmpty = false

    private enum KillAction {
        case keyINT
        case signalTERM

        var title: String {
            switch self {
            case .keyINT:
                return "Kill INT"
            case .signalTERM:
                return "Kill TERM"
            }
        }

        var message: String {
            switch self {
            case .keyINT:
                return "send-keys INT を送信します。対象 pane のプロセスが中断される可能性があります。"
            case .signalTERM:
                return "TERM シグナルを送信します。対象 pane のプロセスが終了する可能性があります。"
            }
        }
    }

    var body: some View {
        ZStack {
            LinearGradient(
                colors: [Color(red: 0.06, green: 0.08, blue: 0.12), Color(red: 0.11, green: 0.09, blue: 0.16)],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            .ignoresSafeArea()

            HSplitView {
                dashboardPanel
                inspectorPanel
                    .frame(minWidth: 340, maxWidth: 400)
            }
            .padding(16)
        }
        .confirmationDialog(
            "Confirm Kill Action",
            isPresented: Binding(
                get: { pendingKillAction != nil },
                set: { newValue in
                    if !newValue {
                        pendingKillAction = nil
                    }
                }
            ),
            titleVisibility: .visible
        ) {
            if let action = pendingKillAction {
                Button(action.title, role: .destructive) {
                    switch action {
                    case .keyINT:
                        model.performKillKeyINT()
                    case .signalTERM:
                        model.performKillSignalTERM()
                    }
                    pendingKillAction = nil
                }
            }
            Button("Cancel", role: .cancel) {
                pendingKillAction = nil
            }
        } message: {
            if let action = pendingKillAction {
                Text(action.message)
            }
        }
        .confirmationDialog(
            "Acknowledge all queue items?",
            isPresented: $showAckAllConfirmation,
            titleVisibility: .visible
        ) {
            Button("Ack All", role: .destructive) {
                model.acknowledgeAllQueueItems()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Mark all pending review queue items as acknowledged.")
        }
    }

    private var dashboardPanel: some View {
        VStack(spacing: 12) {
            headerBar
            summaryGrid
            contentBoard
        }
        .padding(14)
        .background(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .fill(Color.white.opacity(0.08))
        )
    }

    private var inspectorPanel: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Inspector")
                .font(.system(size: 19, weight: .semibold, design: .rounded))
                .foregroundStyle(.white)

            if let pane = model.selectedPane {
                Text("\(pane.identity.target)/\(pane.identity.sessionName)/\(pane.identity.windowID)/\(pane.identity.paneID)")
                    .font(.system(size: 12, weight: .regular, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.85))
                Text("category: \(model.categoryLabel(model.displayCategory(for: pane)))  state: \(model.activityState(for: pane))")
                    .font(.system(size: 12, weight: .regular, design: .rounded))
                    .foregroundStyle(.white.opacity(0.72))
                if model.showTechnicalDetails {
                    Text("runtime: \(pane.runtimeID ?? "-")")
                        .font(.system(size: 11, weight: .regular, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.66))
                    Text("source: \(pane.stateSource ?? "-")  event: \(pane.lastEventType ?? "-")")
                        .font(.system(size: 11, weight: .regular, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.62))
                }
                if let awaiting = model.awaitingResponseKind(for: pane) {
                    Text("awaiting: \(awaiting)")
                        .font(.system(size: 11, weight: .semibold, design: .rounded))
                        .foregroundStyle(Color(red: 1.0, green: 0.85, blue: 0.6))
                }
            } else {
                Text("Pane を選択してください")
                    .font(.system(size: 13, weight: .regular, design: .rounded))
                    .foregroundStyle(.white.opacity(0.72))
            }

            Divider().overlay(Color.white.opacity(0.15))

            VStack(alignment: .leading, spacing: 8) {
                Text("Send")
                    .font(.system(size: 14, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)

                TextField("Send text...", text: $model.sendText, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .lineLimit(3...5)
                    .disabled(!model.hasSelectedPane)

                HStack {
                    Toggle("Enter", isOn: $model.sendEnter)
                    Toggle("Paste", isOn: $model.sendPaste)
                }
                .toggleStyle(.switch)
                .foregroundStyle(.white.opacity(0.92))
                .disabled(!model.hasSelectedPane)

                Button("Send To Pane") {
                    model.performSend()
                }
                .buttonStyle(.borderedProminent)
                .disabled(!model.canSend)
            }

            Divider().overlay(Color.white.opacity(0.15))

            VStack(alignment: .leading, spacing: 8) {
                Text("Actions")
                    .font(.system(size: 14, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)

                HStack {
                    Button("View Output") {
                        model.performViewOutput(lines: 120)
                    }
                    .buttonStyle(.bordered)
                    .disabled(!model.hasSelectedPane)

                    Button("Kill INT") {
                        pendingKillAction = .keyINT
                    }
                    .buttonStyle(.bordered)
                    .disabled(!model.hasSelectedPane)
                }

                Button("Kill TERM (signal)") {
                    pendingKillAction = .signalTERM
                }
                .buttonStyle(.bordered)
                .disabled(!model.hasSelectedPane)

                if !model.outputPreview.isEmpty {
                    ScrollView {
                        Text(model.outputPreview)
                            .font(.system(size: 11, weight: .regular, design: .monospaced))
                            .foregroundStyle(.white.opacity(0.92))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(8)
                    }
                    .frame(minHeight: 120)
                    .background(Color.black.opacity(0.18))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(Color.white.opacity(0.2))
                    )
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
            }

            Divider().overlay(Color.white.opacity(0.15))

            reviewQueuePanel

            Divider().overlay(Color.white.opacity(0.15))

            if !model.errorMessage.isEmpty {
                Text(model.errorMessage)
                    .font(.system(size: 12, weight: .regular, design: .rounded))
                    .foregroundStyle(Color(red: 1.0, green: 0.7, blue: 0.7))
                    .textSelection(.enabled)
            }
            if !model.infoMessage.isEmpty {
                Text(model.infoMessage)
                    .font(.system(size: 12, weight: .regular, design: .rounded))
                    .foregroundStyle(Color(red: 0.7, green: 0.95, blue: 0.8))
            }

            Spacer(minLength: 0)
        }
        .padding(14)
        .background(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .fill(Color.white.opacity(0.08))
        )
    }

    private var headerBar: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("AGTMUX Cockpit")
                        .font(.system(size: 30, weight: .black, design: .rounded))
                        .foregroundStyle(.white)
                    Text("Session-first agent operations on top of tmux")
                        .font(.system(size: 13, weight: .regular, design: .rounded))
                        .foregroundStyle(.white.opacity(0.75))
                }

                Spacer()
            }

            HStack(spacing: 10) {
                Picker("", selection: $model.viewMode) {
                    ForEach(AppViewModel.ViewMode.allCases) { mode in
                        Text(mode.title).tag(mode)
                    }
                }
                .pickerStyle(.segmented)
                .frame(width: 230)

                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass")
                        .font(.system(size: 11, weight: .regular))
                        .foregroundStyle(.white.opacity(0.6))
                    TextField("Search session/pane/agent...", text: $model.searchQuery)
                        .textFieldStyle(.plain)
                        .font(.system(size: 12, weight: .regular, design: .rounded))
                        .foregroundStyle(.white.opacity(0.95))
                    if !model.searchQuery.isEmpty {
                        Button {
                            model.searchQuery = ""
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .foregroundStyle(.white.opacity(0.6))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .frame(width: 250)
                .background(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(Color.white.opacity(0.10))
                )

                Menu {
                    Section("Window Grouping") {
                        Picker("Window Grouping", selection: $model.windowGrouping) {
                            ForEach(AppViewModel.WindowGrouping.allCases) { grouping in
                                Text(grouping.title).tag(grouping)
                            }
                        }
                    }
                    Toggle("Show Window Metadata", isOn: $model.showWindowMetadata)
                    Toggle("Show Session Labels in Status View", isOn: $model.showSessionMetadataInStatusView)
                    Toggle("Show Empty Status Columns", isOn: $model.showEmptyStatusColumns)
                    Toggle("Show Technical Details", isOn: $model.showTechnicalDetails)
                    Toggle("Hide Unmanaged Column", isOn: $model.hideUnmanagedCategory)
                    Toggle("Show Unknown Column", isOn: $model.showUnknownCategory)
                    Divider()
                    Toggle("Review Queue: Unread Only", isOn: $model.reviewUnreadOnly)
                } label: {
                    Label("View Settings", systemImage: "slider.horizontal.3")
                }
                .buttonStyle(.bordered)
            }
        }
    }

    private var summaryGrid: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 130), spacing: 10)], spacing: 10) {
            ForEach(model.summaryCards, id: \.0) { card in
                VStack(alignment: .leading, spacing: 6) {
                    Text(card.0.uppercased())
                        .font(.system(size: 11, weight: .bold, design: .rounded))
                        .foregroundStyle(.white.opacity(0.68))
                    Text("\(card.1)")
                        .font(.system(size: 28, weight: .heavy, design: .rounded))
                        .foregroundStyle(.white)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(Color.white.opacity(0.10))
                )
            }
        }
    }

    private var contentBoard: some View {
        Group {
            switch model.viewMode {
            case .bySession:
                bySessionBoard
            case .byStatus:
                byStatusBoard
            }
        }
    }

    private var bySessionBoard: some View {
        ScrollView {
            LazyVStack(spacing: 12) {
                ForEach(model.sessionSections) { session in
                    sessionCard(session)
                }
            }
            .padding(.vertical, 2)
        }
    }

    private var byStatusBoard: some View {
        let configuredGroups = model.statusGroups.filter { group in
            if model.hideUnmanagedCategory && group.0 == "unmanaged" {
                return false
            }
            if !model.showUnknownCategory && group.0 == "unknown" {
                return false
            }
            return true
        }
        let displayGroups = model.showEmptyStatusColumns ? configuredGroups : configuredGroups.filter { !$0.1.isEmpty }
        return Group {
            if displayGroups.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "rectangle.stack")
                        .font(.system(size: 20, weight: .regular))
                        .foregroundStyle(.white.opacity(0.5))
                    Text("No panes in current filter")
                        .font(.system(size: 13, weight: .semibold, design: .rounded))
                        .foregroundStyle(.white.opacity(0.7))
                    Text("Clear search or adjust View Settings.")
                        .font(.system(size: 11, weight: .regular, design: .rounded))
                        .foregroundStyle(.white.opacity(0.52))
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(Color.white.opacity(0.06))
                )
            } else {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(alignment: .top, spacing: 12) {
                        ForEach(displayGroups, id: \.0) { group in
                            VStack(alignment: .leading, spacing: 8) {
                                HStack {
                                    Text(model.categoryLabel(group.0))
                                        .font(.system(size: 12, weight: .bold, design: .rounded))
                                        .foregroundStyle(.white.opacity(0.9))
                                    Spacer(minLength: 0)
                                    Text("\(group.1.count)")
                                        .font(.system(size: 12, weight: .bold, design: .rounded))
                                        .foregroundStyle(.white.opacity(0.7))
                                }
                                .padding(.bottom, 2)

                                ScrollView {
                                    VStack(spacing: 8) {
                                        ForEach(group.1, id: \.id) { pane in
                                            paneCard(
                                                pane,
                                                showSessionLabel: model.showSessionMetadataInStatusView,
                                                showCategoryBadge: false,
                                                showStateReason: false
                                            )
                                        }
                                        if group.1.isEmpty {
                                            Text("No panes")
                                                .font(.system(size: 12, weight: .regular, design: .rounded))
                                                .foregroundStyle(.white.opacity(0.45))
                                                .frame(maxWidth: .infinity, minHeight: 70)
                                        }
                                    }
                                }
                            }
                            .frame(minWidth: 250, maxWidth: 250, minHeight: 460, maxHeight: .infinity, alignment: .top)
                            .padding(10)
                            .background(
                                RoundedRectangle(cornerRadius: 12, style: .continuous)
                                    .fill(Color.white.opacity(0.09))
                            )
                        }
                    }
                    .padding(.vertical, 2)
                }
            }
        }
    }

    private func sessionCard(_ section: AppViewModel.SessionSection) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(section.sessionName)
                        .font(.system(size: 17, weight: .heavy, design: .rounded))
                        .foregroundStyle(.white)
                    Text(section.target)
                        .font(.system(size: 11, weight: .regular, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.66))
                }
                Spacer(minLength: 0)
                if section.unreadCount > 0 {
                    Text("\(section.unreadCount) queue")
                        .font(.system(size: 11, weight: .bold, design: .rounded))
                        .foregroundStyle(.black.opacity(0.95))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(
                            Capsule(style: .continuous)
                                .fill(Color(red: 1.0, green: 0.66, blue: 0.26))
                        )
                }
                categoryBadge(section.topCategory)
            }

            categoryStrip(section.byCategory)

            if section.windows.isEmpty {
                paneList(section.panes, showSessionLabel: false)
            } else {
                ForEach(section.windows) { window in
                    VStack(alignment: .leading, spacing: 8) {
                        if model.showWindowMetadata {
                            HStack {
                                Text("Window \(window.windowID)")
                                    .font(.system(size: 11, weight: .semibold, design: .rounded))
                                    .foregroundStyle(.white.opacity(0.82))
                                Spacer(minLength: 0)
                                categoryBadge(window.topCategory)
                            }
                        }
                        paneList(window.panes, showSessionLabel: false)
                    }
                    .padding(10)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(Color.white.opacity(0.06))
                    )
                }
            }
        }
        .padding(12)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Color.white.opacity(0.10))
        )
    }

    private func paneList(_ panes: [PaneItem], showSessionLabel: Bool) -> some View {
        VStack(spacing: 8) {
            ForEach(panes, id: \.id) { pane in
                paneCard(pane, showSessionLabel: showSessionLabel)
            }
        }
    }

    private func paneCard(
        _ pane: PaneItem,
        showSessionLabel: Bool,
        showCategoryBadge: Bool = true,
        showStateReason: Bool = true
    ) -> some View {
        let category = model.displayCategory(for: pane)
        return Button {
            model.selectedPane = pane
        } label: {
            VStack(alignment: .leading, spacing: 5) {
                HStack {
                    if showCategoryBadge {
                        categoryBadge(category)
                    }
                    Spacer(minLength: 0)
                    if model.needsUserAction(for: pane) {
                        Label("Action", systemImage: "exclamationmark.circle.fill")
                            .font(.system(size: 11, weight: .semibold, design: .rounded))
                            .foregroundStyle(Color(red: 1.0, green: 0.72, blue: 0.72))
                    }
                }

                Text(model.paneDisplayTitle(for: pane))
                    .font(.system(size: 13, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
                    .lineLimit(1)
                if showSessionLabel {
                    Text("\(pane.identity.target) • \(pane.identity.sessionName)")
                        .font(.system(size: 11, weight: .regular, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.72))
                }
                if model.showWindowMetadata && showCategoryBadge {
                    Text("window: \((pane.windowName ?? "").isEmpty ? pane.identity.windowID : pane.windowName ?? pane.identity.windowID)")
                        .font(.system(size: 10, weight: .regular, design: .rounded))
                        .foregroundStyle(.white.opacity(0.56))
                }
                if showStateReason {
                    Text(model.stateReason(for: pane))
                        .font(.system(size: 11, weight: .regular, design: .rounded))
                        .foregroundStyle(.white.opacity(0.76))
                }
                Text(model.lastActiveLabel(for: pane))
                    .font(.system(size: 10, weight: .regular, design: .rounded))
                    .foregroundStyle(.white.opacity(0.64))
                if model.showTechnicalDetails {
                    Text("agent: \(pane.agentType ?? "unknown")")
                        .font(.system(size: 10, weight: .regular, design: .rounded))
                        .foregroundStyle(.white.opacity(0.6))
                    Text("tmux: \(pane.identity.windowID)/\(pane.identity.paneID)")
                        .font(.system(size: 10, weight: .regular, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.55))
                    if let source = pane.sessionLabelSource, !source.isEmpty {
                        Text("label: \(source)")
                            .font(.system(size: 10, weight: .regular, design: .rounded))
                            .foregroundStyle(.white.opacity(0.55))
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(model.selectedPane?.id == pane.id ? Color(red: 0.26, green: 0.43, blue: 0.96) : Color.white.opacity(0.08))
            )
        }
        .buttonStyle(.plain)
    }

    private var reviewQueuePanel: some View {
        let hasItems = !model.visibleReviewQueue.isEmpty
        let showQueueBody = hasItems || reviewQueueExpandedWhenEmpty
        return VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Review Queue")
                    .font(.system(size: 14, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
                Spacer(minLength: 0)
                Text("\(model.reviewUnreadCount) unread")
                    .font(.system(size: 11, weight: .bold, design: .rounded))
                    .foregroundStyle(.white.opacity(0.78))
            }

            HStack {
                Toggle("Unread only", isOn: $model.reviewUnreadOnly)
                    .toggleStyle(.switch)
                    .font(.system(size: 11, weight: .regular, design: .rounded))
                    .foregroundStyle(.white.opacity(0.86))
                Spacer(minLength: 0)
                if !hasItems {
                    Button(showQueueBody ? "Hide" : "Show") {
                        reviewQueueExpandedWhenEmpty.toggle()
                    }
                    .buttonStyle(.bordered)
                }
                Button("Ack All") {
                    showAckAllConfirmation = true
                }
                .buttonStyle(.bordered)
                .disabled(model.visibleReviewQueue.isEmpty)
            }

            if showQueueBody {
                ScrollView {
                    VStack(spacing: 8) {
                        if model.visibleReviewQueue.isEmpty {
                            Text("No pending queue items")
                                .font(.system(size: 12, weight: .regular, design: .rounded))
                                .foregroundStyle(.white.opacity(0.5))
                                .frame(maxWidth: .infinity, minHeight: 32)
                        } else {
                            ForEach(model.visibleReviewQueue) { item in
                                queueItemRow(item)
                            }
                        }
                    }
                }
                .frame(minHeight: hasItems ? 140 : 34, maxHeight: hasItems ? 260 : 60)
            }
        }
    }

    private func queueItemRow(_ item: AppViewModel.ReviewQueueItem) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Text(item.kind.title)
                    .font(.system(size: 10, weight: .bold, design: .rounded))
                    .foregroundStyle(.black.opacity(0.9))
                    .padding(.horizontal, 7)
                    .padding(.vertical, 3)
                    .background(
                        Capsule(style: .continuous)
                            .fill(queueKindColor(item.kind))
                    )
                Spacer(minLength: 0)
                Text(item.createdAt, style: .time)
                    .font(.system(size: 10, weight: .regular, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.62))
            }

            Text(item.summary)
                .font(.system(size: 12, weight: .semibold, design: .rounded))
                .foregroundStyle(.white.opacity(0.92))
            Text("\(item.target)/\(item.sessionName)/\(item.paneID)")
                .font(.system(size: 10, weight: .regular, design: .monospaced))
                .foregroundStyle(.white.opacity(0.66))

            HStack(spacing: 8) {
                Button("Open") {
                    model.openQueueItem(item)
                }
                .buttonStyle(.bordered)
                Button("Ack") {
                    model.acknowledgeQueueItem(item)
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Color.white.opacity(0.08))
        )
    }

    private func categoryStrip(_ counts: [String: Int]) -> some View {
        HStack(spacing: 8) {
            categoryCounter("attention", count: counts["attention", default: 0])
            categoryCounter("running", count: counts["running", default: 0])
            categoryCounter("idle", count: counts["idle", default: 0])
            categoryCounter("unmanaged", count: counts["unmanaged", default: 0])
            if model.showUnknownCategory {
                categoryCounter("unknown", count: counts["unknown", default: 0])
            }
        }
    }

    private func categoryCounter(_ category: String, count: Int) -> some View {
        HStack(spacing: 4) {
            Circle()
                .fill(colorForCategory(category))
                .frame(width: 7, height: 7)
            Text("\(model.categoryLabel(category)): \(count)")
                .font(.system(size: 10, weight: .regular, design: .rounded))
                .foregroundStyle(.white.opacity(0.74))
        }
        .help(categoryHelpText(category))
    }

    private func categoryBadge(_ category: String) -> some View {
        Text(model.categoryLabel(category))
            .font(.system(size: 10, weight: .bold, design: .rounded))
            .foregroundStyle(.black.opacity(0.9))
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(
                Capsule(style: .continuous)
                    .fill(colorForCategory(category))
            )
            .help(categoryHelpText(category))
    }

    private func categoryHelpText(_ category: String) -> String {
        switch category {
        case "attention":
            return "Agent requires user action (input, approval, or error handling)."
        case "running":
            return "Agent is actively processing."
        case "idle":
            return "Agent is attached but currently idle."
        case "unmanaged":
            return "No managed agent is detected in this pane."
        default:
            return "State detection is inconclusive."
        }
    }

    private func colorForCategory(_ category: String) -> Color {
        switch category {
        case "attention":
            return Color(red: 1.0, green: 0.55, blue: 0.46)
        case "running":
            return Color(red: 0.45, green: 0.95, blue: 0.66)
        case "idle":
            return Color(red: 0.66, green: 0.86, blue: 1.0)
        case "unmanaged":
            return Color(red: 0.75, green: 0.75, blue: 0.78)
        default:
            return Color(red: 0.85, green: 0.82, blue: 0.9)
        }
    }

    private func queueKindColor(_ kind: AppViewModel.ReviewKind) -> Color {
        switch kind {
        case .taskCompleted:
            return Color(red: 0.66, green: 0.86, blue: 1.0)
        case .needsInput:
            return Color(red: 1.0, green: 0.76, blue: 0.44)
        case .needsApproval:
            return Color(red: 1.0, green: 0.62, blue: 0.36)
        case .error:
            return Color(red: 1.0, green: 0.55, blue: 0.46)
        }
    }

}

private struct AppPaths {
    let socketPath: String
    let dbPath: String
    let logPath: String

    static func resolve() -> AppPaths {
        let fm = FileManager.default
        let baseURL = fm.homeDirectoryForCurrentUser
            .appendingPathComponent("Library")
            .appendingPathComponent("Application Support")
            .appendingPathComponent("AGTMUXDesktop", isDirectory: true)
        try? fm.createDirectory(at: baseURL, withIntermediateDirectories: true)
        return AppPaths(
            socketPath: baseURL.appendingPathComponent("agtmuxd.sock").path,
            dbPath: baseURL.appendingPathComponent("state.db").path,
            logPath: baseURL.appendingPathComponent("agtmuxd.log").path
        )
    }
}
