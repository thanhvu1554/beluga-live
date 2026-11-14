import Foundation

/// Reduces session frames (PROTOCOL sections 4 and 7) into a chat transcript: user turns, assistant markdown
/// (streamed with `chat_delta`, settled by `chat`), inline blocks, and live-lesson events folded into text.
/// The app renders it natively when `chat.html` is not available, and uses it to know whether a session is empty.
public struct ChatTranscript: Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        case user(text: String, files: [FileRef])
        /// Assistant markdown; `streaming` while `chat_delta` pieces are still arriving.
        case text(md: String, streaming: Bool)
        /// A board block shown inline (`show`).
        case block(JSONValue)
        /// A live step title.
        case heading(String)
        /// Spoken lines of a live lesson, merged per turn.
        case spoken(String)
        case ask(String)
        case done
    }
    public struct Item: Sendable, Equatable, Identifiable {
        public var id: Int
        public var turn: Int
        public var kind: Kind
    }

    public private(set) var items: [Item] = []
    /// The assistant is working on a turn (thinking or streaming).
    public private(set) var busy = false
    private var seen: Set<Int> = []
    private var nextId = 0

    public init() {}

    public var isEmpty: Bool { items.isEmpty }

    public mutating func reset() { self = ChatTranscript() }

    /// Feed a raw frame (event, `chat_delta` or `state`). Duplicated seqs are ignored.
    public mutating func apply(_ v: JSONValue) {
        guard let t = v["t"]?.string else { return }
        if let seq = v["seq"]?.int {
            guard !seen.contains(seq) else { return }
            seen.insert(seq)
        }
        let turn = v["turn"]?.int ?? 0
        switch t {
        case "state":
            let s = v["state"]?.string ?? ""
            if s == "thinking" || s == "tool" { busy = true } else if s == "idle" || s == "done" { busy = false }
        case "chat_delta":
            busy = true
            let piece = v["text"]?.string ?? ""
            if let i = streamingIndex(turn: turn), case .text(let md, _) = items[i].kind {
                items[i].kind = .text(md: md + piece, streaming: true)
            } else {
                append(turn, .text(md: piece, streaming: true))
            }
        case "chat":
            let md = v["md"]?.string ?? ""
            if let i = streamingIndex(turn: turn) { items[i].kind = .text(md: md, streaming: false) }
            else { append(turn, .text(md: md, streaming: false)) }
        case "user":
            busy = true
            let files = (v["files"]?.array ?? []).compactMap(FileRef.from)
            append(turn, .user(text: v["text"]?.string ?? "", files: files))
        case "show":
            settleStreaming(turn: turn)
            if let block = v["block"] { append(turn, .block(block)) }
        case "step":
            settleStreaming(turn: turn)
            let title = v["title"]?.string ?? ""
            if !title.isEmpty { append(turn, .heading(title)) }
        case "say":
            let text = v["text"]?.string ?? ""
            guard !text.isEmpty else { return }
            if let last = items.last, last.turn == turn, case .spoken(let prev) = last.kind {
                items[items.count - 1].kind = .spoken(prev + " " + text)
            } else {
                append(turn, .spoken(text))
            }
        case "ask":
            append(turn, .ask(v["prompt"]?.string ?? ""))
        case "done":
            append(turn, .done)
        case "turn_end":
            settleStreaming(turn: nil)
            busy = false
        default:
            break
        }
    }

    public mutating func load(_ frames: [JSONValue]) {
        reset()
        for f in frames { apply(f) }
        busy = false
    }

    private func streamingIndex(turn: Int) -> Int? {
        guard let i = items.lastIndex(where: { $0.turn == turn }) else { return nil }
        if case .text(_, true) = items[i].kind { return i }
        return nil
    }

    private mutating func settleStreaming(turn: Int?) {
        for i in items.indices where turn == nil || items[i].turn == turn {
            if case .text(let md, true) = items[i].kind { items[i].kind = .text(md: md, streaming: false) }
        }
    }

    private mutating func append(_ turn: Int, _ kind: Kind) {
        items.append(Item(id: nextId, turn: turn, kind: kind))
        nextId += 1
    }
}
