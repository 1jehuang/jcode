import Foundation
import JCodeKit
import Observation

/// Account sign-in and synced-transcript state for the cloud flow.
@MainActor
@Observable
final class CloudModel {
    enum Phase: Equatable {
        case signedOut
        case awaitingCode(email: String, token: String)
        case signedIn(AccountCredential)
    }

    private(set) var phase: Phase = .signedOut
    private(set) var sessions: [CloudAPI.SessionSummary] = []
    private(set) var isLoading = false
    private(set) var hasSubscription = true
    /// New-session requests not yet turned into a synced session.
    private(set) var pendingCreates: [CloudAPI.Command] = []
    /// Set when a new session the user started becomes available.
    var openSessionID: String?
    var errorMessage: String?

    private let api: CloudAPI
    private let store: AccountStore

    init(api: CloudAPI = CloudAPI(), store: AccountStore = AccountStore()) {
        self.api = api
        self.store = store
        if let saved = store.load() {
            phase = .signedIn(saved)
        }
    }

    var credential: AccountCredential? {
        if case .signedIn(let c) = phase { return c }
        return nil
    }

    // MARK: - Sign in

    func startLogin(email: String) async {
        let trimmed = email.trimmingCharacters(in: .whitespaces).lowercased()
        errorMessage = nil
        isLoading = true
        defer { isLoading = false }
        do {
            let start = try await api.startLogin(email: trimmed)
            phase = .awaitingCode(email: trimmed, token: start.loginToken)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func verify(code: String) async {
        guard case .awaitingCode(_, let token) = phase else { return }
        errorMessage = nil
        isLoading = true
        defer { isLoading = false }
        do {
            let result = try await api.verifyLogin(token: token, code: code.filter(\.isNumber))
            let credential = AccountCredential(email: result.email, apiKey: result.apiKey)
            store.save(credential)
            phase = .signedIn(credential)
            await refresh()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// Signs in with an existing account API key (e.g. from `jcode account`).
    func signIn(apiKey: String) async {
        errorMessage = nil
        isLoading = true
        defer { isLoading = false }
        do {
            let me = try await api.me(apiKey: apiKey)
            let credential = AccountCredential(email: me.email, apiKey: apiKey)
            store.save(credential)
            phase = .signedIn(credential)
            await refresh()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func cancelCode() {
        phase = .signedOut
        errorMessage = nil
    }

    func signOut() {
        store.clear()
        sessions = []
        phase = .signedOut
    }

    // MARK: - Transcripts

    func refresh() async {
        guard let credential else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            sessions = try await api.sessions(apiKey: credential.apiKey)
            hasSubscription = true
            errorMessage = nil
        } catch CloudAPI.APIError.unauthorized {
            signOut()
            errorMessage = "Your sign-in expired. Sign in again."
        } catch CloudAPI.APIError.subscriptionRequired {
            hasSubscription = false
            sessions = []
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func transcript(for id: String) async throws -> CloudAPI.SessionDetail {
        guard let credential else { throw CloudAPI.APIError.unauthorized }
        return try await api.transcript(apiKey: credential.apiKey, sessionID: id)
    }

    // MARK: - Cloud commands

    func createSession(text: String) async throws -> CloudAPI.Enqueued {
        guard let credential else { throw CloudAPI.APIError.unauthorized }
        let queued = try await api.createSession(apiKey: credential.apiKey, text: text)
        pendingCreates.insert(queued.command, at: 0)
        return queued
    }

    func dismissPending(_ id: String) {
        pendingCreates.removeAll { $0.id == id }
    }

    /// Advance pending new-session requests; open the session once it syncs.
    func pollPendingCreates() async {
        guard let credential, !pendingCreates.isEmpty else { return }
        var changed = false
        for (i, cmd) in pendingCreates.enumerated() where cmd.isPending || cmd.resultSessionID != nil {
            guard let fresh = try? await api.command(apiKey: credential.apiKey, id: cmd.id) else { continue }
            if fresh != cmd { pendingCreates[i] = fresh; changed = true }
        }
        if changed || pendingCreates.contains(where: { $0.status == "done" }) { await refresh() }
        for cmd in pendingCreates where cmd.status == "done" {
            if let sid = cmd.resultSessionID, sessions.contains(where: { $0.sessionID == sid }) {
                pendingCreates.removeAll { $0.id == cmd.id }
                openSessionID = sid
            }
        }
    }

    func sendMessage(sessionID: String, text: String) async throws -> CloudAPI.Enqueued {
        guard let credential else { throw CloudAPI.APIError.unauthorized }
        return try await api.sendMessage(apiKey: credential.apiKey, sessionID: sessionID, text: text)
    }

    func command(id: String) async throws -> CloudAPI.Command {
        guard let credential else { throw CloudAPI.APIError.unauthorized }
        return try await api.command(apiKey: credential.apiKey, id: id)
    }

    func commands(sessionID: String) async throws -> [CloudAPI.Command] {
        guard let credential else { throw CloudAPI.APIError.unauthorized }
        return try await api.commands(apiKey: credential.apiKey, sessionID: sessionID)
    }

    /// Human status for a queued command, given the host wake result.
    static func hostMessage(_ state: String?) -> String? {
        switch state {
        case "starting", "pending": return "Waking your cloud machine…"
        case "absent": return "No cloud machine yet. Connect once from jcode desktop (`jcode cloud`) to create it."
        case "allowance_exhausted": return "This month's cloud runtime is used up."
        case "not_entitled": return "Jcode Cloud needs a paid subscription."
        default: return nil
        }
    }
}
