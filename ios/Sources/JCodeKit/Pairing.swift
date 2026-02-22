import Foundation

public struct PairResponse: Codable, Sendable {
    public let token: String
    public let serverName: String
    public let serverVersion: String

    enum CodingKeys: String, CodingKey {
        case token
        case serverName = "server_name"
        case serverVersion = "server_version"
    }
}

public struct PairError: Codable, Sendable {
    public let error: String
}

public struct HealthResponse: Codable, Sendable {
    public let status: String
    public let version: String
    public let gateway: Bool
}

public struct PairingClient: Sendable {
    public let host: String
    public let port: UInt16

    public init(host: String, port: UInt16 = 7643) {
        self.host = host
        self.port = port
    }

    private var baseURL: URL {
        var components = URLComponents()
        components.scheme = "http"
        components.host = host
        components.port = Int(port)
        return components.url!
    }

    public func checkHealth() async throws -> HealthResponse {
        let url = baseURL.appendingPathComponent("health")
        let (data, response) = try await URLSession.shared.data(from: url)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw PairingError.serverUnreachable
        }
        return try JSONDecoder().decode(HealthResponse.self, from: data)
    }

    public func pair(
        code: String,
        deviceId: String,
        deviceName: String,
        apnsToken: String? = nil
    ) async throws -> PairResponse {
        let url = baseURL.appendingPathComponent("pair")
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        var body: [String: String] = [
            "code": code,
            "device_id": deviceId,
            "device_name": deviceName,
        ]
        if let apns = apnsToken {
            body["apns_token"] = apns
        }
        request.httpBody = try JSONEncoder().encode(body)

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw PairingError.serverUnreachable
        }

        switch http.statusCode {
        case 200:
            return try JSONDecoder().decode(PairResponse.self, from: data)
        case 401:
            let err = try? JSONDecoder().decode(PairError.self, from: data)
            throw PairingError.invalidCode(err?.error ?? "Invalid or expired pairing code")
        default:
            let err = try? JSONDecoder().decode(PairError.self, from: data)
            throw PairingError.serverError(err?.error ?? "HTTP \(http.statusCode)")
        }
    }
}

public enum PairingError: Error, Sendable {
    case serverUnreachable
    case invalidCode(String)
    case serverError(String)
}
