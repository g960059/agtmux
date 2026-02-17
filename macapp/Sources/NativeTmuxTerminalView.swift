import AppKit
import SwiftTerm
import SwiftUI

struct NativeTmuxTerminalView: NSViewRepresentable {
    let pane: PaneItem
    let darkMode: Bool
    let content: String
    let cursorX: Int?
    let cursorY: Int?
    let paneCols: Int?
    let paneRows: Int?
    let interactiveInputEnabled: Bool
    let onInputBytes: ([UInt8]) -> Void
    let onResize: (_ cols: Int, _ rows: Int) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(
            onInputBytes: onInputBytes,
            onResize: onResize
        )
    }

    func makeNSView(context: Context) -> TerminalView {
        let terminal = TerminalView(frame: .zero)
        terminal.terminalDelegate = context.coordinator
        terminal.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        terminal.optionAsMetaKey = true
        terminal.allowMouseReporting = false
        terminal.disableFullRedrawOnAnyChanges = true
        context.coordinator.configureAppearance(darkMode: darkMode, terminal: terminal)
        context.coordinator.update(
            pane: pane,
            content: content,
            cursorX: cursorX,
            cursorY: cursorY,
            paneCols: paneCols,
            paneRows: paneRows,
            interactiveInputEnabled: interactiveInputEnabled,
            terminal: terminal
        )
        return terminal
    }

    func updateNSView(_ terminal: TerminalView, context: Context) {
        context.coordinator.configureAppearance(darkMode: darkMode, terminal: terminal)
        context.coordinator.update(
            pane: pane,
            content: content,
            cursorX: cursorX,
            cursorY: cursorY,
            paneCols: paneCols,
            paneRows: paneRows,
            interactiveInputEnabled: interactiveInputEnabled,
            terminal: terminal
        )
    }

    static func dismantleNSView(_ nsView: TerminalView, coordinator: Coordinator) {
        coordinator.detach()
    }

    final class Coordinator: NSObject, TerminalViewDelegate {
        private let onInputBytes: ([UInt8]) -> Void
        private let onResize: (_ cols: Int, _ rows: Int) -> Void

        private weak var terminalView: TerminalView?
        private var currentPaneID = ""
        private var lastRenderedContent = ""
        private var lastCursorX: Int?
        private var lastCursorY: Int?
        private var lastPaneCols: Int?
        private var lastPaneRows: Int?
        private var interactiveInputEnabled = true
        private var didConfigureAppearance = false
        private var appearanceModeIsDark = true

        init(
            onInputBytes: @escaping ([UInt8]) -> Void,
            onResize: @escaping (_ cols: Int, _ rows: Int) -> Void
        ) {
            self.onInputBytes = onInputBytes
            self.onResize = onResize
        }

        func detach() {
            terminalView = nil
            currentPaneID = ""
            lastRenderedContent = ""
            lastCursorX = nil
            lastCursorY = nil
        }

        func update(
            pane: PaneItem,
            content: String,
            cursorX: Int?,
            cursorY: Int?,
            paneCols: Int?,
            paneRows: Int?,
            interactiveInputEnabled: Bool,
            terminal: TerminalView
        ) {
            terminalView = terminal
            self.interactiveInputEnabled = interactiveInputEnabled
            let paneID = pane.identity.paneID.trimmingCharacters(in: .whitespacesAndNewlines)
            if paneID != currentPaneID {
                currentPaneID = paneID
                lastRenderedContent = ""
                lastCursorX = nil
                lastCursorY = nil
                lastPaneCols = nil
                lastPaneRows = nil
                resetTerminal(terminal)
                updateFocusIfNeeded(terminal: terminal)
            }
            renderIfNeeded(
                terminal: terminal,
                content: normalizedTerminalText(content),
                cursorX: cursorX,
                cursorY: cursorY,
                paneCols: paneCols,
                paneRows: paneRows
            )
        }

        private func renderIfNeeded(
            terminal: TerminalView,
            content: String,
            cursorX: Int?,
            cursorY: Int?,
            paneCols: Int?,
            paneRows: Int?
        ) {
            let contentChanged = content != lastRenderedContent
            let cursorChanged = cursorX != lastCursorX || cursorY != lastCursorY
            let paneSizeChanged = paneCols != lastPaneCols || paneRows != lastPaneRows
            guard contentChanged || cursorChanged || paneSizeChanged else {
                return
            }

            let repaint = buildAbsoluteRepaintFrame(
                content: content,
                cursorX: cursorX,
                cursorY: cursorY,
                paneCols: paneCols,
                paneRows: paneRows
            )
            terminal.feed(text: repaint)
            lastRenderedContent = content
            lastCursorX = cursorX
            lastCursorY = cursorY
            lastPaneCols = paneCols
            lastPaneRows = paneRows
        }

        private func normalizedTerminalText(_ raw: String) -> String {
            var normalized = raw.replacingOccurrences(of: "\r\n", with: "\n")
            normalized = normalized.replacingOccurrences(of: "\r", with: "\n")
            return normalized
        }

        private func buildAbsoluteRepaintFrame(
            content: String,
            cursorX: Int?,
            cursorY: Int?,
            paneCols: Int?,
            paneRows: Int?
        ) -> String {
            let rows = max(1, paneRows ?? 1)
            let cols = max(1, paneCols ?? 1)
            var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
            if lines.count > rows {
                lines = Array(lines.suffix(rows))
            } else if lines.count < rows {
                lines.append(contentsOf: Array(repeating: "", count: rows - lines.count))
            }

            var out = ""
            out.reserveCapacity(max(content.count + 256, rows * min(cols, 120)))

            // Paint each row at an absolute y/x position to avoid wrap accumulation artifacts.
            out += "\u{001B}[?25l" // hide cursor
            out += "\u{001B}[?7l"  // disable line wrap
            out += "\u{001B}[H"
            out += "\u{001B}[2J"
            for (idx, rawLine) in lines.enumerated() {
                let row = idx + 1
                out += "\u{001B}[\(row);1H"
                out += clampLine(rawLine, toColumns: cols)
                out += "\u{001B}[K" // clear remainder of line
            }

            if let x = cursorX, let y = cursorY, x >= 0, y >= 0 {
                let clampedX = min(max(x, 0), cols - 1)
                let clampedY = min(max(y, 0), rows - 1)
                out += "\u{001B}[\(clampedY + 1);\(clampedX + 1)H"
            } else {
                out += "\u{001B}[\(rows);1H"
            }

            out += "\u{001B}[?7h"  // enable line wrap
            out += "\u{001B}[?25h" // show cursor
            return out
        }

        private func clampLine(_ raw: String, toColumns cols: Int) -> String {
            guard cols > 0 else {
                return ""
            }
            if raw.count <= cols {
                return raw
            }
            let end = raw.index(raw.startIndex, offsetBy: cols)
            return String(raw[..<end])
        }

        private func updateFocusIfNeeded(terminal: TerminalView) {
            guard terminal.window?.firstResponder !== terminal else {
                return
            }
            DispatchQueue.main.async {
                terminal.window?.makeFirstResponder(terminal)
            }
        }

        private func resetTerminal(_ terminal: TerminalView) {
            terminal.getTerminal().resetToInitialState()
        }

        func configureAppearance(darkMode: Bool, terminal: TerminalView) {
            guard !didConfigureAppearance || darkMode != appearanceModeIsDark else {
                return
            }
            didConfigureAppearance = true
            appearanceModeIsDark = darkMode
            if darkMode {
                terminal.nativeBackgroundColor = NSColor(
                    calibratedRed: 0.02,
                    green: 0.06,
                    blue: 0.11,
                    alpha: 0.74
                )
                terminal.nativeForegroundColor = NSColor(
                    calibratedRed: 0.90,
                    green: 0.93,
                    blue: 0.97,
                    alpha: 1.0
                )
                terminal.caretColor = NSColor(
                    calibratedRed: 0.44,
                    green: 0.74,
                    blue: 1.0,
                    alpha: 0.95
                )
            } else {
                terminal.nativeBackgroundColor = NSColor(
                    calibratedRed: 0.96,
                    green: 0.97,
                    blue: 0.99,
                    alpha: 0.94
                )
                terminal.nativeForegroundColor = NSColor(
                    calibratedRed: 0.10,
                    green: 0.12,
                    blue: 0.16,
                    alpha: 1.0
                )
                terminal.caretColor = NSColor(
                    calibratedRed: 0.12,
                    green: 0.44,
                    blue: 0.90,
                    alpha: 0.90
                )
            }
        }

        func sizeChanged(source _: TerminalView, newCols: Int, newRows: Int) {
            guard newCols > 0, newRows > 0 else {
                return
            }
            onResize(newCols, newRows)
        }

        func setTerminalTitle(source _: TerminalView, title _: String) {}

        func hostCurrentDirectoryUpdate(source _: TerminalView, directory _: String?) {}

        func send(source _: TerminalView, data: ArraySlice<UInt8>) {
            guard interactiveInputEnabled, !data.isEmpty else {
                return
            }
            onInputBytes(Array(data))
        }

        func scrolled(source _: TerminalView, position _: Double) {}

        func requestOpenLink(source _: TerminalView, link: String, params _: [String: String]) {
            guard let url = URL(string: link) else {
                return
            }
            NSWorkspace.shared.open(url)
        }

        func bell(source _: TerminalView) {}

        func clipboardCopy(source _: TerminalView, content: Data) {
            guard let text = String(data: content, encoding: .utf8) else {
                return
            }
            let pb = NSPasteboard.general
            pb.clearContents()
            pb.setString(text, forType: .string)
        }

        func iTermContent(source _: TerminalView, content _: ArraySlice<UInt8>) {}

        func rangeChanged(source _: TerminalView, startY _: Int, endY _: Int) {}
    }
}
