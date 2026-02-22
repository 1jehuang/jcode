import Foundation

public actor ReconnectionManager {
    private let host: String
    private let port: UInt16
    private let authToken: String
    private var reconnectTask: Task<Void, Never>?
    private var attempt = 0
    private let maxBackoff: TimeInterval = 30

    public var onReconnect: (@Sendable () async -> Void)?
    public var onGaveUp: (@Sendable () async -> Void)?

    public init(host: String, port: UInt16, authToken: String) {
        self.host = host
        self.port = port
        self.authToken = authToken
    }

    public func scheduleReconnect() {
        reconnectTask?.cancel()
        reconnectTask = Task { [weak self] in
            guard let self else { return }
            let delay = await self.nextDelay()
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled else { return }
            await self.onReconnect?()
        }
    }

    public func reset() {
        attempt = 0
        reconnectTask?.cancel()
        reconnectTask = nil
    }

    private func nextDelay() -> TimeInterval {
        let delay = min(pow(2.0, Double(attempt)), maxBackoff)
        attempt += 1
        let jitter = Double.random(in: 0...1)
        return delay + jitter
    }
}

public struct ServerDiscovery: Sendable {
    public let host: String
    public let port: UInt16

    public init(host: String, port: UInt16 = 7643) {
        self.host = host
        self.port = port
    }

    public func probe() async -> HealthResponse? {
        let client = PairingClient(host: host, port: port)
        return try? await client.checkHealth()
    }

    public static func probeTailscale(hostname: String, port: UInt16 = 7643) async -> HealthResponse? {
        let discovery = ServerDiscovery(host: hostname, port: port)
        return await discovery.probe()
    }
}
