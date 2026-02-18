import AppKit
import SwiftTerm
import SwiftUI

final class IMEAwareTerminalView: TerminalView {
    private var markedTextStorage: NSAttributedString?
    private var markedSelection: NSRange = NSRange(location: 0, length: 0)
    private weak var markedOverlayContainer: NSView?
    private weak var markedOverlayLabel: NSTextField?
    private var cursorHiddenForComposition = false
    private var compositionCursorX: Int = 0
    private var compositionCursorY: Int = 0
    private var compositionPaneCols: Int = 1
    private var compositionPaneRows: Int = 1
    // Preedit overlay nudged relative to SwiftTerm's real caret rect.
    private let imeVerticalNudge: CGFloat = 0.25

    override var isOpaque: Bool { false }

    override func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let nextMarkedText: NSAttributedString?
        if let attributed = string as? NSAttributedString {
            nextMarkedText = attributed
        } else if let plain = string as? String {
            nextMarkedText = NSAttributedString(string: plain)
        } else if let plain = string as? NSString {
            nextMarkedText = NSAttributedString(string: plain as String)
        } else {
            nextMarkedText = nil
        }
        guard let nextMarkedText, nextMarkedText.length > 0 else {
            unmarkText()
            return
        }
        markedTextStorage = nextMarkedText
        markedSelection = selectedRange
        updateMarkedTextOverlay()
        applyCursorVisibilityForComposition()
    }

    override func unmarkText() {
        markedTextStorage = nil
        markedSelection = NSRange(location: 0, length: 0)
        markedOverlayContainer?.isHidden = true
        markedOverlayLabel?.isHidden = true
        applyCursorVisibilityForComposition()
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
        applyCursorVisibilityForComposition()
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
        applyCursorVisibilityForComposition()
    }

    func syncCursorVisibilityForComposition() {
        applyCursorVisibilityForComposition()
    }

    private func ensureMarkedOverlayComponents() -> (NSView, NSTextField) {
        if let container = markedOverlayContainer, let label = markedOverlayLabel {
            return (container, label)
        }
        let container = NSView(frame: .zero)
        container.wantsLayer = true
        container.layer?.cornerRadius = 3
        container.layer?.masksToBounds = true
        container.isHidden = true
        addSubview(container)

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
        container.addSubview(label)
        markedOverlayContainer = container
        markedOverlayLabel = label
        return (container, label)
    }

    private func updateMarkedTextOverlay() {
        guard let markedTextStorage, markedTextStorage.length > 0 else {
            markedOverlayContainer?.isHidden = true
            markedOverlayLabel?.isHidden = true
            return
        }

        let (container, label) = ensureMarkedOverlayComponents()
        let overlayString = NSMutableAttributedString(attributedString: markedTextStorage)
        let fullRange = NSRange(location: 0, length: overlayString.length)
        let accentColor = caretColor.withAlphaComponent(0.95)
        let selectionBackground = opaqueAccentColor(caretColor)
        overlayString.addAttributes([
            .underlineStyle: NSUnderlineStyle.thick.rawValue,
            .underlineColor: accentColor,
            .foregroundColor: NSColor.black,
        ], range: fullRange)
        if let selectionRange = clampedMarkedSelection(totalLength: overlayString.length) {
            overlayString.addAttributes([
                .backgroundColor: selectionBackground,
                .foregroundColor: NSColor.black,
            ], range: selectionRange)
        }
        label.font = font
        label.attributedStringValue = overlayString
        label.sizeToFit()

        let horizontalPadding: CGFloat = 0
        let verticalPadding: CGFloat = 0
        let labelSize = label.frame.size
        label.frame = CGRect(
            x: horizontalPadding,
            y: verticalPadding,
            width: labelSize.width,
            height: labelSize.height
        ).integral
        let containerSize = NSSize(
            width: labelSize.width + (horizontalPadding * 2),
            height: labelSize.height + (verticalPadding * 2)
        )
        let origin = overlayOrigin(labelSize: containerSize)
        // Keep the opaque preedit background strictly limited to the composing text width.
        container.layer?.backgroundColor = opaqueAccentColor(caretColor).cgColor
        container.frame = CGRect(
            x: origin.x,
            y: origin.y,
            width: containerSize.width,
            height: containerSize.height
        ).integral
        container.isHidden = false
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

    private func applyCursorVisibilityForComposition() {
        let shouldHideCursor = hasMarkedText()
        guard shouldHideCursor != cursorHiddenForComposition else {
            return
        }
        cursorHiddenForComposition = shouldHideCursor
        if shouldHideCursor {
            getTerminal().hideCursor()
        } else {
            getTerminal().showCursor()
        }
    }

    private func clampedMarkedSelection(totalLength: Int) -> NSRange? {
        guard totalLength > 0 else {
            return nil
        }
        guard markedSelection.location != NSNotFound else {
            return nil
        }
        let location = min(max(markedSelection.location, 0), totalLength)
        let maxLength = max(0, totalLength - location)
        let length = min(max(markedSelection.length, 0), maxLength)
        if length > 0 {
            return NSRange(location: location, length: length)
        }
        if location < totalLength {
            return NSRange(location: location, length: 1)
        }
        return NSRange(location: max(0, totalLength - 1), length: 1)
    }

    private func opaqueAccentColor(_ color: NSColor) -> NSColor {
        if let rgb = color.usingColorSpace(.deviceRGB) {
            return rgb.withAlphaComponent(1.0)
        }
        return NSColor.systemBlue
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

    private static func ansi8(_ red: UInt16, _ green: UInt16, _ blue: UInt16) -> SwiftTerm.Color {
        SwiftTerm.Color(red: red * 257, green: green * 257, blue: blue * 257)
    }

    private static func preferredANSIPalette(darkMode: Bool) -> [SwiftTerm.Color] {
        if darkMode {
            // Lift color0 from pure black to reduce heavy black blocks in CLI prompt UIs.
            return [
                ansi8(12, 20, 31),   // black
                ansi8(214, 92, 92),  // red
                ansi8(111, 214, 154), // green
                ansi8(224, 196, 120), // yellow
                ansi8(120, 164, 255), // blue
                ansi8(198, 142, 255), // magenta
                ansi8(111, 203, 224), // cyan
                ansi8(216, 223, 236), // white
                ansi8(52, 70, 95),    // bright black
                ansi8(255, 138, 138), // bright red
                ansi8(152, 235, 188), // bright green
                ansi8(245, 218, 148), // bright yellow
                ansi8(152, 186, 255), // bright blue
                ansi8(226, 172, 255), // bright magenta
                ansi8(148, 225, 245), // bright cyan
                ansi8(238, 243, 251), // bright white
            ]
        }
        return [
            ansi8(20, 25, 33),
            ansi8(196, 58, 58),
            ansi8(39, 143, 84),
            ansi8(165, 120, 35),
            ansi8(52, 109, 207),
            ansi8(142, 78, 196),
            ansi8(33, 131, 150),
            ansi8(230, 234, 241),
            ansi8(98, 108, 124),
            ansi8(223, 85, 85),
            ansi8(47, 167, 98),
            ansi8(186, 137, 45),
            ansi8(72, 127, 222),
            ansi8(164, 101, 215),
            ansi8(47, 151, 171),
            ansi8(245, 247, 251),
        ]
    }

    final class Coordinator: NSObject, TerminalViewDelegate {
        private let onInputBytes: ([UInt8]) -> Void
        private let onResize: (_ cols: Int, _ rows: Int) -> Void

        private weak var terminalView: TerminalView?
        private var currentPaneID = ""
        private var lastRenderedContent = ""
        private var lastRenderedLines: [String] = [""]
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
        private let maxCachedLines = 2400

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
            lastRenderedLines = [""]
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
                lastRenderedLines = [""]
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

            if contentChanged {
                lastRenderedLines = updatedCachedLines(
                    previousContent: lastRenderedContent,
                    nextContent: content,
                    previousLines: lastRenderedLines
                )
            }

            let repaint: RepaintFrame
            if !force, !contentChanged, !paneSizeChanged, cursorChanged {
                repaint = buildCursorOnlyFrame(
                    terminal: terminal,
                    cursorX: cursorX,
                    cursorY: cursorY,
                    paneCols: paneCols,
                    paneRows: paneRows
                )
            } else {
                repaint = buildAbsoluteRepaintFrame(
                    terminal: terminal,
                    lines: lastRenderedLines,
                    cursorX: cursorX,
                    cursorY: cursorY,
                    paneCols: paneCols,
                    paneRows: paneRows
                )
            }
            terminal.feed(text: repaint.frame)
            if let imeTerminal = terminal as? IMEAwareTerminalView {
                imeTerminal.updateCompositionMetrics(
                    cursorX: repaint.cursorX,
                    cursorY: repaint.cursorY,
                    paneCols: repaint.paneCols,
                    paneRows: repaint.paneRows
                )
                imeTerminal.syncCursorVisibilityForComposition()
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

        private func updatedCachedLines(
            previousContent: String,
            nextContent: String,
            previousLines: [String]
        ) -> [String] {
            if previousContent.isEmpty || previousLines.isEmpty {
                return splitLines(nextContent)
            }
            guard nextContent.count >= previousContent.count,
                  nextContent.hasPrefix(previousContent) else {
                return splitLines(nextContent)
            }
            let suffix = nextContent.dropFirst(previousContent.count)
            if suffix.isEmpty {
                return previousLines
            }
            var lines = previousLines
            if lines.isEmpty {
                lines = [""]
            }
            for ch in suffix {
                if ch == "\n" {
                    lines.append("")
                } else {
                    lines[lines.count - 1].append(ch)
                }
            }
            if lines.count > maxCachedLines {
                lines = Array(lines.suffix(maxCachedLines))
            }
            return lines
        }

        private func splitLines(_ content: String) -> [String] {
            let lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
            if lines.isEmpty {
                return [""]
            }
            if lines.count > maxCachedLines {
                return Array(lines.suffix(maxCachedLines))
            }
            return lines
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
            lines sourceLines: [String],
            cursorX: Int?,
            cursorY: Int?,
            paneCols: Int?,
            paneRows: Int?
        ) -> RepaintFrame {
            let terminalRows = max(1, terminal.getTerminal().rows)
            let terminalCols = max(1, terminal.getTerminal().cols)
            let sourceRows = max(1, paneRows ?? terminalRows)
            let sourceCols = max(1, paneCols ?? terminalCols)
            var lines = sourceLines.isEmpty ? [""] : sourceLines
            let maxBufferedLines = max(sourceRows, min(max(sourceRows * 12, 500), 3000))
            if lines.count > maxBufferedLines {
                lines = Array(lines.suffix(maxBufferedLines))
            }
            // Snapshot is pane-sized in tmux terms. Normalize to terminal-sized viewport.
            var sourceWindowLines = Array(lines.suffix(sourceRows))
            if sourceWindowLines.count < sourceRows {
                sourceWindowLines.insert(
                    contentsOf: Array(repeating: "", count: sourceRows - sourceWindowLines.count),
                    at: 0
                )
            }

            let inferredSourceCursorY = inferCursorRow(lines: sourceWindowLines)
            let effectiveSourceCursorY = min(max(cursorY ?? inferredSourceCursorY, 0), sourceRows - 1)
            let sourceCursorLineIndex = min(max(effectiveSourceCursorY, 0), max(sourceWindowLines.count - 1, 0))
            let inferredSourceCursorX = inferCursorColumn(
                line: sourceWindowLines[sourceCursorLineIndex],
                maxCols: sourceCols
            )
            let effectiveSourceCursorX = min(max(cursorX ?? inferredSourceCursorX, 0), sourceCols - 1)

            var renderLines: [String]
            var mappedCursorY: Int
            if sourceRows > terminalRows {
                let start = sourceRows - terminalRows
                renderLines = Array(sourceWindowLines.suffix(terminalRows))
                mappedCursorY = effectiveSourceCursorY - start
            } else {
                let topPadding = terminalRows - sourceRows
                renderLines = Array(repeating: "", count: topPadding) + sourceWindowLines
                mappedCursorY = effectiveSourceCursorY + topPadding
            }
            if renderLines.count < terminalRows {
                renderLines += Array(repeating: "", count: terminalRows - renderLines.count)
            } else if renderLines.count > terminalRows {
                renderLines = Array(renderLines.suffix(terminalRows))
            }
            mappedCursorY = min(max(mappedCursorY, 0), terminalRows - 1)

            var out = ""
            let estimatedContentChars = lines.reduce(into: 0) { $0 += min($1.count, terminalCols) + 1 }
            out.reserveCapacity(max(estimatedContentChars + 256, lines.count * min(terminalCols, 160)))

            // Paint from a complete snapshot while preserving internal scrollback.
            out += "\u{001B}[?25l" // hide cursor
            out += "\u{001B}[?7l"  // disable line wrap
            out += "\u{001B}[H"
            out += "\u{001B}[2J"
            for (idx, rawLine) in renderLines.enumerated() {
                out += clampLine(rawLine, toColumns: terminalCols)
                // Extend current line attributes (including background color) to full width.
                out += "\u{001B}[K"
                if idx < renderLines.count - 1 {
                    out += "\r\n"
                }
            }
            let clampedX = min(max(effectiveSourceCursorX, 0), min(sourceCols - 1, terminalCols - 1))
            out += "\u{001B}[\(mappedCursorY + 1);\(clampedX + 1)H"

            out += "\u{001B}[?7h"  // enable line wrap
            out += "\u{001B}[?25h" // show cursor
            let mappedCursorX = clampedX
            return RepaintFrame(
                frame: out,
                cursorX: mappedCursorX,
                cursorY: mappedCursorY,
                paneCols: terminalCols,
                paneRows: terminalRows
            )
        }

        private func buildCursorOnlyFrame(
            terminal: TerminalView,
            cursorX: Int?,
            cursorY: Int?,
            paneCols: Int?,
            paneRows: Int?
        ) -> RepaintFrame {
            let terminalRows = max(1, terminal.getTerminal().rows)
            let terminalCols = max(1, terminal.getTerminal().cols)
            let sourceRows = max(1, paneRows ?? terminalRows)
            let sourceCols = max(1, paneCols ?? terminalCols)
            let lines = lastRenderedLines.isEmpty ? [""] : lastRenderedLines
            var sourceWindowLines = Array(lines.suffix(sourceRows))
            if sourceWindowLines.count < sourceRows {
                sourceWindowLines.insert(
                    contentsOf: Array(repeating: "", count: sourceRows - sourceWindowLines.count),
                    at: 0
                )
            }

            let inferredCursorY = inferCursorRow(lines: sourceWindowLines)
            let sourceCursorY = min(max(cursorY ?? inferredCursorY, 0), sourceRows - 1)
            let sourceCursorLineIndex = min(max(sourceCursorY, 0), max(sourceWindowLines.count - 1, 0))
            let inferredCursorX = inferCursorColumn(
                line: sourceWindowLines[sourceCursorLineIndex],
                maxCols: sourceCols
            )
            let sourceCursorX = min(max(cursorX ?? inferredCursorX, 0), sourceCols - 1)

            let mapped = mapCursorPosition(
                sourceCursorX: sourceCursorX,
                sourceCursorY: sourceCursorY,
                sourceCols: sourceCols,
                sourceRows: sourceRows,
                terminalCols: terminalCols,
                terminalRows: terminalRows
            )
            let frame = "\u{001B}[?25l\u{001B}[\(mapped.y + 1);\(mapped.x + 1)H\u{001B}[?25h"
            return RepaintFrame(
                frame: frame,
                cursorX: mapped.x,
                cursorY: mapped.y,
                paneCols: terminalCols,
                paneRows: terminalRows
            )
        }

        private func mapCursorPosition(
            sourceCursorX: Int,
            sourceCursorY: Int,
            sourceCols: Int,
            sourceRows: Int,
            terminalCols: Int,
            terminalRows: Int
        ) -> (x: Int, y: Int) {
            var mappedCursorY: Int
            if sourceRows > terminalRows {
                let start = sourceRows - terminalRows
                mappedCursorY = sourceCursorY - start
            } else {
                let topPadding = terminalRows - sourceRows
                mappedCursorY = sourceCursorY + topPadding
            }
            mappedCursorY = min(max(mappedCursorY, 0), terminalRows - 1)
            let mappedCursorX = min(max(sourceCursorX, 0), min(sourceCols - 1, terminalCols - 1))
            return (mappedCursorX, mappedCursorY)
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
            return min(max(0, visibleColumnCount(line)), maxCols - 1)
        }

        private func visibleColumnCount(_ raw: String) -> Int {
            guard !raw.isEmpty else {
                return 0
            }
            var count = 0
            var idx = raw.startIndex
            while idx < raw.endIndex {
                let scalar = raw[idx].unicodeScalars.first?.value
                if scalar == 0x1B {
                    idx = consumeEscapeSequence(in: raw, from: idx)
                    continue
                }
                count += 1
                idx = raw.index(after: idx)
            }
            return count
        }

        private func clampLine(_ raw: String, toColumns cols: Int) -> String {
            guard cols > 0 else {
                return ""
            }
            guard !raw.isEmpty else {
                return ""
            }
            var out = ""
            out.reserveCapacity(min(raw.count, cols + 16))
            var visibleColumns = 0
            var idx = raw.startIndex
            while idx < raw.endIndex {
                let scalar = raw[idx].unicodeScalars.first?.value
                if scalar == 0x1B {
                    let end = consumeEscapeSequence(in: raw, from: idx)
                    out.append(contentsOf: raw[idx..<end])
                    idx = end
                    continue
                }
                if visibleColumns >= cols {
                    break
                }
                out.append(raw[idx])
                visibleColumns += 1
                idx = raw.index(after: idx)
            }
            return out
        }

        private func consumeEscapeSequence(in raw: String, from start: String.Index) -> String.Index {
            var idx = raw.index(after: start)
            guard idx < raw.endIndex else {
                return idx
            }
            let lead = raw[idx].unicodeScalars.first?.value ?? 0
            // CSI: ESC [ ... final-byte(@..~)
            if lead == 0x5B {
                idx = raw.index(after: idx)
                while idx < raw.endIndex {
                    let value = raw[idx].unicodeScalars.first?.value ?? 0
                    idx = raw.index(after: idx)
                    if value >= 0x40 && value <= 0x7E {
                        break
                    }
                }
                return idx
            }
            // OSC: ESC ] ... BEL or ST(ESC \)
            if lead == 0x5D {
                idx = raw.index(after: idx)
                while idx < raw.endIndex {
                    let value = raw[idx].unicodeScalars.first?.value ?? 0
                    if value == 0x07 {
                        return raw.index(after: idx)
                    }
                    if value == 0x1B {
                        let next = raw.index(after: idx)
                        if next < raw.endIndex, (raw[next].unicodeScalars.first?.value ?? 0) == 0x5C {
                            return raw.index(after: next)
                        }
                    }
                    idx = raw.index(after: idx)
                }
                return raw.endIndex
            }
            // DCS/SOS/PM/APC: ESC P/X/^/_ ... ST(ESC \)
            if lead == 0x50 || lead == 0x58 || lead == 0x5E || lead == 0x5F {
                idx = raw.index(after: idx)
                while idx < raw.endIndex {
                    let value = raw[idx].unicodeScalars.first?.value ?? 0
                    if value == 0x1B {
                        let next = raw.index(after: idx)
                        if next < raw.endIndex, (raw[next].unicodeScalars.first?.value ?? 0) == 0x5C {
                            return raw.index(after: next)
                        }
                    }
                    idx = raw.index(after: idx)
                }
                return raw.endIndex
            }
            // Generic two-byte escape sequence.
            return raw.index(after: idx)
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
            terminal.installColors(NativeTmuxTerminalView.preferredANSIPalette(darkMode: darkMode))
            terminal.useBrightColors = true
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
