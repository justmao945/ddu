import AppKit
import Combine
import SwiftUI

/// Borderless panel: shown right below the menu bar and can become key window (text fields accept input/paste).
final class PanelWindow: NSPanel {
    override var canBecomeKey: Bool { true }
}

@MainActor
final class StatusItemController: NSObject {
    /// One menu bar icon per provider; tapping one opens only that provider's usage panel
    private var items: [ProviderID: NSStatusItem] = [:]
    /// Fallback icon when every provider is disabled (otherwise there's nowhere to open the panel/settings)
    private var fallbackItem: NSStatusItem?
    /// Provider the panel currently focuses; nil = the fallback's full view
    private var focusedProvider: ProviderID?
    private let panel: PanelWindow
    private let hosting: NSHostingController<PanelView>
    private var globalMonitor: Any?
    private var keyMonitor: Any?
    private var sizeObservation: NSKeyValueObservation?
    private var refreshCancellable: AnyCancellable?
    private var configCancellable: AnyCancellable?
    private let settingsWindow = SettingsWindowController()
    let vm: ViewModel

    private static let panelWidth: CGFloat = 344

    init(vm: ViewModel) {
        self.vm = vm
        hosting = NSHostingController(rootView: PanelView(vm: vm, focus: nil, onRefresh: {}, onSettings: {}))
        hosting.sizingOptions = .preferredContentSize
        panel = PanelWindow(
            contentRect: NSRect(x: 0, y: 0, width: Self.panelWidth, height: 260),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered, defer: false)
        panel.level = .statusBar
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.contentViewController = hosting
        super.init()

        hosting.rootView = PanelView(vm: vm, focus: nil, onRefresh: { [weak self] in
            Task { await self?.refreshAndRedraw(force: true, withPlan: true, focus: nil) }
        }, onSettings: { [weak self] in
            self?.openSettings()
        })
        sizeObservation = hosting.observe(\.preferredContentSize, options: [.new]) { [weak self] _, _ in
            Task { @MainActor [weak self] in self?.resizePanelKeepingTop() }
        }
        // Menu bar numbers must follow background scheduled refreshes (independent of the panel being open)
        refreshCancellable = vm.$lastRefresh.sink { [weak self] _ in
            self?.redrawStatus()
        }
        // Add/remove icons when providers are enabled/disabled
        configCancellable = vm.$config.sink { [weak self] _ in
            self?.rebuildItems()
        }
        rebuildItems()
    }

    // MARK: - Menu bar icons: one "brand avatar + two-line percentages" group per provider (top 5h, bottom weekly)

    private func rebuildItems() {
        let enabled = vm.enabledIDs()
        for (id, item) in items where !enabled.contains(id) {
            NSStatusBar.system.removeStatusItem(item)
            items[id] = nil
        }
        for id in ProviderID.allCases where enabled.contains(id) && items[id] == nil {
            let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
            item.button?.target = self
            item.button?.action = #selector(statusItemClicked(_:))
            item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
            item.button?.identifier = NSUserInterfaceItemIdentifier(id.rawValue)
            items[id] = item
        }
        if enabled.isEmpty {
            if fallbackItem == nil {
                let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
                item.button?.target = self
                item.button?.action = #selector(statusItemClicked(_:))
                item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
                let img = NSImage(systemSymbolName: "gauge", accessibilityDescription: "UsageTray")
                img?.isTemplate = true
                item.button?.image = img
                fallbackItem = item
            }
        } else if let fallback = fallbackItem {
            NSStatusBar.system.removeStatusItem(fallback)
            fallbackItem = nil
        }
        redrawStatus()
    }

    @objc private func statusItemClicked(_ sender: NSStatusBarButton) {
        let id = sender.identifier.flatMap { ProviderID(rawValue: $0.rawValue) }
        // Tap the same icon again to collapse; tapping another icon switches the focused provider
        if panel.isVisible, focusedProvider == id {
            closePanel()
            return
        }
        focusedProvider = id
        openPanel()
    }

    func openPanel() {
        installDismissMonitors()
        positionPanel()
        hosting.rootView = PanelView(vm: vm, focus: focusedProvider, onRefresh: { [weak self] in
            // Manual refresh inside the panel: only refreshes the focused provider
            Task { await self?.refreshAndRedraw(force: true, withPlan: true, focus: self?.focusedProvider) }
        }, onSettings: { [weak self] in
            self?.openSettings()
        })
        // Show before activating: early after launch the activation handshake isn't stable yet,
        // and show-after-activate gets swallowed by activation jitter / hidesOnDeactivate
        panel.makeKeyAndOrderFront(nil)
        // nonactivating panel: activation is only for keyboard focus (Cmd+V etc.); failing is harmless
        Task { await refreshAndRedraw(force: true, withPlan: true, focus: focusedProvider) }
    }

    func closePanel() {
        panel.orderOut(nil)
        removeDismissMonitors()
    }

    func openSettings() {
        closePanel()
        settingsWindow.show(vm: vm)
    }

    private func installDismissMonitors() {
        guard globalMonitor == nil else { return }
        globalMonitor = NSEvent.addGlobalMonitorForEvents(
            matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in
            Task { @MainActor [weak self] in self?.closePanel() }
        }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] ev in
            if ev.keyCode == 53, self?.panel.isVisible == true {
                self?.closePanel()
                return nil
            }
            return ev
        }
    }

    private func removeDismissMonitors() {
        if let m = globalMonitor { NSEvent.removeMonitor(m); globalMonitor = nil }
        if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil }
    }

    private var focusedButton: NSStatusBarButton? {
        if let id = focusedProvider { return items[id]?.button }
        return fallbackItem?.button
    }

    private func positionPanel() {
        guard let button = focusedButton, let buttonWindow = button.window else { return }
        let buttonRect = buttonWindow.convertToScreen(button.convert(button.bounds, to: nil))
        let size = preferredPanelSize()
        panel.setContentSize(size)
        var x = buttonRect.midX - size.width / 2
        if let screen = buttonWindow.screen ?? NSScreen.main {
            x = min(max(x, screen.visibleFrame.minX + 8),
                    screen.visibleFrame.maxX - size.width - 8)
        }
        // The panel's top edge sits ~6pt below the menu bar button's bottom edge
        panel.setFrameTopLeftPoint(NSPoint(x: x, y: buttonRect.minY - 6))
    }

    private func preferredPanelSize() -> CGSize {
        var size = hosting.preferredContentSize
        if size.width < 1 || size.height < 1 {
            size = hosting.view.fittingSize
        }
        return CGSize(width: Self.panelWidth,
                      height: max(120, min(size.height, 680)))
    }

    private func resizePanelKeepingTop() {
        guard panel.isVisible else { return }
        let currentTop = panel.frame.origin.y + panel.frame.height
        let x = panel.frame.origin.x
        panel.setContentSize(preferredPanelSize())
        panel.setFrameTopLeftPoint(NSPoint(x: x, y: currentTop))
    }

    private func refreshAndRedraw(force: Bool, withPlan: Bool, focus: ProviderID?) async {
        await vm.refresh(force: force, withPlan: withPlan, focus: focus)
        redrawStatus()
        resizePanelKeepingTop()
    }

    // MARK: - Menu bar drawing

    func redrawStatus() {
        let entries = vm.menuBarEntries()
        let byID = Dictionary(uniqueKeysWithValues: entries.map { ($0.id, $0) })
        for (id, item) in items {
            item.button?.image = byID[id].map { Self.drawMenuBarImage([$0]) }
        }
    }

    nonisolated static func drawMenuBarImage(_ entries: [ViewModel.MenuBarEntry]) -> NSImage {
        let font = NSFont.monospacedDigitSystemFont(ofSize: 9, weight: .semibold)
        let groupW: CGFloat = 40
        let gap: CGFloat = 10
        let width = CGFloat(entries.count) * groupW + CGFloat(max(0, entries.count - 1)) * gap + 4
        let height: CGFloat = 22

        let img = NSImage(size: NSSize(width: width, height: height), flipped: false) { _ in
            let attrs: [NSAttributedString.Key: Any] = [
                .font: font,
                .foregroundColor: NSColor.labelColor,
            ]
            var x: CGFloat = 2
            for e in entries {
                // Brand avatar: rounded square + initial
                let avatar = NSRect(x: x, y: 5.5, width: 11.5, height: 11.5)
                let fill: NSColor
                switch e.id {
                case .kimi: fill = .labelColor
                case .zhipu: fill = NSColor(red: 0.16, green: 0.42, blue: 0.98, alpha: 1)
                case .opencode: fill = NSColor(red: 0.12, green: 0.70, blue: 0.45, alpha: 1)
                case .commandcode: fill = NSColor(red: 0.60, green: 0.38, blue: 0.96, alpha: 1)
                }
                fill.setFill()
                NSBezierPath(roundedRect: avatar, xRadius: 3.2, yRadius: 3.2).fill()
                let letter = NSAttributedString(string: e.id.initial, attributes: [
                    .font: NSFont.systemFont(ofSize: 7.5, weight: .heavy),
                    .foregroundColor: e.id == .kimi ? NSColor.windowBackgroundColor : NSColor.white,
                ])
                let ls = letter.size()
                letter.draw(at: NSPoint(x: avatar.midX - ls.width / 2,
                                        y: avatar.midY - ls.height / 2))

                // Two-line percentages: top 5h, bottom weekly
                NSAttributedString(string: e.shortPct, attributes: attrs).draw(at: NSPoint(x: x + 15.5, y: 10.6))
                NSAttributedString(string: e.weeklyPct, attributes: attrs).draw(at: NSPoint(x: x + 15.5, y: 1.6))
                x += groupW + gap
            }
            return true
        }
        return img
    }
}
