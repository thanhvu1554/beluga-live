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

/// Plays the server for `-mock 1`: the bundled live lesson script, plus a scripted chat, folders, knowledge
/// ingest, review, exam generation, usage and greeting (protocol v2).
@MainActor
final class MockTransport: Transport {
    var onEvent: ((TransportEvent) -> Void)?
    let script: MockScript
    private var turnIndex = 0
    private var playTask: Task<Void, Never>?
    private var jobTask: Task<Void, Never>?
    private var examTask: Task<Void, Never>?
    private var photoWaiter: CheckedContinuation<Void, Never>?
    /// When false the transport stays "offline" (for the offline scene).
    var reachable = true

    private var mode: SessionMode = .chat
    private var sessionId = "mock-0010"
    private var folderId: String?
    private var seq = 0
    private var turn = 0
    private var newCount = 0
    private var folders: [JSONValue] = MockData.folders
    private var sessions: [JSONValue] = MockData.sessions
    private var exams: [JSONValue] = MockData.exams
    private var hasKnowledge: Set<String> = ["f-toan"]

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
        case let .hello(_, _, _, sid, _, settings, fid):
            mode = settings.mode
            if let sid, !sid.isEmpty { sessionId = sid }
            folderId = fid
            deliver(ready())
        case .settings(let s):
            mode = s.mode
        case let .userText(text, files, draft):
            if draft { deliver(["t": "heard", "text": .string(Self.tidy(text)), "raw": .string(text), "draft": true]); return }
            if mode == .live { echoUser(text, files: files, via: "voice"); playNextTurn() } else { playChat(text, files: files) }
        case .audioEnd:
            if mode == .live { playNextTurn() }
        case .photo:
            photoWaiter?.resume(); photoWaiter = nil
        case .interrupt:
            playTask?.cancel(); playTask = nil
            photoWaiter?.resume(); photoWaiter = nil
        case .listSessions:
            deliver(["t": "sessions", "items": .array(sessions)])
        case .getProgress:
            deliver(script.progress)
        case .openSession(let id):
            open(id)
        case .listFolders:
            deliver(foldersFrame)
        case let .createFolder(name, subject):
            let id = "f-new-\(folders.count)"
            folders.insert(["id": .string(id), "name": .string(name), "subject": .string(subject), "sessionCount": 0,
                            "updatedAt": "2026-11-15T02:00:00Z", "hasKnowledge": false], at: 0)
            deliver(["t": "folder_created", "id": .string(id)])
            deliver(foldersFrame)
        case let .renameFolder(id, name):
            folders = folders.map { $0["id"]?.string == id ? Self.set($0, "name", .string(name)) : $0 }
            deliver(foldersFrame)
        case .deleteFolder(let id):
            folders.removeAll { $0["id"]?.string == id }
            sessions = sessions.map { $0["folderId"]?.string == id ? Self.set($0, "folderId", .null) : $0 }
            deliver(foldersFrame); deliver(sessionsFrame)
        case let .moveSession(sid, fid):
            sessions = sessions.map { $0["sessionId"]?.string == sid ? Self.set($0, "folderId", fid.map(JSONValue.string) ?? .null) : $0 }
            deliver(sessionsFrame)
        case let .renameSession(sid, title):
            sessions = sessions.map { $0["sessionId"]?.string == sid ? Self.set($0, "title", .string(title)) : $0 }
            deliver(sessionsFrame)
        case .deleteSession(let sid):
            sessions.removeAll { $0["sessionId"]?.string == sid }
            deliver(sessionsFrame)
        case let .newSession(fid, m, kickoff):
            newCount += 1
            sessionId = "mock-new-\(newCount)"
            folderId = fid
            if let m { mode = m }
            seq = 0; turn = 0; turnIndex = 0
            deliver(ready())
            if kickoff == "review_quiz" { playQuiz() }
        case let .ingestStart(fid, files, _):
            simulateIngest(folderId: fid, pages: max(6, min(24, files.count * 2)))
        case .ingestCancel(let fid):
            jobTask?.cancel()
            deliver(["t": "ingest", "folderId": .string(fid), "jobId": "j-mock", "state": "cancelled", "done": 0, "total": 0, "note": ""])
        case .getKnowledge(let fid):
            deliver(["t": "knowledge", "folderId": .string(fid), "data": hasKnowledge.contains(fid) ? Self.knowledge(for: fid) : .null])
        case .getReview(let fid):
            deliver(MockData.review(folderId: fid, weekly: false))
        case .getWeekly(let fid):
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(1.2))
                self?.deliver(MockData.review(folderId: fid, weekly: true))
            }
        case let .genExam(fid, structureId, _):
            simulateExam(folderId: fid, structureId: structureId)
        case .listExams(let fid):
            deliver(["t": "exams", "folderId": .string(fid), "items": .array(fid == "f-toan" ? exams : [])])
        case .deleteExam(let id):
            exams.removeAll { $0["examId"]?.string == id }
            deliver(["t": "exams", "folderId": "f-toan", "items": .array(exams)])
        case .getUsage:
            deliver(MockData.usage)
        case .getGreeting:
            deliver(MockData.greeting)
        default:
            break
        }
    }

    func sendBinary(_ data: Data) {}

    /// The scripted user line for the next turn (what the fake student "says").
    var nextUserLine: String? { turnIndex < script.turns.count ? script.turns[turnIndex].user : nil }

    // MARK: Frames

    private func deliver(_ v: JSONValue) {
        onEvent?(.message(ServerMessage.decode(v)))
    }

    private var foldersFrame: JSONValue { ["t": "folders", "items": .array(folders)] }
    private var sessionsFrame: JSONValue { ["t": "sessions", "items": .array(sessions)] }

    private func ready() -> JSONValue {
        var o = script.ready.object ?? [:]
        o["v"] = 2
        o["sessionId"] = .string(sessionId)
        o["mode"] = .string(mode.rawValue)
        o["models"] = MockData.models
        o["model"] = "sonnet"
        o["effort"] = "medium"
        o["boardVersion"] = 0
        if let folderId { o["folderId"] = .string(folderId) }
        let title = sessions.first { $0["sessionId"]?.string == sessionId }?["title"]?.string
        o["title"] = .string(title ?? "")
        return .object(o)
    }

    private static func set(_ v: JSONValue, _ key: String, _ value: JSONValue) -> JSONValue {
        guard case .object(var o) = v else { return v }
        o[key] = value
        return .object(o)
    }

    private static func knowledge(for fid: String) -> JSONValue { set(MockData.knowledge, "folderId", .string(fid)) }

    /// What the cheap STT-fix model would do to a dictated line.
    private static func tidy(_ text: String) -> String {
        text.replacingOccurrences(of: "x binh phuong", with: "x²")
    }

    private func nextSeq() -> Int { seq += 1; return seq }

    private func echoUser(_ text: String, files: [String], via: String) {
        turn += 1
        var o: [String: JSONValue] = ["t": "user", "seq": .number(Double(nextSeq())), "turn": .number(Double(turn)),
                                      "text": .string(text), "via": .string(via)]
        if !files.isEmpty {
            o["files"] = .array(files.enumerated().map { i, id in
                ["fileId": .string(id), "kind": "image", "name": .string("anh-\(i + 1).jpg")]
            })
        }
        deliver(.object(o))
    }

    // MARK: Chat

    /// User echo, thinking, streamed markdown with math, an inline block, the rest, turn_end.
    private func playChat(_ text: String, files: [String]) {
        echoUser(text, files: files, via: "text")
        streamTurn(intro: nil, answer: MockData.chatAnswer, block: MockData.chatBlock, outro: MockData.chatAnswer2)
    }

    private func playQuiz() {
        turn += 1
        streamTurn(intro: nil, answer: MockData.quizQuestion, block: nil, outro: nil)
    }

    private func streamTurn(intro: String?, answer: String, block: JSONValue?, outro: String?) {
        let t = turn
        playTask?.cancel()
        playTask = Task { [weak self] in
            self?.deliver(["t": "state", "state": "thinking"])
            try? await Task.sleep(for: .milliseconds(900))
            for part in [answer, outro].compactMap({ $0 }) {
                for piece in Self.pieces(part) {
                    guard let self, !Task.isCancelled else { return }
                    self.deliver(["t": "chat_delta", "turn": .number(Double(t)), "text": .string(piece)])
                    try? await Task.sleep(for: .milliseconds(55))
                }
                guard let self, !Task.isCancelled else { return }
                self.deliver(["t": "chat", "seq": .number(Double(self.nextSeq())), "turn": .number(Double(t)), "md": .string(part)])
                if part == answer, let block {
                    self.deliver(["t": "show", "seq": .number(Double(self.nextSeq())), "turn": .number(Double(t)), "step": -1, "block": block])
                }
                try? await Task.sleep(for: .milliseconds(300))
            }
            guard let self, !Task.isCancelled else { return }
            self.deliver(["t": "turn_end", "seq": .number(Double(self.nextSeq())), "turn": .number(Double(t))])
            self.deliver(["t": "state", "state": "idle"])
            if t == 1 { self.deliver(["t": "title", "text": "Toán · Phương trình bậc hai · 15/11"]) }
        }
    }

    /// Splits text into 6-14 character pieces, like a token stream.
    private static func pieces(_ s: String) -> [String] {
        var out: [String] = []
        var current = ""
        for ch in s {
            current.append(ch)
            if current.count >= 6 + (out.count % 9) { out.append(current); current = "" }
        }
        if !current.isEmpty { out.append(current) }
        return out
    }

    // MARK: Live lesson

    private func playNextTurn() {
        guard turnIndex < script.turns.count else { return }
        let lesson = script.turns[turnIndex]
        turnIndex += 1
        // Re-number the script so lesson events follow whatever this session already had (chat turns included).
        let turnNo = turn
        playTask?.cancel()
        playTask = Task { [weak self] in
            var map: [Int: Int] = [:]
            for f in lesson.frames {
                try? await Task.sleep(for: .milliseconds(f.after))
                guard let self, !Task.isCancelled else { return }
                var frame = f.frame
                if case .object(var o) = frame {
                    if let old = o["seq"]?.int {
                        let new = map[old] ?? self.nextSeq()
                        map[old] = new
                        o["seq"] = .number(Double(new))
                    }
                    if o["turn"] != nil { o["turn"] = .number(Double(turnNo)) }
                    frame = .object(o)
                }
                self.deliver(frame)
                if f.frame["t"]?.string == "capture_request" {
                    await withCheckedContinuation { self.photoWaiter = $0 }
                }
            }
        }
    }

    /// Reopened session: `ready` then a `snapshot` of its history (lesson or chat).
    private func open(_ id: String) {
        sessionId = id
        let item = sessions.first { $0["sessionId"]?.string == id }
        folderId = item?["folderId"]?.string
        deliver(ready())
        var events: [JSONValue] = []
        if item?["mode"]?.string == "live" {
            events.append(["t": "user", "seq": 1, "turn": 1, "text": .string(script.turns.first?.user ?? ""), "via": "voice"])
            var n = 1
            for e in script.allEvents {
                n += 1
                var o = e.raw.object ?? [:]
                o["seq"] = .number(Double(n))
                o["turn"] = 1
                events.append(.object(o))
            }
        } else {
            events = [["t": "user", "seq": 1, "turn": 1, "text": .string(MockData.chatQuestion), "via": "text"],
                      ["t": "chat", "seq": 2, "turn": 1, "md": .string(MockData.chatAnswer)],
                      ["t": "show", "seq": 3, "turn": 1, "step": -1, "block": MockData.chatBlock],
                      ["t": "chat", "seq": 4, "turn": 1, "md": .string(MockData.chatAnswer2)],
                      ["t": "turn_end", "seq": 5, "turn": 1]]
        }
        seq = events.count
        turn = 1
        deliver(["t": "snapshot", "events": .array(events)])
    }

    // MARK: Knowledge ingest and exams

    private func simulateIngest(folderId fid: String, pages: Int) {
        jobTask?.cancel()
        jobTask = Task { [weak self] in
            func frame(_ state: String, _ done: Int, _ note: String, found: (Int, Int)? = nil) -> JSONValue {
                var o: [String: JSONValue] = ["t": "ingest", "folderId": .string(fid), "jobId": "j-mock", "state": .string(state),
                                              "done": .number(Double(done)), "total": .number(Double(pages)), "note": .string(note)]
                if let found { o["found"] = ["questions": .number(Double(found.0)), "topics": .number(Double(found.1))] }
                return .object(o)
            }
            self?.deliver(frame("queued", 0, "Đang xếp hàng"))
            try? await Task.sleep(for: .milliseconds(500))
            for p in 1...pages {
                guard let self, !Task.isCancelled else { return }
                self.deliver(frame("reading", p, "Đang đọc trang \(p)/\(pages): Đề ôn giữa kỳ số \(1 + p / 3)", found: (p * 5, min(9, 1 + p / 2))))
                try? await Task.sleep(for: .milliseconds(400))
            }
            guard let self, !Task.isCancelled else { return }
            self.deliver(frame("merging", pages, "Đang gộp vào giáo trình của thư mục", found: (pages * 5, 9)))
            try? await Task.sleep(for: .milliseconds(900))
            guard !Task.isCancelled else { return }
            self.hasKnowledge.insert(fid)
            self.folders = self.folders.map { $0["id"]?.string == fid ? Self.set($0, "hasKnowledge", true) : $0 }
            var done = frame("done", pages, "Xong: \(pages * 5) câu, 9 chủ đề", found: (pages * 5, 9))
            done = Self.set(done, "costUsd", 0.05)
            self.deliver(done)
            self.deliver(["t": "knowledge", "folderId": .string(fid), "data": Self.knowledge(for: fid)])
            self.deliver(self.foldersFrame)
        }
    }

    private func simulateExam(folderId fid: String, structureId: String) {
        let examId = "e-\(exams.count + 1)"
        let title = structureId == "s2" ? "Kiểm tra 45 phút · Lần \(exams.count + 1)" : "Đề giữa kỳ 1 · Lần \(exams.count + 1)"
        examTask?.cancel()
        examTask = Task { [weak self] in
            func frame(_ state: String, _ note: String) -> JSONValue {
                ["t": "exam", "folderId": .string(fid), "examId": .string(examId), "state": .string(state), "note": .string(note), "title": .string(title)]
            }
            self?.deliver(frame("writing", "Đang viết đề theo ma trận: 12 câu trắc nghiệm, 5 câu tự luận"))
            try? await Task.sleep(for: .seconds(2))
            self?.deliver(frame("checking", "Đang giải thử và soát đáp án"))
            try? await Task.sleep(for: .seconds(1.6))
            guard let self, !Task.isCancelled else { return }
            self.exams.insert(["examId": .string(examId), "title": .string(title), "structureId": .string(structureId),
                               "createdAt": "2026-11-15T02:10:00Z", "questions": 17], at: 0)
            self.deliver(frame("done", "Xong"))
            self.deliver(["t": "exams", "folderId": .string(fid), "items": .array(self.exams)])
        }
    }
}
