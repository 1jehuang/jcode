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
}
