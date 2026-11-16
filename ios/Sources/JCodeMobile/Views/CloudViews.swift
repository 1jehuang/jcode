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
    @State private var path: [CloudAPI.SessionSummary] = []
    @State private var composing = false

    var body: some View {
        NavigationStack(path: $path) {
            List {
                if !cloud.pendingCreates.isEmpty {
                    Section("Starting on jcode cloud") {
                        ForEach(cloud.pendingCreates) { cmd in
                            PendingCommandRow(command: cmd) { cloud.dismissPending(cmd.id) }
                                .listRowBackground(Theme.surface)
                        }
                    }
                }
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
                    Button {
                        composing = true
                    } label: {
                        Image(systemName: "square.and.pencil")
                    }
                    .disabled(!cloud.hasSubscription)
                    .accessibilityLabel("New cloud session")
                }
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
        .sheet(isPresented: $composing) {
            NewSessionSheet()
        }
        .onChange(of: cloud.openSessionID) { _, id in
            guard let id, let session = cloud.sessions.first(where: { $0.sessionID == id }) else { return }
            cloud.openSessionID = nil
            path = [session]
        }
        .task {
            await cloud.refresh()
            // Live-ish: poll while the list is on screen; faster while a new
            // session is starting.
            var tick = 0
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 3_000_000_000)
                tick += 1
                if !cloud.pendingCreates.isEmpty {
                    await cloud.pollPendingCreates()
                } else if tick % 5 == 0 {
                    await cloud.refresh()
                }
            }
        }
    }
}

struct PendingCommandRow: View {
    let command: CloudAPI.Command
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            if command.status == "failed" {
                Image(systemName: "exclamationmark.triangle").foregroundStyle(Theme.error)
            } else {
                ProgressView().controlSize(.small)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(command.text)
                    .font(.body.weight(.medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(2)
                Text(statusText)
                    .font(.caption)
                    .foregroundStyle(command.status == "failed" ? Theme.error : Theme.textTertiary)
                    .lineLimit(3)
            }
            Spacer(minLength: 0)
            if command.status == "failed" {
                Button("Dismiss", action: dismiss).font(.caption).buttonStyle(.borderless)
            }
        }
        .padding(.vertical, 4)
    }

    private var statusText: String {
        switch command.status {
        case "queued": return "Waiting for your cloud machine…"
        case "running": return "Working…"
        case "done": return "Syncing…"
        default: return command.error ?? "Failed"
        }
    }
}

/// Compose the first message of a new Jcode Cloud session.
struct NewSessionSheet: View {
    @Environment(CloudModel.self) private var cloud
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var sending = false
    @State private var error: String?
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 12) {
                Text("Runs on your jcode cloud machine. You can close the app; it keeps working.")
                    .font(.footnote)
                    .foregroundStyle(Theme.textTertiary)
                TextField("What should jcode do?", text: $text, axis: .vertical)
                    .lineLimit(4...12)
                    .font(.body)
                    .foregroundStyle(Theme.textPrimary)
                    .tint(Theme.accent)
                    .padding(12)
                    .background(Theme.surfaceElevated)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                    .focused($focused)
                    .accessibilityLabel("First message")
                if let error {
                    Text(error).font(.footnote).foregroundStyle(Theme.error)
                }
                Spacer()
            }
            .padding(16)
            .background(Theme.background)
            .navigationTitle("New session")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button {
                        start()
                    } label: {
                        if sending { ProgressView() } else { Text("Start").bold() }
                    }
                    .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || sending)
                }
            }
            .onAppear { focused = true }
        }
        .tint(Theme.accent)
        .presentationDetents([.medium, .large])
    }

    private func start() {
        sending = true
        error = nil
        Task {
            defer { sending = false }
            do {
                let queued = try await cloud.createSession(text: text.trimmingCharacters(in: .whitespacesAndNewlines))
                if let note = CloudModel.hostMessage(queued.host?.state), queued.host?.state != "starting" {
                    error = note
                    try? await Task.sleep(nanoseconds: 2_500_000_000)
                }
                dismiss()
            } catch {
                self.error = error.localizedDescription
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
    @State private var draft = ""
    @State private var pending: [CloudAPI.Command] = []
    @State private var sending = false
    @State private var hostNote: String?

    private var isCloud: Bool { summary.source == "cloud" }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if let error {
                        Text(error).foregroundStyle(Theme.error).font(.subheadline)
                    }
                    ForEach(messages.filter { !$0.text.hasPrefix("<system-reminder>") }) { message in
                        CloudMessageView(message: message).id(message.seq)
                    }
                    ForEach(pending) { cmd in
                        VStack(alignment: .leading, spacing: 6) {
                            CloudMessageView(message: .init(seq: -1, role: "user", text: cmd.text, createdAt: nil))
                                .opacity(0.7)
                            HStack(spacing: 6) {
                                if cmd.status == "failed" {
                                    Image(systemName: "exclamationmark.triangle")
                                    Text(cmd.error ?? "Failed")
                                } else {
                                    ProgressView().controlSize(.mini)
                                    Text(cmd.status == "running" ? "jcode is working…" : (hostNote ?? "Queued for your cloud machine…"))
                                }
                            }
                            .font(.caption)
                            .foregroundStyle(cmd.status == "failed" ? Theme.error : Theme.textTertiary)
                        }
                        .id(cmd.id)
                    }
                }
                .padding(16)
                .readableColumn()
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Theme.background)
            .overlay { if !loaded { ProgressView() } }
            .safeAreaInset(edge: .bottom) {
                if isCloud { composer } else { localNote }
            }
            .onChange(of: messages.last?.seq) { _, last in
                if let last { withAnimation { proxy.scrollTo(last, anchor: .bottom) } }
            }
        }
        .navigationTitle(summary.displayTitle)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            if isCloud, let cmds = try? await cloud.commands(sessionID: summary.sessionID) {
                pending = cmds.filter { $0.kind == "message" && $0.isPending }.reversed()
            }
            while !Task.isCancelled {
                await reload()
                loaded = true
                try? await Task.sleep(nanoseconds: pending.contains(where: \.isPending) ? 3_000_000_000 : 8_000_000_000)
            }
        }
    }

    private func reload() async {
        do {
            let detail = try await cloud.transcript(for: summary.sessionID)
            if detail.messages != messages { messages = detail.messages }
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        for (i, cmd) in pending.enumerated() where cmd.isPending {
            if let fresh = try? await cloud.command(id: cmd.id) { pending[i] = fresh }
        }
        // Once done and synced, the real transcript shows the message.
        pending.removeAll { $0.status == "done" }
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Message jcode cloud", text: $draft, axis: .vertical)
                .lineLimit(1...6)
                .foregroundStyle(Theme.textPrimary)
                .tint(Theme.accent)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .background(Theme.surfaceElevated)
                .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
                .accessibilityLabel("Message")
            Button(action: send) {
                Image(systemName: sending ? "ellipsis.circle.fill" : "arrow.up.circle.fill")
                    .font(.system(size: 32))
                    .foregroundStyle(canSend ? Theme.accent : Theme.textTertiary)
            }
            .disabled(!canSend)
            .accessibilityLabel("Send")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.bar)
    }

    private var localNote: some View {
        Label("Running on \(summary.host ?? "your computer"). Reply from there; only cloud sessions take messages from the phone.", systemImage: "laptopcomputer")
            .font(.caption)
            .foregroundStyle(Theme.textTertiary)
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.bar)
    }

    private var canSend: Bool {
        !sending && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        sending = true
        Task {
            defer { sending = false }
            do {
                let queued = try await cloud.sendMessage(sessionID: summary.sessionID, text: text)
                draft = ""
                hostNote = CloudModel.hostMessage(queued.host?.state)
                pending.append(queued.command)
            } catch {
                self.error = error.localizedDescription
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
