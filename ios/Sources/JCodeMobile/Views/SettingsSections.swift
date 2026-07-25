import JCodeKit
import SwiftUI

/// Servers and info sections, split out to keep view files small.
struct SettingsServersSection: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @Binding var showPairNew: Bool

    var body: some View {
        Section("Servers") {
            ForEach(model.servers) { server in
                let isActive = server.id == model.activeServer?.id
                Button {
                    model.connect(to: server)
                    dismiss()
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(server.serverName)
                                .font(.body)
                                .foregroundStyle(Theme.textPrimary)
                            Text("\(server.host):\(String(server.port))")
                                .font(Theme.mono(11))
                                .foregroundStyle(Theme.textTertiary)
                        }
                        Spacer()
                        if isActive {
                            Circle()
                                .fill(Theme.mint)
                                .frame(width: 8, height: 8)
                                .accessibilityHidden(true)
                        }
                    }
                }
                .listRowBackground(Theme.surface)
                .accessibilityLabel(server.serverName)
                .accessibilityValue(isActive ? "Connected" : "")
                .accessibilityHint("Connects to this server")
                .accessibilityAddTraits(isActive ? [.isSelected] : [])
                .swipeActions {
                    Button(role: .destructive) {
                        model.removeServer(server)
                    } label: {
                        Label("Remove", systemImage: "trash")
                    }
                }
            }
            Button {
                showPairNew = true
            } label: {
                Label("Pair new server", systemImage: "plus")
                    .foregroundStyle(Theme.mint)
            }
            .listRowBackground(Theme.surface)
            .accessibilityHint("Opens pairing to add a server")
        }
    }
}

struct SettingsInfoSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section("Info") {
            row("Server version", model.session.serverVersion ?? "unknown")
            row("Provider", model.session.providerName ?? "unknown")
            row(
                "Tokens",
                "\(model.session.tokenInput) in / \(model.session.tokenOutput) out"
            )
            if let detail = model.session.statusDetail {
                row("Status", detail)
            }
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label)
                .font(.callout)
                .foregroundStyle(Theme.textSecondary)
            Spacer()
            Text(value)
                .font(Theme.mono(12))
                .foregroundStyle(Theme.textTertiary)
                .lineLimit(1)
        }
        .listRowBackground(Theme.surface)
        .accessibilityElement(children: .combine)
    }
}
