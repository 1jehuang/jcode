import SwiftUI
import JCodeKit

#if canImport(UIKit)
import UIKit
#endif

private extension Color {
    static var jcSurface: Color {
#if canImport(UIKit)
        Color(uiColor: .systemBackground)
#else
        Color.white
#endif
    }

    static var jcSubtleSurface: Color {
#if canImport(UIKit)
        Color(uiColor: .secondarySystemBackground)
#else
        Color.gray.opacity(0.15)
#endif
    }

    static var jcSeparator: Color {
#if canImport(UIKit)
        Color(uiColor: .separator)
#else
        Color.gray.opacity(0.35)
#endif
    }

    static var jcAssistantBubble: Color {
#if canImport(UIKit)
        Color(uiColor: .systemGray5)
#else
        Color.gray.opacity(0.2)
#endif
    }
}

struct RootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        NavigationSplitView {
            ServerPanelView()
        } detail: {
            ChatPanelView()
        }
        .navigationSplitViewStyle(.balanced)
    }
}

private struct ServerPanelView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        List {
            Section("Connect") {
                TextField("Host (e.g. yashmacbook)", text: $model.hostInput)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled(true)
                TextField("Port", text: $model.portInput)
                    .keyboardType(.numberPad)
                HStack {
                    Button("Check Health") {
                        Task { await model.probeServer() }
                    }
                    .buttonStyle(.bordered)

                    Button("Pair") {
                        Task { await model.pairAndSave() }
                    }
                    .buttonStyle(.borderedProminent)
                }
            }

            Section("Pairing") {
                TextField("Pair code", text: $model.pairCodeInput)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled(true)
                TextField("Device name", text: $model.deviceNameInput)
            }

            Section("Saved Servers") {
                if model.savedServers.isEmpty {
                    Text("No paired servers yet")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(model.savedServers, id: \.self) { credential in
                        ServerRow(
                            credential: credential,
                            isSelected: model.selectedServer?.host == credential.host && model.selectedServer?.port == credential.port,
                            onSelect: {
                                model.selectedServer = credential
                                model.hostInput = credential.host
                                model.portInput = String(credential.port)
                            },
                            onDelete: {
                                Task { await model.deleteServer(credential) }
                            }
                        )
                    }
                }
            }

            Section("Session") {
                if model.connectionState == .connected {
                    if !model.activeSessionId.isEmpty {
                        Label(model.activeSessionId, systemImage: "bolt.horizontal.circle")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    if model.sessions.isEmpty {
                        Text("No sessions reported yet")
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(model.sessions, id: \.self) { sessionId in
                            Button {
                                Task { await model.switchToSession(sessionId) }
                            } label: {
                                HStack {
                                    Text(sessionId)
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                    Spacer()
                                    if sessionId == model.activeSessionId {
                                        Image(systemName: "checkmark.circle.fill")
                                            .foregroundStyle(.green)
                                    }
                                }
                            }
                        }
                    }
                } else {
                    Text("Connect to load sessions")
                        .foregroundStyle(.secondary)
                }
            }

            if model.connectionState == .connected && !model.availableModels.isEmpty {
                Section("Model") {
                    HStack {
                        Text(model.modelName.isEmpty ? "Unknown" : model.modelName)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer()
                        Menu {
                            ForEach(model.availableModels, id: \.self) { m in
                                Button {
                                    Task { await model.changeModel(m) }
                                } label: {
                                    HStack {
                                        Text(m)
                                        if m == model.modelName {
                                            Image(systemName: "checkmark")
                                        }
                                    }
                                }
                            }
                        } label: {
                            Image(systemName: "chevron.up.chevron.down")
                                .font(.caption)
                        }
                    }
                }
            }

            Section("Status") {
                if let status = model.statusMessage {
                    Text(status)
                        .foregroundStyle(.green)
                }
                if let error = model.errorMessage {
                    Text(error)
                        .foregroundStyle(.red)
                }

                HStack {
                    Button("Connect") {
                        Task { await model.connectSelected() }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.connectionState == .connecting)

                    Button("Disconnect") {
                        Task { await model.disconnect() }
                    }
                    .buttonStyle(.bordered)
                    .disabled(model.connectionState == .disconnected)
                }
            }
        }
        .navigationTitle("Servers")
        .task {
            await model.loadSavedServers()
        }
    }
}

private struct ServerRow: View {
    let credential: ServerCredential
    let isSelected: Bool
    let onSelect: () -> Void
    let onDelete: () -> Void

    var body: some View {
        HStack {
            Button(action: onSelect) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(credential.serverName)
                            .font(.headline)
                        Text(credential.serverVersion)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Text("\(credential.host):\(credential.port)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)

            if isSelected {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.green)
            }

            Button(role: .destructive, action: onDelete) {
                Image(systemName: "trash")
            }
            .buttonStyle(.borderless)
        }
    }
}

private struct ChatPanelView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(spacing: 0) {
            ChatHeaderView()

            Divider()

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 12) {
                        ForEach(model.messages) { message in
                            ChatBubble(message: message)
                                .id(message.id)
                        }
                    }
                    .padding(16)
                }
                .background(Color.jcSubtleSurface)
                .onChange(of: model.messages.count) {
                    scrollToBottom(proxy)
                }
                .onChange(of: model.messages.last?.text) {
                    scrollToBottom(proxy)
                }
            }

            Divider()

            MessageComposer()
                .padding(12)
                .background(Color.jcSurface)
        }
        .navigationTitle("Chat")
    }

    private func scrollToBottom(_ proxy: ScrollViewProxy) {
        if let id = model.messages.last?.id {
            withAnimation(.easeOut(duration: 0.15)) {
                proxy.scrollTo(id, anchor: .bottom)
            }
        }
    }
}

private struct ChatHeaderView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.serverName.isEmpty ? "jcode" : model.serverName)
                    .font(.headline)
                HStack(spacing: 8) {
                    Text(model.serverVersion)
                    Text(model.modelName)
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }

            Spacer()

            if model.isProcessing {
                ProgressView()
                    .controlSize(.small)
            }

            Label(connectionText, systemImage: connectionImage)
                .font(.caption)
                .foregroundStyle(connectionColor)

            Button {
                Task { await model.refreshHistory() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.bordered)
            .disabled(model.connectionState != .connected)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Color.jcSurface)
    }

    private var connectionText: String {
        switch model.connectionState {
        case .connected:
            return "Connected"
        case .connecting:
            return "Connecting"
        case .disconnected:
            return "Offline"
        }
    }

    private var connectionImage: String {
        switch model.connectionState {
        case .connected:
            return "checkmark.circle.fill"
        case .connecting:
            return "hourglass"
        case .disconnected:
            return "wifi.slash"
        }
    }

    private var connectionColor: Color {
        switch model.connectionState {
        case .connected:
            return .green
        case .connecting:
            return .orange
        case .disconnected:
            return .red
        }
    }
}

private struct ChatBubble: View {
    let message: AppModel.ChatEntry

    var body: some View {
        VStack(alignment: alignment, spacing: 6) {
            Text(roleLabel)
                .font(.caption2)
                .foregroundStyle(.secondary)

            if message.role == .assistant && !message.text.isEmpty {
                MarkdownText(text: message.text)
                    .padding(10)
                    .frame(maxWidth: 520, alignment: .leading)
                    .background(bubbleColor)
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            } else {
                Text(message.text.isEmpty ? "..." : message.text)
                    .textSelection(.enabled)
                    .padding(10)
                    .frame(maxWidth: 520, alignment: .leading)
                    .background(bubbleColor)
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            }

            if !message.toolCalls.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(message.toolCalls, id: \.id) { tool in
                        ToolCard(tool: tool)
                    }
                }
                .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity, alignment: message.role == .user ? .trailing : .leading)
    }

    private var alignment: HorizontalAlignment {
        message.role == .user ? .trailing : .leading
    }

    private var roleLabel: String {
        switch message.role {
        case .assistant:
            return "Assistant"
        case .system:
            return "System"
        case .user:
            return "You"
        }
    }

    private var bubbleColor: Color {
        switch message.role {
        case .assistant:
            return .jcAssistantBubble
        case .system:
            return Color.orange.opacity(0.2)
        case .user:
            return Color.blue.opacity(0.2)
        }
    }
}

private struct ToolCard: View {
    let tool: ToolCallInfo
    @State private var isExpanded: Bool = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.easeInOut(duration: 0.2)) {
                    isExpanded.toggle()
                }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                    Text(tool.name)
                        .font(.subheadline.weight(.semibold))
                    Spacer()
                    Text(stateLabel)
                        .font(.caption2)
                        .foregroundStyle(stateColor)
                }
            }
            .buttonStyle(.plain)

            if isExpanded {
                if !tool.input.isEmpty {
                    Text(tool.input)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(12)
                        .textSelection(.enabled)
                }

                if let output = tool.output, !output.isEmpty {
                    Text(output)
                        .font(.caption.monospaced())
                        .lineLimit(12)
                        .textSelection(.enabled)
                }

                if let error = tool.error, !error.isEmpty {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
            }
        }
        .padding(10)
        .background(Color.jcSurface)
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(Color.jcSeparator, lineWidth: 1)
        )
    }

    private var stateLabel: String {
        switch tool.state {
        case .streaming:
            return "streaming"
        case .executing:
            return "running"
        case .done:
            return "done"
        case .failed:
            return "failed"
        }
    }

    private var stateColor: Color {
        switch tool.state {
        case .streaming:
            return .orange
        case .executing:
            return .blue
        case .done:
            return .green
        case .failed:
            return .red
        }
    }
}

private struct MessageComposer: View {
    @EnvironmentObject private var model: AppModel
    @State private var showInterruptSheet = false
    @State private var interruptMessage = ""

    var body: some View {
        VStack(spacing: 8) {
            if model.isProcessing {
                HStack(spacing: 12) {
                    Button {
                        Task { await model.cancelGeneration() }
                    } label: {
                        Label("Stop", systemImage: "stop.fill")
                            .font(.caption)
                    }
                    .buttonStyle(.bordered)
                    .tint(.red)

                    Button {
                        showInterruptSheet = true
                    } label: {
                        Label("Interrupt", systemImage: "bolt.fill")
                            .font(.caption)
                    }
                    .buttonStyle(.bordered)
                    .tint(.orange)

                    Spacer()
                }
            }

            HStack(spacing: 10) {
                TextField("Message jcode...", text: $model.draftMessage, axis: .vertical)
                    .lineLimit(1 ... 6)
                    .textFieldStyle(.roundedBorder)

                Button {
                    Task { await model.sendDraft() }
                } label: {
                    Image(systemName: "paperplane.fill")
                        .padding(8)
                }
                .buttonStyle(.borderedProminent)
                .disabled(model.connectionState != .connected)
            }
        }
        .sheet(isPresented: $showInterruptSheet) {
            NavigationStack {
                Form {
                    TextField("Interrupt message", text: $interruptMessage, axis: .vertical)
                        .lineLimit(2...6)
                    Toggle("Urgent", isOn: .constant(false))
                }
                .navigationTitle("Interrupt Agent")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { showInterruptSheet = false }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Send") {
                            let msg = interruptMessage.trimmingCharacters(in: .whitespacesAndNewlines)
                            guard !msg.isEmpty else { return }
                            Task { await model.interruptAgent(msg) }
                            interruptMessage = ""
                            showInterruptSheet = false
                        }
                        .disabled(interruptMessage.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
            }
            .presentationDetents([.medium])
        }
    }
}
