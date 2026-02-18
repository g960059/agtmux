import AppKit
import Darwin
import SwiftUI

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
                nativeTmuxTerminalEnabled: true
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
    @State private var pendingKillAction: KillAction?
    @State private var pendingKillPane: PaneItem?
    @State private var pendingKillSession: SessionKillRequest?
    @State private var detailPane: PaneItem?
    @State private var windowTopInset: CGFloat = 0
    @State private var hoveringPaneID: String?
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
                return "This sends INT and may interrupt the process in this pane."
            case .signalTERM:
                return "This sends TERM and may terminate the process in this pane."
            }
        }
    }

    private struct SessionKillRequest: Identifiable {
        let target: String
        let sessionName: String

        var id: String { "\(target)|\(sessionName)" }
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
        .confirmationDialog(
            "Confirm Kill Action",
            isPresented: Binding(
                get: { pendingKillAction != nil },
                set: { newValue in
                    if !newValue {
                        pendingKillAction = nil
                        pendingKillPane = nil
                    }
                }
            ),
            titleVisibility: .visible
        ) {
            if let action = pendingKillAction {
                Button(action.title, role: .destructive) {
                    switch action {
                    case .keyINT:
                        model.performKillKeyINT(for: pendingKillPane)
                    case .signalTERM:
                        model.performKillSignalTERM(for: pendingKillPane)
                    }
                    pendingKillAction = nil
                    pendingKillPane = nil
                }
            }
            Button("Cancel", role: .cancel) {
                pendingKillAction = nil
                pendingKillPane = nil
            }
        } message: {
            if let action = pendingKillAction {
                Text(action.message)
            }
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
                }

                if let pane = model.selectedPane,
                   model.nativeTmuxTerminalEnabled,
                   model.supportsNativeTmuxTerminal(for: pane) {
                    ZStack {
                        NativeTmuxTerminalView(
                            pane: pane,
                            darkMode: colorScheme == .dark,
                            content: model.outputPreview,
                            cursorX: model.terminalCursorX,
                            cursorY: model.terminalCursorY,
                            paneCols: model.terminalPaneCols,
                            paneRows: model.terminalPaneRows,
                            interactiveInputEnabled: model.interactiveTerminalInputEnabled,
                            onInputBytes: { bytes in
                                model.performInteractiveInput(bytes: bytes)
                            },
                            onResize: { cols, rows in
                                model.performTerminalResize(cols: cols, rows: rows)
                            }
                        )
                        .id("native-terminal-\(pane.id)")
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
            }
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .background {
            ZStack {
                Rectangle().fill(.ultraThinMaterial).opacity(0.10)
                Rectangle().fill(Color.black.opacity(colorScheme == .dark ? 0.14 : 0.06))
            }
        }
        .clipped()
    }

    private var contentBoard: some View {
        VStack(alignment: .leading, spacing: 8) {
            sidebarSectionHeader

            ScrollView {
                LazyVStack(spacing: 8) {
                    switch model.viewMode {
                    case .bySession:
                        ForEach(model.sessionSections) { section in
                            sessionSection(section)
                        }
                    case .byStatus:
                        ForEach(Array(model.statusGroups.enumerated()), id: \.offset) { _, entry in
                            statusSection(category: entry.0, panes: entry.1)
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

    private var sidebarSectionHeader: some View {
        HStack(spacing: 8) {
            Text("Sessions")
                .font(.system(size: 15, weight: .semibold, design: .rounded))
                .foregroundStyle(palette.textPrimary)
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
            .help("Sort / filter")
            .popover(isPresented: $showSortPopover, arrowEdge: .top) {
                sortPopoverContent
            }
        }
        .padding(.horizontal, 2)
        .padding(.bottom, 8)
    }

    private var sidebarFooter: some View {
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
            .popover(isPresented: $showSettingsPopover, arrowEdge: .bottom) {
                settingsPopoverContent
            }
        }
        .padding(.horizontal, 2)
        .padding(.bottom, 1)
    }

    private var sortPopoverContent: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Organize")
                .font(.system(size: 12, weight: .semibold, design: .rounded))
                .foregroundStyle(palette.textPrimary)
            Picker("View", selection: $model.viewMode) {
                Text("By Session").tag(AppViewModel.ViewMode.bySession)
                Text("By Status").tag(AppViewModel.ViewMode.byStatus)
                Text("By Chronological").tag(AppViewModel.ViewMode.byChronological)
            }
            .pickerStyle(.menu)
            Picker("Session Order", selection: $model.sessionSortMode) {
                Text("Stable").tag(AppViewModel.SessionSortMode.stable)
                Text("Recent Activity").tag(AppViewModel.SessionSortMode.recentActivity)
                Text("Name").tag(AppViewModel.SessionSortMode.name)
            }
            .pickerStyle(.menu)
            Toggle("Group By tmux Window", isOn: $model.showWindowGroupBackground)
                .toggleStyle(.switch)
        }
        .padding(12)
        .frame(width: 250)
    }

    private var settingsPopoverContent: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Settings")
                .font(.system(size: 12, weight: .semibold, design: .rounded))
                .foregroundStyle(palette.textPrimary)
            Picker("Window Grouping", selection: $model.windowGrouping) {
                Text("Off").tag(AppViewModel.WindowGrouping.off)
                Text("Auto").tag(AppViewModel.WindowGrouping.auto)
                Text("On").tag(AppViewModel.WindowGrouping.on)
            }
            .pickerStyle(.menu)
            Toggle("Show Unmanaged Panes", isOn: Binding(
                get: { !model.hideUnmanagedCategory },
                set: { model.hideUnmanagedCategory = !$0 }
            ))
            .toggleStyle(.switch)
            Toggle("Show Unknown Panes", isOn: $model.showUnknownCategory)
                .toggleStyle(.switch)
            Toggle("Show Window Group Cards", isOn: $model.showWindowGroupBackground)
                .toggleStyle(.switch)
            Toggle("Show Technical Details", isOn: $model.showTechnicalDetails)
                .toggleStyle(.switch)
            Divider()
            Button("Refresh Now") {
                model.manualRefresh()
            }
        }
        .padding(12)
        .frame(width: 260)
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
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 8) {
                HStack(spacing: 7) {
                    Image(systemName: "folder.fill")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(palette.textSecondary)
                    Text(section.sessionName)
                        .font(.system(size: 14, weight: .semibold, design: .rounded))
                        .foregroundStyle(palette.textPrimary)
                        .lineLimit(1)
                }
                .padding(.leading, 4)
                Spacer(minLength: 0)
                Text(model.sessionLastActiveShortLabel(for: section))
                    .font(.system(size: 10, weight: .regular, design: .monospaced))
                    .foregroundStyle(palette.textMuted)
                Text(section.target)
                    .font(.system(size: 10, weight: .regular, design: .monospaced))
                    .foregroundStyle(palette.textSecondary)
                    .lineLimit(1)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(
                        Capsule(style: .continuous)
                            .fill(palette.rowFill)
                    )
            }
            .contentShape(Rectangle())
            .contextMenu {
                Button("Kill Session", role: .destructive) {
                    requestSessionKill(section)
                }
            }

            paneList(
                section.panes,
                showWindowGroups: model.showWindowGroupBackground,
                indentWhenFlat: true
            )
        }
        .padding(.bottom, 2)
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
        return Button {
            withAnimation(.easeInOut(duration: 0.14)) {
                model.selectedPane = pane
            }
        } label: {
            HStack(spacing: 10) {
                Circle()
                    .fill(selected ? colorForCategory(category) : colorForCategory(category).opacity(0.9))
                    .frame(width: 8, height: 8)

                Text(model.paneDisplayTitle(for: pane, among: titleCandidates))
                    .font(.system(size: 13, weight: selected ? .semibold : .regular, design: .rounded))
                    .foregroundStyle(selected ? palette.textPrimary : palette.textSecondary)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)

                if model.needsUserAction(for: pane) {
                    Image(systemName: "exclamationmark.circle.fill")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(palette.attention)
                }

                Text(model.lastActiveShortLabel(for: pane))
                    .font(.system(size: 11, weight: .regular, design: .rounded))
                    .foregroundStyle(selected ? palette.textSecondary : palette.textMuted)
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
            Button("Open in External Terminal") {
                model.selectedPane = pane
                model.openSelectedPaneInExternalTerminal()
            }
            Divider()
            Button("Pane Details") {
                detailPane = pane
            }
            Button("Copy Pane Path") {
                copyPanePath(pane)
            }
            Divider()
            Button("Kill INT", role: .destructive) {
                requestKill(.keyINT, for: pane)
            }
            Button("Kill TERM", role: .destructive) {
                requestKill(.signalTERM, for: pane)
            }
        }
        .help("\(pane.identity.target)/\(pane.identity.sessionName)/\(pane.identity.paneID)")
        .buttonStyle(.plain)
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

    private func requestKill(_ action: KillAction, for pane: PaneItem) {
        model.selectedPane = pane
        pendingKillPane = pane
        pendingKillAction = action
    }

    private func requestSessionKill(_ section: AppViewModel.SessionSection) {
        pendingKillSession = SessionKillRequest(
            target: section.target,
            sessionName: section.sessionName
        )
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
