import SwiftUI
import JCodeKit

#if canImport(UIKit)
import UIKit
#endif

// MARK: - Root

struct RootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Group {
            if model.savedServers.isEmpty && model.connectionState == .disconnected {
                OnboardingView()
            } else {
                MainView()
            }
        }
        .preferredColorScheme(.dark)
        .task {
            await model.loadSavedServers()
            if model.selectedServer != nil && model.connectionState == .disconnected {
                await model.connectSelected()
            }
        }
    }
}

// MARK: - Onboarding

struct OnboardingView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showQRScanner = false
    @State private var showManualEntry = false

    var body: some View {
        ZStack {
            JC.Colors.background.ignoresSafeArea()

            ScrollView {
                VStack(spacing: JC.Spacing.xxl) {
                    Spacer().frame(height: 60)

                    VStack(spacing: JC.Spacing.lg) {
                        TerminalPrompt()
                            .frame(width: 80, height: 80)

                        Text("jcode")
                            .font(JC.Fonts.largeTitle)
                            .foregroundStyle(JC.Colors.textPrimary)

                        Text("Your AI coding assistant,\nright in your pocket.")
                            .font(JC.Fonts.body)
                            .foregroundStyle(JC.Colors.textSecondary)
                            .multilineTextAlignment(.center)
                    }

                    Spacer().frame(height: 20)

                    VStack(spacing: JC.Spacing.lg) {
                        Button {
                            showQRScanner = true
                        } label: {
                            HStack(spacing: JC.Spacing.md) {
                                Image(systemName: "qrcode.viewfinder")
                                    .font(.system(size: 24))
                                Text("Scan QR Code")
                                    .font(JC.Fonts.headline)
                            }
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, JC.Spacing.xl)
                        }
                        .buttonStyle(AccentButton())

                        Text("Run **jcode pair** on your computer\nto generate a QR code.")
                            .font(JC.Fonts.callout)
                            .foregroundStyle(JC.Colors.textSecondary)
                            .multilineTextAlignment(.center)
                    }
                    .padding(.horizontal, JC.Spacing.xxl)

                    if let error = model.errorMessage {
                        HStack(spacing: JC.Spacing.sm) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .font(.caption)
                            Text(error)
                                .font(JC.Fonts.caption)
                        }
                        .foregroundStyle(JC.Colors.destructive)
                        .padding(.horizontal, JC.Spacing.xxl)
                    }

                    if let status = model.statusMessage {
                        HStack(spacing: JC.Spacing.sm) {
                            Image(systemName: "checkmark.circle.fill")
                                .font(.caption)
                            Text(status)
                                .font(JC.Fonts.caption)
                        }
                        .foregroundStyle(JC.Colors.accent)
                        .padding(.horizontal, JC.Spacing.xxl)
                    }

                    Spacer().frame(height: 20)

                    VStack(spacing: JC.Spacing.md) {
                        Button {
                            withAnimation(JC.Animation.smooth) {
                                showManualEntry.toggle()
                            }
                        } label: {
                            HStack(spacing: JC.Spacing.xs) {
                                Text("Connect manually")
                                    .font(JC.Fonts.caption)
                                Image(systemName: showManualEntry ? "chevron.up" : "chevron.down")
                                    .font(.system(size: 10, weight: .semibold))
                            }
                            .foregroundStyle(JC.Colors.textTertiary)
                        }
                        .buttonStyle(.plain)

                        if showManualEntry {
                            ManualEntryFields()
                                .transition(.opacity.combined(with: .move(edge: .top)))
                        }
                    }
                    .padding(.horizontal, JC.Spacing.xxl)
                    .padding(.bottom, JC.Spacing.xxxl)
                }
            }
        }
        .sheet(isPresented: $showQRScanner) {
            QRScannerView(isPresented: $showQRScanner) { host, port, code in
                model.hostInput = host
                model.portInput = String(port)
                model.pairCodeInput = code
                Task { await model.pairAndSave() }
            }
        }
    }
}

struct ManualEntryFields: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(spacing: JC.Spacing.md) {
            JCTextField(
                label: "Host",
                placeholder: "e.g. my-macbook",
                text: $model.hostInput,
                icon: "server.rack"
            )

            JCTextField(
                label: "Port",
                placeholder: "7643",
                text: $model.portInput,
                icon: "number",
                keyboardType: .numberPad
            )

            JCTextField(
                label: "Pair Code",
                placeholder: "6-digit code from jcode pair",
                text: $model.pairCodeInput,
                icon: "key.fill"
            )

            JCTextField(
                label: "Device Name",
                placeholder: "My iPhone",
                text: $model.deviceNameInput,
                icon: "iphone"
            )

            Button {
                Task { await model.pairAndSave() }
            } label: {
                HStack(spacing: JC.Spacing.sm) {
                    Image(systemName: "link")
                    Text("Pair & Connect")
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(AccentButton())
        }
    }
}

// MARK: - Terminal Prompt Animation

struct TerminalPrompt: View {
    @State private var cursorVisible = true

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: JC.Radius.lg, style: .continuous)
                .fill(JC.Colors.surface)
                .overlay(
                    RoundedRectangle(cornerRadius: JC.Radius.lg, style: .continuous)
                        .stroke(JC.Colors.border, lineWidth: 1)
                )

            HStack(spacing: 2) {
                Text("j")
                    .font(.system(size: 32, weight: .bold, design: .monospaced))
                    .foregroundStyle(JC.Colors.accent)
                Text(">")
                    .font(.system(size: 32, weight: .bold, design: .monospaced))
                    .foregroundStyle(JC.Colors.textSecondary)
                Rectangle()
                    .fill(JC.Colors.accent)
                    .frame(width: 3, height: 28)
                    .opacity(cursorVisible ? 1 : 0)
            }
        }
        .onAppear {
            withAnimation(.easeInOut(duration: 0.6).repeatForever(autoreverses: true)) {
                cursorVisible.toggle()
            }
        }
    }
}

// MARK: - Custom Text Field

struct JCTextField: View {
    let label: String
    let placeholder: String
    @Binding var text: String
    var icon: String = ""
    var keyboardType: UIKeyboardType = .default

    @FocusState private var isFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: JC.Spacing.xs) {
            Text(label)
                .font(JC.Fonts.caption)
                .foregroundStyle(JC.Colors.textTertiary)

            HStack(spacing: JC.Spacing.sm) {
                if !icon.isEmpty {
                    Image(systemName: icon)
                        .font(.system(size: 14))
                        .foregroundStyle(isFocused ? JC.Colors.accent : JC.Colors.textTertiary)
                        .frame(width: 20)
                }

                TextField(placeholder, text: $text)
                    .font(JC.Fonts.body)
                    .foregroundStyle(JC.Colors.textPrimary)
                    .focused($isFocused)
                    .keyboardType(keyboardType)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled(true)
            }
            .padding(.horizontal, JC.Spacing.md)
            .padding(.vertical, JC.Spacing.md)
            .background(JC.Colors.surfaceElevated)
            .clipShape(RoundedRectangle(cornerRadius: JC.Radius.sm, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: JC.Radius.sm, style: .continuous)
                    .stroke(isFocused ? JC.Colors.borderFocused : JC.Colors.border, lineWidth: 1)
            )
            .animation(JC.Animation.quick, value: isFocused)
        }
    }
}

// MARK: - Main App (Connected State)

struct MainView: View {
    @EnvironmentObject private var model: AppModel
    @StateObject private var speech = SpeechRecognizer()
    @State private var showSettings = false
    @State private var floatingAttachments: [ImageAttachment] = []
    @State private var showFloatingCamera = false

    var body: some View {
        NavigationStack {
            ZStack {
                JC.Colors.background.ignoresSafeArea()

                VStack(spacing: 0) {
                    StreamView()
                    ChatInputBar(externalAttachments: $floatingAttachments)
                }

                FloatingActions(
                    speech: speech,
                    showCamera: $showFloatingCamera,
                    draftMessage: $model.draftMessage,
                    cameraEnabled: !model.isProcessing
                )
            }
            .sheet(isPresented: $showSettings) {
                SettingsSheet()
            }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showSettings = true
                    } label: {
                        Image(systemName: "gearshape.fill")
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundStyle(JC.Colors.accent)
                    }
                    .accessibilityLabel("Settings")
                    .accessibilityHint("Open connection, pairing, and session settings")
                }
            }
            .fullScreenCover(isPresented: $showFloatingCamera) {
                CameraPickerView { image in
                    if let attachment = ImageAttachment.from(image: image) {
                        floatingAttachments.append(attachment)
                    }
                }
            }
        }
    }
}

// MARK: - Floating Action Buttons (middle-right)

struct FloatingActions: View {
    @ObservedObject var speech: SpeechRecognizer
    @Binding var showCamera: Bool
    @Binding var draftMessage: String
    var cameraEnabled: Bool = true

    @State private var prefixBeforeDictation = ""

    var body: some View {
        VStack(spacing: JC.Spacing.md) {
            if UIImagePickerController.isSourceTypeAvailable(.camera) {
                FloatingActionButton(
                    icon: "camera.fill",
                    color: JC.Colors.cyan,
                    isActive: false,
                    isEnabled: cameraEnabled
                ) {
                    showCamera = true
                }
            }

            FloatingActionButton(
                icon: speech.isRecording ? "waveform" : "mic.fill",
                color: JC.Colors.pink,
                isActive: speech.isRecording,
                isEnabled: true
            ) {
                if !speech.isRecording {
                    prefixBeforeDictation = draftMessage
                }
                speech.toggleRecording()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .trailing)
        .padding(.trailing, JC.Spacing.md)
        .onChange(of: speech.transcript) { _, newValue in
            let dictated = newValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !dictated.isEmpty else { return }
            if prefixBeforeDictation.isEmpty {
                draftMessage = dictated
            } else {
                draftMessage = prefixBeforeDictation + " " + dictated
            }
        }
    }
}

struct FloatingActionButton: View {
    let icon: String
    let color: Color
    let isActive: Bool
    let isEnabled: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(isActive ? .white : color.opacity(isEnabled ? 1 : 0.45))
                .frame(width: 48, height: 48)
                .background(
                    Circle()
                        .fill(isActive ? color : color.opacity(isEnabled ? 0.15 : 0.08))
                )
                .overlay(
                    Circle()
                        .stroke(color.opacity(isActive ? 0 : (isEnabled ? 0.3 : 0.15)), lineWidth: 1)
                )
                .shadow(color: isActive ? color.opacity(0.5) : .clear, radius: 8)
                .scaleEffect(isActive ? 1.08 : 1.0)
                .animation(JC.Animation.quick, value: isActive)
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled)
    }
}

// MARK: - Stream View (flat text, no bubbles)

struct StreamView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    if model.messages.isEmpty && model.connectionState == .connected {
                        emptyState
                    }

                    ForEach(model.messages) { message in
                        StreamEntry(message: message)
                            .id(message.id)
                    }
                }
                .padding(.horizontal, JC.Spacing.lg)
                .padding(.vertical, JC.Spacing.md)
            }
            .background(JC.Colors.background)
            .onChange(of: model.messages.count) {
                scrollToBottom(proxy)
            }
            .onChange(of: model.messages.last?.text) {
                scrollToBottom(proxy)
            }
        }
    }

    private var emptyState: some View {
        VStack(spacing: JC.Spacing.md) {
            Spacer().frame(height: 120)
            Text("jcode")
                .font(.system(size: 28, weight: .bold))
                .foregroundStyle(JC.Colors.accent)
            Text("Send a message to start.")
                .font(JC.Fonts.callout)
                .foregroundStyle(JC.Colors.textTertiary)
        }
        .frame(maxWidth: .infinity)
    }

    private func scrollToBottom(_ proxy: ScrollViewProxy) {
        if let id = model.messages.last?.id {
            withAnimation(JC.Animation.quick) {
                proxy.scrollTo(id, anchor: .bottom)
            }
        }
    }
}

// MARK: - Stream Entry (single message)

struct StreamEntry: View {
    let message: AppModel.ChatEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if !message.images.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: JC.Spacing.sm) {
                        ForEach(Array(message.images.enumerated()), id: \.offset) { _, pair in
                            if let data = Data(base64Encoded: pair.1),
                               let uiImage = UIImage(data: data) {
                                Image(uiImage: uiImage)
                                    .resizable()
                                    .aspectRatio(contentMode: .fit)
                                    .frame(maxWidth: 200, maxHeight: 200)
                                    .clipShape(RoundedRectangle(cornerRadius: JC.Radius.sm))
                            }
                        }
                    }
                }
            }

            if !message.text.isEmpty {
                switch message.role {
                case .user:
                    Text(message.text)
                        .font(JC.Fonts.stream)
                        .foregroundStyle(JC.Colors.userText)
                        .textSelection(.enabled)
                case .assistant:
                    MarkdownText(text: message.text)
                case .system:
                    Text(message.text)
                        .font(.system(size: 12))
                        .foregroundStyle(JC.Colors.systemText)
                }
            }

            if !message.toolCalls.isEmpty {
                ToolChainView(tools: message.toolCalls)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 2)
    }
}

// MARK: - Tool Chain (collapsible)

struct ToolChainView: View {
    let tools: [ToolCallInfo]
    @State private var isExpanded = false

    private var allDone: Bool {
        tools.allSatisfy { $0.state == .done || $0.state == .failed }
    }

    private var hasLive: Bool {
        tools.contains { $0.state == .streaming || $0.state == .executing }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if allDone {
                Button {
                    withAnimation(JC.Animation.quick) { isExpanded.toggle() }
                } label: {
                    HStack(spacing: JC.Spacing.xs) {
                        Circle()
                            .fill(tools.contains(where: { $0.state == .failed }) ? JC.Colors.red : JC.Colors.green)
                            .frame(width: 5, height: 5)
                            .shadow(color: (tools.contains(where: { $0.state == .failed }) ? JC.Colors.red : JC.Colors.green).opacity(0.6), radius: 4)

                        Text(tools.map(\.name).joined(separator: ", "))
                            .font(JC.Fonts.streamSmall)
                            .foregroundStyle(JC.Colors.toolText)
                            .lineLimit(1)

                        Text("\(tools.count) tool\(tools.count == 1 ? "" : "s")")
                            .font(JC.Fonts.streamSmall)
                            .foregroundStyle(JC.Colors.textTertiary)
                    }
                }
                .buttonStyle(.plain)

                if isExpanded {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(tools, id: \.id) { tool in
                            ToolDetailLine(tool: tool)
                        }
                    }
                    .padding(.leading, 14)
                    .overlay(
                        Rectangle()
                            .fill(JC.Colors.border)
                            .frame(width: 1),
                        alignment: .leading
                    )
                    .padding(.top, 2)
                }
            } else {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(tools, id: \.id) { tool in
                        ToolDetailLine(tool: tool)
                    }
                }
                .padding(.leading, 10)
                .overlay(
                    Rectangle()
                        .fill(JC.Colors.amber.opacity(0.2))
                        .frame(width: 1),
                    alignment: .leading
                )
            }
        }
    }
}

// MARK: - Tool Detail Line

struct ToolDetailLine: View {
    let tool: ToolCallInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: JC.Spacing.xs) {
                Circle()
                    .fill(dotColor)
                    .frame(width: 5, height: 5)
                    .shadow(color: dotColor.opacity(0.6), radius: 3)

                Text(tool.name)
                    .font(JC.Fonts.streamSmall)
                    .foregroundStyle(JC.Colors.toolText)

                if !tool.input.isEmpty {
                    Text(tool.input)
                        .font(JC.Fonts.streamSmall)
                        .foregroundStyle(JC.Colors.textTertiary)
                        .lineLimit(1)
                }

                if tool.state == .executing || tool.state == .streaming {
                    ProgressView()
                        .controlSize(.mini)
                        .tint(JC.Colors.amber)
                }
            }

            if let output = tool.output, !output.isEmpty {
                Text(output)
                    .font(JC.Fonts.streamSmall)
                    .foregroundStyle(JC.Colors.textTertiary)
                    .lineLimit(3)
                    .padding(.leading, 14)
            }

            if let error = tool.error, !error.isEmpty {
                Text(error)
                    .font(JC.Fonts.streamSmall)
                    .foregroundStyle(JC.Colors.red)
                    .lineLimit(3)
                    .padding(.leading, 14)
            }
        }
    }

    private var dotColor: Color {
        switch tool.state {
        case .streaming, .executing: JC.Colors.amber
        case .done: JC.Colors.green
        case .failed: JC.Colors.red
        }
    }
}

// MARK: - Chat Input Bar

struct ChatInputBar: View {
    @EnvironmentObject private var model: AppModel
    @Binding var externalAttachments: [ImageAttachment]
    @State private var attachments: [ImageAttachment] = []
    @FocusState private var inputFocused: Bool

    private var allAttachments: [ImageAttachment] {
        attachments + externalAttachments
    }

    var body: some View {
        VStack(spacing: JC.Spacing.sm) {
            if model.isProcessing {
                HStack(spacing: JC.Spacing.sm) {
                    Button {
                        Task { await model.cancelGeneration() }
                    } label: {
                        HStack(spacing: JC.Spacing.xs) {
                            Image(systemName: "stop.fill")
                                .font(.system(size: 10))
                            Text("Stop")
                                .font(JC.Fonts.caption)
                        }
                        .foregroundStyle(JC.Colors.destructive)
                        .padding(.horizontal, JC.Spacing.md)
                        .padding(.vertical, JC.Spacing.xs + 2)
                        .background(JC.Colors.destructive.opacity(0.12))
                        .clipShape(Capsule())
                    }
                    .buttonStyle(.plain)

                    Spacer()
                }
                .padding(.horizontal, JC.Spacing.xs)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            if !allAttachments.isEmpty {
                AttachmentStrip(attachments: Binding(
                    get: { allAttachments },
                    set: { newValue in
                        attachments = []
                        externalAttachments = []
                        for item in newValue {
                            attachments.append(item)
                        }
                    }
                ))
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            HStack(alignment: .bottom, spacing: JC.Spacing.sm) {
                PhotoPickerButton(attachments: $attachments, isEnabled: !model.isProcessing)

                HStack(spacing: 0) {
                    TextField("Message jcode...", text: $model.draftMessage, axis: .vertical)
                        .font(JC.Fonts.body)
                        .foregroundStyle(JC.Colors.textPrimary)
                        .lineLimit(1...6)
                        .focused($inputFocused)
                        .padding(.horizontal, JC.Spacing.md)
                        .padding(.vertical, JC.Spacing.sm + 2)
                }
                .background(JC.Colors.surfaceElevated)
                .clipShape(RoundedRectangle(cornerRadius: JC.Radius.xl, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: JC.Radius.xl, style: .continuous)
                        .stroke(inputFocused ? JC.Colors.borderFocused : JC.Colors.border, lineWidth: 1)
                )

                Button {
                    let pendingImages = allAttachments.map { ($0.mediaType, $0.base64Data) }
                    Task {
                        let sent = await model.sendDraft(images: pendingImages)
                        if sent {
                            attachments.removeAll()
                            externalAttachments.removeAll()
                        }
                    }
                } label: {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundStyle(canSend ? JC.Colors.textOnAccent : JC.Colors.textTertiary)
                        .frame(width: 32, height: 32)
                        .background(canSend ? JC.Colors.accent : JC.Colors.surfaceElevated)
                        .clipShape(Circle())
                }
                .buttonStyle(.plain)
                .disabled(!canSend)
                .animation(JC.Animation.quick, value: canSend)
            }
        }
        .padding(.horizontal, JC.Spacing.md)
        .padding(.vertical, JC.Spacing.sm + 2)
        .background(JC.Colors.surface)
        .animation(JC.Animation.standard, value: model.isProcessing)
    }

    private var canSend: Bool {
        let hasText = !model.draftMessage.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let hasAttachments = !allAttachments.isEmpty

        guard model.selectedServer != nil else { return false }
        if model.connectionState != .connected {
            return hasText && !hasAttachments && !model.isProcessing
        }
        if model.isProcessing {
            return hasText && !hasAttachments
        }
        return hasText || hasAttachments
    }
}

// MARK: - Settings Sheet

struct SettingsSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var showQRScanner = false
    @State private var showAddServer = false

    var body: some View {
        NavigationStack {
            ZStack {
                JC.Colors.background.ignoresSafeArea()

                ScrollView {
                    VStack(spacing: JC.Spacing.xl) {
                        connectionSection
                        repairPairingSection
                        serversSection
                        sessionsSection
                        modelSection
                    }
                    .padding(JC.Spacing.lg)
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .foregroundStyle(JC.Colors.accent)
                }
            }
        }
        .presentationBackground(JC.Colors.background)
        .sheet(isPresented: $showQRScanner) {
            QRScannerView(isPresented: $showQRScanner) { host, port, code in
                model.hostInput = host
                model.portInput = String(port)
                model.pairCodeInput = code
                Task { await model.pairAndSave() }
            }
        }
        .sheet(isPresented: $showAddServer) {
            AddServerSheet(isPresented: $showAddServer)
        }
    }

    private var repairPairingSection: some View {
        VStack(alignment: .leading, spacing: JC.Spacing.md) {
            SectionHeader(title: "Repair / Pair Desktop")

            VStack(alignment: .leading, spacing: JC.Spacing.md) {
                Text("Use this if your iPhone loses pairing, you switch Macs, or the desktop gateway token changes.")
                    .font(JC.Fonts.callout)
                    .foregroundStyle(JC.Colors.textSecondary)

                HStack(spacing: JC.Spacing.sm) {
                    Button {
                        showQRScanner = true
                    } label: {
                        Label("Scan QR", systemImage: "qrcode.viewfinder")
                            .font(JC.Fonts.caption)
                    }
                    .buttonStyle(GhostButton())

                    Button {
                        showAddServer = true
                    } label: {
                        Label("Pair Manually", systemImage: "keyboard")
                            .font(JC.Fonts.caption)
                    }
                    .buttonStyle(GhostButton())
                }
            }
            .glassCard()
        }
    }

    private var connectionSection: some View {
        VStack(alignment: .leading, spacing: JC.Spacing.md) {
            SectionHeader(title: "Connection")

            HStack(spacing: JC.Spacing.md) {
                StatusDot(
                    color: statusColor,
                    animated: model.connectionState == .connecting
                )

                VStack(alignment: .leading, spacing: 2) {
                    Text(statusText)
                        .font(JC.Fonts.headline)
                        .foregroundStyle(JC.Colors.textPrimary)
                    if let server = model.selectedServer {
                        Text("\(server.host):\(server.port)")
                            .font(JC.Fonts.monoSmall)
                            .foregroundStyle(JC.Colors.textTertiary)
                    }
                }

                Spacer()

                if model.connectionState == .connected {
                    Button {
                        Task { await model.disconnect() }
                    } label: {
                        Text("Disconnect")
                            .font(JC.Fonts.caption)
                    }
                    .buttonStyle(GhostButton())
                } else {
                    Button {
                        Task { await model.connectSelected() }
                    } label: {
                        Text("Connect")
                            .font(JC.Fonts.caption)
                    }
                    .buttonStyle(GhostButton())
                    .disabled(model.selectedServer == nil || model.connectionState == .connecting)
                }
            }
            .glassCard()

            if let error = model.errorMessage {
                HStack(spacing: JC.Spacing.sm) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.caption)
                    Text(error)
                        .font(JC.Fonts.caption)
                }
                .foregroundStyle(JC.Colors.destructive)
            }
        }
    }

    private var serversSection: some View {
        VStack(alignment: .leading, spacing: JC.Spacing.md) {
            HStack {
                SectionHeader(title: "Servers")
                Spacer()
                HStack(spacing: JC.Spacing.sm) {
                    Button {
                        showQRScanner = true
                    } label: {
                        Image(systemName: "qrcode.viewfinder")
                            .font(.system(size: 16))
                            .foregroundStyle(JC.Colors.accent)
                    }
                    Button {
                        showAddServer = true
                    } label: {
                        Image(systemName: "plus.circle.fill")
                            .font(.system(size: 16))
                            .foregroundStyle(JC.Colors.accent)
                    }
                }
            }

            if model.savedServers.isEmpty {
                VStack(spacing: JC.Spacing.sm) {
                    Image(systemName: "server.rack")
                        .font(.system(size: 24))
                        .foregroundStyle(JC.Colors.textTertiary)
                    Text("No paired servers")
                        .font(JC.Fonts.callout)
                        .foregroundStyle(JC.Colors.textSecondary)
                }
                .frame(maxWidth: .infinity)
                .glassCard()
            } else {
                VStack(spacing: JC.Spacing.sm) {
                    ForEach(model.savedServers, id: \.self) { credential in
                        ServerCard(
                            credential: credential,
                            isSelected: model.selectedServer?.host == credential.host && model.selectedServer?.port == credential.port
                        )
                    }
                }
            }
        }
    }

    private var sessionsSection: some View {
        VStack(alignment: .leading, spacing: JC.Spacing.md) {
            SectionHeader(title: "Sessions")

            if model.connectionState != .connected {
                Text("Connect to view sessions")
                    .font(JC.Fonts.callout)
                    .foregroundStyle(JC.Colors.textTertiary)
                    .frame(maxWidth: .infinity)
                    .glassCard()
            } else if model.sessions.isEmpty {
                Text("No active sessions")
                    .font(JC.Fonts.callout)
                    .foregroundStyle(JC.Colors.textTertiary)
                    .frame(maxWidth: .infinity)
                    .glassCard()
            } else {
                VStack(spacing: JC.Spacing.xs) {
                    ForEach(model.sessions, id: \.self) { sessionId in
                        Button {
                            Task { await model.switchToSession(sessionId) }
                        } label: {
                            HStack(spacing: JC.Spacing.sm) {
                                Image(systemName: "terminal")
                                    .font(.system(size: 12))
                                    .foregroundStyle(JC.Colors.textTertiary)
                                    .frame(width: 20)

                                Text(sessionId)
                                    .font(JC.Fonts.mono)
                                    .foregroundStyle(JC.Colors.textPrimary)
                                    .lineLimit(1)
                                    .truncationMode(.middle)

                                Spacer()

                                if sessionId == model.activeSessionId {
                                    Image(systemName: "checkmark.circle.fill")
                                        .font(.system(size: 14))
                                        .foregroundStyle(JC.Colors.accent)
                                }
                            }
                            .padding(.horizontal, JC.Spacing.md)
                            .padding(.vertical, JC.Spacing.sm + 2)
                            .background(
                                sessionId == model.activeSessionId
                                    ? JC.Colors.accentDim
                                    : JC.Colors.surface
                            )
                            .clipShape(RoundedRectangle(cornerRadius: JC.Radius.sm, style: .continuous))
                            .overlay(
                                RoundedRectangle(cornerRadius: JC.Radius.sm, style: .continuous)
                                    .stroke(
                                        sessionId == model.activeSessionId
                                            ? JC.Colors.borderFocused
                                            : JC.Colors.border,
                                        lineWidth: 1
                                    )
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }

    private var modelSection: some View {
        Group {
            if model.connectionState == .connected && !model.availableModels.isEmpty {
                VStack(alignment: .leading, spacing: JC.Spacing.md) {
                    SectionHeader(title: "Model")

                    VStack(spacing: JC.Spacing.xs) {
                        ForEach(model.availableModels, id: \.self) { m in
                            Button {
                                Task { await model.changeModel(m) }
                            } label: {
                                HStack(spacing: JC.Spacing.sm) {
                                    Image(systemName: "cpu")
                                        .font(.system(size: 12))
                                        .foregroundStyle(JC.Colors.textTertiary)
                                        .frame(width: 20)

                                    Text(m)
                                        .font(JC.Fonts.mono)
                                        .foregroundStyle(JC.Colors.textPrimary)
                                        .lineLimit(1)
                                        .truncationMode(.middle)

                                    Spacer()

                                    if m == model.modelName {
                                        Image(systemName: "checkmark.circle.fill")
                                            .font(.system(size: 14))
                                            .foregroundStyle(JC.Colors.accent)
                                    }
                                }
                                .padding(.horizontal, JC.Spacing.md)
                                .padding(.vertical, JC.Spacing.sm + 2)
                                .background(
                                    m == model.modelName
                                        ? JC.Colors.accentDim
                                        : JC.Colors.surface
                                )
                                .clipShape(RoundedRectangle(cornerRadius: JC.Radius.sm, style: .continuous))
                                .overlay(
                                    RoundedRectangle(cornerRadius: JC.Radius.sm, style: .continuous)
                                        .stroke(
                                            m == model.modelName
                                                ? JC.Colors.borderFocused
                                                : JC.Colors.border,
                                            lineWidth: 1
                                        )
                                )
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
        }
    }

    private var statusColor: Color {
        switch model.connectionState {
        case .connected: JC.Colors.statusOnline
        case .connecting: JC.Colors.statusConnecting
        case .disconnected: JC.Colors.statusOffline
        }
    }

    private var statusText: String {
        switch model.connectionState {
        case .connected: "Connected"
        case .connecting: "Connecting..."
        case .disconnected: "Disconnected"
        }
    }
}

// MARK: - Section Header

struct SectionHeader: View {
    let title: String

    var body: some View {
        Text(title.uppercased())
            .font(JC.Fonts.caption)
            .foregroundStyle(JC.Colors.textTertiary)
            .tracking(1.2)
    }
}

// MARK: - Server Card

struct ServerCard: View {
    @EnvironmentObject private var model: AppModel
    let credential: ServerCredential
    let isSelected: Bool

    var body: some View {
        Button {
            model.selectedServer = credential
            model.hostInput = credential.host
            model.portInput = String(credential.port)
        } label: {
            HStack(spacing: JC.Spacing.md) {
                ZStack {
                    RoundedRectangle(cornerRadius: JC.Radius.sm, style: .continuous)
                        .fill(isSelected ? JC.Colors.accentDim : JC.Colors.surfaceElevated)
                        .frame(width: 40, height: 40)
                    Image(systemName: "server.rack")
                        .font(.system(size: 16))
                        .foregroundStyle(isSelected ? JC.Colors.accent : JC.Colors.textTertiary)
                }

                VStack(alignment: .leading, spacing: 2) {
                    Text(credential.serverName)
                        .font(JC.Fonts.headline)
                        .foregroundStyle(JC.Colors.textPrimary)
                    HStack(spacing: JC.Spacing.xs) {
                        Text("\(credential.host):\(credential.port)")
                            .font(JC.Fonts.monoSmall)
                            .foregroundStyle(JC.Colors.textTertiary)
                        Text(credential.serverVersion)
                            .font(JC.Fonts.monoCaption)
                            .foregroundStyle(JC.Colors.textTertiary)
                    }
                }

                Spacer()

                if isSelected {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(JC.Colors.accent)
                }

                Button(role: .destructive) {
                    Task { await model.deleteServer(credential) }
                } label: {
                    Image(systemName: "trash")
                        .font(.system(size: 14))
                        .foregroundStyle(JC.Colors.destructive.opacity(0.6))
                }
                .buttonStyle(.plain)
            }
            .padding(JC.Spacing.md)
            .background(isSelected ? JC.Colors.accentDim.opacity(0.3) : JC.Colors.surface)
            .clipShape(RoundedRectangle(cornerRadius: JC.Radius.md, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: JC.Radius.md, style: .continuous)
                    .stroke(isSelected ? JC.Colors.borderFocused : JC.Colors.border, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Add Server Sheet

struct AddServerSheet: View {
    @EnvironmentObject private var model: AppModel
    @Binding var isPresented: Bool

    var body: some View {
        NavigationStack {
            ZStack {
                JC.Colors.background.ignoresSafeArea()

                ScrollView {
                    VStack(spacing: JC.Spacing.xl) {
                        VStack(spacing: JC.Spacing.sm) {
                            Text("Add Server")
                                .font(JC.Fonts.title2)
                                .foregroundStyle(JC.Colors.textPrimary)

                            Text("Run `jcode pair` on your machine\nto get a pairing code.")
                                .font(JC.Fonts.callout)
                                .foregroundStyle(JC.Colors.textSecondary)
                                .multilineTextAlignment(.center)
                        }

                        VStack(spacing: JC.Spacing.md) {
                            JCTextField(
                                label: "Host",
                                placeholder: "e.g. my-macbook",
                                text: $model.hostInput,
                                icon: "server.rack"
                            )
                            JCTextField(
                                label: "Port",
                                placeholder: "7643",
                                text: $model.portInput,
                                icon: "number",
                                keyboardType: .numberPad
                            )
                            JCTextField(
                                label: "Pair Code",
                                placeholder: "6-digit code",
                                text: $model.pairCodeInput,
                                icon: "key.fill"
                            )
                        }

                        if let error = model.errorMessage {
                            HStack(spacing: JC.Spacing.sm) {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .font(.caption)
                                Text(error)
                                    .font(JC.Fonts.caption)
                            }
                            .foregroundStyle(JC.Colors.destructive)
                        }

                        Button {
                            Task {
                                await model.pairAndSave()
                                if model.errorMessage == nil {
                                    isPresented = false
                                }
                            }
                        } label: {
                            HStack {
                                Image(systemName: "link")
                                Text("Pair & Connect")
                            }
                            .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(AccentButton())
                    }
                    .padding(JC.Spacing.xl)
                }
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { isPresented = false }
                        .foregroundStyle(JC.Colors.textSecondary)
                }
            }
        }
        .presentationDetents([.medium])
        .presentationBackground(JC.Colors.background)
    }
}
