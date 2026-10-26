import Foundation

/// Describes a jcode server gateway endpoint.
///
/// Endpoints use HTTP/WS by default, or HTTPS/WSS when `secure` is enabled.
public struct Gateway: Hashable, Sendable {
    public static let defaultPort: UInt16 = 7643

    public var host: String
    public var port: UInt16
    public var secure: Bool

    public init(host: String, port: UInt16 = Gateway.defaultPort, secure: Bool = false) {
        self.host = host
        self.port = port
        self.secure = secure
    }

    public var healthURL: URL {
        url(scheme: secure ? "https" : "http", path: "/health")
    }

    public var pairURL: URL {
        url(scheme: secure ? "https" : "http", path: "/pair")
    }

    public var webSocketURL: URL {
        url(scheme: secure ? "wss" : "ws", path: "/ws")
    }

    private func url(scheme: String, path: String) -> URL {
        var components = URLComponents()
        components.scheme = scheme
        components.host = host
        components.port = Int(port)
        components.path = path
        guard let url = components.url else {
            // Host strings from pairing are validated before reaching here;
            // a failure indicates programmer error.
            preconditionFailure("invalid gateway endpoint: \(scheme)://\(host):\(port)\(path)")
        }
        return url
    }
}

/// Parses `jcode://pair?host=H&port=P&code=C` URIs from QR codes.
public enum PairURI {
    public struct Payload: Equatable, Sendable {
        public var gateway: Gateway
        public var code: String

        public init(gateway: Gateway, code: String) {
            self.gateway = gateway
            self.code = code
        }
    }

    public static func parse(_ string: String) -> Payload? {
        guard let components = URLComponents(string: string),
            components.scheme == "jcode",
            components.host == "pair" || components.path == "pair"
                || components.host == nil && components.path == "/pair"
        else { return nil }
        let items = components.queryItems ?? []
        func value(_ name: String) -> String? {
            items.first(where: { $0.name == name })?.value
        }
        guard let host = value("host"), !host.isEmpty,
            let code = value("code"), !code.isEmpty
        else { return nil }
        let port = value("port").flatMap(UInt16.init) ?? Gateway.defaultPort
        let scheme = value("scheme")
        guard scheme.map({ ["http", "https", "ws", "wss"].contains($0) }) ?? true,
            items.filter({ $0.name == "scheme" }).count <= 1
        else { return nil }
        return Payload(
            gateway: Gateway(host: host, port: port, secure: scheme == "https" || scheme == "wss"),
            code: code
        )
    }
}
