import Foundation

/// Exchanges a pairing code for a long-lived auth token via `POST /pair`.
public struct PairingClient: Sendable {
    public struct Response: Equatable, Sendable {
        public var token: String
        public var serverName: String
        public var serverVersion: String

        public init(token: String, serverName: String, serverVersion: String) {
            self.token = token
            self.serverName = serverName
            self.serverVersion = serverVersion
        }
    }

    public enum PairingError: Error, Equatable {
        case invalidCode(String)
        case serverError(statusCode: Int, message: String)
        case invalidResponse
    }

    private let session: URLSession

    public init(session: URLSession = .shared) {
        self.session = session
    }

    public func pair(
        gateway: Gateway,
        code: String,
        deviceID: String,
        deviceName: String
    ) async throws -> Response {
        var request = URLRequest(url: gateway.pairURL)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.timeoutInterval = 15
        let body: [String: String] = [
            "code": code,
            "device_id": deviceID,
            "device_name": deviceName,
        ]
        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw PairingError.invalidResponse
        }
        let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        guard http.statusCode == 200 else {
            let message = object["error"] as? String ?? "HTTP \(http.statusCode)"
            if http.statusCode == 401 {
                throw PairingError.invalidCode(message)
            }
            throw PairingError.serverError(statusCode: http.statusCode, message: message)
        }
        guard let token = object["token"] as? String, !token.isEmpty else {
            throw PairingError.invalidResponse
        }
        return Response(
            token: token,
            serverName: object["server_name"] as? String ?? "jcode",
            serverVersion: object["server_version"] as? String ?? "unknown"
        )
    }

    /// Probes `GET /health`. Returns true when the gateway is reachable.
    public func checkHealth(gateway: Gateway) async -> Bool {
        var request = URLRequest(url: gateway.healthURL)
        request.timeoutInterval = 5
        guard let (_, response) = try? await session.data(for: request),
            let http = response as? HTTPURLResponse
        else { return false }
        return http.statusCode == 200
    }
}

public enum PairingFailure {
    public static func message(for error: Error, gateway: Gateway) -> String {
        switch error as? PairingClient.PairingError {
        case .invalidCode(let message), .serverError(_, let message):
            return message
        case .invalidResponse:
            return "Unexpected response from server"
        case nil:
            break
        }
        let target = "\(gateway.host):\(gateway.port)"
        if (error as? URLError)?.code == .appTransportSecurityRequiresSecureConnection {
            return "Could not reach \(target): iOS blocked the plain HTTP connection to this address. Use a local network address."
        }
        return "Could not reach \(target): \(error.localizedDescription)"
    }
}
