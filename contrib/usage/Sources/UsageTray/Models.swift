import Foundation

enum ProviderID: String, CaseIterable {
    case kimi
    case zhipu
    case opencode
    case commandcode

    var displayName: String {
        switch self {
        case .kimi: return "Kimi"
        case .zhipu: return "Zhipu GLM"
        case .opencode: return "OpenCode"
        case .commandcode: return "Command Code"
        }
    }
    /// Plan usage page, opened by tapping the plan capsule in the panel
    var planURL: URL {
        switch self {
        case .kimi: return URL(string: "https://www.kimi.com/membership/subscription?tab=quota")!
        case .zhipu: return URL(string: "https://bigmodel.cn/coding-plan/personal/usage")!
        case .opencode: return URL(string: "https://opencode.ai/go")!
        case .commandcode: return URL(string: "https://commandcode.ai/usage")!
        }
    }
}

/// One usage window (5-hour / weekly / monthly tool quota).
struct WindowUsage: Identifiable {
    let id: String
    /// Window name, e.g. "5h window", "Weekly", "Tools (Monthly)"
    let label: String
    /// Used percent, 0-100
    let usedPercent: Double
    /// Reset countdown text, e.g. "Resets in 2h 13m"
    let resetText: String?

    var colorTier: UsageColor { usageColor(forUsed: usedPercent) }
}

enum UsageColor {
    case normal, warn, critical

    var ordinal: Int {
        switch self {
        case .normal: return 0
        case .warn: return 1
        case .critical: return 2
        }
    }

}

func usageColor(forUsed percent: Double) -> UsageColor {
    if percent >= 90 { return .critical }
    if percent >= 70 { return .warn }
    return .normal
}

/// Full snapshot for a single provider.
struct ProviderSnapshot {
    let id: ProviderID
    var plan: String?
    var windows: [WindowUsage]
    var error: String?
    var fetchedAt: Date?

    static func failed(_ id: ProviderID, _ message: String) -> ProviderSnapshot {
        ProviderSnapshot(id: id, plan: nil, windows: [], error: message, fetchedAt: nil)
    }
}

// MARK: - Lenient JSON accessors (providers mix string/number fields; no strict Codable)

enum JSON {
    static func dict(_ any: Any?) -> [String: Any]? { any as? [String: Any] }
    static func array(_ any: Any?) -> [Any]? { any as? [Any] }

    static func int(_ any: Any?) -> Int? {
        if let i = any as? Int { return i }
        if let d = any as? Double { return Int(d) }
        if let s = any as? String { return Int(s) }
        return nil
    }

    static func double(_ any: Any?) -> Double? {
        if let d = any as? Double { return d }
        if let i = any as? Int { return Double(i) }
        if let s = any as? String { return Double(s) }
        return nil
    }

    static func string(_ any: Any?) -> String? {
        if let s = any as? String { return s }
        if let i = any as? Int { return String(i) }
        if let d = any as? Double { return String(d) }
        return nil
    }
}

// MARK: - Time formatting

enum TimeFmt {
    /// "2026-03-12T16:55:13Z" / ISO with a timezone
    static func isoDate(_ s: String?) -> Date? {
        guard let s, !s.isEmpty else { return nil }
        let fmt = ISO8601DateFormatter()
        fmt.formatOptions = [.withInternetDateTime]
        if let d = fmt.date(from: s) { return d }
        fmt.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fmt.date(from: s)
    }

    static func epochMs(_ any: Any?) -> Date? {
        guard let ms = JSON.double(any) else { return nil }
        return Date(timeIntervalSince1970: ms / 1000)
    }

    /// Numeric timestamp (seconds or milliseconds, ≤0 means none) or ISO string — Command Code has used both
    static func flexibleDate(_ any: Any?) -> Date? {
        if let d = JSON.double(any) {
            guard d > 0 else { return nil }
            return Date(timeIntervalSince1970: d >= 1e12 ? d / 1000 : d)
        }
        guard let s = JSON.string(any), !s.isEmpty else { return nil }
        if let d = isoDate(s) { return d }
        // Timezone-less ISO is treated as UTC
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        fmt.timeZone = TimeZone(identifier: "UTC")
        return fmt.date(from: s)
    }

    /// "2h 13m" / "3d 4h" / "45m" / "Reset"
    static func countdown(until date: Date, now: Date = .init()) -> String {
        let interval = date.timeIntervalSince(now)
        if interval <= 0 { return "Reset" }
        let totalMinutes = Int(interval / 60)
        let days = totalMinutes / (60 * 24)
        let hours = (totalMinutes % (60 * 24)) / 60
        let minutes = totalMinutes % 60
        var parts: [String] = []
        if days > 0 { parts.append("\(days)d") }
        if hours > 0 { parts.append("\(hours)h") }
        if minutes > 0 || parts.isEmpty { parts.append("\(minutes)m") }
        return parts.joined(separator: " ")
    }

    /// "Resets in 2h 13m"; an expired window just shows "Reset"
    static func resetText(until date: Date, now: Date = .init()) -> String {
        let cd = countdown(until: date, now: now)
        return cd == "Reset" ? cd : "Resets in \(cd)"
    }
}
