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
    private var pingTask: Task<Void, Never>?
    private let session: URLSession = {
        let c = URLSessionConfiguration.default
        c.timeoutIntervalForRequest = 10
        c.waitsForConnectivity = false
        return URLSession(configuration: c)
    }()

    init(url: URL) { self.url = url }

    func connect() {
        disconnect()
        generation += 1
        let gen = generation
        var request = URLRequest(url: url)
        request.timeoutInterval = 8
        let task = session.webSocketTask(with: request)
        task.maximumMessageSize = 32 * 1024 * 1024
        self.task = task
        task.resume()
        receiveLoop(task, gen: gen)
        pingTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(15))
                guard let self, !Task.isCancelled else { return }
                self.ping(gen: gen)
            }
        }
    }

    func disconnect() {
        pingTask?.cancel(); pingTask = nil
        generation += 1
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
    }

    func send(_ message: ClientMessage) {
        guard let task else { return }
        let gen = generation
        task.send(.string(message.text)) { [weak self] error in
            guard let error else { return }
            Task { @MainActor in self?.fail(gen: gen, error.localizedDescription) }
        }
    }

    func sendBinary(_ data: Data) {
        guard let task else { return }
        let gen = generation
        task.send(.data(data)) { [weak self] error in
            guard let error else { return }
            Task { @MainActor in self?.fail(gen: gen, error.localizedDescription) }
        }
    }

    private func ping(gen: Int) {
        guard gen == generation, let task else { return }
        task.sendPing { [weak self] error in
            guard let error else { return }
            Task { @MainActor in self?.fail(gen: gen, error.localizedDescription) }
        }
    }

    private func receiveLoop(_ task: URLSessionWebSocketTask, gen: Int) {
        Task { [weak self] in
            while true {
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
        }
    }

    private func fail(gen: Int, _ reason: String) {
        guard gen == generation else { return }
        disconnect()
        onEvent?(.closed(reason: reason))
    }
}
