import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusController: StatusItemController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        installEditMenu()
        let vm = ViewModel()
        let controller = StatusItemController(vm: vm)
        statusController = controller
        vm.startAutoRefresh()
        let args = CommandLine.arguments
        if args.contains("--show-panel") || args.contains("--show-settings") {
            // Pop up after the status item layout settles
            Timer.scheduledTimer(withTimeInterval: 1.0, repeats: false) { _ in
                Task { @MainActor in
                    if args.contains("--show-settings") {
                        controller.openSettings()
                    } else {
                        controller.openPanel()
                    }
                }
            }
        }
    }

    /// Accessory apps have no main menu by default, so Cmd+C/V are dead;
    /// install a minimal Edit menu so text fields can copy/paste
    private func installEditMenu() {
        let main = NSMenu()
        let edit = NSMenu(title: "Edit")
        let actions: [(String, Selector, String)] = [
            ("Cut", #selector(NSText.cut(_:)), "x"),
            ("Copy", #selector(NSText.copy(_:)), "c"),
            ("Paste", #selector(NSText.paste(_:)), "v"),
            ("Select All", #selector(NSText.selectAll(_:)), "a"),
        ]
        for (title, action, key) in actions {
            edit.addItem(NSMenuItem(title: title, action: action, keyEquivalent: key))
        }
        let root = NSMenuItem()
        root.submenu = edit
        main.addItem(root)
        NSApp.mainMenu = main
    }
}

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    app.setActivationPolicy(.accessory)
    app.run()
}
