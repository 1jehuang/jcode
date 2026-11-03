import Foundation

/// Jcode account + mobile transcript sync API (api.jcode.sh).
///
/// Sign-in uses the email-code flow: `startLogin` mails a 6-digit code and
/// `verifyLogin` exchanges it for an account API key. The key reads the
/// transcripts that jcode uploads while `jcode mobile enable` is on.
public struct CloudAPI: Sendable {
    public static let defaultBase = URL(string: "https://api.jcode.sh/v1")!

    public var base: URL
    public var session: URLSession

    public init(base: URL = CloudAPI.defaultBase, session: URLSession = .shared) {
        self.base = base
        self.session = session
    }

    public enum APIError: Error, LocalizedError, Equatable {
        case unauthorized
        case subscriptionRequired
        case server(code: String, message: String)
        case invalidResponse

        public var errorDescription: String? {
            switch self {
            case .unauthorized: return "Your sign-in expired. Sign in again."
            case .subscriptionRequired: return "Mobile sync needs an active jcode subscription."
            case .server(_, let message): return message
            case .invalidResponse: return "Unexpected response from jcode.sh"
            }
        }
    }

    // MARK: - Models

    public struct LoginStart: Decodable, Sendable {
        public let loginToken: String
        public let expiresIn: Int
        enum CodingKeys: String, CodingKey {
            case loginToken = "login_token"
            case expiresIn = "expires_in"
        }
    }

    public struct LoginResult: Decodable, Sendable {
        public let apiKey: String
        public let email: String
        enum CodingKeys: String, CodingKey {
            case apiKey = "api_key"
            case email
        }
    }

    public struct Me: Decodable, Sendable {
        public let email: String
        public let tier: String?
        public let capabilities: Capabilities
        public struct Capabilities: Decodable, Sendable {
            public let mobileSync: Bool
            enum CodingKeys: String, CodingKey { case mobileSync = "mobile_sync" }
            public init(from decoder: Decoder) throws {
                let c = try decoder.container(keyedBy: CodingKeys.self)
                mobileSync = try c.decodeIfPresent(Bool.self, forKey: .mobileSync) ?? false
            }
        }
    }

    public struct SessionSummary: Decodable, Identifiable, Hashable, Sendable {
        public let sessionID: String
        public let title: String?
        public let source: String
        public let host: String?
        public let cwd: String?
        public let model: String?
        public let messageCount: Int
        public let lastMessagePreview: String?
        public let updatedAt: String
        public var id: String { sessionID }

        public var displayTitle: String {
            if let title, !title.isEmpty { return title }
            if let preview = lastMessagePreview, !preview.isEmpty { return String(preview.prefix(60)) }
            return sessionID
        }

        public var updatedDate: Date? { CloudAPI.parseDate(updatedAt) }

        enum CodingKeys: String, CodingKey {
            case sessionID = "session_id"
            case title, source, host, cwd, model
            case messageCount = "message_count"
            case lastMessagePreview = "last_message_preview"
            case updatedAt = "updated_at"
        }
    }

    public struct Message: Decodable, Identifiable, Hashable, Sendable {
        public let seq: Int
        public let role: String
        public let text: String
        public let createdAt: String?
        public var id: Int { seq }
        enum CodingKeys: String, CodingKey {
            case seq, role, text
            case createdAt = "created_at"
        }
    }

    public struct SessionDetail: Decodable, Sendable {
        public let session: SessionSummary
        public let messages: [Message]
        public let more: Bool
    }

    // MARK: - Calls

    public func startLogin(email: String) async throws -> LoginStart {
        try await send("auth/email/start", method: "POST", body: ["email": email, "client_name": "jcode-ios"])
    }

    public func verifyLogin(token: String, code: String) async throws -> LoginResult {
        try await send("auth/email/verify", method: "POST", body: ["login_token": token, "code": code])
    }

    public func me(apiKey: String) async throws -> Me {
        try await send("me", apiKey: apiKey)
    }

    public func sessions(apiKey: String) async throws -> [SessionSummary] {
        struct List: Decodable { let sessions: [SessionSummary] }
        let list: List = try await send("mobile/sessions?limit=200", apiKey: apiKey)
        return list.sessions
    }

    /// Full transcript, following `after_seq` pages.
    public func transcript(apiKey: String, sessionID: String) async throws -> SessionDetail {
        var all: [Message] = []
        var after = -1
        var last: SessionDetail
        let escaped = sessionID.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? sessionID
        repeat {
            last = try await send("mobile/sessions/\(escaped)?after_seq=\(after)", apiKey: apiKey)
            all += last.messages
            after = last.messages.last?.seq ?? after
        } while last.more && !last.messages.isEmpty
        return SessionDetail(session: last.session, messages: all, more: false)
    }

    // MARK: - Plumbing

    static func parseDate(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }

    private struct ErrorEnvelope: Decodable {
        struct Body: Decodable { let code: String?; let message: String? }
        let error: Body?
    }

    private func send<T: Decodable>(
        _ path: String,
        method: String = "GET",
        apiKey: String? = nil,
        body: [String: String]? = nil
    ) async throws -> T {
        guard let url = URL(string: base.absoluteString + "/" + path) else { throw APIError.invalidResponse }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = 30
        if let apiKey { request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization") }
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200..<300).contains(http.statusCode) else {
            if http.statusCode == 401 { throw APIError.unauthorized }
            if http.statusCode == 402 { throw APIError.subscriptionRequired }
            let env = try? JSONDecoder().decode(ErrorEnvelope.self, from: data)
            throw APIError.server(
                code: env?.error?.code ?? "http_\(http.statusCode)",
                message: env?.error?.message ?? "Request failed (\(http.statusCode))"
            )
        }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.invalidResponse
        }
    }
}
