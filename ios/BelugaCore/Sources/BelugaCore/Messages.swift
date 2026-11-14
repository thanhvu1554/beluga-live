import Foundation

// Wire models for docs/PROTOCOL.md section 4 (protocol v1).

public enum Lang: String, Codable, Sendable, CaseIterable { case vi, en }
public enum ModelChoice: String, Codable, Sendable, CaseIterable { case sonnet, opus, haiku }
public enum Rigor: String, Codable, Sendable, CaseIterable { case light, medium, hard }

public struct TutorSettings: Codable, Sendable, Equatable {
    public var lang: Lang
    public var model: ModelChoice
    public var rigor: Rigor
    public init(lang: Lang = .vi, model: ModelChoice = .sonnet, rigor: Rigor = .medium) {
        self.lang = lang; self.model = model; self.rigor = rigor
    }
}

// MARK: - Server -> phone

public struct Ready: Sendable, Equatable {
    public var v: Int
    public var sessionId: String
    public var title: String
    public var tts: String          // "device" | "server"
    public var stt: String          // "device" | "server"
    public var boardVersion: Int
    public var pairToken: String?
    public var serverTTS: Bool { tts == "server" }
    public var serverSTT: Bool { stt == "server" }
}

public struct SessionItem: Sendable, Equatable, Identifiable {
    public var sessionId: String
    public var title: String
    public var updatedAt: String
    public var id: String { sessionId }
    public init(sessionId: String, title: String, updatedAt: String) {
        self.sessionId = sessionId; self.title = title; self.updatedAt = updatedAt
    }
}

/// A sequenced lesson event (say/step/show/ask/done/turn_end). `raw` is the original frame,
/// forwarded verbatim to the board's `board.apply(event)`.
public struct LessonEvent: Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        case say(text: String, audio: Bool)
        case step(index: Int, title: String)
        case show(step: Int, block: JSONValue)
        case ask(kind: String, prompt: String)
        case done
        case turnEnd(interrupted: Bool)
    }
    public var seq: Int
    public var turn: Int
    public var kind: Kind
    public var raw: JSONValue

    public var isSay: Bool { if case .say = kind { return true }; return false }
    public var isTurnEnd: Bool { if case .turnEnd = kind { return true }; return false }
    /// Events the board renders.
    public var isBoardEvent: Bool {
        switch kind {
        case .step, .show, .ask, .done: return true
        default: return false
        }
    }

    public init(seq: Int, turn: Int, kind: Kind, raw: JSONValue? = nil) {
        self.seq = seq; self.turn = turn; self.kind = kind
        self.raw = raw ?? LessonEvent.makeRaw(seq: seq, turn: turn, kind: kind)
    }

    static func makeRaw(seq: Int, turn: Int, kind: Kind) -> JSONValue {
        var o: [String: JSONValue] = ["seq": .number(Double(seq)), "turn": .number(Double(turn))]
        switch kind {
        case .say(let text, let audio): o["t"] = "say"; o["text"] = .string(text); o["audio"] = .bool(audio)
        case .step(let index, let title): o["t"] = "step"; o["index"] = .number(Double(index)); o["title"] = .string(title)
        case .show(let step, let block): o["t"] = "show"; o["step"] = .number(Double(step)); o["block"] = block
        case .ask(let k, let prompt): o["t"] = "ask"; o["kind"] = .string(k); o["prompt"] = .string(prompt)
        case .done: o["t"] = "done"
        case .turnEnd(let interrupted): o["t"] = "turn_end"; o["interrupted"] = .bool(interrupted)
        }
        return .object(o)
    }

    /// Parses a lesson event frame; returns nil for anything else.
    public static func from(_ v: JSONValue) -> LessonEvent? {
        guard let t = v["t"]?.string, let seq = v["seq"]?.int else { return nil }
        let turn = v["turn"]?.int ?? 0
        let kind: Kind
        switch t {
        case "say": kind = .say(text: v["text"]?.string ?? "", audio: v["audio"]?.bool ?? false)
        case "step": kind = .step(index: v["index"]?.int ?? 0, title: v["title"]?.string ?? "")
        case "show":
            guard let block = v["block"] else { return nil }
            kind = .show(step: v["step"]?.int ?? 0, block: block)
        case "ask": kind = .ask(kind: v["kind"]?.string ?? "next", prompt: v["prompt"]?.string ?? "")
        case "done": kind = .done
        case "turn_end": kind = .turnEnd(interrupted: v["interrupted"]?.bool ?? false)
        default: return nil
        }
        return LessonEvent(seq: seq, turn: turn, kind: kind, raw: v)
    }
}

public enum ServerMessage: Sendable, Equatable {
    case ready(Ready)
    case state(state: String, tool: String?)
    case heard(text: String)
    case event(LessonEvent)
    case audio(seq: Int, mime: String, data: Data)
    case title(text: String)
    case captureRequest(requestId: String)
    case snapshot(events: [LessonEvent])
    case sessions([SessionItem])
    case notice(code: String, text: String)
    case error(code: String, text: String, fatal: Bool)
    case unknown(type: String)

    public static func decode(_ data: Data) throws -> ServerMessage {
        decode(try JSONValue.parse(data))
    }

    public static func decode(_ text: String) throws -> ServerMessage {
        try decode(Data(text.utf8))
    }

    public static func decode(_ v: JSONValue) -> ServerMessage {
        let t = v["t"]?.string ?? ""
        switch t {
        case "ready":
            return .ready(Ready(
                v: v["v"]?.int ?? 1,
                sessionId: v["sessionId"]?.string ?? "",
                title: v["title"]?.string ?? "",
                tts: v["tts"]?.string ?? "device",
                stt: v["stt"]?.string ?? "device",
                boardVersion: v["boardVersion"]?.int ?? 0,
                pairToken: v["pairToken"]?.string))
        case "state":
            return .state(state: v["state"]?.string ?? "idle", tool: v["tool"]?.string)
        case "heard":
            return .heard(text: v["text"]?.string ?? "")
        case "say", "step", "show", "ask", "done", "turn_end":
            if let e = LessonEvent.from(v) { return .event(e) }
            return .unknown(type: t)
        case "audio":
            let data = Data(base64Encoded: v["data"]?.string ?? "") ?? Data()
            return .audio(seq: v["seq"]?.int ?? -1, mime: v["mime"]?.string ?? "audio/mpeg", data: data)
        case "title":
            return .title(text: v["text"]?.string ?? "")
        case "capture_request":
            return .captureRequest(requestId: v["requestId"]?.string ?? "")
        case "snapshot":
            return .snapshot(events: (v["events"]?.array ?? []).compactMap(LessonEvent.from))
        case "sessions":
            let items = (v["items"]?.array ?? []).compactMap { i -> SessionItem? in
                guard let id = i["sessionId"]?.string else { return nil }
                let updated = i["updatedAt"]?.string ?? i["updatedAt"]?.double.map { String(Int($0)) } ?? ""
                return SessionItem(sessionId: id, title: i["title"]?.string ?? "", updatedAt: updated)
            }
            return .sessions(items)
        case "notice":
            return .notice(code: v["code"]?.string ?? "", text: v["text"]?.string ?? "")
        case "error":
            return .error(code: v["code"]?.string ?? "", text: v["text"]?.string ?? "", fatal: v["fatal"]?.bool ?? false)
        default:
            return .unknown(type: t)
        }
    }
}

// MARK: - Phone -> server

public enum ClientMessage: Sendable, Equatable {
    case hello(pairCode: String?, pairToken: String?, device: String, sessionId: String?, lastSeq: Int?, settings: TutorSettings)
    case settings(TutorSettings)
    case userText(String)
    case audioStart(sampleRate: Int)
    case audioEnd
    case interrupt
    case photo(requestId: String?, jpeg: Data, w: Int, h: Int)
    case played(seq: Int)
    case listSessions
    case openSession(sessionId: String)

    public var json: JSONValue {
        func settingsObject(_ s: TutorSettings) -> JSONValue {
            ["lang": .string(s.lang.rawValue), "model": .string(s.model.rawValue), "rigor": .string(s.rigor.rawValue)]
        }
        switch self {
        case let .hello(pairCode, pairToken, device, sessionId, lastSeq, settings):
            var o: [String: JSONValue] = ["t": "hello", "v": .number(Double(BelugaCore.protocolVersion)),
                                          "device": .string(device), "settings": settingsObject(settings)]
            if let pairCode, !pairCode.isEmpty { o["pairCode"] = .string(pairCode) }
            if let pairToken, !pairToken.isEmpty { o["pairToken"] = .string(pairToken) }
            o["sessionId"] = .string(sessionId ?? "")
            if let lastSeq { o["lastSeq"] = .number(Double(lastSeq)) }
            return .object(o)
        case .settings(let s):
            var o: [String: JSONValue] = ["t": "settings"]
            if case .object(let so) = settingsObject(s) { o.merge(so) { a, _ in a } }
            return .object(o)
        case .userText(let text): return ["t": "user_text", "text": .string(text)]
        case .audioStart(let rate): return ["t": "audio_start", "mime": "audio/wav", "sampleRate": .number(Double(rate))]
        case .audioEnd: return ["t": "audio_end"]
        case .interrupt: return ["t": "interrupt"]
        case let .photo(requestId, jpeg, w, h):
            var o: [String: JSONValue] = ["t": "photo", "jpeg": .string(jpeg.base64EncodedString()),
                                          "w": .number(Double(w)), "h": .number(Double(h))]
            if let requestId { o["requestId"] = .string(requestId) }
            return .object(o)
        case .played(let seq): return ["t": "played", "seq": .number(Double(seq))]
        case .listSessions: return ["t": "list_sessions"]
        case .openSession(let id): return ["t": "open_session", "sessionId": .string(id)]
        }
    }

    public var text: String { json.jsonString }
}
