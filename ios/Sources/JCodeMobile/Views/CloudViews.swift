import JCodeKit
import SwiftUI

/// First-run screen: sign in to your jcode account with an emailed code.
struct SignInView: View {
    @Environment(CloudModel.self) private var cloud
    @Environment(AppModel.self) private var model
    @State private var email = ""
    @State private var code = ""

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                header

                if let message = cloud.errorMessage {
                    ErrorBanner(message: message) { cloud.errorMessage = nil }
                        .padding(.horizontal, -16)
                }

                switch cloud.phase {
                case .awaitingCode(let sentTo, _):
                    codeStep(sentTo: sentTo)
                default:
                    emailStep
                }

                demoLink

                Text("Turn on sync on your computer with `jcode mobile enable`. Sessions running locally and on jcode cloud show up here. Requires a jcode subscription.")
                    .font(.footnote)
                    .foregroundStyle(Theme.textTertiary)
            }
            .padding(16)
            .readableColumn()
        }
        .scrollDismissesKeyboard(.interactively)
        .dynamicTypeSize(.large ... .accessibility3)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                BrandMark(size: 40)
                Text("jcode")
                    .font(.largeTitle.weight(.semibold))
                    .foregroundStyle(Theme.textPrimary)
            }
            Text("Sign in to see your sessions")
                .font(.subheadline)
                .foregroundStyle(Theme.textSecondary)
        }
        .padding(.top, 28)
        .padding(.bottom, 4)
    }

    private var emailStep: some View {
        VStack(alignment: .leading, spacing: 16) {
            Card {
                field("Email", text: $email, placeholder: "you@example.com")
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .keyboardType(.emailAddress)
                    .textContentType(.emailAddress)
                    .submitLabel(.go)
                    .onSubmit(sendCode)
            }
            primaryButton(cloud.isLoading ? "Sending…" : "Email me a code", enabled: email.contains("@"), action: sendCode)
        }
    }

    private func codeStep(sentTo: String) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("We sent a 6-digit code to \(sentTo).")
                .font(.subheadline)
                .foregroundStyle(Theme.textSecondary)
            Card {
                field("Code", text: $code, placeholder: "123456")
                    .keyboardType(.numberPad)
                    .textContentType(.oneTimeCode)
                    .onChange(of: code) { _, new in
                        if new.filter(\.isNumber).count == 6 { verify() }
                    }
            }
            primaryButton(cloud.isLoading ? "Signing in…" : "Sign in", enabled: code.filter(\.isNumber).count == 6, action: verify)
            Button("Use a different email") {
                code = ""
                cloud.cancelCode()
            }
            .font(.subheadline)
            .foregroundStyle(Theme.accent)
        }
    }

    private var demoLink: some View {
        Button {
            model.startDemo()
        } label: {
            Label("Try the offline demo", systemImage: "play.circle")
                .font(.subheadline.weight(.medium))
                .foregroundStyle(Theme.accent)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
                .background(Theme.surface)
                .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                        .stroke(Theme.border, lineWidth: 1)
                )
        }
        .buttonStyle(PressableButtonStyle(scale: 0.98))
    }

    private func sendCode() {
        guard email.contains("@"), !cloud.isLoading else { return }
        Task { await cloud.startLogin(email: email) }
    }

    private func verify() {
        guard !cloud.isLoading else { return }
        Task { await cloud.verify(code: code) }
    }

    private func primaryButton(_ title: String, enabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if cloud.isLoading { ProgressView().tint(Theme.onAccent).controlSize(.small) }
                Text(title).font(.headline)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                    .fill(enabled ? Theme.accent : Theme.surfaceElevated)
            )
            .foregroundStyle(enabled ? Theme.onAccent : Theme.textTertiary)
        }
        .buttonStyle(PressableButtonStyle(scale: 0.98))
        .disabled(!enabled || cloud.isLoading)
    }

    private func field(_ label: String, text: Binding<String>, placeholder: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.caption.weight(.medium))
                .foregroundStyle(Theme.textTertiary)
                .textCase(.uppercase)
                .tracking(0.5)
            TextField(placeholder, text: text)
                .font(Theme.mono(16))
                .foregroundStyle(Theme.textPrimary)
                .tint(Theme.accent)
                .padding(12)
                .background(Theme.surfaceElevated)
                .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.small, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.Radius.small, style: .continuous)
                        .stroke(Theme.border, lineWidth: 1)
                )
                .accessibilityLabel(label)
        }
    }
}

/// Signed-in home: synced sessions from local machines and jcode cloud.
struct CloudSessionsView: View {
    @Environment(CloudModel.self) private var cloud
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationStack {
            List {
                if !cloud.hasSubscription {
                    Section {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("Subscription required")
                                .font(.headline)
                                .foregroundStyle(Theme.textPrimary)
                            Text("Mobile sync is included with a jcode subscription. Subscribe at jcode.sh/pricing, then pull to refresh.")
                                .font(.subheadline)
                                .foregroundStyle(Theme.textSecondary)
                            Link("Open jcode.sh/pricing", destination: URL(string: "https://jcode.sh/pricing")!)
                                .foregroundStyle(Theme.accent)
                        }
                        .padding(.vertical, 6)
                    }
                    .listRowBackground(Theme.surface)
                } else if cloud.sessions.isEmpty && !cloud.isLoading {
                    Section {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("No sessions yet")
                                .font(.headline)
                                .foregroundStyle(Theme.textPrimary)
                            Text("On your computer, run `jcode mobile enable`. Sessions appear here within a few seconds and stay up to date while jcode runs.")
                                .font(.subheadline)
                                .foregroundStyle(Theme.textSecondary)
                        }
                        .padding(.vertical, 6)
                    }
                    .listRowBackground(Theme.surface)
                }
                ForEach(cloud.sessions) { session in
                    NavigationLink(value: session) {
                        CloudSessionRow(session: session)
                    }
                    .listRowBackground(Theme.surface)
                }
            }
            .scrollContentBackground(.hidden)
            .background(Theme.background)
            .navigationTitle("Sessions")
            .navigationDestination(for: CloudAPI.SessionSummary.self) { session in
                CloudTranscriptView(summary: session)
            }
            .refreshable { await cloud.refresh() }
            .overlay {
                if cloud.isLoading && cloud.sessions.isEmpty { ProgressView() }
            }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        if let email = cloud.credential?.email {
                            Text(email)
                        }
                        Button("Offline demo", systemImage: "play.circle") { model.startDemo() }
                        Button("Sign out", systemImage: "rectangle.portrait.and.arrow.right", role: .destructive) {
                            cloud.signOut()
                        }
                    } label: {
                        Image(systemName: "person.crop.circle")
                    }
                    .accessibilityLabel("Account")
                }
            }
        }
        .tint(Theme.accent)
        .task {
            await cloud.refresh()
            // Live-ish: poll while the list is on screen.
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 15_000_000_000)
                await cloud.refresh()
            }
        }
    }
}

struct CloudSessionRow: View {
    let session: CloudAPI.SessionSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: session.source == "cloud" ? "cloud" : "laptopcomputer")
                    .font(.caption)
                    .foregroundStyle(Theme.textTertiary)
                    .accessibilityLabel(session.source == "cloud" ? "Cloud" : "Local")
                Text(session.displayTitle)
                    .font(.body.weight(.medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
            }
            if let preview = session.lastMessagePreview, !preview.isEmpty {
                Text(preview)
                    .font(.subheadline)
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(2)
            }
            HStack(spacing: 6) {
                if let host = session.host { Text(host) }
                if let date = session.updatedDate {
                    Text(date, style: .relative) + Text(" ago")
                }
                Text("· \(session.messageCount) msgs")
            }
            .font(.caption)
            .foregroundStyle(Theme.textTertiary)
            .lineLimit(1)
        }
        .padding(.vertical, 4)
    }
}

/// Read-only transcript of a synced session. Refreshes while open so running
/// sessions update live.
struct CloudTranscriptView: View {
    @Environment(CloudModel.self) private var cloud
    let summary: CloudAPI.SessionSummary
    @State private var messages: [CloudAPI.Message] = []
    @State private var error: String?
    @State private var loaded = false

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if let error {
                        Text(error).foregroundStyle(Theme.error).font(.subheadline)
                    }
                    ForEach(messages) { message in
                        CloudMessageView(message: message).id(message.seq)
                    }
                }
                .padding(16)
                .readableColumn()
            }
            .background(Theme.background)
            .overlay { if !loaded { ProgressView() } }
            .onChange(of: messages.last?.seq) { _, last in
                if let last { withAnimation { proxy.scrollTo(last, anchor: .bottom) } }
            }
        }
        .navigationTitle(summary.displayTitle)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            while !Task.isCancelled {
                do {
                    let detail = try await cloud.transcript(for: summary.sessionID)
                    if detail.messages != messages { messages = detail.messages }
                    error = nil
                } catch {
                    self.error = error.localizedDescription
                }
                loaded = true
                try? await Task.sleep(nanoseconds: 8_000_000_000)
            }
        }
    }
}

struct CloudMessageView: View {
    let message: CloudAPI.Message
    @State private var expanded = false

    var body: some View {
        switch message.role {
        case "user":
            MarkdownText(message.text)
                .foregroundStyle(Theme.textPrimary)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.surfaceElevated)
                .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.bubble, style: .continuous))
        case "tool":
            Button {
                expanded.toggle()
            } label: {
                Text(expanded ? message.text : String(message.text.prefix(160)))
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
                    .background(Theme.codeBackground)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.small, style: .continuous))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.Radius.small, style: .continuous)
                            .stroke(Theme.border, lineWidth: 1)
                    )
            }
            .buttonStyle(.plain)
            .accessibilityHint(expanded ? "Collapses tool output" : "Expands tool output")
        case "system":
            Text(message.text)
                .font(.caption)
                .foregroundStyle(Theme.textTertiary)
        default:
            MarkdownText(message.text)
                .foregroundStyle(Theme.textPrimary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
