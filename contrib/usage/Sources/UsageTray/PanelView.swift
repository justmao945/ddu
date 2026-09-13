import SwiftUI

extension ProviderID {
    var initial: String {
        switch self {
        case .kimi: return "K"
        case .zhipu: return "Z"
        case .opencode: return "O"
        case .commandcode: return "C"
        }
    }

    /// Kimi black (adapts in dark mode), Zhipu blue, OpenCode terminal green, Command Code purple
    var brandColor: Color {
        switch self {
        case .kimi: return Color(nsColor: .labelColor)
        case .zhipu: return Color(red: 0.16, green: 0.42, blue: 0.98)
        case .opencode: return Color(red: 0.12, green: 0.70, blue: 0.45)
        case .commandcode: return Color(red: 0.60, green: 0.38, blue: 0.96)
        }
    }

    var avatarTextColor: Color {
        self == .kimi ? Color(nsColor: .windowBackgroundColor) : .white
    }

    var nsBrandColor: NSColor {
        switch self {
        case .kimi: return .labelColor
        case .zhipu: return NSColor(red: 0.16, green: 0.42, blue: 0.98, alpha: 1)
        case .opencode: return NSColor(red: 0.12, green: 0.70, blue: 0.45, alpha: 1)
        case .commandcode: return NSColor(red: 0.60, green: 0.38, blue: 0.96, alpha: 1)
        }
    }

}

extension UsageColor {
    var swiftColor: Color {
        switch self {
        case .normal: return .accentColor
        case .warn: return .orange
        case .critical: return .red
        }
    }
}

struct PanelView: View {
    @ObservedObject var vm: ViewModel
    /// Focus on a single provider (when its menu bar icon is tapped); nil = fallback view
    let focus: ProviderID?
    let onRefresh: () -> Void
    let onSettings: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            let ids = focus.map { [$0] } ?? ProviderID.allCases.filter { vm.enabledIDs().contains($0) }
            if ids.isEmpty {
                Label("All providers are disabled — enable them in Settings", systemImage: "power")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(ids, id: \.self) { providerCard($0) }
            }
            footer
        }
        .padding(12)
        .frame(width: 344)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Color.primary.opacity(0.06)))
    }

    private func providerCard(_ id: ProviderID) -> some View {
        let snap = vm.snapshots[id]
        return VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                ProviderAvatar(id: id, size: 24)
                Text(id.displayName)
                    .font(.system(size: 13, weight: .semibold))
                if let plan = snap?.plan, snap?.error == nil {
                    Link(destination: id.planURL) {
                        Text(plan)
                            .font(.caption2)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(id.brandColor.opacity(id == .kimi ? 0.10 : 0.14), in: Capsule())
                            .foregroundStyle(id.brandColor)
                    }
                    .buttonStyle(.plain)
                    .onHover { hovering in
                        if hovering { NSCursor.pointingHand.push() } else { NSCursor.pop() }
                    }
                    .help("Open the \(id.displayName) usage page")
                }
                Spacer()
            }
            if let error = snap?.error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
            } else if let snap {
                ForEach(snap.windows) { window in
                    WindowRow(window: window)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 10))
    }

    private var footer: some View {
        HStack(spacing: 8) {
            if let last = vm.lastRefresh {
                Text("Updated \(last.formatted(date: .omitted, time: .shortened))")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if vm.refreshing {
                ProgressView().controlSize(.mini)
            } else {
                Button { onRefresh() } label: {
                    Image(systemName: "arrow.clockwise").font(.system(size: 11))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Refresh now")
            }
            Button { onSettings() } label: {
                Image(systemName: "gearshape").font(.system(size: 11))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("Settings (separate window)")
        }
    }
}

struct ProviderAvatar: View {
    let id: ProviderID
    var size: CGFloat = 24

    var body: some View {
        ZStack {
            Circle().fill(
                LinearGradient(colors: [id.brandColor, id.brandColor.opacity(0.7)],
                               startPoint: .topLeading, endPoint: .bottomTrailing))
            Text(id.initial)
                .font(.system(size: size * 0.46, weight: .heavy, design: .rounded))
                .foregroundStyle(id.avatarTextColor)
        }
        .frame(width: size, height: size)
    }
}

struct WindowRow: View {
    let window: WindowUsage

    private var tint: Color { window.colorTier.swiftColor }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .firstTextBaseline) {
                Text(window.label)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                Text("\(Int(window.usedPercent.rounded()))%")
                    .font(.callout.weight(.bold).monospacedDigit())
                    .foregroundStyle(tint)
            }
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.primary.opacity(0.08))
                    Capsule().fill(tint)
                        .frame(width: max(3, geo.size.width * window.usedPercent / 100))
                }
            }
            .frame(height: 5)
            HStack {
                Spacer()
                if let reset = window.resetText {
                    Text(reset)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
            }
        }
    }
}
