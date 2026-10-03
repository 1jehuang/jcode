import Foundation
import JCodeKit
import Observation

/// Observable glue between JCodeKit and the SwiftUI views.
///
/// Owns the credential store, the active `Connection`, and the derived
/// `SessionState`. Contains no protocol or state-transition logic itself;
/// everything flows through `SessionReducer`.
@MainActor
@Observable
final class AppModel {
    // MARK: - Published state

    private(set) var session = SessionState()
    private(set) var servers: [ServerCredential] = []
    var activeServer: ServerCredential?

    /// Composer draft.
    var draft = ""

    // MARK: - Internals

    private let store: any CredentialStore
    private var connection: Connection?
    private var pumpTask: Task<Void, Never>?
    private var unconfirmedWorkspaceFallback: ServerCredential?

    init(store: any CredentialStore = KeychainCredentialStore()) {
        self.store = store
        servers = store.loadAll()
        activeServer = servers.last
    }

    var isConnected: Bool {
        session.phase == .connected
    }

    var needsReconnect: Bool {
        switch session.phase {
        case .connected, .connecting: false
        case .reconnecting, .disconnected, .failed: true
        }
    }

    var activeWorkspace: String? {
        activeServer?.activeWorkspace
    }

    var needsWorkspace: Bool {
        activeServer != nil && activeWorkspace == nil
    }

    // MARK: - Pairing

    func pair(gateway: Gateway, code: String, deviceName: String) async throws {
        let client = PairingClient()
        let response = try await client.pair(
            gateway: gateway,
            code: code,
            deviceID: deviceID(),
            deviceName: deviceName
        )
        let credential = ServerCredential(
            host: gateway.host,
            port: gateway.port,
            token: response.token,
            serverName: response.serverName,
            serverVersion: response.serverVersion
        ).keepingWorkspaces(from: servers)
        store.save(credential)
        servers = store.loadAll()
        activeServer = credential
        connect(to: credential)
    }

    func removeServer(_ credential: ServerCredential) {
        store.remove(id: credential.id)
        servers = store.loadAll()
        if activeServer?.id == credential.id {
            disconnect()
            activeServer = servers.last
        }
    }

    func leaveServer() {
        unconfirmedWorkspaceFallback = nil
        disconnect()
        activeServer = nil
    }

    // MARK: - Connection lifecycle

    func connect(to credential: ServerCredential, sessionID: String? = nil) {
        unconfirmedWorkspaceFallback = nil
        session = SessionState()
        activeServer = credential
        guard credential.activeWorkspace != nil || sessionID != nil else {
            disconnect()
            return
        }
        open(credential, sessionID: sessionID)
    }

    /// Reconnects to the active server without discarding the rendered
    /// transcript; the history resync replaces it once the socket is back.
    func retryConnection() {
        guard let activeServer else { return }
        guard activeServer.activeWorkspace != nil || session.sessionID != nil else { return }
        open(activeServer, sessionID: session.sessionID)
    }

    @discardableResult
    func selectWorkspace(_ path: String) -> Bool {
        guard let activeServer, let updated = activeServer.selectingWorkspace(path) else {
            return false
        }
        let fallback = ServerCredential.workspaceFallback(
            pending: unconfirmedWorkspaceFallback, current: activeServer)
        connect(to: updated)
        unconfirmedWorkspaceFallback = fallback
        return true
    }

    func forgetWorkspace(_ path: String) {
        guard let activeServer else { return }
        let updated = activeServer.forgettingWorkspace(path)
        persist(updated)
        if updated.activeWorkspace != activeServer.activeWorkspace {
            connect(to: updated)
        }
    }

    private func observe(_ output: ConnectionOutput) {
        guard let fallback = unconfirmedWorkspaceFallback else { return }
        switch output {
        case .event(.sessionID):
            unconfirmedWorkspaceFallback = nil
            if let activeServer {
                persist(activeServer)
            }
        case .phase(.failed):
            unconfirmedWorkspaceFallback = nil
            activeServer = fallback
        default:
            break
        }
    }

    private func persist(_ credential: ServerCredential) {
        store.save(credential)
        servers = store.loadAll()
        activeServer = credential
    }

    private func open(_ credential: ServerCredential, sessionID: String?) {
        tearDownConnection()
        activeServer = credential
        let connection = Connection(
            configuration: .init(
                gateway: credential.gateway,
                authToken: credential.token
            )
        )
        self.connection = connection
        let workingDirectory = credential.activeWorkspace
        pumpTask = Task { [weak self] in
            let stream = await connection.start(
                resumeSessionID: sessionID,
                workingDirectory: workingDirectory
            )
            for await output in stream {
                guard !Task.isCancelled, let self, self.connection === connection else { return }
                self.session = SessionReducer.reduce(self.session, output)
                self.observe(output)
            }
        }
    }

    func disconnect() {
        tearDownConnection()
        session = SessionReducer.reduce(session, .phase(.disconnected))
    }

    private func tearDownConnection() {
        pumpTask?.cancel()
        pumpTask = nil
        let connection = connection
        self.connection = nil
        Task { await connection?.stop() }
    }

    // MARK: - Actions

    func sendDraft() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        draft = ""
        if session.isProcessing {
            session = SessionReducer.reduce(session, intent: .userQueuedInterrupt(text))
            send { .softInterrupt(id: $0, content: text, urgent: false) }
        } else {
            session = SessionReducer.reduce(session, intent: .userSentMessage(text))
            send { .message(id: $0, content: text) }
        }
    }

    func interrupt() {
        send { .cancel(id: $0) }
    }

    func switchSession(_ sessionID: String) {
        guard let activeServer else { return }
        connect(to: activeServer, sessionID: sessionID)
    }

    func setModel(_ model: String) {
        send { .setModel(id: $0, model: model) }
    }

    func setReasoningEffort(_ effort: String) {
        send { .setReasoningEffort(id: $0, effort: effort) }
    }

    /// Asks the server to compact the conversation context.
    func compactConversation() {
        send { .compact(id: $0) }
    }

    func renameSession(_ title: String) {
        send { .renameSession(id: $0, title: title.isEmpty ? nil : title) }
    }

    func dismissError() {
        session = SessionReducer.reduce(session, intent: .dismissError)
    }

    func dismissNotice(_ id: UUID) {
        session = SessionReducer.reduce(session, intent: .dismissNotice(id))
    }

    /// Clears the current conversation on the server and optimistically locally.
    func clearConversation() {
        session = SessionReducer.reduce(session, intent: .clearedConversation)
        send { .clear(id: $0) }
    }

    /// Drops any soft-interrupt messages queued mid-run before they inject.
    func cancelQueuedInterrupts() {
        session = SessionReducer.reduce(session, intent: .cancelledQueuedInterrupts)
        send { .cancelSoftInterrupts(id: $0) }
    }

    // MARK: - Helpers

    private func send(_ build: @escaping @Sendable (UInt64) -> Request) {
        guard let connection else { return }
        Task {
            do {
                try await connection.send(build)
            } catch {
                // Connection drops surface via phase changes; nothing to do here.
            }
        }
    }

    private func deviceID() -> String {
        let key = "jcode.device.id"
        if let existing = UserDefaults.standard.string(forKey: key) {
            return existing
        }
        let fresh = UUID().uuidString
        UserDefaults.standard.set(fresh, forKey: key)
        return fresh
    }
}
