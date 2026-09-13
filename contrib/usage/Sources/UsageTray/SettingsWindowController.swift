import AppKit
import SwiftUI

@MainActor
final class SettingsWindowController {
    private var window: NSWindow?
    private weak var vm: ViewModel?

    func show(vm: ViewModel) {
        self.vm = vm
        if window == nil {
            // Size the window to the content so long notes wrap fully instead of getting clipped
            let content = NSHostingView(rootView: SettingsView(vm: vm))
            let size = content.fittingSize
            let w = NSWindow(
                contentRect: NSRect(x: 0, y: 0,
                                    width: max(420, size.width),
                                    height: max(400, size.height)),
                styleMask: [.titled, .closable],
                backing: .buffered, defer: false)
            w.title = "UsageTray Settings"
            w.isReleasedWhenClosed = false
            w.contentView = content
            w.center()
            window = w
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}
