import Foundation

public struct ServerCredential: Codable, Sendable, Hashable {
    public let host: String
    public let port: UInt16
    public let authToken: String
    public let serverName: String
    public let serverVersion: String
    public let deviceId: String
    public let pairedAt: Date

    enum CodingKeys: String, CodingKey {
        case host, port
        case authToken = "auth_token"
        case serverName = "server_name"
        case serverVersion = "server_version"
        case deviceId = "device_id"
        case pairedAt = "paired_at"
    }
}

public actor CredentialStore {
    private let fileURL: URL
    private var credentials: [ServerCredential] = []

    public init() {
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dir = appSupport.appendingPathComponent("jcode", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        self.fileURL = dir.appendingPathComponent("servers.json")
        self.credentials = Self.load(from: fileURL)
    }

    public func all() -> [ServerCredential] {
        credentials
    }

    public func find(host: String) -> ServerCredential? {
        credentials.first { $0.host == host }
    }

    public func find(host: String, port: UInt16) -> ServerCredential? {
        credentials.first { $0.host == host && $0.port == port }
    }

    public func save(_ credential: ServerCredential) throws {
        credentials.removeAll { $0.host == credential.host && $0.port == credential.port }
        credentials.append(credential)
        try persist()
    }

    public func remove(host: String) throws {
        credentials.removeAll { $0.host == host }
        try persist()
    }

    public func remove(host: String, port: UInt16) throws {
        credentials.removeAll { $0.host == host && $0.port == port }
        try persist()
    }

    private func persist() throws {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = .prettyPrinted
        let data = try encoder.encode(credentials)
        try data.write(to: fileURL, options: .atomic)
    }

    private static func load(from url: URL) -> [ServerCredential] {
        guard let data = try? Data(contentsOf: url) else { return [] }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return (try? decoder.decode([ServerCredential].self, from: data)) ?? []
    }
}
