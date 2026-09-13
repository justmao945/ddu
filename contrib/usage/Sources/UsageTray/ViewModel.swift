import Foundation
import SwiftUI

@MainActor
final class ViewModel: ObservableObject {
    @Published var config: AppConfig {
        didSet {
            ConfigStore.save(config)
            guard config != oldValue else { return }
            if config.zhipu.apiKey != oldValue.zhipu.apiKey { planName = nil }
            scheduleConfigRefresh()
            if config.refreshSeconds != oldValue.refreshSeconds {
                restartTimer()
            }
        }
    }

    @Published var snapshots: [ProviderID: ProviderSnapshot] = [:]
    @Published var refreshing = false
    @Published var lastRefresh: Date?
    /// Zhipu plan name: fetched only by refreshes carrying withPlan (once per process); background polling skips it
    private var planName: String?

    static let throttleInterval: TimeInterval = 20
    private var timer: Timer?
    private var configRefreshTask: Task<Void, Never>?

    init() {
        config = ConfigStore.load()
    }

    func enabledIDs(_ config: AppConfig? = nil) -> Set<ProviderID> {
        let c = config ?? self.config
        var result: Set<ProviderID> = []
        if c.kimi.enabled { result.insert(.kimi) }
        if c.zhipu.enabled { result.insert(.zhipu) }
        if c.opencode.enabled { result.insert(.opencode) }
        if c.commandcode.enabled { result.insert(.commandcode) }
        return result
    }

    func startAutoRefresh() {
        restartTimer()
        Task { await refresh(force: true) }
    }

    private func restartTimer() {
        timer?.invalidate()
        let interval = TimeInterval(max(30, config.refreshSeconds))
        timer = Timer.scheduledTimer(withTimeInterval: interval, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                await self?.refresh(force: false)
            }
        }
    }

    /// Debounced 800ms forced refresh after config changes (including saving keys)
    private func scheduleConfigRefresh() {
        configRefreshTask?.cancel()
        configRefreshTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 800_000_000)
            guard !Task.isCancelled else { return }
            await self?.refresh(force: true)
        }
    }

    /// Single-flight: reject while a round is in flight (including panel-open refreshes); at most one round at any moment
    /// force=true skips the 20s throttle; withPlan=true additionally fetches the plan name (once per process)
    /// focus=one provider refreshes only that one (tapping its menu bar icon); nil refreshes all
    func refresh(force: Bool, withPlan: Bool = false, focus: ProviderID? = nil) async {
        if refreshing { return }
        if !force, let last = lastRefresh, Date().timeIntervalSince(last) < Self.throttleInterval { return }
        refreshing = true
        defer { refreshing = false; lastRefresh = Date() }
        let wantPlan = withPlan && planName == nil && (focus == nil || focus == .zhipu)
            && config.zhipu.enabled && !config.zhipu.apiKey.isEmpty
        let wantKimi = (focus == nil || focus == .kimi) && config.kimi.enabled
        let wantZhipu = (focus == nil || focus == .zhipu) && config.zhipu.enabled
        let wantOpencode = (focus == nil || focus == .opencode) && config.opencode.enabled
        let wantCommandcode = (focus == nil || focus == .commandcode) && config.commandcode.enabled
        async let planTask: String? = wantPlan
            ? ZhipuProvider.fetchPlanName(apiKey: config.zhipu.apiKey) : nil
        async let kimiTask: ProviderSnapshot? = wantKimi
            ? KimiProvider.fetch(apiKey: config.kimi.apiKey) : nil
        async let zhipuTask: ProviderSnapshot? = wantZhipu
            ? ZhipuProvider.fetch(apiKey: config.zhipu.apiKey, plan: planName) : nil
        async let opencodeTask: ProviderSnapshot? = wantOpencode
            ? OpenCodeProvider.fetch(apiKey: config.opencode.apiKey) : nil
        async let commandcodeTask: ProviderSnapshot? = wantCommandcode
            ? CommandCodeProvider.fetch(apiKey: config.commandcode.apiKey) : nil
        if let k = await kimiTask {
            snapshots[.kimi] = k
        }
        if let z = await zhipuTask {
            snapshots[.zhipu] = z
        }
        if let o = await opencodeTask {
            snapshots[.opencode] = o
        }
        if let cc = await commandcodeTask {
            snapshots[.commandcode] = cc
        }
        if let p = await planTask {
            planName = p
            snapshots[.zhipu]?.plan = p
        }
    }


    // MARK: - Menu bar data

    struct MenuBarEntry {
        let id: ProviderID
        let tier: UsageColor?
        /// Top: 5-hour window; bottom: weekly
        let shortPct: String
        let weeklyPct: String
    }

    func menuBarEntries() -> [MenuBarEntry] {
        ProviderID.allCases.filter { enabledIDs().contains($0) }.map { id in
            menuBarEntry(for: id)
        }
    }

    private func menuBarEntry(for id: ProviderID) -> MenuBarEntry {
        guard let snap = snapshots[id], snap.error == nil else {
            return MenuBarEntry(id: id, tier: nil, shortPct: "—", weeklyPct: "—")
        }
        let fiveHour = snap.windows.first { $0.label.contains("5h") } ?? snap.windows.first
        let weekly = snap.windows.first { $0.label == "Weekly" } ?? snap.windows.first

        var tier: UsageColor? = nil
        for color in [fiveHour?.colorTier, weekly?.colorTier].compactMap({ $0 }) {
            if tier == nil || color.ordinal > tier!.ordinal { tier = color }
        }

        let short: String
        if let used = fiveHour?.usedPercent { short = "\(Int(used.rounded()))%" } else { short = "—" }
        let weeklyText: String
        if let used = weekly?.usedPercent { weeklyText = "\(Int(used.rounded()))%" } else { weeklyText = "—" }
        return MenuBarEntry(id: id, tier: tier, shortPct: short, weeklyPct: weeklyText)
    }
}
