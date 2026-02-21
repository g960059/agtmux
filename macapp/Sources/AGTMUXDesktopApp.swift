import AppKit
import Darwin
import SwiftUI
import UniformTypeIdentifiers

@main
struct AGTMUXDesktopApp: App {
    private let model: AppViewModel?
    private let launchError: String?

    init() {
        // Avoid process-level termination when writing to a closed pipe/socket.
        // We handle EPIPE as a recoverable runtime error.
        signal(SIGPIPE, SIG_IGN)
        Self.disableBrokenWindowRestoration()
        do {
            let paths = AppPaths.resolve()
            appendLauncherLog(baseDir: paths.baseDir, message: "app init: start")
            let daemon = try DaemonManager(
                socketPath: paths.socketPath,
                dbPath: paths.dbPath,
                logPath: paths.logPath
            )
            let client = try AGTMUXCLIClient(socketPath: paths.socketPath)
            model = AppViewModel(
                daemon: daemon,
                client: client,
                nativeTmuxTerminalEnabled: true,
                allowTerminalV1Fallback: false
            )
            launchError = nil
            appendLauncherLog(baseDir: paths.baseDir, message: "app init: model ready")
            model?.bootstrap()
            appendLauncherLog(baseDir: paths.baseDir, message: "app init: bootstrap dispatched")
        } catch {
            model = nil
            launchError = error.localizedDescription
            let paths = AppPaths.resolve()
            appendLauncherLog(baseDir: paths.baseDir, message: "app init: failed \(error.localizedDescription)")
        }
    }

    private static func disableBrokenWindowRestoration() {
        UserDefaults.standard.set(false, forKey: "NSQuitAlwaysKeepsWindows")
        guard let bundleID = Bundle.main.bundleIdentifier else {
            return
        }
        let savedStatePath = ("~/Library/Saved Application State/\(bundleID).savedState" as NSString).expandingTildeInPath
        if FileManager.default.fileExists(atPath: savedStatePath) {
            try? FileManager.default.removeItem(atPath: savedStatePath)
        }
    }

    var body: some Scene {
        WindowGroup {
            if let model {
                CockpitView()
                    .environmentObject(model)
                    .frame(minWidth: 1180, minHeight: 760)
                    .task {
                        let paths = AppPaths.resolve()
                        appendLauncherLog(baseDir: paths.baseDir, message: "app task: bootstrap request")
                        model.bootstrap()
                    }
            } else {
                LaunchErrorView(message: launchError ?? "Unknown startup error")
                    .frame(minWidth: 980, minHeight: 560)
            }
        }
        .defaultSize(width: 1320, height: 840)
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
    @Environment(\.colorScheme) private var colorScheme
    @State private var pendingKillSession: SessionKillRequest?
    @State private var detailPane: PaneItem?
    @State private var pendingSessionRename: SessionRenameRequest?
    @State private var pendingPaneRename: PaneRenameRequest?
    @State private var sessionRenameDraft = ""
    @State private var paneRenameDraft = ""
    @State private var windowTopInset: CGFloat = 0
    @State private var hoveringPaneID: String?
    @State private var hoveringSessionID: String?
    @State private var sessionCursorActiveID: String?
    @State private var collapsedSessionIDs: Set<String> = []
    @State private var draggingSessionID: String?
    @State private var dropTargetSessionID: String?
    @State private var hoveringMenuRowID: String?
    @State private var showSortPopover = false
    @State private var showSettingsPopover = false
    @State private var showAddTargetSheet = false
    @State private var newTargetName = ""
    @State private var newTargetKind: AddTargetKind = .ssh
    @State private var newTargetConnectionRef = ""
    @State private var newTargetIsDefault = false
    @State private var newTargetConnectAfterAdd = true

    private struct WindowPaneGroup: Identifiable {
        let id: String
        let panes: [PaneItem]
    }

    private enum AddTargetKind: String, CaseIterable, Identifiable {
        case ssh
        case local

        var id: String { rawValue }

        var title: String {
            switch self {
            case .ssh:
                return "SSH"
            case .local:
                return "Local"
            }
        }
    }

    private var palette: CockpitPalette {
        CockpitPalette.forScheme(colorScheme)
    }

    private struct SessionKillRequest: Identifiable {
        let target: String
        let sessionName: String

        var id: String { "\(target)|\(sessionName)" }
    }

    private struct SessionRenameRequest: Identifiable {
        let target: String
        let sessionName: String

        var id: String { "\(target)|\(sessionName)" }
    }

    private struct PaneRenameRequest: Identifiable {
        let pane: PaneItem

        var id: String { pane.id }
    }

    var body: some View {
        ZStack {
            WindowBackdropView()
                .ignoresSafeArea()

            workspaceBoard
                .padding(0)
                .padding(.top, -max(0, windowTopInset))
                .ignoresSafeArea(.container, edges: .all)
        }
        .background(WindowStyleConfigurator { inset in
            let normalized = max(0, min(80, inset))
            if abs(normalized - windowTopInset) > 0.5 {
                windowTopInset = normalized
            }
        })
        .preferredColorScheme(.dark)
        .onAppear {
            selectDefaultPaneIfNeeded()
        }
        .onChange(of: model.panes.count) { _, _ in
            selectDefaultPaneIfNeeded()
        }
        .onChange(of: model.sessionSections.map(\.id)) { _, ids in
            let live = Set(ids)
            collapsedSessionIDs = collapsedSessionIDs.intersection(live)
        }
        .confirmationDialog(
            "Confirm Session Kill",
            isPresented: Binding(
                get: { pendingKillSession != nil },
                set: { newValue in
                    if !newValue {
                        pendingKillSession = nil
                    }
                }
            ),
            titleVisibility: .visible
        ) {
            if let request = pendingKillSession {
                Button("Kill Session", role: .destructive) {
                    model.performKillSession(target: request.target, sessionName: request.sessionName)
                    pendingKillSession = nil
                }
            }
            Button("Cancel", role: .cancel) {
                pendingKillSession = nil
            }
        } message: {
            if let request = pendingKillSession {
                Text("This will terminate tmux session '\(request.sessionName)' on target '\(request.target)'.")
            }
        }
        .alert(
            "Rename Session",
            isPresented: Binding(
                get: { pendingSessionRename != nil },
                set: { newValue in
                    if !newValue {
                        pendingSessionRename = nil
                        sessionRenameDraft = ""
                    }
                }
            )
        ) {
            TextField("New session name", text: $sessionRenameDraft)
            Button("Cancel", role: .cancel) {
                pendingSessionRename = nil
                sessionRenameDraft = ""
            }
            Button("Rename") {
                if let request = pendingSessionRename {
                    model.performRenameSession(
                        target: request.target,
                        sessionName: request.sessionName,
                        newName: sessionRenameDraft
                    )
                }
                pendingSessionRename = nil
                sessionRenameDraft = ""
            }
        } message: {
            if let request = pendingSessionRename {
                Text("Rename '\(request.sessionName)'")
            }
        }
        .alert(
            "Rename Pane",
            isPresented: Binding(
                get: { pendingPaneRename != nil },
                set: { newValue in
                    if !newValue {
                        pendingPaneRename = nil
                        paneRenameDraft = ""
                    }
                }
            )
        ) {
            TextField("New pane name", text: $paneRenameDraft)
            Button("Cancel", role: .cancel) {
                pendingPaneRename = nil
                paneRenameDraft = ""
            }
            Button("Rename") {
                if let request = pendingPaneRename {
                    model.performRenamePane(request.pane, newName: paneRenameDraft)
                }
                pendingPaneRename = nil
                paneRenameDraft = ""
            }
        } message: {
            if let request = pendingPaneRename {
                Text("Rename \(request.pane.identity.paneID)")
            }
        }
        .sheet(item: $detailPane) { pane in
            paneDetailSheet(for: pane)
                .frame(minWidth: 460, minHeight: 340)
        }
        .sheet(isPresented: $showAddTargetSheet) {
            addTargetSheet
                .frame(minWidth: 460, minHeight: 320)
        }
    }

    private var workspaceBoard: some View {
        HSplitView {
            paneNavigatorPanel
                .frame(minWidth: 220, idealWidth: 250, maxWidth: 320)
                .zIndex(10)
            terminalWorkspacePanel
                .frame(minWidth: 560, maxWidth: .infinity)
                .zIndex(1)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background {
            ZStack {
                Rectangle().fill(.ultraThinMaterial).opacity(0.18)
                LinearGradient(
                    colors: [
                        Color(red: 0.05, green: 0.09, blue: 0.14).opacity(0.18),
                        Color(red: 0.02, green: 0.03, blue: 0.06).opacity(0.12),
                    ],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
            }
        }
        .overlay {
            Rectangle()
                .stroke(palette.workspaceStroke, lineWidth: 1)
        }
        .accessibilityIdentifier("workspace.board")
    }

    private var paneNavigatorPanel: some View {
        VStack(spacing: 10) {
            contentBoard
                .padding(.horizontal, 10)
                .padding(.top, 34)
            sidebarFooter
                .padding(.horizontal, 10)
                .padding(.bottom, 10)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .background(palette.sidebarFill)
        .overlay(alignment: .trailing) {
            Rectangle()
                .fill(palette.sidebarDivider)
                .frame(width: 1)
        }
        .accessibilityIdentifier("sidebar.panel")
    }

    private var terminalWorkspacePanel: some View {
        VStack(alignment: .leading, spacing: 0) {
            if model.selectedPane != nil {
                if let pane = model.selectedPane {
                    HStack(spacing: 8) {
                        Text(model.paneDisplayTitle(for: pane))
                            .font(.system(size: 13, weight: .semibold, design: .rounded))
                            .foregroundStyle(palette.textPrimary)
                            .lineLimit(1)
                        Spacer(minLength: 0)
                        Button {
                            model.openSelectedPaneInExternalTerminal()
                        } label: {
                            Image(systemName: "arrow.up.right.square")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(palette.textMuted)
                                .frame(width: 22, height: 20)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .help("Open in External Terminal")
                        Text("\(pane.identity.sessionName)  \(pane.identity.paneID)")
                            .font(.system(size: 11, weight: .regular, design: .monospaced))
                            .foregroundStyle(palette.textMuted)
                    }
                    .frame(height: 22)
                    .padding(.horizontal, 10)
                    .padding(.top, 9)
                    .padding(.bottom, 8)
                    .transition(.opacity)
                    .accessibilityIdentifier("terminal.header")
                }

                if let pane = model.selectedPane,
                   model.nativeTmuxTerminalEnabled,
                   model.supportsNativeTmuxTerminal(for: pane) {
                    ZStack {
                        NativeTmuxTerminalView(
                            pane: pane,
                            darkMode: colorScheme == .dark,
                            content: model.outputPreview,
                            frameSource: model.terminalRenderSource,
                            renderVersion: model.terminalRenderVersion,
                            cursorX: model.terminalCursorX,
                            cursorY: model.terminalCursorY,
                            paneCols: model.terminalPaneCols,
                            paneRows: model.terminalPaneRows,
                            interactiveInputEnabled: model.interactiveTerminalInputEnabled,
                            onInputBytes: { bytes in
                                model.enqueueInteractiveInput(bytes: bytes)
                            },
                            onResize: { cols, rows in
                                model.performTerminalResize(cols: cols, rows: rows)
                            },
                            onFrameRendered: {
                                model.noteTerminalFrameRendered()
                            }
                        )
                        .id("native-terminal-\(pane.id)")
                        .accessibilityIdentifier("terminal.native")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(Color.clear)
                        .clipShape(Rectangle())
                        .clipped()
                        .padding(15)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .clipped()
                } else {
                    VStack(spacing: 8) {
                        Image(systemName: "exclamationmark.triangle")
                            .font(.system(size: 18, weight: .regular))
                            .foregroundStyle(palette.textMuted)
                        Text("Native tmux terminal is available only for local targets.")
                            .font(.system(size: 12, weight: .semibold, design: .rounded))
                            .foregroundStyle(palette.textSecondary)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(palette.surfaceMuted)
                    )
                    .accessibilityIdentifier("terminal.unsupported")
                }

                if !model.errorMessage.isEmpty {
                    Text(model.errorMessage)
                        .font(.system(size: 12, weight: .regular, design: .rounded))
                        .foregroundStyle(palette.errorText)
                        .textSelection(.enabled)
                        .padding(.horizontal, 10)
                        .padding(.bottom, 10)
                } else if !model.infoMessage.isEmpty {
                    Text(model.infoMessage)
                        .font(.system(size: 12, weight: .regular, design: .rounded))
                        .foregroundStyle(palette.infoText)
                        .padding(.horizontal, 10)
                        .padding(.bottom, 10)
                }
            } else {
                VStack(spacing: 8) {
                    Image(systemName: "terminal")
                        .font(.system(size: 22, weight: .regular))
                        .foregroundStyle(palette.textMuted)
                    Text("Select a pane to open terminal view")
                        .font(.system(size: 13, weight: .semibold, design: .rounded))
                        .foregroundStyle(palette.textSecondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(palette.surfaceMuted)
                )
                .accessibilityIdentifier("terminal.empty")
            }
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .background {
            ZStack {
                Rectangle().fill(.ultraThinMaterial).opacity(colorScheme == .dark ? 0.22 : 0.10)
                Rectangle().fill(Color.black.opacity(colorScheme == .dark ? 0.54 : 0.18))
            }
        }
        .clipped()
        .accessibilityIdentifier("terminal.panel")
    }

    private var contentBoard: some View {
        VStack(alignment: .leading, spacing: 8) {
            sidebarSectionHeader
            statusFilterChips

            ScrollView {
                LazyVStack(spacing: 8) {
                    switch model.viewMode {
                    case .bySession:
                        ForEach(model.sessionSections) { section in
                            sessionSection(section)
                        }
                    case .byChronological:
                        chronologicalSection
                    }
                }
                .padding(.vertical, 4)
            }
            .scrollIndicators(.hidden)
        }
    }

    private var statusFilterChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(AppViewModel.StatusFilter.allCases) { filter in
                    let active = model.statusFilter == filter
                    Button {
                        model.statusFilter = filter
                    } label: {
                        Text(filter.title)
                            .font(.system(size: 11, weight: .semibold, design: .rounded))
                            .foregroundStyle(active ? palette.textPrimary : palette.textSecondary)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 5)
                            .background(
                                Capsule(style: .continuous)
                                    .fill(active ? palette.rowSelectedFill : palette.rowFill)
                            )
                            .overlay(
                                Capsule(style: .continuous)
                                    .stroke(active ? palette.rowSelectedStroke : palette.rowHoverStroke.opacity(0.6), lineWidth: 1)
                            )
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 2)
        }
        .padding(.bottom, 2)
        .accessibilityIdentifier("sidebar.status_filters")
    }

    private var sidebarSectionHeader: some View {
        HStack(spacing: 8) {
            Text("Sessions")
                .font(.system(size: 15, weight: .semibold, design: .rounded))
                .foregroundStyle(palette.textPrimary)
                .accessibilityIdentifier("sidebar.sessions_title")
            Spacer(minLength: 0)
            Button {
                resetAddTargetForm()
                showAddTargetSheet = true
            } label: {
                Image(systemName: "folder.badge.plus")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.textMuted)
                    .frame(width: 30, height: 26)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Add target")
            .accessibilityIdentifier("sidebar.add_target_button")

            Button {
                showSortPopover.toggle()
            } label: {
                Image(systemName: "line.3.horizontal.decrease")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.textMuted)
                    .frame(width: 30, height: 26)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Organize")
            .accessibilityIdentifier("sidebar.organize_button")
            .popover(isPresented: $showSortPopover, arrowEdge: .top) {
                sortPopoverContent
            }
        }
        .padding(.horizontal, 2)
        .padding(.bottom, 8)
        .accessibilityIdentifier("sidebar.header")
    }

    private var sidebarFooter: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                HStack(spacing: 8) {
                    Image(systemName: "switch.2")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(palette.textPrimary)
                        .frame(width: 22, height: 22)
                        .background(
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(palette.rowFill)
                        )
                    Text("AGTMUX")
                        .font(.system(size: 11, weight: .semibold, design: .rounded))
                        .foregroundStyle(palette.textMuted)
                }
                Spacer(minLength: 0)
                Button {
                    showSettingsPopover.toggle()
                } label: {
                    Image(systemName: "gearshape")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(palette.textSecondary)
                        .frame(width: 26, height: 24)
                        .background(
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(palette.rowFill)
                        )
                }
                .buttonStyle(.plain)
                .help("View settings")
                .accessibilityIdentifier("sidebar.settings_button")
                .popover(isPresented: $showSettingsPopover, arrowEdge: .bottom) {
                    settingsPopoverContent
                }
            }
            if model.showTechnicalDetails {
                Text(model.terminalPerformanceSummary)
                    .font(.system(size: 10, weight: .medium, design: .monospaced))
                    .foregroundStyle(model.terminalPerformanceWithinBudget ? palette.textMuted : palette.attention)
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, 2)
        .padding(.bottom, 1)
        .accessibilityIdentifier("sidebar.footer")
    }

    private func menuSectionTitle(_ title: String) -> some View {
        Text(title)
            .font(.system(size: 11, weight: .semibold, design: .rounded))
            .foregroundStyle(palette.textMuted)
            .padding(.horizontal, 3)
            .padding(.top, 6)
            .padding(.bottom, 2)
    }

    private func menuActionRow(
        id: String,
        title: String,
        systemImage: String,
        selected: Bool,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 7) {
                Image(systemName: systemImage)
                    .font(.system(size: 11, weight: .regular))
                    .foregroundStyle(palette.textSecondary)
                    .frame(width: 13, alignment: .center)
                Text(title)
                    .font(.system(size: 12, weight: .regular, design: .rounded))
                    .foregroundStyle(palette.textPrimary)
                    .lineLimit(1)
                Spacer(minLength: 0)
                if selected {
                    Image(systemName: "checkmark")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(palette.textPrimary)
                }
            }
            .frame(maxWidth: .infinity, minHeight: 22, maxHeight: 22, alignment: .leading)
            .padding(.horizontal, 5)
            .contentShape(Rectangle())
            .background(
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .fill(hoveringMenuRowID == id ? palette.rowHoverFill : Color.clear)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .stroke(hoveringMenuRowID == id ? palette.rowHoverStroke : Color.clear, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .onHover { inside in
            if inside {
                hoveringMenuRowID = id
            } else if hoveringMenuRowID == id {
                hoveringMenuRowID = nil
            }
        }
    }

    private func flatMenuCard<Content: View>(
        width: CGFloat,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            content()
        }
        .padding(8)
        .frame(width: width, alignment: .leading)
    }

    private var sortPopoverContent: some View {
        flatMenuCard(width: 288) {
            menuSectionTitle("Organize")
            menuActionRow(
                id: "organize.by_session",
                title: "By Session",
                systemImage: "folder",
                selected: model.viewMode == .bySession
            ) {
                model.viewMode = .bySession
            }
            menuActionRow(
                id: "organize.chrono",
                title: "Chronological List",
                systemImage: "clock",
                selected: model.viewMode == .byChronological
            ) {
                model.viewMode = .byChronological
            }
            Divider().padding(.vertical, 4)
            menuSectionTitle("Sort by")
            menuActionRow(
                id: "organize.sort.manual",
                title: "Manual Order",
                systemImage: "arrow.up.arrow.down",
                selected: model.sessionSortMode == .stable
            ) {
                model.sessionSortMode = .stable
            }
            menuActionRow(
                id: "organize.sort.updated",
                title: "Updated",
                systemImage: "clock.arrow.circlepath",
                selected: model.sessionSortMode == .recentActivity
            ) {
                model.sessionSortMode = .recentActivity
            }
            menuActionRow(
                id: "organize.sort.name",
                title: "Name",
                systemImage: "textformat",
                selected: model.sessionSortMode == .name
            ) {
                model.sessionSortMode = .name
            }
            Divider().padding(.vertical, 4)
            menuActionRow(
                id: "organize.show_tmux_windows",
                title: "Show tmux window groups",
                systemImage: "rectangle.3.group",
                selected: model.showWindowGroupBackground
            ) {
                model.showWindowGroupBackground.toggle()
            }
        }
    }

    private var settingsPopoverContent: some View {
        flatMenuCard(width: 180) {
            menuSectionTitle("Settings")
            Text("No settings for now.")
                .font(.system(size: 11, weight: .regular, design: .rounded))
                .foregroundStyle(palette.textMuted)
                .padding(.horizontal, 5)
                .padding(.vertical, 8)
        }
    }

    private var addTargetSheet: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Add Target")
                .font(.system(size: 18, weight: .bold, design: .rounded))
                .foregroundStyle(palette.textPrimary)

            VStack(alignment: .leading, spacing: 6) {
                Text("Target Name")
                    .font(.system(size: 11, weight: .semibold, design: .rounded))
                    .foregroundStyle(palette.textMuted)
                TextField("e.g. dev-vm", text: $newTargetName)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 13, weight: .regular, design: .rounded))
            }

            VStack(alignment: .leading, spacing: 6) {
                Text("Kind")
                    .font(.system(size: 11, weight: .semibold, design: .rounded))
                    .foregroundStyle(palette.textMuted)
                Picker("Kind", selection: $newTargetKind) {
                    ForEach(AddTargetKind.allCases) { kind in
                        Text(kind.title).tag(kind)
                    }
                }
                .pickerStyle(.segmented)
            }

            if newTargetKind == .ssh {
                VStack(alignment: .leading, spacing: 6) {
                    Text("SSH Connection Ref")
                        .font(.system(size: 11, weight: .semibold, design: .rounded))
                        .foregroundStyle(palette.textMuted)
                    TextField("vm-host or ssh://vm-host", text: $newTargetConnectionRef)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 13, weight: .regular, design: .rounded))
                    Text("Uses your local SSH config aliases.")
                        .font(.system(size: 11, weight: .regular, design: .rounded))
                        .foregroundStyle(palette.textMuted)
                }
            }

            Toggle("Set as default target", isOn: $newTargetIsDefault)
                .toggleStyle(.switch)
                .font(.system(size: 12, weight: .regular, design: .rounded))

            if newTargetKind == .ssh {
                Toggle("Connect immediately after add", isOn: $newTargetConnectAfterAdd)
                    .toggleStyle(.switch)
                    .font(.system(size: 12, weight: .regular, design: .rounded))
            }

            Spacer(minLength: 0)

            HStack(spacing: 10) {
                Spacer(minLength: 0)
                Button("Cancel") {
                    showAddTargetSheet = false
                }
                .buttonStyle(.bordered)
                Button("Add Target") {
                    submitAddTarget()
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canSubmitAddTarget)
            }
        }
        .padding(18)
        .background(
            LinearGradient(
                colors: [palette.windowTop, palette.windowBottom],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
        )
    }

    private func sessionSection(_ section: AppViewModel.SessionSection) -> some View {
        let pinned = model.isSessionPinned(target: section.target, sessionName: section.sessionName)
        let targetHealth = model.targetHealth(for: section.target)
        let targetToken = section.target.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let showTargetLabel = !(targetToken == "local" || targetToken.isEmpty)
        let collapsed = collapsedSessionIDs.contains(section.id)
        let creatingPane = model.isPaneCreationInFlight(target: section.target, sessionName: section.sessionName)
        let hovering = hoveringSessionID == section.id
        let actionableAttentionCount = model.actionableAttentionCount(target: section.target, sessionName: section.sessionName)
        let anchorPaneID: String? = {
            if let selected = model.selectedPane,
               selected.identity.target == section.target,
               selected.identity.sessionName == section.sessionName {
                return selected.identity.paneID
            }
            return section.panes.first?.identity.paneID
        }()
        return VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 8) {
                HStack(spacing: 7) {
                    if creatingPane {
                        ProgressView()
                            .controlSize(.small)
                            .scaleEffect(0.75)
                            .frame(width: 12, height: 12)
                    } else if hovering {
                        Button {
                            toggleSessionCollapsed(section.id)
                        } label: {
                            Image(systemName: collapsed ? "chevron.right" : "chevron.down")
                                .font(.system(size: 10, weight: .regular))
                                .foregroundStyle(palette.textSecondary)
                                .frame(width: 12, height: 12)
                        }
                        .buttonStyle(.plain)
                        .help(collapsed ? "Expand session" : "Collapse session")
                    } else {
                        Image(systemName: "folder")
                            .font(.system(size: 10, weight: .regular))
                            .foregroundStyle(palette.textSecondary)
                            .frame(width: 12, height: 12)
                    }
                    Text(section.sessionName)
                        .font(.system(size: 13, weight: .regular, design: .rounded))
                        .foregroundStyle(palette.textPrimary)
                        .lineLimit(1)
                    if pinned {
                        Image(systemName: "pin.fill")
                            .font(.system(size: 8, weight: .regular))
                            .foregroundStyle(palette.idle)
                    }
                }
                .padding(.leading, 4)
                Spacer(minLength: 0)
                if showTargetLabel {
                    HStack(spacing: 5) {
                        Circle()
                            .fill(colorForTargetHealth(targetHealth))
                            .frame(width: 5, height: 5)
                        Text(section.target)
                            .font(.system(size: 9, weight: .regular, design: .monospaced))
                            .foregroundStyle(palette.textSecondary)
                            .lineLimit(1)
                    }
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(
                        Capsule(style: .continuous)
                            .fill(palette.rowFill)
                    )
                }
                if actionableAttentionCount > 0 {
                    Text("A\(actionableAttentionCount)")
                        .font(.system(size: 10, weight: .semibold, design: .rounded))
                        .foregroundStyle(palette.attention)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(
                            Capsule(style: .continuous)
                                .fill(palette.rowFill)
                        )
                        .overlay(
                            Capsule(style: .continuous)
                                .stroke(palette.attention.opacity(0.5), lineWidth: 1)
                        )
                }
                Group {
                    if creatingPane {
                        ProgressView()
                            .controlSize(.small)
                            .scaleEffect(0.75)
                            .frame(width: 22, height: 20)
                    } else {
                        Button {
                            model.performCreatePane(
                                target: section.target,
                                sessionName: section.sessionName,
                                anchorPaneID: anchorPaneID
                            )
                        } label: {
                            Image(systemName: "square.and.pencil")
                                .font(.system(size: 11, weight: .regular))
                                .foregroundStyle(palette.textMuted)
                                .frame(width: 22, height: 20)
                        }
                        .buttonStyle(.plain)
                        .help("Create New Pane")
                        .accessibilityIdentifier("sidebar.session.\(section.id).create_pane")
                    }
                }
                .opacity(hovering ? 1 : 0)
                .allowsHitTesting(hovering && !creatingPane)
            }
            .frame(minHeight: 26, maxHeight: 26, alignment: .center)
            .padding(.horizontal, 6)
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(hovering ? palette.rowHoverFill : Color.clear)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .stroke(hovering ? palette.rowHoverStroke : Color.clear, lineWidth: 1)
            )
            .contentShape(Rectangle())
            .onHover { inside in
                if inside {
                    hoveringSessionID = section.id
                    if sessionCursorActiveID != section.id {
                        if sessionCursorActiveID != nil {
                            NSCursor.pop()
                        }
                        NSCursor.openHand.push()
                        sessionCursorActiveID = section.id
                    }
                } else if hoveringSessionID == section.id {
                    hoveringSessionID = nil
                    if sessionCursorActiveID == section.id {
                        NSCursor.pop()
                        sessionCursorActiveID = nil
                    }
                }
            }
            .contextMenu {
                Button("Rename Session") { beginSessionRename(section) }
                Button(pinned ? "Unpin Session" : "Pin Session") {
                    model.setSessionPinned(target: section.target, sessionName: section.sessionName, pinned: !pinned)
                }
                Divider()
                Button("Kill Session", role: .destructive) {
                    requestSessionKill(section)
                }
            }
            .accessibilityIdentifier("sidebar.session.\(section.id)")

            if !collapsed {
                paneList(
                    section.panes,
                    showWindowGroups: model.showWindowGroupBackground,
                    indentWhenFlat: true
                )
                .transition(
                    .asymmetric(
                        insertion: .move(edge: .top).combined(with: .opacity),
                        removal: .move(edge: .top).combined(with: .opacity)
                    )
                )
            }
        }
        .padding(.bottom, 2)
        .animation(.easeInOut(duration: 0.18), value: collapsed)
        .overlay(alignment: .top) {
            if dropTargetSessionID == section.id,
               let dragging = draggingSessionID,
               dragging != section.id {
                Rectangle()
                    .fill(palette.idle.opacity(0.85))
                    .frame(height: 2)
                    .padding(.horizontal, 4)
            }
        }
        .contentShape(Rectangle())
        .onDrag {
            draggingSessionID = section.id
            return NSItemProvider(object: NSString(string: section.id))
        }
        .onDrop(
            of: [UTType.plainText],
            delegate: SessionReorderDropDelegate(
                targetSessionID: section.id,
                draggingSessionID: $draggingSessionID,
                dropTargetSessionID: $dropTargetSessionID
            ) { sourceID, destinationID in
                model.reorderSessionSections(sourceID: sourceID, destinationID: destinationID)
            }
        )
        .onDisappear {
            if sessionCursorActiveID == section.id {
                NSCursor.pop()
                sessionCursorActiveID = nil
            }
        }
        .opacity(draggingSessionID == section.id ? 0.8 : 1.0)
    }

    private func statusSection(category: String, panes: [PaneItem]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(model.categoryLabel(category))
                .font(.system(size: 11, weight: .semibold, design: .rounded))
                .foregroundStyle(palette.textMuted)
                .padding(.leading, 4)
            paneList(panes, showWindowGroups: false, indentWhenFlat: false)
        }
        .padding(.bottom, 2)
    }

    @ViewBuilder
    private var chronologicalSection: some View {
        let panes = model.chronologicalPanes
        if panes.isEmpty {
            Text("No panes")
                .font(.system(size: 12, weight: .regular, design: .rounded))
                .foregroundStyle(palette.textMuted)
                .padding(.leading, 4)
        } else {
            paneList(panes, showWindowGroups: false, indentWhenFlat: false)
        }
    }

    private func paneList(_ panes: [PaneItem], showWindowGroups: Bool, indentWhenFlat: Bool) -> some View {
        VStack(spacing: showWindowGroups ? 6 : 0) {
            if showWindowGroups {
                ForEach(windowGroups(for: panes)) { group in
                    VStack(spacing: 0) {
                        ForEach(group.panes, id: \.id) { pane in
                            paneRow(pane, titleCandidates: group.panes, compactSpacing: false)
                        }
                    }
                    .padding(.horizontal, 4)
                    .padding(.vertical, 5)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(palette.windowGroupFill)
                    )
                }
            } else {
                ForEach(panes, id: \.id) { pane in
                    paneRow(pane, titleCandidates: panes, compactSpacing: true)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.leading, (!showWindowGroups && indentWhenFlat) ? 8 : 0)
    }

    private func paneRow(_ pane: PaneItem, titleCandidates: [PaneItem], compactSpacing: Bool) -> some View {
        let category = model.displayCategory(for: pane)
        let selected = model.selectedPane?.id == pane.id
        let hovered = hoveringPaneID == pane.id
        let killInFlight = model.isPaneKillInFlight(pane.id)
        return Button {
            guard !killInFlight else {
                return
            }
            withAnimation(.easeInOut(duration: 0.14)) {
                model.selectedPane = pane
            }
        } label: {
            HStack(spacing: 10) {
                if killInFlight {
                    ProgressView()
                        .controlSize(.mini)
                        .scaleEffect(0.75)
                        .frame(width: 8, height: 8)
                } else {
                    Circle()
                        .fill(selected ? colorForCategory(category) : colorForCategory(category).opacity(0.9))
                        .frame(width: 8, height: 8)
                }

                Text(model.paneDisplayTitle(for: pane, among: titleCandidates))
                    .font(.system(size: 13, weight: selected ? .semibold : .regular, design: .rounded))
                    .foregroundStyle(killInFlight ? palette.textMuted : (selected ? palette.textPrimary : palette.textSecondary))
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)

                if model.needsUserAction(for: pane) {
                    Image(systemName: "exclamationmark.circle.fill")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(palette.attention)
                }

                Text(killInFlight ? "killing..." : model.lastActiveShortLabel(for: pane))
                    .font(.system(size: 11, weight: .regular, design: .rounded))
                    .foregroundStyle(killInFlight ? palette.attention : (selected ? palette.textSecondary : palette.textMuted))
                    .monospacedDigit()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .padding(.vertical, compactSpacing ? 6 : 8)
            .padding(.horizontal, 10)
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(selected ? palette.rowSelectedFill : (hovered ? palette.rowHoverFill : Color.clear))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .stroke(selected ? palette.rowSelectedStroke : (hovered ? palette.rowHoverStroke : Color.clear), lineWidth: 1)
            )
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .opacity(killInFlight ? 0.72 : 1.0)
        .disabled(killInFlight)
        .onHover { inside in
            if inside {
                hoveringPaneID = pane.id
            } else if hoveringPaneID == pane.id {
                hoveringPaneID = nil
            }
        }
        .contextMenu {
            Button("Open") {
                model.selectedPane = pane
            }
            .disabled(killInFlight)
            Button("Open in External Terminal") {
                model.selectedPane = pane
                model.openSelectedPaneInExternalTerminal()
            }
            .disabled(killInFlight)
            Divider()
            Button("Rename Pane") {
                beginPaneRename(pane)
            }
            .disabled(killInFlight)
            Button("Kill Pane", role: .destructive) {
                model.performKillPane(pane)
            }
            .disabled(killInFlight)
            Divider()
            Button("Pane Details") {
                detailPane = pane
            }
            Button("Copy Pane Path") {
                copyPanePath(pane)
            }
        }
        .help("\(pane.identity.target)/\(pane.identity.sessionName)/\(pane.identity.paneID)")
        .buttonStyle(.plain)
        .accessibilityIdentifier("sidebar.pane.\(pane.id)")
    }

    private func windowGroups(for panes: [PaneItem]) -> [WindowPaneGroup] {
        var order: [String] = []
        var grouped: [String: [PaneItem]] = [:]
        for pane in panes {
            let key = pane.identity.windowID
            if grouped[key] == nil {
                order.append(key)
                grouped[key] = []
            }
            grouped[key, default: []].append(pane)
        }
        return order.map { key in
            WindowPaneGroup(id: key, panes: grouped[key] ?? [])
        }
    }

    private func requestSessionKill(_ section: AppViewModel.SessionSection) {
        pendingKillSession = SessionKillRequest(
            target: section.target,
            sessionName: section.sessionName
        )
    }

    private func beginSessionRename(_ section: AppViewModel.SessionSection) {
        pendingSessionRename = SessionRenameRequest(target: section.target, sessionName: section.sessionName)
        sessionRenameDraft = section.sessionName
    }

    private func beginPaneRename(_ pane: PaneItem) {
        pendingPaneRename = PaneRenameRequest(pane: pane)
        paneRenameDraft = model.paneDisplayTitle(for: pane)
    }

    private func toggleSessionCollapsed(_ sessionID: String) {
        withAnimation(.easeInOut(duration: 0.2)) {
            if collapsedSessionIDs.contains(sessionID) {
                collapsedSessionIDs.remove(sessionID)
            } else {
                collapsedSessionIDs.insert(sessionID)
            }
        }
    }

    private func copyPanePath(_ pane: PaneItem) {
        let value = "\(pane.identity.target)/\(pane.identity.sessionName)/\(pane.identity.windowID)/\(pane.identity.paneID)"
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(value, forType: .string)
        model.infoMessage = "Copied pane path"
    }

    private var canSubmitAddTarget: Bool {
        let name = newTargetName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else {
            return false
        }
        if newTargetKind == .ssh {
            let conn = newTargetConnectionRef.trimmingCharacters(in: .whitespacesAndNewlines)
            return !conn.isEmpty
        }
        return true
    }

    private func submitAddTarget() {
        model.performAddTarget(
            name: newTargetName,
            kind: newTargetKind.rawValue,
            connectionRef: newTargetConnectionRef,
            isDefault: newTargetIsDefault,
            connectAfterAdd: newTargetKind == .ssh ? newTargetConnectAfterAdd : false
        )
        showAddTargetSheet = false
    }

    private func resetAddTargetForm() {
        newTargetName = ""
        newTargetKind = .ssh
        newTargetConnectionRef = ""
        newTargetIsDefault = false
        newTargetConnectAfterAdd = true
    }

    private func paneDetailSheet(for pane: PaneItem) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Pane Details")
                .font(.system(size: 18, weight: .bold, design: .rounded))
                .foregroundStyle(palette.textPrimary)
            detailRow("Path", "\(pane.identity.target)/\(pane.identity.sessionName)/\(pane.identity.windowID)/\(pane.identity.paneID)")
            detailRow("Category", model.categoryLabel(model.displayCategory(for: pane)))
            detailRow("State", model.activityState(for: pane))
            detailRow("Reason", model.stateReason(for: pane))
            detailRow("Last Active", model.lastActiveLabel(for: pane))
            detailRow("Title", model.paneDisplayTitle(for: pane))
            if let runtime = pane.runtimeID, !runtime.isEmpty {
                detailRow("Runtime ID", runtime)
            }
            if let agent = pane.agentType, !agent.isEmpty {
                detailRow("Agent", agent)
            }
            Spacer(minLength: 0)
            HStack {
                Spacer(minLength: 0)
                Button("Close") {
                    detailPane = nil
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(18)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(
            LinearGradient(
                colors: [palette.windowTop, palette.windowBottom],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
        )
    }

    private func detailRow(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label.uppercased())
                .font(.system(size: 10, weight: .bold, design: .rounded))
                .foregroundStyle(palette.textMuted)
            Text(value)
                .font(.system(size: 12, weight: .regular, design: .monospaced))
                .foregroundStyle(palette.textPrimary)
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(palette.surfaceMuted)
        )
    }

    private func colorForCategory(_ category: String) -> Color {
        switch category {
        case "attention":
            return palette.attention
        case "running":
            return palette.running
        case "idle":
            return palette.idle
        case "unmanaged":
            return palette.unmanaged
        default:
            return palette.unknown
        }
    }

    private func colorForTargetHealth(_ health: String) -> Color {
        switch health {
        case "ok":
            return palette.running
        case "degraded":
            return palette.attention
        case "down":
            return palette.errorText
        default:
            return palette.textMuted
        }
    }

    private func selectDefaultPaneIfNeeded() {
        guard model.selectedPane == nil else {
            return
        }
        if let attention = model.panes.first(where: { model.needsUserAction(for: $0) }) {
            model.selectedPane = attention
            return
        }
        model.selectedPane = model.panes.first
    }
}

private struct SessionReorderDropDelegate: DropDelegate {
    let targetSessionID: String
    @Binding var draggingSessionID: String?
    @Binding var dropTargetSessionID: String?
    let onMove: (_ sourceID: String, _ destinationID: String) -> Void

    func dropEntered(info _: DropInfo) {
        guard let sourceID = draggingSessionID else {
            return
        }
        guard sourceID != targetSessionID else {
            if dropTargetSessionID == targetSessionID {
                dropTargetSessionID = nil
            }
            return
        }
        dropTargetSessionID = targetSessionID
    }

    func dropExited(info _: DropInfo) {
        if dropTargetSessionID == targetSessionID {
            dropTargetSessionID = nil
        }
    }

    func performDrop(info _: DropInfo) -> Bool {
        defer {
            dropTargetSessionID = nil
            draggingSessionID = nil
        }
        guard let sourceID = draggingSessionID else {
            return false
        }
        guard sourceID != targetSessionID else {
            return false
        }
        onMove(sourceID, targetSessionID)
        return true
    }

    func dropUpdated(info _: DropInfo) -> DropProposal? {
        DropProposal(operation: .move)
    }

    func validateDrop(info _: DropInfo) -> Bool {
        true
    }
}

private struct CockpitPalette {
    let windowTop: Color
    let windowBottom: Color
    let workspaceTintTop: Color
    let workspaceTintBottom: Color
    let workspaceStroke: Color
    let sidebarFill: Color
    let sidebarDivider: Color
    let surfaceMuted: Color
    let windowGroupFill: Color
    let rowFill: Color
    let rowHoverFill: Color
    let rowHoverStroke: Color
    let rowSelectedFill: Color
    let rowSelectedStroke: Color
    let terminalBackground: Color
    let terminalDividerLeading: Color
    let terminalDividerTrailing: Color
    let terminalText: Color
    let textPrimary: Color
    let textSecondary: Color
    let textMuted: Color
    let infoText: Color
    let errorText: Color
    let attention: Color
    let running: Color
    let idle: Color
    let unmanaged: Color
    let unknown: Color

    static func forScheme(_ scheme: ColorScheme) -> CockpitPalette {
        if scheme == .dark {
            return CockpitPalette(
                windowTop: Color(red: 0.05, green: 0.07, blue: 0.11),
                windowBottom: Color(red: 0.08, green: 0.09, blue: 0.13),
                workspaceTintTop: Color(red: 0.03, green: 0.05, blue: 0.09),
                workspaceTintBottom: Color(red: 0.04, green: 0.06, blue: 0.10),
                workspaceStroke: Color.white.opacity(0.08),
                sidebarFill: Color(red: 0.10, green: 0.22, blue: 0.33).opacity(0.34),
                sidebarDivider: Color.white.opacity(0.06),
                surfaceMuted: Color.white.opacity(0.05),
                windowGroupFill: Color.white.opacity(0.045),
                rowFill: Color.white.opacity(0.045),
                rowHoverFill: Color.white.opacity(0.085),
                rowHoverStroke: Color.white.opacity(0.16),
                rowSelectedFill: Color(red: 0.22, green: 0.39, blue: 0.88).opacity(0.42),
                rowSelectedStroke: Color(red: 0.47, green: 0.62, blue: 1.0).opacity(0.85),
                terminalBackground: Color.black.opacity(0.35),
                terminalDividerLeading: Color.white.opacity(0.18),
                terminalDividerTrailing: Color.white.opacity(0.04),
                terminalText: Color.white.opacity(0.93),
                textPrimary: Color.white.opacity(0.95),
                textSecondary: Color.white.opacity(0.82),
                textMuted: Color.white.opacity(0.56),
                infoText: Color(red: 0.64, green: 0.90, blue: 0.75),
                errorText: Color(red: 1.0, green: 0.68, blue: 0.64),
                attention: Color(red: 1.0, green: 0.54, blue: 0.44),
                running: Color(red: 0.42, green: 0.90, blue: 0.62),
                idle: Color(red: 0.58, green: 0.78, blue: 1.0),
                unmanaged: Color(red: 0.76, green: 0.76, blue: 0.79),
                unknown: Color(red: 0.84, green: 0.82, blue: 0.88)
            )
        }
        return CockpitPalette(
            windowTop: Color(red: 0.92, green: 0.95, blue: 0.98),
            windowBottom: Color(red: 0.88, green: 0.92, blue: 0.97),
            workspaceTintTop: Color(red: 0.89, green: 0.93, blue: 0.99),
            workspaceTintBottom: Color(red: 0.85, green: 0.90, blue: 0.97),
            workspaceStroke: Color.black.opacity(0.05),
            sidebarFill: Color(red: 0.82, green: 0.88, blue: 0.96).opacity(0.50),
            sidebarDivider: Color.black.opacity(0.05),
            surfaceMuted: Color.black.opacity(0.03),
            windowGroupFill: Color.black.opacity(0.04),
            rowFill: Color.black.opacity(0.025),
            rowHoverFill: Color.black.opacity(0.06),
            rowHoverStroke: Color.black.opacity(0.10),
            rowSelectedFill: Color(red: 0.58, green: 0.72, blue: 0.98).opacity(0.34),
            rowSelectedStroke: Color(red: 0.34, green: 0.52, blue: 0.90).opacity(0.78),
            terminalBackground: Color.white.opacity(0.92),
            terminalDividerLeading: Color.black.opacity(0.15),
            terminalDividerTrailing: Color.black.opacity(0.04),
            terminalText: Color.black.opacity(0.80),
            textPrimary: Color.black.opacity(0.82),
            textSecondary: Color.black.opacity(0.70),
            textMuted: Color.black.opacity(0.46),
            infoText: Color(red: 0.12, green: 0.50, blue: 0.32),
            errorText: Color(red: 0.73, green: 0.24, blue: 0.19),
            attention: Color(red: 0.86, green: 0.36, blue: 0.29),
            running: Color(red: 0.23, green: 0.63, blue: 0.38),
            idle: Color(red: 0.25, green: 0.48, blue: 0.84),
            unmanaged: Color(red: 0.48, green: 0.48, blue: 0.53),
            unknown: Color(red: 0.58, green: 0.52, blue: 0.63)
        )
    }
}

private struct WindowBackdropView: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .hudWindow
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {
        nsView.material = .hudWindow
        nsView.blendingMode = .behindWindow
        nsView.state = .active
    }
}

private struct WindowStyleConfigurator: NSViewRepresentable {
    let onInsetChanged: (CGFloat) -> Void

    init(onInsetChanged: @escaping (CGFloat) -> Void = { _ in }) {
        self.onInsetChanged = onInsetChanged
    }

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        DispatchQueue.main.async {
            configure(window: view.window)
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async {
            configure(window: nsView.window)
        }
    }

    private func configure(window: NSWindow?) {
        guard let window else {
            return
        }
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = true
        window.styleMask.insert(.fullSizeContentView)
        applyTrafficLightInsets(window)
        let inset = max(0, window.frame.height - window.contentLayoutRect.height)
        onInsetChanged(inset)
    }

    private func applyTrafficLightInsets(_ window: NSWindow) {
        guard
            let closeButton = window.standardWindowButton(.closeButton),
            let minimizeButton = window.standardWindowButton(.miniaturizeButton),
            let zoomButton = window.standardWindowButton(.zoomButton),
            let container = closeButton.superview
        else {
            return
        }

        let buttons = [closeButton, minimizeButton, zoomButton]
        let leftInset: CGFloat = 15
        let topInset: CGFloat = 15
        let spacing: CGFloat = 6
        let buttonWidth = closeButton.frame.width
        let buttonHeight = closeButton.frame.height
        let y = max(0, container.bounds.height - buttonHeight - topInset)

        for (index, button) in buttons.enumerated() {
            let x = leftInset + CGFloat(index) * (buttonWidth + spacing)
            button.setFrameOrigin(NSPoint(x: x, y: y))
        }
    }
}

private struct AppPaths {
    let baseDir: URL
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
            baseDir: baseURL,
            socketPath: baseURL.appendingPathComponent("agtmuxd.sock").path,
            dbPath: baseURL.appendingPathComponent("state.db").path,
            logPath: baseURL.appendingPathComponent("agtmuxd.log").path
        )
    }
}

private func appendLauncherLog(baseDir: URL, message: String) {
    let logURL = baseDir.appendingPathComponent("launcher.log", isDirectory: false)
    let timestamp = launcherTimestampFormatter.string(from: Date())
    let line = "\(timestamp) \(message)\n"
    guard let data = line.data(using: .utf8) else {
        return
    }
    if FileManager.default.fileExists(atPath: logURL.path) == false {
        _ = FileManager.default.createFile(atPath: logURL.path, contents: Data())
    }
    if let handle = try? FileHandle(forWritingTo: logURL) {
        handle.seekToEndOfFile()
        handle.write(data)
        try? handle.close()
    }
}

private let launcherTimestampFormatter: DateFormatter = {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.timeZone = TimeZone(secondsFromGMT: 0)
    formatter.dateFormat = "yyyy-MM-dd'T'HH:mm:ss.SSS'Z'"
    return formatter
}()
