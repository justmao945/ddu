import Foundation

// MARK: - Shared HTTP

enum Api {
    struct Response {
        let data: Data
        let status: Int
    }

    static func getJSON(_ url: URL, apiKey: String, userAgent: String? = nil) async throws -> Response {
        var req = URLRequest(url: url)
        req.timeoutInterval = 15
        req.httpMethod = "GET"
        req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if let userAgent { req.setValue(userAgent, forHTTPHeaderField: "User-Agent") }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse else {
            throw ApiError.network("Non-HTTP response")
        }
        return Response(data: data, status: http.statusCode)
    }

    static func errorText(status: Int, body: Data, provider: String) -> String {
        let snippet = String(data: body.prefix(200), encoding: .utf8) ?? ""
        switch status {
        case 401, 403:
            return "\(provider) auth failed (\(status)): API key invalid or expired"
        case 429:
            return "\(provider) rate limited (429)"
        default:
            return "\(provider) API error HTTP \(status) \(snippet)"
        }
    }
}

struct ApiError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
    init(_ message: String) { self.message = message }
    static func network(_ m: String) -> ApiError { ApiError(m) }
}

// MARK: - Kimi Code (api.kimi.com/coding/v1/usages)

enum KimiProvider {
    static let baseURL = URL(string: "https://api.kimi.com/coding/v1")!

    /// Remember the working path (/usages or fallback /usage) per session to avoid an extra probe request each round
    private static let pathLock = NSLock()
    private static var usagePath: String?

    private static func cachedUsagePath() -> String? {
        pathLock.lock(); defer { pathLock.unlock() }
        return usagePath
    }

    private static func setUsagePath(_ p: String) {
        pathLock.lock(); defer { pathLock.unlock() }
        usagePath = p
    }

    static func fetch(apiKey: String?) async -> ProviderSnapshot {
        guard let apiKey, !apiKey.isEmpty else {
            return .failed(.kimi, "No API key configured (sk-kimi-…, add it in Settings)")
        }
        do {
            // Documented path /usages, falling back to /usage on 404; the fallback result is reused for the whole process
            let resp: Api.Response
            if let known = cachedUsagePath() {
                resp = try await Api.getJSON(baseURL.appendingPathComponent(known),
                                             apiKey: apiKey, userAgent: "KimiCLI/1.6")
            } else {
                let first = try await Api.getJSON(baseURL.appendingPathComponent("usages"),
                                                  apiKey: apiKey, userAgent: "KimiCLI/1.6")
                if first.status == 404 {
                    resp = try await Api.getJSON(baseURL.appendingPathComponent("usage"),
                                                 apiKey: apiKey, userAgent: "KimiCLI/1.6")
                    setUsagePath("usage")
                } else {
                    resp = first
                    setUsagePath("usages")
                }
            }
            guard resp.status == 200 else {
                throw ApiError(Api.errorText(status: resp.status, body: resp.data, provider: "Kimi"))
            }
            return parse(resp.data)
        } catch let err as ApiError {
            return .failed(.kimi, err.message)
        } catch {
            return .failed(.kimi, "Kimi network error: \(error.localizedDescription)")
        }
    }

    static func parse(_ data: Data) -> ProviderSnapshot {
        guard let root = JSON.dict(try? JSONSerialization.jsonObject(with: data)) else {
            return .failed(.kimi, "Failed to parse Kimi response")
        }
        var windows: [WindowUsage] = []

        // Weekly window: payload.usage { limit, remaining, resetTime }
        if let usage = JSON.dict(root["usage"]),
           let row = windowRow(dict: usage, id: "kimi-week", label: "Weekly") {
            windows.append(row)
        }

        // 5-hour window: limits[] entries with window.duration == 300 MINUTE
        if let limits = JSON.array(root["limits"]) {
            for (idx, item) in limits.enumerated() {
                guard let item = JSON.dict(item) else { continue }
                let win = JSON.dict(item["window"]) ?? [:]
                let detail = JSON.dict(item["detail"]) ?? item
                let duration = JSON.int(win["duration"]) ?? 0
                let unit = (JSON.string(win["timeUnit"]) ?? "").uppercased()
                guard unit.contains("MINUTE") else { continue }
                let label = duration >= 60 && duration % 60 == 0
                    ? "5h window"
                    : "\(duration)-minute window"
                if let row = windowRow(dict: detail, id: "kimi-win-\(idx)", label: label) {
                    windows.append(row)
                }
            }
        }

        // Plan tier: user.membership.level
        let level = JSON.string(JSON.dict(JSON.dict(root["user"])?["membership"])?["level"]) ?? ""
        let plan: String? = {
            // Official tier names come from kimi.com ListGoods (membershipLevel → title)
            switch level {
            case "LEVEL_FREE": return "Adagio"
            case "LEVEL_TRIAL": return "Andante"
            case "LEVEL_BASIC": return "Moderato"
            case "LEVEL_INTERMEDIATE": return "Allegretto"
            case "LEVEL_ADVANCED": return "Allegro"
            default: return level.isEmpty ? nil : level
            }
        }()
        if windows.isEmpty {
            return .failed(.kimi, "No usage windows in Kimi response")
        }
        return ProviderSnapshot(id: .kimi, plan: plan, windows: Self.orderWindows(windows),
                                error: nil, fetchedAt: Date())
    }

    /// Unified window order: 5-hour → weekly → other
    static func orderWindows(_ ws: [WindowUsage]) -> [WindowUsage] {
        func rank(_ w: WindowUsage) -> Int {
            if w.label.contains("5h") { return 0 }
            if w.label == "Weekly" { return 1 }
            return 2
        }
        return ws.enumerated().sorted { a, b in
            rank(a.element) != rank(b.element) ? rank(a.element) < rank(b.element) : a.offset < b.offset
        }.map(\.element)
    }

    /// Kimi reports quota points (strings): used = limit - remaining
    private static func windowRow(dict: [String: Any], id: String, label: String) -> WindowUsage? {
        let limit = JSON.int(dict["limit"]) ?? JSON.int(dict["limit_amount"])
        let remaining = JSON.int(dict["remaining"])
        let used = JSON.int(dict["used"]) ?? JSON.int(dict["used_amount"])
            ?? limit.flatMap { l in remaining.map { l - $0 } }
        guard let limit, limit > 0, let used else { return nil }
        let pct = min(100, max(0, Double(used) / Double(limit) * 100))
        var resetText: String?
        if let resetDate = TimeFmt.isoDate(JSON.string(dict["resetTime"]))
            ?? TimeFmt.isoDate(JSON.string(dict["reset_at"])) {
            resetText = TimeFmt.resetText(until: resetDate)
        }
        return WindowUsage(id: id, label: label, usedPercent: pct, resetText: resetText)
    }
}

// MARK: - Zhipu GLM Coding Plan (open.bigmodel.cn)

enum ZhipuProvider {
    static let baseURL = URL(string: "https://open.bigmodel.cn")!

    /// Plan name: fetched once only by panel-open / manual refreshes; background polling skips this endpoint
    static func fetchPlanName(apiKey: String) async -> String? {
        guard let subs = try? await Api.getJSON(baseURL.appendingPathComponent("api/biz/subscription/list"),
                                                apiKey: apiKey), subs.status == 200 else { return nil }
        return parsePlanName(subs.data)
    }

    static func fetch(apiKey: String?, plan: String?) async -> ProviderSnapshot {
        guard let apiKey, !apiKey.isEmpty else {
            return .failed(.zhipu, "No API key configured (add it in Settings)")
        }
        do {
            let quota = try await Api.getJSON(baseURL.appendingPathComponent("api/monitor/usage/quota/limit"),
                                              apiKey: apiKey)
            guard quota.status == 200 else {
                throw ApiError(Api.errorText(status: quota.status, body: quota.data, provider: "Zhipu"))
            }
            return parseQuota(quota.data, plan: plan)
        } catch let err as ApiError {
            return .failed(.zhipu, err.message)
        } catch {
            return .failed(.zhipu, "Zhipu network error: \(error.localizedDescription)")
        }
    }

    static func parsePlanName(_ data: Data) -> String? {
        guard let root = JSON.dict(try? JSONSerialization.jsonObject(with: data)),
              let list = JSON.array(root["data"]),
              let first = JSON.dict(list.first) else { return nil }
        return JSON.string(first["productName"])
    }

    static func parseQuota(_ data: Data, plan: String?) -> ProviderSnapshot {
        guard let root = JSON.dict(try? JSONSerialization.jsonObject(with: data)) else {
            return .failed(.zhipu, "Failed to parse Zhipu response")
        }
        // Zhipu wraps 401-style business errors inside HTTP 200: {"code":401,"success":false,"msg":"..."}
        if let code = JSON.int(root["code"]), code != 200 {
            let msg = JSON.string(root["msg"]) ?? ""
            let message = code == 401
                ? "Zhipu auth failed (401): API key invalid or expired \(msg)"
                : "Zhipu API error code=\(code) \(msg)"
            return .failed(.zhipu, message)
        }
        guard let dataDict = JSON.dict(root["data"]),
              let limits = JSON.array(dataDict["limits"]) else {
            return .failed(.zhipu, "Failed to parse Zhipu response")
        }
        var windows: [WindowUsage] = []
        for (idx, item) in limits.enumerated() {
            guard let item = JSON.dict(item) else { continue }
            let type = JSON.string(item["type"]) ?? ""
            let unit = JSON.int(item["unit"]) ?? 0
            let pct = JSON.double(item["percentage"]) ?? 0
            var resetText: String?
            if let resetDate = TimeFmt.epochMs(item["nextResetTime"]) {
                resetText = TimeFmt.resetText(until: resetDate)
            }
            // The API actually returns CREDIT_LIMIT (older docs say TOKENS_LIMIT, same field layout)
            if type == "CREDIT_LIMIT" || type == "TOKENS_LIMIT" {
                let label = unit == 3 ? "5h window" : unit == 6 ? "Weekly" : "Window #\(idx + 1)"
                windows.append(WindowUsage(
                    id: "zhipu-\(type)-\(unit)-\(idx)", label: label,
                    usedPercent: min(100, max(0, pct)),
                    resetText: resetText))
            } else if type == "TIME_LIMIT" {
                windows.append(WindowUsage(
                    id: "zhipu-\(type)-\(idx)", label: "Tools (Monthly)",
                    usedPercent: min(100, max(0, pct)),
                    resetText: resetText))
            }
        }
        if windows.isEmpty {
            return .failed(.zhipu, "No usage windows in Zhipu response")
        }
        return ProviderSnapshot(id: .zhipu, plan: plan, windows: KimiProvider.orderWindows(windows),
                                error: nil, fetchedAt: Date())
    }
}

// MARK: - Local CLI login reuse (auto-resolves the key when Settings is left empty)

enum LocalAuth {
    /// OpenCode Go: the "opencode-go" entry in ~/.local/share/opencode/auth.json (XDG_DATA_HOME overrides the root)
    static func opencodeGoKey() -> String? {
        let base = ProcessInfo.processInfo.environment["XDG_DATA_HOME"].map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/share", isDirectory: true)
        let file = base.appendingPathComponent("opencode/auth.json")
        guard let root = JSON.dict(try? JSONSerialization.jsonObject(with: Data(contentsOf: file))),
              let entry = JSON.dict(root["opencode-go"]) else { return nil }
        if let key = JSON.string(entry["key"]), !key.isEmpty { return key }
        // Tolerates the tokens-array shape
        if let tokens = JSON.array(entry["tokens"]),
           let first = JSON.string(tokens.first), !first.isEmpty { return first }
        return nil
    }

    /// Command Code: the "apiKey" field in ~/.commandcode/auth.json (user_…)
    static func commandCodeKey() -> String? {
        let file = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".commandcode/auth.json")
        guard let root = JSON.dict(try? JSONSerialization.jsonObject(with: Data(contentsOf: file))),
              let key = JSON.string(root["apiKey"]), !key.isEmpty else { return nil }
        return key
    }

    /// A key exists in Settings or the matching CLI is logged in (drives the Settings toggle and ConfigStore)
    static func hasCredential(id: ProviderID, settingsKey: String) -> Bool {
        if !settingsKey.isEmpty { return true }
        switch id {
        case .opencode: return opencodeGoKey() != nil
        case .commandcode: return commandCodeKey() != nil
        case .kimi, .zhipu: return false
        }
    }
}

// MARK: - OpenCode Go (opencode.ai/zen/go/v1/usage)

enum OpenCodeProvider {
    static let usageURL = URL(string: "https://opencode.ai/zen/go/v1/usage")!

    static func fetch(apiKey: String?) async -> ProviderSnapshot {
        let key = (apiKey?.isEmpty == false ? apiKey : nil) ?? LocalAuth.opencodeGoKey()
        guard let key else {
            return .failed(.opencode, "No API key configured (add it in Settings, or log in via opencode to auto-read)")
        }
        do {
            let resp = try await Api.getJSON(usageURL, apiKey: key)
            guard resp.status == 200 else {
                throw ApiError(Api.errorText(status: resp.status, body: resp.data, provider: "OpenCode"))
            }
            return parse(resp.data)
        } catch let err as ApiError {
            return .failed(.opencode, err.message)
        } catch {
            return .failed(.opencode, "OpenCode network error: \(error.localizedDescription)")
        }
    }

    static func parse(_ data: Data) -> ProviderSnapshot {
        guard let root = JSON.dict(try? JSONSerialization.jsonObject(with: data)),
              let usage = JSON.dict(root["usage"]) else {
            return .failed(.opencode, "Failed to parse OpenCode response")
        }
        var windows: [WindowUsage] = []
        for (name, id, label) in [("rolling", "oc-rolling", "5h window"),
                                  ("weekly", "oc-weekly", "Weekly"),
                                  ("monthly", "oc-monthly", "Monthly")] {
            guard let w = JSON.dict(usage[name]) else { continue }
            let pct = JSON.double(w["percent"]) ?? JSON.double(w["usagePercent"]) ?? 0
            var resetText: String?
            if let resetDate = TimeFmt.isoDate(JSON.string(w["resetsAt"])) {
                resetText = TimeFmt.resetText(until: resetDate)
            }
            windows.append(WindowUsage(id: id, label: label,
                                       usedPercent: min(100, max(0, pct)),
                                       resetText: resetText))
        }
        if windows.isEmpty {
            return .failed(.opencode, "No usage windows in OpenCode response")
        }
        return ProviderSnapshot(id: .opencode, plan: "OpenCode Go",
                                windows: KimiProvider.orderWindows(windows),
                                error: nil, fetchedAt: Date())
    }
}

// MARK: - Command Code (api.commandcode.ai/alpha)

enum CommandCodeProvider {
    static let baseURL = URL(string: "https://api.commandcode.ai")!

    static func fetch(apiKey: String?) async -> ProviderSnapshot {
        let key = (apiKey?.isEmpty == false ? apiKey : nil) ?? LocalAuth.commandCodeKey()
        guard let key else {
            return .failed(.commandcode, "No API key configured (add it in Settings, or log in via cmd to auto-read ~/.commandcode/auth.json)")
        }
        do {
            async let creditsTask = Api.getJSON(baseURL.appendingPathComponent("alpha/billing/credits"), apiKey: key)
            async let subsTask = Api.getJSON(baseURL.appendingPathComponent("alpha/billing/subscriptions"), apiKey: key)
            let credits = try await creditsTask
            guard credits.status == 200 else {
                throw ApiError(Api.errorText(status: credits.status, body: credits.data, provider: "Command Code"))
            }
            let subs = try? await subsTask
            let plan = subs.flatMap { $0.status == 200 ? parsePlanName($0.data) : nil }
            // Monthly usage: monthly credits consumed this period / (consumed + remaining balance), reset = subscription period end
            let monthly = await monthlyUsage(apiKey: key, subs: subs)
            return parse(credits.data, plan: plan, monthly: monthly)
        } catch let err as ApiError {
            return .failed(.commandcode, err.message)
        } catch {
            return .failed(.commandcode, "Command Code network error: \(error.localizedDescription)")
        }
    }

    /// Plan name: normalized planId (individual-goat → GoAT, …-go → Go)
    static func parsePlanName(_ data: Data) -> String? {
        guard let root = JSON.dict(try? JSONSerialization.jsonObject(with: data)),
              let dataDict = JSON.dict(root["data"]),
              let planId = JSON.string(dataDict["planId"]) else { return nil }
        if planId.contains("goat") { return "GoAT" }
        if planId == "go" || planId.hasSuffix("-go") { return "Go" }
        return planId
    }

    static func parse(_ data: Data, plan: String?, monthly: (used: Double, reset: Date?)? = nil) -> ProviderSnapshot {
        guard let root = JSON.dict(try? JSONSerialization.jsonObject(with: data)),
              let limits = JSON.dict(root["windowLimits"]) else {
            return .failed(.commandcode, "Failed to parse Command Code response")
        }
        var windows: [WindowUsage] = []
        if let w = JSON.dict(limits["fiveHour"]),
           let row = windowRow(w, id: "cc-5h", label: "5h window") {
            windows.append(row)
        }
        if let w = JSON.dict(limits["weekly"]),
           let row = windowRow(w, id: "cc-weekly", label: "Weekly") {
            windows.append(row)
        }
        // Monthly quota: skipped when the summary is missing or no balance remains
        if let monthly,
           let creditsDict = JSON.dict(root["credits"]),
           let remaining = JSON.double(creditsDict["monthlyCredits"]),
           monthly.used + remaining > 0 {
            windows.append(WindowUsage(
                id: "cc-monthly", label: "Monthly",
                usedPercent: min(100, max(0, monthly.used / (monthly.used + remaining) * 100)),
                resetText: monthly.reset.map { TimeFmt.resetText(until: $0) }))
        }
        if windows.isEmpty {
            return .failed(.commandcode, "No usage windows in Command Code response")
        }
        return ProviderSnapshot(id: .commandcode, plan: plan,
                                windows: KimiProvider.orderWindows(windows),
                                error: nil, fetchedAt: Date())
    }

    /// Monthly credits consumed this period and the period end; nil when the summary or subscription info is missing
    private static func monthlyUsage(apiKey: String, subs: Api.Response?) async -> (used: Double, reset: Date?)? {
        guard let subs, subs.status == 200,
              let root = JSON.dict(try? JSONSerialization.jsonObject(with: subs.data)),
              let dataDict = JSON.dict(root["data"]) else { return nil }
        let reset = TimeFmt.isoDate(JSON.string(dataDict["currentPeriodEnd"]))
        do {
            var url = baseURL.appendingPathComponent("alpha/usage/summary")
            if let start = JSON.string(dataDict["currentPeriodStart"]) {
                var comps = URLComponents(url: url, resolvingAgainstBaseURL: false)!
                comps.queryItems = [URLQueryItem(name: "since", value: start)]
                url = comps.url!
            }
            let resp = try await Api.getJSON(url, apiKey: apiKey)
            guard resp.status == 200,
                  let root = JSON.dict(try? JSONSerialization.jsonObject(with: resp.data)) else { return nil }
            let used = JSON.double(root["totalMonthlyCredits"]) ?? JSON.double(root["totalCost"])
            return used.map { ($0, reset) }
        } catch {
            return nil
        }
    }

    /// windowLimits are credit-denominated points: used / cap; resetAt is a second/millisecond epoch (0 = window not started)
    private static func windowRow(_ w: [String: Any], id: String, label: String) -> WindowUsage? {
        guard let cap = JSON.double(w["cap"]), cap > 0, let used = JSON.double(w["used"]) else { return nil }
        var resetText: String?
        if let resetDate = TimeFmt.flexibleDate(w["resetAt"]) {
            resetText = TimeFmt.resetText(until: resetDate)
        }
        return WindowUsage(id: id, label: label,
                           usedPercent: min(100, max(0, used / cap * 100)),
                           resetText: resetText)
    }
}
