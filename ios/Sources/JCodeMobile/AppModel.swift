import Foundation
import Combine
import JCodeKit

#if canImport(UIKit)
import UIKit
#endif

@MainActor
final class AppModel: ObservableObject {
    enum ConnectionState: Equatable {
        case disconnected
        case connecting
        case connected
    }

    struct ChatEntry: Identifiable, Equatable {
        let id: UUID
        let role: Role
        var text: String
        var toolCalls: [ToolCallInfo]

        enum Role: String {
            case user
            case assistant
            case system
        }

        init(id: UUID = UUID(), role: Role, text: String, toolCalls: [ToolCallInfo] = []) {
            self.id = id
            self.role = role
            self.text = text
            self.toolCalls = toolCalls
        }

        static func == (lhs: ChatEntry, rhs: ChatEntry) -> Bool {
            lhs.id == rhs.id && lhs.text == rhs.text && lhs.toolCalls.count == rhs.toolCalls.count
        }
    }

    @Published var connectionState: ConnectionState = .disconnected
    @Published var savedServers: [ServerCredential] = []
    @Published var selectedServer: ServerCredential? {
        didSet {
            guard let selectedServer else {
                UserDefaults.standard.removeObject(forKey: "jcode.selected.host")
                UserDefaults.standard.removeObject(forKey: "jcode.selected.port")
                return
            }
            UserDefaults.standard.set(selectedServer.host, forKey: "jcode.selected.host")
            UserDefaults.standard.set(Int(selectedServer.port), forKey: "jcode.selected.port")

            if connectionState == .disconnected {
                hostInput = selectedServer.host
                portInput = String(selectedServer.port)
            }
        }
    }

    @Published var hostInput: String = ""
    @Published var portInput: String = "7643"
    @Published var pairCodeInput: String = ""
    @Published var deviceNameInput: String = {
        #if canImport(UIKit)
        return UIDevice.current.name
        #else
        return Host.current().localizedName ?? "Mac"
        #endif
    }()

    @Published var statusMessage: String?
    @Published var errorMessage: String?

    @Published var messages: [ChatEntry] = []
    @Published var draftMessage: String = ""
    @Published var activeSessionId: String = ""
    @Published var sessions: [String] = []
    @Published var serverName: String = ""
    @Published var serverVersion: String = ""
    @Published var modelName: String = ""

    private let credentialStore = CredentialStore()
    private var client: JCodeClient?
    private var clientDelegate: ClientDelegate?
    private var reconnecting = false
    private var shouldAutoReconnect = false
    private var connectionGeneration: UInt64 = 0

    private var lastAssistantMessageId: UUID?
    private var lastAssistantIndex: Int?
    private var inFlightTools: [String: ToolCallInfo] = [:]
    private var lastToolId: String?
    private var toolMessageIndex: [String: Int] = [:]
    private var toolSubIndex: [String: Int] = [:]

    private let deviceId: String = {
        if let existing = UserDefaults.standard.string(forKey: "jcode.device.id") {
            return existing
        }
        let generated = "ios-" + UUID().uuidString.lowercased()
        UserDefaults.standard.set(generated, forKey: "jcode.device.id")
        return generated
    }()

    func loadSavedServers() async {
        let all = await credentialStore.all()
        let creds = all.sorted {
            if $0.host == $1.host {
                return $0.port < $1.port
            }
            return $0.host < $1.host
        }

        savedServers = creds

        let rememberedHost = UserDefaults.standard.string(forKey: "jcode.selected.host")
        let rememberedPort = UserDefaults.standard.integer(forKey: "jcode.selected.port")

        if let selected = selectedServer {
            let exists = creds.contains(where: { $0.host == selected.host && $0.port == selected.port })
            if !exists {
                selectedServer = nil
            }
        }

        if selectedServer == nil,
           let rememberedHost,
           rememberedPort > 0,
           let remembered = creds.first(where: { $0.host == rememberedHost && Int($0.port) == rememberedPort }) {
            selectedServer = remembered
        }

        if selectedServer == nil {
            selectedServer = creds.first
        }
    }

    func parsePort() -> UInt16? {
        guard let value = UInt16(portInput.trimmingCharacters(in: .whitespacesAndNewlines)) else {
            return nil
        }
        return value
    }

    func probeServer() async {
        clearTransientMessages()

        guard let port = parsePort() else {
            errorMessage = "Port must be a number from 0 to 65535."
            return
        }

        let host = hostInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !host.isEmpty else {
            errorMessage = "Host cannot be empty."
            return
        }

        let client = PairingClient(host: host, port: port)
        do {
            let response = try await client.checkHealth()
            statusMessage = "Server reachable: \(response.version)"
            errorMessage = nil
        } catch {
            errorMessage = "Unable to reach server. Verify Tailscale and gateway settings."
        }
    }

    func pairAndSave() async {
        clearTransientMessages()

        guard let port = parsePort() else {
            errorMessage = "Port must be a number from 0 to 65535."
            return
        }

        let host = hostInput.trimmingCharacters(in: .whitespacesAndNewlines)
        let code = pairCodeInput.trimmingCharacters(in: .whitespacesAndNewlines)
        let deviceName = deviceNameInput.trimmingCharacters(in: .whitespacesAndNewlines)

        guard !host.isEmpty else {
            errorMessage = "Host cannot be empty."
            return
        }
        guard !code.isEmpty else {
            errorMessage = "Enter the 6-digit pairing code from jcode pair."
            return
        }
        guard !deviceName.isEmpty else {
            errorMessage = "Device name cannot be empty."
            return
        }

        do {
            let pairClient = PairingClient(host: host, port: port)
            let response = try await pairClient.pair(code: code, deviceId: deviceId, deviceName: deviceName)

            let credential = ServerCredential(
                host: host,
                port: port,
                authToken: response.token,
                serverName: response.serverName,
                serverVersion: response.serverVersion,
                deviceId: deviceId,
                pairedAt: Date()
            )

            try await credentialStore.save(credential)
            await loadSavedServers()
            selectedServer = credential
            statusMessage = "Paired with \(response.serverName) (\(response.serverVersion))."
            pairCodeInput = ""
            errorMessage = nil
        } catch let error as PairingError {
            switch error {
            case .serverUnreachable:
                errorMessage = "Server unreachable. Confirm host/port and gateway status."
            case .invalidCode(let message):
                errorMessage = message
            case .serverError(let message):
                errorMessage = message
            }
        } catch {
            errorMessage = "Pairing failed: \(error.localizedDescription)"
        }
    }

    func deleteServer(_ credential: ServerCredential) async {
        if connectionState != .disconnected,
           selectedServer?.host == credential.host,
           selectedServer?.port == credential.port {
            await disconnect()
        }

        do {
            try await credentialStore.remove(host: credential.host, port: credential.port)
            if selectedServer?.host == credential.host && selectedServer?.port == credential.port {
                selectedServer = nil
            }
            await loadSavedServers()
            statusMessage = "Removed \(credential.host):\(credential.port)"
        } catch {
            errorMessage = "Failed to remove server: \(error.localizedDescription)"
        }
    }

    fileprivate func markNewGeneration() -> UInt64 {
        connectionGeneration &+= 1
        return connectionGeneration
    }

    fileprivate func isCurrentGeneration(_ generation: UInt64) -> Bool {
        connectionGeneration == generation
    }

    func connectSelected() async {
        clearTransientMessages()

        guard let credential = selectedServer else {
            errorMessage = "Select a paired server first."
            return
        }

        let generation = markNewGeneration()

        if let current = client {
            shouldAutoReconnect = false
            reconnecting = false
            await current.disconnect()
        }
        client = nil
        clientDelegate = nil

        connectionState = .connecting
        shouldAutoReconnect = true
        reconnecting = false

        messages = []
        inFlightTools.removeAll()
        lastToolId = nil
        lastAssistantMessageId = nil
        lastAssistantIndex = nil
        toolMessageIndex.removeAll()
        toolSubIndex.removeAll()
        activeSessionId = ""
        sessions = []
        serverName = credential.serverName
        serverVersion = credential.serverVersion
        modelName = ""

        let newClient = JCodeClient(host: credential.host, port: credential.port, authToken: credential.authToken)
        let delegate = ClientDelegate(model: self, generation: generation)
        clientDelegate = delegate
        await newClient.setDelegate(delegate)

        do {
            try await newClient.connect()
            client = newClient
            connectionState = .connected
            reconnecting = false
            statusMessage = "Connected to \(credential.host):\(credential.port)"
        } catch {
            connectionState = .disconnected
            shouldAutoReconnect = false
            reconnecting = false
            clientDelegate = nil
            errorMessage = "Connect failed: \(error.localizedDescription)"
        }
    }

    func disconnect() async {
        _ = markNewGeneration()
        shouldAutoReconnect = false
        reconnecting = false

        guard let client else {
            connectionState = .disconnected
            return
        }

        await client.disconnect()
        self.client = nil
        self.clientDelegate = nil
        connectionState = .disconnected
        statusMessage = "Disconnected"
    }

    func sendDraft() async {
        clearTransientMessages()

        guard connectionState == .connected else {
            errorMessage = "Not connected."
            return
        }

        let trimmed = draftMessage.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return
        }

        guard let client else {
            errorMessage = "Not connected."
            return
        }

        do {
            messages.append(ChatEntry(role: .user, text: trimmed))
            draftMessage = ""

            let assistantPlaceholder = ChatEntry(role: .assistant, text: "")
            messages.append(assistantPlaceholder)
            lastAssistantMessageId = assistantPlaceholder.id

            try await client.send(trimmed)
        } catch {
            if let id = lastAssistantMessageId,
               let idx = messages.firstIndex(where: { $0.id == id }) {
                messages.remove(at: idx)
            }
            lastAssistantMessageId = messages.last(where: { $0.role == .assistant })?.id
            errorMessage = "Send failed: \(error.localizedDescription)"
        }
    }

    func refreshHistory() async {
        guard let client else {
            errorMessage = "Not connected."
            return
        }

        do {
            try await client.refreshHistory()
        } catch {
            errorMessage = "Could not refresh history: \(error.localizedDescription)"
        }
    }

    func switchToSession(_ sessionId: String) async {
        guard !sessionId.isEmpty else {
            return
        }

        guard let client else {
            errorMessage = "Not connected."
            return
        }

        do {
            try await client.switchSession(sessionId)
            activeSessionId = sessionId
            statusMessage = "Switched to \(sessionId)"
            // History will be refreshed by server event.
        } catch {
            errorMessage = "Session switch failed: \(error.localizedDescription)"
        }
    }

    private func applyConnectedServerInfo(_ info: ServerInfo) {
        activeSessionId = info.sessionId
        sessions = info.allSessions
        serverName = info.serverName ?? "jcode"
        serverVersion = info.serverVersion ?? ""
        modelName = info.providerModel ?? ""
    }

    private func applyHistory(_ history: [HistoryMessage]) {
        var mapped: [ChatEntry] = []
        mapped.reserveCapacity(history.count)
        for item in history {
            let role: ChatEntry.Role
            switch item.role {
            case "assistant":
                role = .assistant
            case "system":
                role = .system
            default:
                role = .user
            }

            var toolCalls: [ToolCallInfo] = []
            if let tool = item.toolData, let id = tool.id, let name = tool.name {
                var info = ToolCallInfo(id: id, name: name)
                info.input = tool.input ?? ""
                info.output = tool.output
                info.state = .done
                toolCalls.append(info)
            }

            mapped.append(ChatEntry(role: role, text: item.content, toolCalls: toolCalls))
        }
        messages = mapped
        lastAssistantMessageId = messages.last(where: { $0.role == .assistant })?.id
        if let id = lastAssistantMessageId {
            lastAssistantIndex = messages.lastIndex(where: { $0.id == id })
        } else {
            lastAssistantIndex = nil
        }
        inFlightTools.removeAll()
        lastToolId = nil
        toolMessageIndex.removeAll()
        toolSubIndex.removeAll()
    }

    private func appendAssistantChunk(_ delta: String) {
        if let idx = lastAssistantIndex, idx < messages.count,
           messages[idx].id == lastAssistantMessageId {
            messages[idx].text += delta
            return
        }
        if let id = lastAssistantMessageId,
           let idx = messages.firstIndex(where: { $0.id == id }) {
            lastAssistantIndex = idx
            messages[idx].text += delta
            return
        }

        let entry = ChatEntry(role: .assistant, text: delta)
        messages.append(entry)
        lastAssistantMessageId = entry.id
        lastAssistantIndex = messages.count - 1
    }

    private func replaceAssistantText(_ text: String) {
        if let idx = lastAssistantIndex, idx < messages.count,
           messages[idx].id == lastAssistantMessageId {
            messages[idx].text = text
            return
        }
        if let id = lastAssistantMessageId,
           let idx = messages.firstIndex(where: { $0.id == id }) {
            lastAssistantIndex = idx
            messages[idx].text = text
            return
        }

        let entry = ChatEntry(role: .assistant, text: text)
        messages.append(entry)
        lastAssistantMessageId = entry.id
        lastAssistantIndex = messages.count - 1
    }

    private func attachTool(_ tool: ToolCallInfo) {
        inFlightTools[tool.id] = tool
        lastToolId = tool.id

        if let idx = lastAssistantIndex, idx < messages.count,
           messages[idx].id == lastAssistantMessageId {
            messages[idx].toolCalls.append(tool)
            toolMessageIndex[tool.id] = idx
            toolSubIndex[tool.id] = messages[idx].toolCalls.count - 1
        } else if let id = lastAssistantMessageId,
                  let idx = messages.firstIndex(where: { $0.id == id }) {
            lastAssistantIndex = idx
            messages[idx].toolCalls.append(tool)
            toolMessageIndex[tool.id] = idx
            toolSubIndex[tool.id] = messages[idx].toolCalls.count - 1
        } else {
            let entry = ChatEntry(role: .assistant, text: "", toolCalls: [tool])
            messages.append(entry)
            lastAssistantMessageId = entry.id
            lastAssistantIndex = messages.count - 1
            toolMessageIndex[tool.id] = messages.count - 1
            toolSubIndex[tool.id] = 0
        }
    }

    private func updateLatestTool(_ toolId: String, _ mutate: (inout ToolCallInfo) -> Void) {
        guard var tool = inFlightTools[toolId] else {
            return
        }

        mutate(&tool)
        inFlightTools[toolId] = tool

        if let msgIdx = toolMessageIndex[toolId], msgIdx < messages.count,
           let tIdx = toolSubIndex[toolId], tIdx < messages[msgIdx].toolCalls.count,
           messages[msgIdx].toolCalls[tIdx].id == toolId {
            messages[msgIdx].toolCalls[tIdx] = tool
            return
        }

        for msgIdx in messages.indices {
            if let toolIdx = messages[msgIdx].toolCalls.firstIndex(where: { $0.id == toolId }) {
                toolMessageIndex[toolId] = msgIdx
                toolSubIndex[toolId] = toolIdx
                messages[msgIdx].toolCalls[toolIdx] = tool
                break
            }
        }
    }

    private func clearTransientMessages() {
        statusMessage = nil
        errorMessage = nil
    }

    fileprivate func onConnected(_ info: ServerInfo) {
        connectionState = .connected
        reconnecting = false
        applyConnectedServerInfo(info)
    }

    fileprivate func onDisconnected(error: String?) {
        client = nil
        clientDelegate = nil
        connectionState = .disconnected
        inFlightTools.removeAll()
        lastToolId = nil
        lastAssistantMessageId = nil
        lastAssistantIndex = nil
        toolMessageIndex.removeAll()
        toolSubIndex.removeAll()

        if let error, !error.isEmpty {
            errorMessage = error
        }

        guard shouldAutoReconnect else {
            reconnecting = false
            return
        }

        if reconnecting {
            return
        }

        // Lightweight reconnect trigger for transient network drops.
        reconnecting = true
        Task {
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            guard reconnecting else { return }
            await connectSelected()
        }
    }

    fileprivate func onTextDelta(_ text: String) {
        appendAssistantChunk(text)
    }

    fileprivate func onTextReplace(_ text: String) {
        replaceAssistantText(text)
    }

    fileprivate func onInterrupted(_ interrupt: InterruptInfo) {
        if let id = lastAssistantMessageId,
           let idx = messages.firstIndex(where: { $0.id == id }),
           messages[idx].text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
           messages[idx].toolCalls.isEmpty {
            messages.remove(at: idx)
        }

        messages.append(ChatEntry(role: .system, text: interrupt.message))

        inFlightTools.removeAll()
        lastToolId = nil
        lastAssistantMessageId = nil
    }

    fileprivate func onToolStart(_ tool: ToolCallInfo) {
        attachTool(tool)
    }

    fileprivate func onToolInput(_ delta: String) {
        guard let toolId = lastToolId else {
            return
        }
        updateLatestTool(toolId) { tool in
            tool.input += delta
            tool.state = .streaming
        }
    }

    fileprivate func onToolExec(id: String, name _: String) {
        updateLatestTool(id) { tool in
            tool.state = .executing
        }
    }

    fileprivate func onToolDone(id: String, name _: String, output: String, error: String?) {
        updateLatestTool(id) { tool in
            tool.output = output
            tool.error = error
            tool.state = error == nil ? .done : .failed
        }
    }

    fileprivate func onTurnDone(id _: UInt64) {
        inFlightTools.removeAll()
        lastToolId = nil
        lastAssistantMessageId = nil
        lastAssistantIndex = nil
        toolMessageIndex.removeAll()
        toolSubIndex.removeAll()
    }

    fileprivate func onServerError(id _: UInt64, message: String) {
        errorMessage = message
    }

    fileprivate func onModelChanged(model: String, provider _: String?) {
        modelName = model
    }

    fileprivate func onHistory(_ history: [HistoryMessage]) {
        applyHistory(history)
    }
}

@MainActor
private final class ClientDelegate: JCodeClientDelegate {
    unowned let model: AppModel
    let generation: UInt64

    init(model: AppModel, generation: UInt64) {
        self.model = model
        self.generation = generation
    }

    private func guardCurrent() -> Bool {
        model.isCurrentGeneration(generation)
    }

    func clientDidConnect(serverInfo: ServerInfo) {
        guard guardCurrent() else { return }
        model.onConnected(serverInfo)
    }

    func clientDidDisconnect(error: String?) {
        guard guardCurrent() else { return }
        model.onDisconnected(error: error)
    }

    func clientDidReceiveText(_ text: String) {
        guard guardCurrent() else { return }
        model.onTextDelta(text)
    }

    func clientDidReplaceText(_ text: String) {
        guard guardCurrent() else { return }
        model.onTextReplace(text)
    }

    func clientDidStartTool(_ tool: ToolCallInfo) {
        guard guardCurrent() else { return }
        model.onToolStart(tool)
    }

    func clientDidReceiveToolInput(_ delta: String) {
        guard guardCurrent() else { return }
        model.onToolInput(delta)
    }

    func clientDidExecuteTool(id: String, name: String) {
        guard guardCurrent() else { return }
        model.onToolExec(id: id, name: name)
    }

    func clientDidFinishTool(id: String, name: String, output: String, error: String?) {
        guard guardCurrent() else { return }
        model.onToolDone(id: id, name: name, output: output, error: error)
    }

    func clientDidFinishTurn(id: UInt64) {
        guard guardCurrent() else { return }
        model.onTurnDone(id: id)
    }

    func clientDidReceiveError(id: UInt64, message: String) {
        guard guardCurrent() else { return }
        model.onServerError(id: id, message: message)
    }

    func clientDidUpdateTokens(_ update: TokenUpdate) {
        guard guardCurrent() else { return }
        _ = update
    }

    func clientDidChangeModel(model: String, provider: String?) {
        guard guardCurrent() else { return }
        self.model.onModelChanged(model: model, provider: provider)
    }

    func clientDidReceiveHistory(messages: [HistoryMessage]) {
        guard guardCurrent() else { return }
        model.onHistory(messages)
    }

    func clientDidInterrupt(_ interrupt: InterruptInfo) {
        guard guardCurrent() else { return }
        model.onInterrupted(interrupt)
    }
}
