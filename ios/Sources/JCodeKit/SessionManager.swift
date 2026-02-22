import Foundation

public struct SessionInfo: Sendable {
    public let sessionId: String
    public let friendlyName: String?
}

public actor SessionManager {
    private let connection: JCodeConnection
    private var currentSessionId: String?
    private var allSessions: [String] = []

    public init(connection: JCodeConnection) {
        self.connection = connection
    }

    public var activeSessionId: String? { currentSessionId }
    public var sessions: [String] { allSessions }

    public func setActiveSession(_ sessionId: String) {
        currentSessionId = sessionId
    }

    public func updateSessions(from payload: HistoryPayload) {
        currentSessionId = payload.sessionId
        allSessions = payload.allSessions
    }

    public func switchSession(_ sessionId: String) async throws {
        try await connection.resumeSession(sessionId)
        currentSessionId = sessionId
    }
}
