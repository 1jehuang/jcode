import Foundation

/// Exchanges a pairing code for a long-lived auth token via `POST /pair`.
public struct PairingClient: Sendable {
    public struct Response: Equatable, Sendable {
        public var token: String
        public var serverName: String
        public var serverVersion: String
        /// Absolute directory the server wants remote clients to subscribe
        /// against. Older servers omit it; the client then falls back to
        /// probing `/health`, and finally to no directory at all.
        public var workingDir: String?

        public init(
            token: String, serverName: String, serverVersion: String,
            workingDir: String? = nil
        ) {
            self.token = token
            self.serverName = serverName
            self.serverVersion = serverVersion
            self.workingDir = workingDir
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
            serverVersion: object["server_version"] as? String ?? "unknown",
            workingDir: (object["working_dir"] as? String).flatMap {
                $0.isEmpty ? nil : $0
            }
        )
    }

    /// Probes `GET /health`. Returns true when the gateway is reachable.
    public func checkHealth(gateway: Gateway) async -> Bool {
        await health(gateway: gateway) != nil
    }

    /// Probes `GET /health` and returns the parsed body, which carries the
    /// working directory remote clients must subscribe against. Used to recover
    /// that directory for credentials paired before the field existed.
    public func health(gateway: Gateway) async -> [String: Any]? {
        var request = URLRequest(url: gateway.healthURL)
        request.timeoutInterval = 5
        guard let (data, response) = try? await session.data(for: request),
            let http = response as? HTTPURLResponse, http.statusCode == 200
        else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }
}
