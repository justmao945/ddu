import Foundation

struct ProviderConfig: Codable, Equatable {
    var enabled: Bool = true
    var apiKey: String = ""
}

struct AppConfig: Codable, Equatable {
    var refreshSeconds: Int = 120
    var kimi = ProviderConfig()
    var zhipu = ProviderConfig()
    var opencode = ProviderConfig()
    var commandcode = ProviderConfig()

    /// Forward compatible: missing fields fall back to defaults
    init() {}
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        refreshSeconds = try c.decodeIfPresent(Int.self, forKey: .refreshSeconds) ?? 120
        kimi = try c.decodeIfPresent(ProviderConfig.self, forKey: .kimi) ?? ProviderConfig()
        zhipu = try c.decodeIfPresent(ProviderConfig.self, forKey: .zhipu) ?? ProviderConfig()
        opencode = try c.decodeIfPresent(ProviderConfig.self, forKey: .opencode) ?? ProviderConfig()
        commandcode = try c.decodeIfPresent(ProviderConfig.self, forKey: .commandcode) ?? ProviderConfig()
    }

    enum CodingKeys: String, CodingKey {
        case refreshSeconds, kimi, zhipu, opencode, commandcode
    }
}

enum ConfigStore {
    static var dir: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("UsageTray", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        return base
    }
    static var file: URL { dir.appendingPathComponent("config.json") }

    static func load() -> AppConfig {
        var config = (try? JSONDecoder().decode(AppConfig.self, from: Data(contentsOf: file))) ?? AppConfig()
        // Providers without a credential can't stay enabled; OpenCode / Command Code can fall back to CLI logins
        if config.kimi.apiKey.isEmpty { config.kimi.enabled = false }
        if config.zhipu.apiKey.isEmpty { config.zhipu.enabled = false }
        if !LocalAuth.hasCredential(id: .opencode, settingsKey: config.opencode.apiKey) { config.opencode.enabled = false }
        if !LocalAuth.hasCredential(id: .commandcode, settingsKey: config.commandcode.apiKey) { config.commandcode.enabled = false }
        return config
    }

    static func save(_ config: AppConfig) {
        guard let data = try? JSONEncoder().encode(config) else { return }
        try? data.write(to: file, options: .atomic)
        try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
    }
}
