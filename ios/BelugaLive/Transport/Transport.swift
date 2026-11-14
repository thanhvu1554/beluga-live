import Foundation
import BelugaCore

enum TransportEvent: Sendable {
    case message(ServerMessage)
    case closed(reason: String?)
}

@MainActor
protocol Transport: AnyObject {
    var onEvent: ((TransportEvent) -> Void)? { get set }
    func connect()
    func disconnect()
    func send(_ message: ClientMessage)
    func sendBinary(_ data: Data)
}

/// WebSocket to the PC server (`ws://<pc>:8787/ws`, plain text inside Tailscale).
@MainActor
final class WebSocketTransport: Transport {
    var onEvent: ((TransportEvent) -> Void)?
    private let url: URL
    private var task: URLSessionWebSocketTask?
    private var generation = 0
    private var tasks: [Task<Void, Never>] = []
    private var outbox: AsyncStream<URLSessionWebSocketTask.Message>.Continuation?
    private let session: URLSession = {
        let c = URLSessionConfiguration.default
        c.timeoutIntervalForRequest = 10
        c.waitsForConnectivity = false
        return URLSession(configuration: c)
    }()

    init(url: URL) { self.url = url }

    func connect() {
        disconnect()
        let gen = generation
        var request = URLRequest(url: url)
        request.timeoutInterval = 8
        let task = session.webSocketTask(with: request)
        task.maximumMessageSize = 32 * 1024 * 1024
        self.task = task
        task.resume()

        // Ordered sender.
        let (stream, continuation) = AsyncStream<URLSessionWebSocketTask.Message>.makeStream()
        outbox = continuation
        tasks.append(Task { [weak self] in
            for await message in stream {
                do { try await task.send(message) } catch {
                    self?.fail(gen: gen, error.localizedDescription); return
                }
            }
        })
        // Receiver.
        tasks.append(Task { [weak self] in
            while !Task.isCancelled {
                do {
                    let msg = try await task.receive()
                    guard let self, gen == self.generation else { return }
                    switch msg {
                    case .string(let text):
                        if let m = try? ServerMessage.decode(text) { self.onEvent?(.message(m)) }
                    case .data(let data):
                        if let m = try? ServerMessage.decode(data) { self.onEvent?(.message(m)) }
                    @unknown default: break
                    }
                } catch {
                    self?.fail(gen: gen, error.localizedDescription)
                    return
                }
            }
        })
        // Keepalive: detects a dead link (Wi-Fi switch, PC asleep) within ~20 s.
        tasks.append(Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(15))
                guard !Task.isCancelled else { return }
                Self.ping(task) { reason in
                    Task { @MainActor in self?.fail(gen: gen, reason) }
                }
            }
        })
    }

    private nonisolated static func ping(_ task: URLSessionWebSocketTask, onError: @escaping @Sendable (String) -> Void) {
        task.sendPing { error in
            if let error { onError(error.localizedDescription) }
        }
    }

    func disconnect() {
        generation += 1
        for t in tasks { t.cancel() }
        tasks.removeAll()
        outbox?.finish(); outbox = nil
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
    }

    func send(_ message: ClientMessage) {
        outbox?.yield(.string(message.text))
    }

    func sendBinary(_ data: Data) {
        outbox?.yield(.data(data))
    }

    private func fail(gen: Int, _ reason: String) {
        guard gen == generation else { return }
        disconnect()
        onEvent?(.closed(reason: reason))
    }
}
