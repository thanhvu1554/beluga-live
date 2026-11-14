import Foundation
import BelugaCore

/// Mock lesson script bundled as `mock-lesson.json`: real server frames (PROTOCOL section 4) with delays.
struct MockScript: Sendable {
    struct Frame: Sendable { var after: Int; var frame: JSONValue }
    struct Turn: Sendable { var user: String; var frames: [Frame] }

    var ready: JSONValue
    var sessions: JSONValue
    var progress: JSONValue
    /// Frames of the "offer to review" opening turn (say + show review_offer + turn_end).
    var review: [JSONValue]
    var turns: [Turn]

    static func load() -> MockScript {
        guard let url = Bundle.main.url(forResource: "mock-lesson", withExtension: "json"),
              let data = try? Data(contentsOf: url), let v = try? JSONValue.parse(data) else {
            return MockScript(ready: ["t": "ready"], sessions: ["t": "sessions", "items": []],
                              progress: ["t": "progress"], review: [], turns: [])
        }
        let turns = (v["turns"]?.array ?? []).map { t in
            Turn(user: t["user"]?.string ?? "",
                 frames: (t["frames"]?.array ?? []).map { Frame(after: $0["after"]?.int ?? 0, frame: $0["frame"] ?? .null) })
        }
        return MockScript(ready: v["ready"] ?? ["t": "ready"], sessions: v["sessions"] ?? ["t": "sessions"],
                          progress: v["progress"] ?? ["t": "progress"], review: v["review"]?.array ?? [], turns: turns)
    }

    /// All lesson events of the script, in order.
    var allEvents: [LessonEvent] { turns.flatMap { $0.frames.compactMap { LessonEvent.from($0.frame) } } }
}

/// Replays the bundled script as if it came from the server.
@MainActor
final class MockTransport: Transport {
    var onEvent: ((TransportEvent) -> Void)?
    let script: MockScript
    private var turnIndex = 0
    private var playTask: Task<Void, Never>?
    private var photoWaiter: CheckedContinuation<Void, Never>?
    /// When false the transport stays "offline" (for the offline scene).
    var reachable = true

    init(script: MockScript = .load()) { self.script = script }

    func connect() {
        guard reachable else {
            Task { @MainActor in self.onEvent?(.closed(reason: "mock offline")) }
            return
        }
    }

    func disconnect() { playTask?.cancel(); playTask = nil }

    func send(_ message: ClientMessage) {
        switch message {
        case .hello:
            deliver(script.ready)
        case .userText, .audioEnd:
            playNextTurn()
        case .photo:
            photoWaiter?.resume(); photoWaiter = nil
        case .interrupt:
            playTask?.cancel(); playTask = nil
            photoWaiter?.resume(); photoWaiter = nil
        case .listSessions:
            deliver(script.sessions)
        case .getProgress:
            deliver(script.progress)
        case .openSession:
            deliver(script.ready)
        default:
            break
        }
    }

    func sendBinary(_ data: Data) {}

    /// The scripted user line for the next turn (what the fake student "says").
    var nextUserLine: String? { turnIndex < script.turns.count ? script.turns[turnIndex].user : nil }

    private func deliver(_ v: JSONValue) {
        onEvent?(.message(ServerMessage.decode(v)))
    }

    private func playNextTurn() {
        guard turnIndex < script.turns.count else { return }
        let turn = script.turns[turnIndex]
        turnIndex += 1
        playTask?.cancel()
        playTask = Task { [weak self] in
            for f in turn.frames {
                try? await Task.sleep(for: .milliseconds(f.after))
                guard let self, !Task.isCancelled else { return }
                self.deliver(f.frame)
                if f.frame["t"]?.string == "capture_request" {
                    await withCheckedContinuation { self.photoWaiter = $0 }
                }
            }
        }
    }
}
