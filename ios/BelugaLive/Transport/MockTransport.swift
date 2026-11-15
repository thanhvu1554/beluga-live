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
    private var voice = "5yEPTrA0uAhZsBEhHjfi"
    private var backupLastAt = "2026-11-16T03:00:12Z"
    private var backupBytes = 18_400_000
    private var backupFrame: JSONValue {
        ["t": "backup", "enabled": true, "provider": "r2", "lastAt": .string(backupLastAt),
         "lastBytes": .number(Double(backupBytes)), "nextAt": "2026-11-17T03:00:00Z"]
    }

    init(script: MockScript = .load()) { self.script = script }

    func connect() {
        guard reachable else {
            Task { @MainActor in self.onEvent?(.closed(reason: "mock offline")) }
            return
        }
    }

    func disconnect() { playTask?.cancel(); playTask = nil }

    func send(_ message: ClientMessage) {
        // An unreachable server hears nothing.
        guard reachable else { return }
        switch message {
        case let .hello(_, _, _, sid, _, settings, fid):
            mode = settings.mode
            if let sid, !sid.isEmpty { sessionId = sid }
            folderId = fid
            deliver(ready())
            if UserDefaults.standard.bool(forKey: "mockOffer"), mode == .chat { offerReview() }
        case .settings(let s):
            mode = s.mode
            if let v = s.voice { voice = v; deliver(MockData.voices(current: voice)) }
        case .getVoices:
            deliver(MockData.voices(current: voice))
        case let .userText(text, files, draft, _):
            if draft { deliver(["t": "heard", "text": .string(Self.tidy(text)), "raw": .string(text), "draft": true]); return }
            if mode == .live { echoUser(text, files: files, via: "voice"); playNextTurn() }
            else if UserDefaults.standard.bool(forKey: "mockRateLimit") { rateLimited(text) }
            else { playChat(text, files: files) }
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
        case let .createFolder(name, about, subject, clientId):
            let id = "f-new-\(folders.count)"
            let kind = subject ?? Self.kind(name + " " + (about ?? ""))
            folders.insert(["id": .string(id), "name": .string(name), "subject": .string(kind), "kind": .string(kind),
                            "about": .string(about ?? ""), "sessionCount": 0,
                            "updatedAt": "2026-11-15T02:00:00Z", "hasKnowledge": false], at: 0)
            deliver(["t": "folder_created", "id": .string(id), "clientId": clientId.map(JSONValue.string) ?? .null])
            deliver(foldersFrame)
        case let .renameFolder(id, name):
            folders = folders.map { $0["id"]?.string == id ? Self.set($0, "name", .string(name)) : $0 }
            deliver(foldersFrame)
        case let .updateFolder(id, name, about):
            folders = folders.map { f in
                guard f["id"]?.string == id else { return f }
                var out = f
                if let name { out = Self.set(out, "name", .string(name)) }
                if let about { out = Self.set(out, "about", .string(about)) }
                let kind = Self.kind((out["name"]?.string ?? "") + " " + (out["about"]?.string ?? ""))
                return Self.set(Self.set(out, "kind", .string(kind)), "subject", .string(kind))
            }
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
        case let .getGreeting(_, _, fid):
            // A v3 server names the folder when its second line talks about review (section 18).
            var g = MockData.greeting
            let target = fid ?? "f-toan"
            let name = folders.first { $0["id"]?.string == target }?["name"]?.string ?? ""
            let count = target == "f-toan" ? 2 : (target == "f-anh" ? 1 : 0)
            g = Self.set(g, "sub", count > 0 ? .string("Có \(count) mục tới hạn ôn trong \(name)") : .null)
            deliver(g)
        case .getHome(let fid):
            deliver(home(fid))
        case .getBackup:
            deliver(backupFrame)
        case .backupNow:
            // The server answers at once with the current state, then again when the new backup is up.
            deliver(backupFrame)
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(2.2))
                guard let self else { return }
                let f = ISO8601DateFormatter()
                self.backupLastAt = f.string(from: Date())
                self.backupBytes += 41_000
                self.deliver(self.backupFrame)
            }
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

    /// `home` as the v3 server builds it (PROTOCOL section 18): scoped to `fid` when given, else the root with every
    /// folder card carrying its folder's name.
    private func home(_ fid: String?) -> JSONValue {
        let due: [String: [String]] = ["f-toan": ["Rút gọn biểu thức chứa căn", "Trục căn thức ở mẫu"], "f-anh": ["Câu bị động"]]
        func name(_ id: String) -> String { folders.first { $0["id"]?.string == id }?["name"]?.string ?? "" }
        func kind(_ id: String) -> String { folders.first { $0["id"]?.string == id }?["kind"]?.string ?? "chung" }
        let scoped = sessions.filter { fid == nil || $0["folderId"]?.string == fid }
        let recent = scoped.max { ($0["updatedAt"]?.string ?? "") < ($1["updatedAt"]?.string ?? "") }
        var o: [String: JSONValue] = ["t": "home", "quick": []]
        if let fid { o["folderId"] = .string(fid) }
        if let recent, let sid = recent["sessionId"]?.string {
            var c: [String: JSONValue] = ["sessionId": .string(sid), "title": recent["title"] ?? "", "updatedAt": recent["updatedAt"] ?? ""]
            if let f = recent["folderId"]?.string { c["folderId"] = .string(f); c["folderName"] = .string(name(f)) }
            o["continue"] = .object(c)
        }
        let dueIds = due.keys.sorted { (due[$0]?.count ?? 0, $1) > (due[$1]?.count ?? 0, $0) }
            .filter { fid == nil || $0 == fid }
            .filter { id in folders.contains { $0["id"]?.string == id } }
        o["due"] = .array(dueIds.map { id in
            ["folderId": .string(id), "folderName": .string(name(id)), "kind": .string(kind(id)),
             "count": .number(Double(due[id]?.count ?? 0)), "topics": .array((due[id] ?? []).map(JSONValue.string))]
        })
        return .object(o)
    }
    private var sessionsFrame: JSONValue { ["t": "sessions", "items": .array(sessions)] }

    private func ready() -> JSONValue {
        var o = script.ready.object ?? [:]
        o["v"] = 3
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

    /// What the server's utility model does with a folder's name and about: a teaching kind (icon only).
    static func kind(_ text: String) -> String {
        let t = text.lowercased()
        let table: [(String, [String])] = [("toan", ["toán", "đại", "hình học"]), ("ly", ["lý", "vật lí", "vật lý"]),
                                           ("hoa", ["hóa", "hoá"]), ("van", ["văn"]), ("anh-ssat", ["ssat"]),
                                           ("duc", ["đức"]), ("tin-cpp", ["tin", "c++", "lập trình"]),
                                           ("ngoai-ngu", ["anh", "english", "nhật", "trung", "pháp", "hàn"])]
        return table.first { _, words in words.contains { t.contains($0) } }?.0 ?? "chung"
    }

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

    /// `-mockOffer 1`: what the real server pushes right after `ready` in a new chat session when a review is due.
    private func offerReview() {
        turn += 1
        let t = JSONValue.number(Double(turn))
        deliver(["t": "chat", "seq": .number(Double(nextSeq())), "turn": t,
                 "md": "Lần trước cậu hay nhầm hằng số bù trong hàm từng khúc, ôn một câu không?"])
        deliver(["t": "chat", "seq": .number(Double(nextSeq())), "turn": t, "md": "### Ôn lại"])
        deliver(["t": "show", "seq": .number(Double(nextSeq())), "turn": t, "step": -1,
                 "block": ["type": "review_offer", "id": "b3", "topic": "Hằng số bù trong hàm từng khúc", "subject": "Toán",
                           "lastSeen": "2026-11-14", "note": "Lúng túng khi thấy số 2.200 trong nhánh giữa.",
                           "fallback": "**Ôn lại** · Toán · Hằng số bù trong hàm từng khúc"]])
        deliver(["t": "turn_end", "seq": .number(Double(nextSeq())), "turn": t])
    }

    /// `-mockRateLimit 1`: the plan's limit is reached, as the real server answers it (echo, error, empty turn).
    private func rateLimited(_ text: String) {
        echoUser(text, files: [], via: "text")
        let t = turn
        playTask?.cancel()
        playTask = Task { [weak self] in
            self?.deliver(["t": "state", "state": "thinking"])
            try? await Task.sleep(for: .milliseconds(1600))
            guard let self, !Task.isCancelled else { return }
            self.deliver(["t": "error", "code": "rate_limited", "text": "Hết hạn mức gói Claude, thử lại sau.", "fatal": false])
            self.deliver(["t": "turn_end", "seq": .number(Double(self.nextSeq())), "turn": .number(Double(t))])
            self.deliver(["t": "state", "state": "idle"])
        }
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
            // `-mockSlowThink 1`: the tutor thinks five seconds before its first frame (wait clips).
            var slow = UserDefaults.standard.bool(forKey: "mockSlowThink") ? 5000 : 0
            for f in lesson.frames {
                try? await Task.sleep(for: .milliseconds(f.after + slow))
                slow = 0
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
        let events = sessionEvents(item)
        seq = events.count
        turn = 1
        deliver(["t": "snapshot", "events": .array(events)])
    }

    /// The journal of a stored session: the scripted lesson for a live one, the scripted chat otherwise.
    private func sessionEvents(_ item: JSONValue?) -> [JSONValue] {
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
        return events
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

// MARK: - Replica sync (PROTOCOL section 19)

/// The mock answers `/sync/*` from its own state, with etags that change when the data does.
extension MockTransport: SyncSource {
    func fetch(_ path: String, etag: String?) async -> SyncFetch {
        try? await Task.sleep(for: .milliseconds(60))
        guard reachable else { return .failed("mock offline") }
        guard let body = syncBody(path) else { return .failed("HTTP 404") }
        let text = body.jsonString
        let tag = Self.hash(text)
        if path != "manifest", etag == tag { return .notModified }
        return .body(Data(text.utf8), etag: tag)
    }

    private func syncBody(_ path: String) -> JSONValue? {
        if path == "manifest" {
            let fs: [JSONValue] = folders.compactMap { f in
                guard let id = f["id"]?.string, let b = syncBody("folder/" + id) else { return nil }
                return ["id": .string(id), "etag": .string(Self.hash(b.jsonString))]
            }
            let ss: [JSONValue] = sessions.compactMap { s in
                guard let id = s["sessionId"]?.string, let b = syncBody("session/" + id) else { return nil }
                var o: [String: JSONValue] = ["sessionId": .string(id), "etag": .string(Self.hash(b.jsonString))]
                if let f = s["folderId"]?.string { o["folderId"] = .string(f) }
                return .object(o)
            }
            return ["v": 1, "serverTime": "2026-11-16T02:00:00Z", "profile": ["etag": .string(Self.hash(script.progress.jsonString))],
                    "folders": .array(fs), "sessions": .array(ss), "voices": ["etag": "v1"]]
        }
        if path == "profile" { return ["md": "", "progress": script.progress] }
        if path.hasPrefix("folder/") {
            let id = String(path.dropFirst(7))
            guard let f = folders.first(where: { $0["id"]?.string == id }) else { return nil }
            let list: [JSONValue] = id == "f-toan" ? exams.map { Self.set($0, "html", .string(MockData.examHTML(title: $0["title"]?.string ?? ""))) } : []
            return ["folder": f, "knowledge": hasKnowledge.contains(id) ? Self.knowledge(for: id) : .null,
                    "review": MockData.review(folderId: id, weekly: false), "exams": .array(list), "memoryMd": ""]
        }
        if path.hasPrefix("session/") {
            let id = String(path.dropFirst(8))
            guard let s = sessions.first(where: { $0["sessionId"]?.string == id }) else { return nil }
            var o = s.object ?? [:]
            o["preview"] = nil
            o["events"] = .array(sessionEvents(s))
            return .object(o)
        }
        return nil
    }

    /// FNV-1a, enough to tell versions apart.
    static func hash(_ s: String) -> String {
        var h: UInt64 = 0xcbf2_9ce4_8422_2325
        for b in s.utf8 { h = (h ^ UInt64(b)) &* 0x100_0000_01b3 }
        return String(h, radix: 16)
    }
}
