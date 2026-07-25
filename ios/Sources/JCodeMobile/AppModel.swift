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

    init(store: any CredentialStore = KeychainCredentialStore()) {
        self.store = store
        servers = store.loadAll()
        activeServer = servers.last
    }

    var isConnected: Bool {
        session.phase == .connected
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
        )
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

    // MARK: - Connection lifecycle

    func connect(to credential: ServerCredential, sessionID: String? = nil) {
        session = SessionState()
        open(credential, sessionID: sessionID)
    }

    /// Reconnects to the active server without discarding the rendered
    /// transcript; the history resync replaces it once the socket is back.
    func retryConnection() {
        guard let activeServer else { return }
        open(activeServer, sessionID: session.sessionID)
    }

    private func open(_ credential: ServerCredential, sessionID: String?) {
        disconnect()
        activeServer = credential
        let connection = Connection(
            configuration: .init(
                gateway: credential.gateway,
                authToken: credential.token
            )
        )
        self.connection = connection
        pumpTask = Task { [weak self] in
            let stream = await connection.start(resumeSessionID: sessionID)
            for await output in stream {
                guard let self else { return }
                self.session = SessionReducer.reduce(self.session, output)
            }
        }
    }

    func disconnect() {
        pumpTask?.cancel()
        pumpTask = nil
        let connection = connection
        self.connection = nil
        Task { await connection?.stop() }
        session = SessionReducer.reduce(session, .phase(.disconnected))
    }

    // MARK: - Actions

    /// Submit the composer draft. Whether it sends, queues, or is ignored is
    /// decided by `ComposerRules` so the Return key and the send button always
    /// agree (and the rule stays unit tested without a UI).
    func sendDraft() {
        let action = ComposerRules.submitAction(
            draft: draft,
            isConnected: isConnected,
            isProcessing: session.isProcessing
        )
        switch action {
        case .ignore, .newline:
            return
        case .send(let text):
            draft = ""
            session = SessionReducer.reduce(session, intent: .userSentMessage(text))
            send { .message(id: $0, content: text) }
        case .queue(let text):
            draft = ""
            session = SessionReducer.reduce(session, intent: .userQueuedInterrupt(text))
            send { .softInterrupt(id: $0, content: text, urgent: false) }
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
