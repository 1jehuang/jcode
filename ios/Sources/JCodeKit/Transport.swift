import Foundation

/// Abstraction over a WebSocket so `Connection` is testable with a fake.
public protocol WebSocketTransport: Sendable {
    /// Opens the socket. Throws if the connection or auth fails.
    func connect(url: URL, authToken: String) async throws
    /// Sends one text frame.
    func send(text: String) async throws
    /// Receives the next text frame. Returns nil when the socket closes.
    func receiveText() async throws -> String?
    /// Closes the socket.
    func close() async
}

/// URLSession-backed transport used in production.
///
/// Auth is sent via `Authorization: Bearer <token>` on the upgrade request,
/// matching the gateway's preferred auth source.
public actor URLSessionWebSocketTransport: WebSocketTransport {
    private var task: URLSessionWebSocketTask?

    public init() {}

    public func connect(url: URL, authToken: String) async throws {
        var request = URLRequest(url: url)
        request.setValue("Bearer \(authToken)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 10
        let task = URLSession.shared.webSocketTask(with: request)
        task.resume()
        self.task = task
    }

    public func send(text: String) async throws {
        guard let task else { throw TransportError.notConnected }
        do {
            try await task.send(.string(text))
        } catch {
            throw Self.classify(error, task: task)
        }
    }

    public func receiveText() async throws -> String? {
        guard let task else { throw TransportError.notConnected }
        while true {
            let message: URLSessionWebSocketTask.Message
            do {
                message = try await task.receive()
            } catch {
                // Treat close as end-of-stream rather than error when the
                // task reports a normal closure.
                if task.closeCode != .invalid {
                    return nil
                }
                throw Self.classify(error, task: task)
            }
            switch message {
            case .string(let text):
                return text
            case .data(let data):
                if let text = String(data: data, encoding: .utf8) {
                    return text
                }
            @unknown default:
                continue
            }
        }
    }

    public func close() async {
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
    }

    private static func classify(_ error: Error, task: URLSessionWebSocketTask) -> Error {
        if let http = task.response as? HTTPURLResponse, http.statusCode == 401 {
            return TransportError.unauthorized
        }
        return error
    }
}

public enum TransportError: Error, Equatable {
    case notConnected
    /// The server rejected the WebSocket upgrade as unauthorized (401):
    /// the pairing token is unknown or was revoked. Reconnecting with the
    /// same token cannot succeed; the device must re-pair.
    case unauthorized
}
