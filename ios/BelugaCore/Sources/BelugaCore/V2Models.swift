import Foundation

// Models for protocol v2 frames (docs/PROTOCOL.md sections 8-15). Every parser is tolerant:
// missing or mistyped fields become defaults, unknown fields are ignored.

// MARK: - Models (section 10)

public struct ModelInfo: Sendable, Equatable, Identifiable, Hashable {
    public var id: String
    public var label: String
    /// "claude" (subscription, usage tracked) or "anthropic-compatible".
    public var provider: String
    public var efforts: [String]
    public var defaultEffort: String?
    public var vision: Bool
    public var tracked: Bool

    public init(id: String, label: String, provider: String = "claude", efforts: [String] = [],
                defaultEffort: String? = nil, vision: Bool = true, tracked: Bool = false) {
        self.id = id; self.label = label; self.provider = provider; self.efforts = efforts
        self.defaultEffort = defaultEffort; self.vision = vision; self.tracked = tracked
    }

    static func from(_ v: JSONValue) -> ModelInfo? {
        guard let id = v["id"]?.nonEmptyString else { return nil }
        return ModelInfo(id: id, label: v["label"]?.nonEmptyString ?? id, provider: v["provider"]?.string ?? "claude",
                         efforts: (v["efforts"]?.strings ?? []).uniqued { $0 }, defaultEffort: v["defaultEffort"]?.nonEmptyString,
                         vision: v["vision"]?.bool ?? true, tracked: v["tracked"]?.bool ?? false)
    }

    /// Used before the server sent `ready.models` (v1 server or offline).
    public static let fallback: [ModelInfo] = [
        ModelInfo(id: "sonnet", label: "Claude Sonnet", efforts: ["low", "medium", "high"], defaultEffort: "medium", tracked: true),
        ModelInfo(id: "opus", label: "Claude Opus", efforts: ["low", "medium", "high"], defaultEffort: "low", tracked: true),
        ModelInfo(id: "haiku", label: "Claude Haiku", tracked: true),
    ]
}

// MARK: - Folders (section 8)

public struct Folder: Sendable, Equatable, Identifiable, Hashable {
    public var id: String
    public var name: String
    /// Subject skill id: toan, ly, hoa, van, anh-ssat, duc, tin-cpp, chung.
    public var subject: String
    public var sessionCount: Int
    public var updatedAt: String
    public var hasKnowledge: Bool
    public var ingest: IngestStatus?

    public init(id: String, name: String, subject: String = "chung", sessionCount: Int = 0, updatedAt: String = "",
                hasKnowledge: Bool = false, ingest: IngestStatus? = nil) {
        self.id = id; self.name = name; self.subject = subject; self.sessionCount = sessionCount
        self.updatedAt = updatedAt; self.hasKnowledge = hasKnowledge; self.ingest = ingest
    }

    static func from(_ v: JSONValue) -> Folder? {
        guard let id = v["id"]?.nonEmptyString else { return nil }
        let ingest = v["ingest"].flatMap { $0.object == nil ? nil : IngestStatus.from($0) }
        return Folder(id: id, name: v["name"]?.string ?? "", subject: v["subject"]?.nonEmptyString ?? "chung",
                      sessionCount: v["sessionCount"]?.integer ?? 0, updatedAt: v["updatedAt"]?.dateText ?? "",
                      hasKnowledge: v["hasKnowledge"]?.bool ?? false, ingest: ingest)
    }

    /// Subject ids the server knows (tutor skills).
    public static let subjects = ["toan", "ly", "hoa", "van", "anh-ssat", "duc", "tin-cpp", "chung"]
}

// MARK: - Knowledge ingest (section 11)

public struct IngestStatus: Sendable, Equatable, Hashable {
    public enum State: String, Sendable { case queued, reading, merging, done, error, cancelled }
    public var folderId: String
    public var jobId: String
    public var state: State
    public var done: Int
    public var total: Int
    public var note: String
    public var foundQuestions: Int?
    public var foundTopics: Int?
    public var costUsd: Double?

    public init(folderId: String, jobId: String = "", state: State = .queued, done: Int = 0, total: Int = 0, note: String = "",
                foundQuestions: Int? = nil, foundTopics: Int? = nil, costUsd: Double? = nil) {
        self.folderId = folderId; self.jobId = jobId; self.state = state; self.done = done; self.total = total
        self.note = note; self.foundQuestions = foundQuestions; self.foundTopics = foundTopics; self.costUsd = costUsd
    }

    static func from(_ v: JSONValue) -> IngestStatus {
        IngestStatus(folderId: v["folderId"]?.string ?? "", jobId: v["jobId"]?.string ?? "",
                     state: v["state"]?.string.flatMap(State.init(rawValue:)) ?? .reading,
                     done: v["done"]?.integer ?? 0, total: v["total"]?.integer ?? 0,
                     note: v["note"]?.string ?? "",
                     foundQuestions: v["found"]?["questions"]?.integer,
                     foundTopics: v["found"]?["topics"]?.integer,
                     costUsd: v["costUsd"]?.numeric)
    }

    /// Still running (queued, reading or merging).
    public var isActive: Bool { state == .queued || state == .reading || state == .merging }
    /// 0...1, or nil while the total is unknown.
    public var fraction: Double? {
        guard total > 0 else { return state == .done ? 1 : nil }
        return min(1, max(0, Double(done) / Double(total)))
    }

    /// The raw frame for `knowledge.progress(event)`.
    public var json: JSONValue {
        var o: [String: JSONValue] = ["t": "ingest", "folderId": .string(folderId), "jobId": .string(jobId),
                                      "state": .string(state.rawValue), "done": .number(Double(done)),
                                      "total": .number(Double(total)), "note": .string(note)]
        if foundQuestions != nil || foundTopics != nil {
            o["found"] = ["questions": .number(Double(foundQuestions ?? 0)), "topics": .number(Double(foundTopics ?? 0))]
        }
        if let costUsd { o["costUsd"] = .number(costUsd) }
        return .object(o)
    }
}

/// `knowledge.json` of a folder. The page `knowledge.html` gets the raw data; the app reads a summary
/// (for its native fallback and for the "Tạo đề" choices).
public struct Knowledge: Sendable, Equatable {
    public struct Topic: Sendable, Equatable, Identifiable {
        public var id: String
        public var title: String
        public var count: Int
        public var share: Double
        public var forms: Int
    }
    public struct Chapter: Sendable, Equatable, Identifiable {
        public var id: String
        public var title: String
        public var topics: [Topic]
    }
    public struct Structure: Sendable, Equatable, Identifiable {
        public var id: String
        public var title: String
        public var durationMin: Int?
        public var totalPoints: Double?
        public var sections: [String]
        public var questions: Int
    }

    public var raw: JSONValue
    public var subject: String
    public var grade: String?
    public var goal: String?
    public var updatedAt: String?
    public var sources: Int
    public var questions: Int
    /// nb, th, vd, vdc counts.
    public var byLevel: [String: Int]
    public var chapters: [Chapter]
    public var structures: [Structure]
    public var currentTopicId: String?
    public var coveredTopicIds: [String]
    public var progressNote: String?

    public static func from(_ v: JSONValue) -> Knowledge? {
        guard v.object != nil else { return nil }
        let chapters = (v["curriculum"]?.array ?? []).compactMap { c -> Chapter? in
            guard let id = c["id"]?.string else { return nil }
            let topics = (c["topics"]?.array ?? []).compactMap { t -> Topic? in
                guard let tid = t["id"]?.string else { return nil }
                return Topic(id: tid, title: t["title"]?.string ?? tid, count: t["count"]?.integer ?? 0,
                             share: t["share"]?.numeric ?? 0, forms: t["forms"]?.array?.count ?? 0)
            }
            return Chapter(id: id, title: c["title"]?.string ?? id, topics: topics.uniquedByID())
        }.uniquedByID()
        let structures = (v["structures"]?.array ?? []).compactMap { s -> Structure? in
            guard let id = s["id"]?.string else { return nil }
            let sections = s["sections"]?.array ?? []
            let count = sections.reduce(0) { $0 + ($1["count"]?.integer ?? 0) }
            return Structure(id: id, title: s["title"]?.string ?? id, durationMin: s["durationMin"]?.integer,
                             totalPoints: s["totalPoints"]?.numeric, sections: sections.compactMap { $0["title"]?.string },
                             questions: count)
        }.uniquedByID()
        var byLevel: [String: Int] = [:]
        for (k, n) in v["stats"]?["byLevel"]?.object ?? [:] { byLevel[k] = n.integer ?? 0 }
        let sp = v["schoolProgress"]
        return Knowledge(raw: v, subject: v["subject"]?.string ?? "chung", grade: v["grade"]?.dateText, goal: v["goal"]?.nonEmptyString,
                         updatedAt: v["updatedAt"]?.dateText, sources: v["sources"]?.array?.count ?? 0,
                         questions: v["stats"]?["questions"]?.integer ?? 0, byLevel: byLevel,
                         chapters: chapters, structures: structures, currentTopicId: sp?["currentTopicId"]?.nonEmptyString,
                         coveredTopicIds: sp?["coveredTopicIds"]?.strings ?? [], progressNote: sp?["note"]?.nonEmptyString)
    }

    public func topicTitle(_ id: String) -> String? {
        for c in chapters { for t in c.topics where t.id == id { return t.title } }
        return nil
    }
}

/// The `knowledge` frame.
public struct FolderKnowledge: Sendable, Equatable {
    public var folderId: String
    public var data: Knowledge?
    public var ingest: IngestStatus?

    public init(folderId: String, data: Knowledge?, ingest: IngestStatus? = nil) {
        self.folderId = folderId; self.data = data; self.ingest = ingest
    }

    static func from(_ v: JSONValue) -> FolderKnowledge {
        let folderId = v["folderId"]?.string ?? ""
        var ingest: IngestStatus?
        if let i = v["ingest"], i.object != nil {
            ingest = IngestStatus.from(i)
            if ingest?.folderId.isEmpty == true { ingest?.folderId = folderId }
        }
        return FolderKnowledge(folderId: folderId, data: v["data"].flatMap(Knowledge.from), ingest: ingest)
    }
}

// MARK: - Review (section 12)

public struct FolderReview: Sendable, Equatable {
    public struct Topic: Sendable, Equatable, Identifiable {
        public enum State: String, Sendable { case new, learning, weak, solid }
        public var topicId: String
        public var title: String
        public var chapter: String
        public var state: State
        public var wrong: Int
        public var right: Int
        public var lastSeen: String?
        public var id: String { topicId }
    }
    public struct Weekly: Sendable, Equatable {
        public var from: String
        public var to: String
        public var md: String
    }
    public struct Lesson: Sendable, Equatable, Identifiable {
        public var sessionId: String
        public var title: String
        public var updatedAt: String
        public var mode: SessionMode
        public var steps: Int
        public var id: String { sessionId }
    }

    public var folderId: String
    public var topics: [Topic]
    public var weak: [WeakTopic]
    public var due: [WeakTopic]
    public var weekly: Weekly?
    public var sessions: [Lesson]
    public var updatedAt: String?
    /// The frame as received, for `knowledge.render(data, review)`.
    public var raw: JSONValue = .null

    public init(folderId: String, topics: [Topic] = [], weak: [WeakTopic] = [], due: [WeakTopic] = [],
                weekly: Weekly? = nil, sessions: [Lesson] = [], updatedAt: String? = nil) {
        self.folderId = folderId; self.topics = topics; self.weak = weak; self.due = due
        self.weekly = weekly; self.sessions = sessions; self.updatedAt = updatedAt
    }

    static func weakTopic(_ v: JSONValue) -> WeakTopic? {
        guard let topic = v["topic"]?.string else { return nil }
        return WeakTopic(topic: topic, note: v["note"]?.string ?? "", count: v["count"]?.integer ?? 0,
                         lastSeen: v["lastSeen"]?.dateText, due: v["due"]?.dateText)
    }

    static func from(_ v: JSONValue) -> FolderReview {
        let topics = (v["topics"]?.array ?? []).compactMap { t -> Topic? in
            guard let id = t["topicId"]?.string ?? t["title"]?.string else { return nil }
            return Topic(topicId: id, title: t["title"]?.string ?? id, chapter: t["chapter"]?.string ?? "",
                         state: t["state"]?.string.flatMap(Topic.State.init(rawValue:)) ?? .new,
                         wrong: t["wrong"]?.integer ?? 0, right: t["right"]?.integer ?? 0,
                         lastSeen: t["lastSeen"]?.dateText)
        }.uniquedByID()
        var weekly: Weekly?
        if let w = v["weekly"], let md = w["md"]?.string {
            weekly = Weekly(from: w["from"]?.dateText ?? "", to: w["to"]?.dateText ?? "", md: md)
        }
        let lessons = (v["sessions"]?.array ?? []).compactMap { s -> Lesson? in
            guard let id = s["sessionId"]?.nonEmptyString else { return nil }
            return Lesson(sessionId: id, title: s["title"]?.string ?? "", updatedAt: s["updatedAt"]?.dateText ?? "",
                          mode: s["mode"]?.string.flatMap(SessionMode.init(rawValue:)) ?? .live,
                          steps: s["steps"]?.integer ?? 0)
        }.uniquedByID()
        var review = FolderReview(folderId: v["folderId"]?.string ?? "", topics: topics,
                                  weak: (v["weak"]?.array ?? []).compactMap(weakTopic).uniquedByID(),
                                  due: (v["due"]?.array ?? []).compactMap(weakTopic).uniquedByID(),
                                  weekly: weekly, sessions: lessons, updatedAt: v["updatedAt"]?.dateText)
        review.raw = v
        return review
    }

    /// Topic counts per state, for the progress bar.
    public func count(_ state: Topic.State) -> Int { topics.filter { $0.state == state }.count }
}

// MARK: - Exams (section 13)

public struct ExamItem: Sendable, Equatable, Identifiable, Hashable {
    public var examId: String
    public var title: String
    public var structureId: String
    public var createdAt: String
    public var questions: Int
    public var id: String { examId }

    public init(examId: String, title: String, structureId: String = "", createdAt: String = "", questions: Int = 0) {
        self.examId = examId; self.title = title; self.structureId = structureId; self.createdAt = createdAt; self.questions = questions
    }

    static func from(_ v: JSONValue) -> ExamItem? {
        guard let id = v["examId"]?.nonEmptyString else { return nil }
        return ExamItem(examId: id, title: v["title"]?.string ?? "", structureId: v["structureId"]?.string ?? "",
                        createdAt: v["createdAt"]?.dateText ?? "", questions: v["questions"]?.integer ?? 0)
    }
}

public struct ExamStatus: Sendable, Equatable {
    public enum State: String, Sendable { case writing, checking, done, error }
    public var folderId: String
    public var examId: String
    public var state: State
    public var note: String
    public var title: String?

    public init(folderId: String, examId: String, state: State, note: String = "", title: String? = nil) {
        self.folderId = folderId; self.examId = examId; self.state = state; self.note = note; self.title = title
    }

    static func from(_ v: JSONValue) -> ExamStatus {
        ExamStatus(folderId: v["folderId"]?.string ?? "", examId: v["examId"]?.string ?? "",
                   state: v["state"]?.string.flatMap(State.init(rawValue:)) ?? .writing,
                   note: v["note"]?.string ?? "", title: v["title"]?.nonEmptyString)
    }

    public var isActive: Bool { state == .writing || state == .checking }
}

// MARK: - Usage (section 10)

public struct UsageBucket: Sendable, Equatable {
    public var inputTokens: Int
    public var outputTokens: Int
    public var cacheReadTokens: Int
    public var cacheWriteTokens: Int
    public var costUsd: Double
    public var turns: Int

    public init(inputTokens: Int = 0, outputTokens: Int = 0, cacheReadTokens: Int = 0, cacheWriteTokens: Int = 0,
                costUsd: Double = 0, turns: Int = 0) {
        self.inputTokens = inputTokens; self.outputTokens = outputTokens; self.cacheReadTokens = cacheReadTokens
        self.cacheWriteTokens = cacheWriteTokens; self.costUsd = costUsd; self.turns = turns
    }

    static func from(_ v: JSONValue?) -> UsageBucket {
        func i(_ k: String) -> Int { v?[k]?.integer ?? 0 }
        return UsageBucket(inputTokens: i("inputTokens"), outputTokens: i("outputTokens"), cacheReadTokens: i("cacheReadTokens"),
                           cacheWriteTokens: i("cacheWriteTokens"), costUsd: v?["costUsd"]?.numeric ?? 0, turns: i("turns"))
    }

    /// Every token counted (input, output and cache).
    public var totalTokens: Int { inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens }
}

public struct Usage: Sendable, Equatable {
    public struct Limit: Sendable, Equatable {
        public var status: String
        public var resetsAt: String?
        public var type: String?
        /// 0...1 of the window used, when the server knows it.
        public var utilization: Double?
    }
    public var session: UsageBucket
    public var today: UsageBucket
    public var week: UsageBucket
    public var limit: Limit?
    public var otherToday: UsageBucket
    public var ingestToday: Double
    public var ingestTotal: Double
    /// Exam generation (section 13), real money.
    public var examToday: Double = 0
    public var examTotal: Double = 0

    public init(session: UsageBucket = .init(), today: UsageBucket = .init(), week: UsageBucket = .init(), limit: Limit? = nil,
                otherToday: UsageBucket = .init(), ingestToday: Double = 0, ingestTotal: Double = 0) {
        self.session = session; self.today = today; self.week = week; self.limit = limit
        self.otherToday = otherToday; self.ingestToday = ingestToday; self.ingestTotal = ingestTotal
    }

    static func from(_ v: JSONValue) -> Usage {
        let c = v["claude"]
        var limit: Limit?
        if let l = c?["limit"], let status = l["status"]?.string {
            limit = Limit(status: status, resetsAt: l["resetsAt"]?.dateText, type: l["type"]?.nonEmptyString,
                          utilization: l["utilization"]?.numeric)
        }
        var u = Usage(session: .from(c?["session"]), today: .from(c?["today"]), week: .from(c?["week"]), limit: limit,
                      otherToday: .from(v["other"]?["today"]), ingestToday: v["ingest"]?["today"]?["costUsd"]?.numeric ?? 0,
                      ingestTotal: v["ingest"]?["total"]?["costUsd"]?.numeric ?? 0)
        u.examToday = v["exam"]?["today"]?["costUsd"]?.numeric ?? 0
        u.examTotal = v["exam"]?["total"]?["costUsd"]?.numeric ?? 0
        return u
    }
}

// MARK: - Greeting (section 15)

/// The line drawing shown next to the greeting. Unknown names fall back to `catWalk`.
public enum Doodle: String, Sendable, CaseIterable {
    case catWalk = "cat_walk", catSleep = "cat_sleep", whaleSwim = "whale_swim", birdHop = "bird_hop"
    case snail, paperPlane = "paper_plane"

    public init(name: String?) { self = name.flatMap(Doodle.init(rawValue:)) ?? .catWalk }
}

public struct Greeting: Sendable, Equatable {
    public var text: String
    public var sub: String?
    public var doodle: Doodle

    public init(text: String, sub: String? = nil, doodle: Doodle = .catWalk) {
        self.text = text; self.sub = sub; self.doodle = doodle
    }

    static func from(_ v: JSONValue) -> Greeting {
        Greeting(text: v["text"]?.string ?? "", sub: v["sub"]?.nonEmptyString, doodle: Doodle(name: v["doodle"]?.string))
    }

    /// Built-in lines used until the server answers (or when it cannot). Cute, short, tớ/cậu.
    public static func fallback(hour: Int, seed: Int, lang: Lang = .vi) -> Greeting {
        let lines: [(String, Doodle)]
        switch (lang, hour) {
        case (.vi, 5..<11):
            lines = [("Chào buổi sáng, cậu ơi", .birdHop), ("Sáng nay học gì nhỉ?", .paperPlane), ("Dậy sớm ghê, giỏi quá", .catWalk)]
        case (.vi, 11..<14):
            lines = [("Trưa rồi, ăn cơm chưa đó?", .catSleep), ("Nghỉ trưa xíu rồi học nha", .snail)]
        case (.vi, 14..<18):
            lines = [("Chiều nay mình học gì đây?", .whaleSwim), ("Tớ sẵn sàng rồi nè", .catWalk), ("Làm vài câu cho vui nhé", .paperPlane)]
        case (.vi, 18..<23):
            lines = [("Tối rồi, cùng ôn bài nha", .whaleSwim), ("Hôm nay của cậu thế nào?", .catWalk), ("Học xíu rồi đi ngủ sớm nhé", .snail)]
        case (.vi, _):
            lines = [("Khuya rồi đó, cậu ơi", .catSleep), ("Học nốt chút rồi ngủ nha", .catSleep)]
        case (.en, 5..<12):
            lines = [("Good morning, friend", .birdHop), ("What shall we learn today?", .paperPlane)]
        case (.en, 12..<18):
            lines = [("Ready when you are", .whaleSwim), ("A few problems for fun?", .catWalk)]
        case (.en, 18..<23):
            lines = [("Evening study time", .whaleSwim), ("How was your day?", .catWalk)]
        case (.en, _):
            lines = [("It's late, friend", .catSleep)]
        }
        let pick = lines[abs(seed) % lines.count]
        return Greeting(text: pick.0, doodle: pick.1)
    }
}
