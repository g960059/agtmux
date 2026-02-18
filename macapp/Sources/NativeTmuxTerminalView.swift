import AppKit
import SwiftTerm
import SwiftUI

final class IMEAwareTerminalView: TerminalView {
    private var markedTextStorage: NSAttributedString?
    private var markedSelection: NSRange = NSRange(location: 0, length: 0)
    private weak var markedOverlayLabel: NSTextField?
    private var compositionCursorX: Int = 0
    private var compositionCursorY: Int = 0
    private var compositionPaneCols: Int = 1
    private var compositionPaneRows: Int = 1
    // Preedit overlay nudged relative to SwiftTerm's real caret rect.
    private let imeVerticalNudge: CGFloat = 0.25

    override var isOpaque: Bool { false }

    override func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        if let attributed = string as? NSAttributedString {
            markedTextStorage = attributed
        } else if let plain = string as? String {
            markedTextStorage = NSAttributedString(string: plain)
        } else if let plain = string as? NSString {
            markedTextStorage = NSAttributedString(string: plain as String)
        } else {
            markedTextStorage = nil
        }
        markedSelection = selectedRange
        updateMarkedTextOverlay()
    }

    override func unmarkText() {
        markedTextStorage = nil
        markedSelection = NSRange(location: 0, length: 0)
        markedOverlayLabel?.isHidden = true
    }

    override func hasMarkedText() -> Bool {
        (markedTextStorage?.length ?? 0) > 0
    }

    override func markedRange() -> NSRange {
        guard let markedTextStorage, markedTextStorage.length > 0 else {
            return NSRange(location: NSNotFound, length: 0)
        }
        return NSRange(location: 0, length: markedTextStorage.length)
    }

    override func selectedRange() -> NSRange {
        if hasMarkedText() {
            return markedSelection
        }
        return super.selectedRange()
    }

    override func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        guard let markedTextStorage, markedTextStorage.length > 0 else {
            return super.attributedSubstring(forProposedRange: range, actualRange: actualRange)
        }
        let location = max(0, min(range.location, markedTextStorage.length))
        let length = max(0, min(range.length, markedTextStorage.length - location))
        let clipped = NSRange(location: location, length: length)
        actualRange?.pointee = clipped
        return markedTextStorage.attributedSubstring(from: clipped)
    }

    override func validAttributesForMarkedText() -> [NSAttributedString.Key] {
        [.underlineStyle, .foregroundColor, .backgroundColor]
    }

    override func insertText(_ string: Any, replacementRange: NSRange) {
        super.insertText(string, replacementRange: replacementRange)
        unmarkText()
    }

    override func layout() {
        super.layout()
        if hasMarkedText() {
            updateMarkedTextOverlay()
        }
    }

    override func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        super.firstRect(forCharacterRange: range, actualRange: actualRange)
    }

    func updateCompositionMetrics(cursorX: Int?, cursorY: Int?, paneCols: Int?, paneRows: Int?) {
        compositionCursorX = max(0, cursorX ?? compositionCursorX)
        compositionCursorY = max(0, cursorY ?? compositionCursorY)
        compositionPaneCols = max(1, paneCols ?? compositionPaneCols)
        compositionPaneRows = max(1, paneRows ?? compositionPaneRows)
        if hasMarkedText() {
            updateMarkedTextOverlay()
        }
    }

    private func ensureMarkedOverlayLabel() -> NSTextField {
        if let label = markedOverlayLabel {
            return label
        }
        let label = NSTextField(labelWithString: "")
        label.isEditable = false
        label.isBordered = false
        label.drawsBackground = false
        label.lineBreakMode = .byClipping
        label.maximumNumberOfLines = 1
        label.alignment = .left
        label.alphaValue = 0.95
        label.font = font
        label.isHidden = true
        addSubview(label)
        markedOverlayLabel = label
        return label
    }

    private func updateMarkedTextOverlay() {
        guard let markedTextStorage, markedTextStorage.length > 0 else {
            markedOverlayLabel?.isHidden = true
            return
        }

        let label = ensureMarkedOverlayLabel()
        let overlayString = NSMutableAttributedString(attributedString: markedTextStorage)
        overlayString.addAttributes([
            .underlineStyle: NSUnderlineStyle.single.rawValue,
            .foregroundColor: NSColor.labelColor,
            .backgroundColor: NSColor.clear,
        ], range: NSRange(location: 0, length: overlayString.length))
        label.font = font
        label.attributedStringValue = overlayString
        label.sizeToFit()
        let origin = overlayOrigin(labelSize: label.frame.size)
        var frame = label.frame
        frame.origin = origin
        label.frame = frame.integral
        label.isHidden = false
    }

    private func overlayOrigin(labelSize: NSSize) -> CGPoint {
        return overlayOriginFromCaret(labelSize: labelSize)
    }

    private func overlayOriginFromCaret(labelSize: NSSize) -> CGPoint {
        let caretRect = imeCaretRectInLocalCoordinates()
        let x = caretRect.origin.x + 1
        let y = caretRect.origin.y + max(0, (caretRect.height - labelSize.height) * 0.5) + imeVerticalNudge
        return clampOverlayOrigin(CGPoint(x: x, y: y), labelSize: labelSize)
    }

    private func gridIMECaretRectInLocalCoordinates() -> CGRect {
        let textRect = gridIMETextRectInLocalCoordinates()
        let width = max(2, min(6, textRect.width * 0.12))
        return CGRect(x: textRect.origin.x, y: textRect.origin.y, width: width, height: max(2, textRect.height))
    }

    private func gridIMETextRectInLocalCoordinates() -> CGRect {
        let caretRect = gridCaretRectInLocalCoordinates()
        let fontHeight = max(8, font.ascender - font.descender + font.leading)
        let textHeight = min(caretRect.height, fontHeight)
        let y = caretRect.origin.y + max(0, (caretRect.height - textHeight) * 0.5)
        return CGRect(x: caretRect.origin.x, y: y, width: caretRect.width, height: textHeight)
    }

    private func gridCaretRectInLocalCoordinates() -> CGRect {
        let cols = max(1, compositionPaneCols)
        let rows = max(1, compositionPaneRows)
        let xIndex = min(max(compositionCursorX, 0), cols - 1)
        let yIndex = min(max(CGFloat(compositionCursorY), 0), CGFloat(rows - 1))
        let cellWidth = max(1, bounds.width / CGFloat(cols))
        let cellHeight = max(1, bounds.height / CGFloat(rows))
        let x = CGFloat(xIndex) * cellWidth
        let y = bounds.height - ((yIndex + 1) * cellHeight)
        return CGRect(x: x, y: y, width: cellWidth, height: cellHeight)
    }

    private func imeCaretRectInLocalCoordinates() -> CGRect {
        guard let window else {
            return gridIMECaretRectInLocalCoordinates()
        }
        let screenRect = super.firstRect(forCharacterRange: NSRange(location: 0, length: 0), actualRange: nil)
        guard !screenRect.isNull, !screenRect.isEmpty else {
            return gridIMECaretRectInLocalCoordinates()
        }
        let windowRect = window.convertFromScreen(screenRect)
        let localRect = convert(windowRect, from: nil)
        if localRect.isEmpty || localRect.isNull {
            return gridIMECaretRectInLocalCoordinates()
        }
        return localRect
    }

    private func clampOverlayOrigin(_ origin: CGPoint, labelSize: NSSize) -> CGPoint {
        let maxX = max(4, bounds.width - labelSize.width - 4)
        let maxY = max(4, bounds.height - labelSize.height - 4)
        return CGPoint(
            x: min(max(origin.x, 4), maxX),
            y: min(max(origin.y, 4), maxY)
        )
    }
}

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
        let terminal = IMEAwareTerminalView(frame: .zero)
        terminal.terminalDelegate = context.coordinator
        terminal.font = Self.preferredTerminalFont(size: 13)
        terminal.optionAsMetaKey = true
        terminal.allowMouseReporting = false
        terminal.disableFullRedrawOnAnyChanges = true
        terminal.wantsLayer = true
        terminal.layer?.masksToBounds = true
        terminal.layer?.isOpaque = false
        terminal.layer?.backgroundColor = NSColor.clear.cgColor
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

    private static func preferredTerminalFont(size: CGFloat) -> NSFont {
        let nerdCandidates = [
            "JetBrainsMonoNFM-Regular",
            "JetBrainsMonoNF-Regular",
            "JetBrainsMonoNLNFM-Regular",
            "UDEV Gothic 35JPDOC",
            "UDEV Gothic 35JPDOCNerd",
            "JetBrainsMono Nerd Font Mono",
            "Hack Nerd Font Mono",
            "CaskaydiaCove Nerd Font Mono",
            "MesloLGS NF",
            "SauceCodePro Nerd Font Mono",
        ]
        for name in nerdCandidates {
            if let font = NSFont(name: name, size: size) {
                return font
            }
        }
        return NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
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
        private var pendingContent = ""
        private var pendingCursorX: Int?
        private var pendingCursorY: Int?
        private var pendingPaneCols: Int?
        private var pendingPaneRows: Int?
        private var holdRepaintWhileScrolled = false
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
            lastPaneCols = nil
            lastPaneRows = nil
            pendingContent = ""
            pendingCursorX = nil
            pendingCursorY = nil
            pendingPaneCols = nil
            pendingPaneRows = nil
            holdRepaintWhileScrolled = false
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
            let normalizedContent = normalizedTerminalText(content)
            pendingContent = normalizedContent
            pendingCursorX = cursorX
            pendingCursorY = cursorY
            pendingPaneCols = paneCols
            pendingPaneRows = paneRows
            let paneID = pane.identity.paneID.trimmingCharacters(in: .whitespacesAndNewlines)
            if paneID != currentPaneID {
                currentPaneID = paneID
                lastRenderedContent = ""
                lastCursorX = nil
                lastCursorY = nil
                lastPaneCols = nil
                lastPaneRows = nil
                holdRepaintWhileScrolled = false
                resetTerminal(terminal)
                updateFocusIfNeeded(terminal: terminal)
            }
            renderIfNeeded(
                terminal: terminal,
                content: normalizedContent,
                cursorX: cursorX,
                cursorY: cursorY,
                paneCols: paneCols,
                paneRows: paneRows,
                force: false
            )
        }

        private func renderIfNeeded(
            terminal: TerminalView,
            content: String,
            cursorX: Int?,
            cursorY: Int?,
            paneCols: Int?,
            paneRows: Int?,
            force: Bool
        ) {
            let contentChanged = content != lastRenderedContent
            let cursorChanged = cursorX != lastCursorX || cursorY != lastCursorY
            let paneSizeChanged = paneCols != lastPaneCols || paneRows != lastPaneRows
            guard force || contentChanged || cursorChanged || paneSizeChanged else {
                return
            }
            if holdRepaintWhileScrolled && !force {
                return
            }

            let repaint = buildAbsoluteRepaintFrame(
                terminal: terminal,
                content: content,
                cursorX: cursorX,
                cursorY: cursorY,
                paneCols: paneCols,
                paneRows: paneRows
            )
            terminal.feed(text: repaint.frame)
            if let imeTerminal = terminal as? IMEAwareTerminalView {
                imeTerminal.updateCompositionMetrics(
                    cursorX: repaint.cursorX,
                    cursorY: repaint.cursorY,
                    paneCols: repaint.paneCols,
                    paneRows: repaint.paneRows
                )
            }
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

        private struct RepaintFrame {
            let frame: String
            let cursorX: Int?
            let cursorY: Int?
            let paneCols: Int
            let paneRows: Int
        }

        private func buildAbsoluteRepaintFrame(
            terminal: TerminalView,
            content: String,
            cursorX: Int?,
            cursorY: Int?,
            paneCols: Int?,
            paneRows: Int?
        ) -> RepaintFrame {
            let terminalRows = max(1, terminal.getTerminal().rows)
            let terminalCols = max(1, terminal.getTerminal().cols)
            let sourceRows = max(1, paneRows ?? terminalRows)
            let sourceCols = max(1, paneCols ?? terminalCols)
            var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
            if lines.isEmpty {
                lines = [""]
            }
            let maxBufferedLines = max(sourceRows, min(max(sourceRows * 12, 500), 3000))
            if lines.count > maxBufferedLines {
                lines = Array(lines.suffix(maxBufferedLines))
            }

            if lines.count < sourceRows {
                let clampedCursorY = min(max(cursorY ?? (lines.count - 1), 0), sourceRows - 1)
                let leadingPadding = max(0, min(sourceRows - lines.count, clampedCursorY + 1 - lines.count))
                if leadingPadding > 0 {
                    lines.insert(contentsOf: Array(repeating: "", count: leadingPadding), at: 0)
                }
                if lines.count < sourceRows {
                    lines.append(contentsOf: Array(repeating: "", count: sourceRows - lines.count))
                }
            }

            let historyOffset = max(0, lines.count - sourceRows)
            let visibleWindowLines = Array(lines.suffix(sourceRows))
            let inferredSourceCursorY = inferCursorRow(lines: visibleWindowLines)
            let effectiveSourceCursorY = min(
                max(cursorY ?? inferredSourceCursorY, 0),
                sourceRows - 1
            )
            let sourceCursorLineIndex = min(
                max(0, historyOffset + effectiveSourceCursorY),
                max(lines.count - 1, 0)
            )
            let inferredSourceCursorX = inferCursorColumn(
                line: lines[sourceCursorLineIndex],
                maxCols: sourceCols
            )
            let effectiveSourceCursorX = min(
                max(cursorX ?? inferredSourceCursorX, 0),
                sourceCols - 1
            )

            var visibleRowsForOverlay = sourceRows
            var mappedCursorY: Int
            if terminalRows >= sourceRows {
                let topPadding = terminalRows - sourceRows
                mappedCursorY = effectiveSourceCursorY + topPadding
                visibleRowsForOverlay = terminalRows
            } else {
                let start = sourceRows - terminalRows
                mappedCursorY = effectiveSourceCursorY - start
                visibleRowsForOverlay = terminalRows
            }
            mappedCursorY = min(max(mappedCursorY, 0), terminalRows - 1)

            var out = ""
            out.reserveCapacity(max(content.count + 256, lines.count * min(terminalCols, 160)))

            // Paint from a complete snapshot while preserving internal scrollback.
            out += "\u{001B}[?25l" // hide cursor
            out += "\u{001B}[?7l"  // disable line wrap
            out += "\u{001B}[H"
            out += "\u{001B}[2J"
            for (idx, rawLine) in lines.enumerated() {
                out += clampLine(rawLine, toColumns: terminalCols)
                if idx < lines.count - 1 {
                    out += "\r\n"
                }
            }

            let upFromBottom = max(0, sourceRows - 1 - effectiveSourceCursorY)
            if upFromBottom > 0 {
                out += "\u{001B}[\(upFromBottom)A"
            }
            let clampedX = min(max(effectiveSourceCursorX, 0), min(sourceCols - 1, terminalCols - 1))
            out += "\u{001B}[\(clampedX + 1)G"

            out += "\u{001B}[?7h"  // enable line wrap
            out += "\u{001B}[?25h" // show cursor
            let mappedCursorX = clampedX
            return RepaintFrame(
                frame: out,
                cursorX: mappedCursorX,
                cursorY: mappedCursorY,
                paneCols: terminalCols,
                paneRows: visibleRowsForOverlay
            )
        }

        private func inferCursorRow(lines: [String]) -> Int {
            guard !lines.isEmpty else {
                return 0
            }
            var idx = lines.count - 1
            while idx > 0 {
                if !lines[idx].isEmpty {
                    return idx
                }
                idx -= 1
            }
            return 0
        }

        private func inferCursorColumn(line: String, maxCols: Int) -> Int {
            guard maxCols > 0 else {
                return 0
            }
            return min(max(0, line.count), maxCols - 1)
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
                terminal.nativeBackgroundColor = .clear
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
                terminal.nativeBackgroundColor = .clear
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
            if holdRepaintWhileScrolled {
                holdRepaintWhileScrolled = false
                if let terminalView {
                    renderIfNeeded(
                        terminal: terminalView,
                        content: pendingContent,
                        cursorX: pendingCursorX,
                        cursorY: pendingCursorY,
                        paneCols: pendingPaneCols,
                        paneRows: pendingPaneRows,
                        force: true
                    )
                }
            }
            onInputBytes(Array(data))
        }

        func scrolled(source _: TerminalView, position: Double) {
            let atBottom = position >= 0.999
            if atBottom {
                if holdRepaintWhileScrolled {
                    holdRepaintWhileScrolled = false
                    if let terminalView {
                        renderIfNeeded(
                            terminal: terminalView,
                            content: pendingContent,
                            cursorX: pendingCursorX,
                            cursorY: pendingCursorY,
                            paneCols: pendingPaneCols,
                            paneRows: pendingPaneRows,
                            force: true
                        )
                    }
                }
                return
            }
            holdRepaintWhileScrolled = true
        }

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
