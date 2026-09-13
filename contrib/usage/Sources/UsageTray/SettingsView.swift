import SwiftUI

struct SettingsView: View {
    @ObservedObject var vm: ViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            providerSection($vm.config.kimi, id: .kimi,
                            placeholder: "sk-kimi-…",
                            note: "Key created in the Kimi Code console (not the open-platform sk-…)")
            providerSection($vm.config.zhipu, id: .zhipu,
                            placeholder: "Zhipu GLM Coding Plan key",
                            note: "API key for bigmodel.cn GLM Coding Plan")
            providerSection($vm.config.opencode, id: .opencode,
                            placeholder: "sk-…",
                            note: "OpenCode Go usage; leave empty to reuse the opencode login (~/.local/share/opencode/auth.json)")
            providerSection($vm.config.commandcode, id: .commandcode,
                            placeholder: "user_…",
                            note: "Command Code usage; leave empty to reuse the cmd login (~/.commandcode/auth.json)")

            Divider()

            HStack {
                Text("Auto refresh interval")
                    .font(.system(size: 13))
                Spacer()
                Picker("", selection: $vm.config.refreshSeconds) {
                    Text("30s").tag(30)
                    Text("1 min").tag(60)
                    Text("2 min").tag(120)
                    Text("5 min").tag(300)
                }
                .labelsHidden()
                .frame(width: 120)
            }
        }
        .padding(20)
        .frame(width: 420, alignment: .leading)
    }

    private func providerSection(_ binding: Binding<ProviderConfig>, id: ProviderID,
                                 placeholder: String, note: String) -> some View {
        let hasKey = LocalAuth.hasCredential(id: id, settingsKey: binding.apiKey.wrappedValue)
        let enabled = Binding<Bool>(
            get: { binding.wrappedValue.enabled && hasKey },
            set: { binding.wrappedValue.enabled = $0 }
        )
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                ProviderAvatar(id: id, size: 22)
                Text(id.displayName)
                    .font(.system(size: 13, weight: .medium))
                Spacer()
                Toggle("", isOn: enabled)
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .labelsHidden()
                    .disabled(!hasKey)
                    .help(hasKey ? "" : "Enter an API key or log in with the CLI to enable")
            }
            HStack(spacing: 8) {
                Text("API Key")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .frame(width: 56, alignment: .leading)
                SecureField(placeholder, text: binding.apiKey)
                    .textFieldStyle(.roundedBorder)
                    .controlSize(.small)
            }
            Text(note)
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .padding(.leading, 64)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 10))
    }
}
