import Foundation

// Wire models for docs/PROTOCOL.md section 4 (v1) and sections 7-15 (v2). Decoding is tolerant:
// unknown frame types become `.unknown`, missing fields get defaults, unknown fields are ignored.

public enum Lang: String, Codable, Sendable, CaseIterable { case vi, en }
public enum Rigor: String, Codable, Sendable, CaseIterable { case light, medium, hard }
/// v2 (PROTOCOL section 7): text chat (default) or the voice + camera + board lesson.
public enum SessionMode: String, Codable, Sendable, CaseIterable { case chat, live }

public struct TutorSettings: Codable, Sendable, Equatable {
    public var lang: Lang
    /// Model id from `ready.models` (v1 ids "sonnet" | "opus" | "haiku" still work).
    public var model: String
    /// nil = the model's default effort (sent as JSON null).
    public var effort: String?
    public var rigor: Rigor
    public var mode: SessionMode
    /// Let the server fix speech recognition with a cheap model (section 7).
    public var sttFix: Bool
    /// Tutor voice id from `voices`. Only sent when the user picks one (nil = leave the server's choice alone).
    public var voice: String?
    public init(lang: Lang = .vi, model: String = "sonnet", effort: String? = nil, rigor: Rigor = .medium,
                mode: SessionMode = .chat, sttFix: Bool = true, voice: String? = nil) {
        self.lang = lang; self.model = model; self.effort = effort; self.rigor = rigor; self.mode = mode; self.sttFix = sttFix
        self.voice = voice
    }

    var json: JSONValue {
        var o: [String: JSONValue] = ["lang": .string(lang.rawValue), "model": .string(model), "rigor": .string(rigor.rawValue),
                                      "mode": .string(mode.rawValue), "sttFix": .bool(sttFix),
                                      "effort": effort.map(JSONValue.string) ?? .null]
        if let voice, !voice.isEmpty { o["voice"] = .string(voice) }
        return .object(o)
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
    /// Max wait for a say's `audio` frame, counted from when that say is due (default 3000 ms).
    public var audioWaitMs: Int
    // v2
    public var folderId: String?
    public var mode: SessionMode
    public var models: [ModelInfo]
    public var model: String?
    public var effort: String?
    public var serverTTS: Bool { tts == "server" }
    public var serverSTT: Bool { stt == "server" }

    public init(v: Int = 2, sessionId: String, title: String = "", tts: String = "device", stt: String = "device",
                boardVersion: Int = 0, pairToken: String? = nil, audioWaitMs: Int = 3000, folderId: String? = nil,
                mode: SessionMode = .chat, models: [ModelInfo] = [], model: String? = nil, effort: String? = nil) {
        self.v = v; self.sessionId = sessionId; self.title = title; self.tts = tts; self.stt = stt
        self.boardVersion = boardVersion; self.pairToken = pairToken; self.audioWaitMs = audioWaitMs
        self.folderId = folderId; self.mode = mode; self.models = models; self.model = model; self.effort = effort
    }

    static func from(_ v: JSONValue) -> Ready {
        Ready(v: v["v"]?.int ?? 1,
              sessionId: v["sessionId"]?.string ?? "",
              title: v["title"]?.string ?? "",
              tts: v["tts"]?.string ?? "device",
              stt: v["stt"]?.string ?? "device",
              boardVersion: v["boardVersion"]?.int ?? 0,
              pairToken: v["pairToken"]?.string,
              audioWaitMs: v["audioWaitMs"]?.int ?? 3000,
              folderId: v["folderId"]?.nonEmptyString,
              mode: v["mode"]?.string.flatMap(SessionMode.init(rawValue:)) ?? .chat,
              models: (v["models"]?.array ?? []).compactMap(ModelInfo.from).uniquedByID(),
              model: v["model"]?.nonEmptyString,
              effort: v["effort"]?.nonEmptyString)
    }
}

public struct SessionItem: Sendable, Equatable, Identifiable, Hashable {
    public var sessionId: String
    public var title: String
    public var updatedAt: String
    public var folderId: String?
    public var mode: SessionMode
    public var preview: String?
    public var id: String { sessionId }
    public init(sessionId: String, title: String, updatedAt: String, folderId: String? = nil,
                mode: SessionMode = .live, preview: String? = nil) {
        self.sessionId = sessionId; self.title = title; self.updatedAt = updatedAt
        self.folderId = folderId; self.mode = mode; self.preview = preview
    }

    static func from(_ i: JSONValue) -> SessionItem? {
        guard let id = i["sessionId"]?.string, !id.isEmpty else { return nil }
        return SessionItem(sessionId: id, title: i["title"]?.string ?? "", updatedAt: i["updatedAt"]?.dateText ?? "",
                           folderId: i["folderId"]?.nonEmptyString,
                           mode: i["mode"]?.string.flatMap(SessionMode.init(rawValue:)) ?? .live,
                           preview: i["preview"]?.nonEmptyString)
    }
}

/// A file attached to a user turn (PROTOCOL section 9).
public struct FileRef: Sendable, Equatable, Hashable {
    public var fileId: String
    public var kind: String      // "image" | "pdf"
    public var name: String
    public init(fileId: String, kind: String, name: String) { self.fileId = fileId; self.kind = kind; self.name = name }
    static func from(_ v: JSONValue) -> FileRef? {
        guard let id = v["fileId"]?.string else { return nil }
        return FileRef(fileId: id, kind: v["kind"]?.string ?? "image", name: v["name"]?.string ?? "")
    }
}

/// A sequenced lesson event (say/step/show/ask/done/turn_end, and v2 user/chat). `raw` is the original frame,
/// forwarded verbatim to the web pages (`board.apply(event)`, `chat.apply(event)`).
public struct LessonEvent: Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        case say(text: String, audio: Bool)
        case step(index: Int, title: String)
        /// `step == -1`: the block sits inline in the chat transcript (v2).
        case show(step: Int, block: JSONValue)
        case ask(kind: String, prompt: String)
        case done
        case turnEnd(interrupted: Bool)
        /// Echo of the user's turn (v2), logged in both modes.
        case user(text: String, files: [FileRef], via: String)
        /// A finished markdown paragraph of the assistant in chat mode (v2).
        case chat(md: String)
    }
    public var seq: Int
    public var turn: Int
    public var kind: Kind
    public var raw: JSONValue

    public var isSay: Bool { if case .say = kind { return true }; return false }
    public var isTurnEnd: Bool { if case .turnEnd = kind { return true }; return false }
    /// Events the board renders (a chat-inline `show`, step -1, is not one of them).
    public var isBoardEvent: Bool {
        switch kind {
        case .step, .ask, .done: return true
        case .show(let step, _): return step >= 0
        default: return false
        }
    }
    /// Live-lesson events (they make a turn a lesson turn).
    public var isLiveEvent: Bool {
        switch kind {
        case .say, .step: return true
        case .show(let step, _): return step >= 0
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
        case .user(let text, let files, let via):
            o["t"] = "user"; o["text"] = .string(text); o["via"] = .string(via)
            if !files.isEmpty {
                o["files"] = .array(files.map { ["fileId": .string($0.fileId), "kind": .string($0.kind), "name": .string($0.name)] })
            }
        case .chat(let md): o["t"] = "chat"; o["md"] = .string(md)
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
        case "user":
            kind = .user(text: v["text"]?.string ?? "", files: (v["files"]?.array ?? []).compactMap(FileRef.from).uniqued { $0.fileId },
                         via: v["via"]?.string ?? "text")
        case "chat": kind = .chat(md: v["md"]?.string ?? "")
        default: return nil
        }
        return LessonEvent(seq: seq, turn: turn, kind: kind, raw: v)
    }
}

public enum ServerMessage: Sendable, Equatable {
    case ready(Ready)
    case state(state: String, tool: String?)
    /// STT result. `draft` (dictation in chat): put it in the composer, it was not sent to Claude.
    case heard(text: String, raw: String?, draft: Bool)
    case event(LessonEvent)
    /// Streaming markdown piece of the assistant (chat mode). No seq, not logged.
    case chatDelta(turn: Int, text: String)
    case audio(seq: Int, mime: String, data: Data)
    /// No server audio for this say: speak it with the system voice now.
    case audioSkip(seq: Int)
    case title(text: String)
    case captureRequest(requestId: String)
    case snapshot(events: [LessonEvent])
    case sessions([SessionItem])
    case progress(LearningProgress)
    case notice(code: String, text: String)
    case error(code: String, text: String, fatal: Bool)
    // v2
    case folders([Folder])
    /// `clientId` echoes the id the phone chose (v3); nil from older servers.
    case folderCreated(id: String, clientId: String?)
    case ingest(IngestStatus)
    case knowledge(FolderKnowledge)
    case review(FolderReview)
    case exam(ExamStatus)
    case exams(folderId: String, items: [ExamItem])
    case usage(Usage)
    case greeting(Greeting)
    case voices(VoiceCatalog)
    // v3
    case home(HomeFrame)
    case sync(SyncHint)
    case backup(BackupStatus)
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
            return .ready(Ready.from(v))
        case "state":
            return .state(state: v["state"]?.string ?? "idle", tool: v["tool"]?.string)
        case "heard":
            return .heard(text: v["text"]?.string ?? "", raw: v["raw"]?.nonEmptyString, draft: v["draft"]?.bool ?? false)
        case "say", "step", "show", "ask", "done", "turn_end", "user", "chat":
            if let e = LessonEvent.from(v) { return .event(e) }
            return .unknown(type: t)
        case "chat_delta":
            return .chatDelta(turn: v["turn"]?.int ?? 0, text: v["text"]?.string ?? "")
        case "audio":
            let data = Data(base64Encoded: v["data"]?.string ?? "") ?? Data()
            return .audio(seq: v["seq"]?.int ?? -1, mime: v["mime"]?.string ?? "audio/mpeg", data: data)
        case "audio_skip":
            return .audioSkip(seq: v["seq"]?.int ?? -1)
        case "title":
            return .title(text: v["text"]?.string ?? "")
        case "capture_request":
            return .captureRequest(requestId: v["requestId"]?.string ?? "")
        case "snapshot":
            return .snapshot(events: (v["events"]?.array ?? []).compactMap(LessonEvent.from))
        case "sessions":
            return .sessions((v["items"]?.array ?? []).compactMap(SessionItem.from).uniquedByID())
        case "progress":
            return .progress(LearningProgress.from(v))
        case "notice":
            return .notice(code: v["code"]?.string ?? "", text: v["text"]?.string ?? "")
        case "error":
            return .error(code: v["code"]?.string ?? "", text: v["text"]?.string ?? "", fatal: v["fatal"]?.bool ?? false)
        case "folders":
            return .folders((v["items"]?.array ?? []).compactMap(Folder.from).uniquedByID())
        case "folder_created":
            return .folderCreated(id: v["id"]?.string ?? v["folderId"]?.string ?? "", clientId: v["clientId"]?.nonEmptyString)
        case "ingest":
            return .ingest(IngestStatus.from(v))
        case "knowledge":
            return .knowledge(FolderKnowledge.from(v))
        case "review":
            return .review(FolderReview.from(v))
        case "exam":
            return .exam(ExamStatus.from(v))
        case "exams":
            return .exams(folderId: v["folderId"]?.string ?? "", items: (v["items"]?.array ?? []).compactMap(ExamItem.from).uniquedByID())
        case "usage":
            return .usage(Usage.from(v))
        case "greeting":
            return .greeting(Greeting.from(v))
        case "voices":
            return .voices(VoiceCatalog.from(v))
        case "home":
            return .home(HomeFrame.from(v))
        case "sync":
            return .sync(SyncHint.from(v))
        case "backup":
            return .backup(BackupStatus.from(v))
        default:
            return .unknown(type: t)
        }
    }
}

// MARK: - Phone -> server

public enum ClientMessage: Sendable, Equatable {
    case hello(pairCode: String?, pairToken: String?, device: String, sessionId: String?, lastSeq: Int?,
               settings: TutorSettings, folderId: String? = nil)
    case settings(TutorSettings)
    /// `files`: ids from `POST /upload`. `draft`: dictation, the server only answers with `heard{draft}`.
    /// `via: "voice"` when the text came from the device's speech recognition (typed text is never corrected).
    case userText(_ text: String, files: [String] = [], draft: Bool = false, via: String? = nil)
    case audioStart(sampleRate: Int, draft: Bool = false)
    case audioEnd
    case interrupt
    case photo(requestId: String?, jpeg: Data, w: Int, h: Int)
    case played(seq: Int)
    case listSessions(folderId: String? = nil)
    case openSession(sessionId: String)
    case getProgress
    // v2: folders and sessions (section 8)
    case listFolders
    /// v3: free name, optional one-line `about`, `clientId` so a replay after an offline create never makes two.
    /// `subject` is only for a v2 server (a v3 server picks the kind itself).
    case createFolder(name: String, about: String? = nil, subject: String? = nil, clientId: String? = nil)
    case renameFolder(id: String, name: String)
    /// v3: change name and/or about (nil = leave as is).
    case updateFolder(id: String, name: String?, about: String?)
    case deleteFolder(id: String)
    case moveSession(sessionId: String, folderId: String?)
    case renameSession(sessionId: String, title: String)
    case deleteSession(sessionId: String)
    case newSession(folderId: String?, mode: SessionMode?, kickoff: String? = nil)
    // v2: knowledge, review, exams, usage, greeting (sections 10-15)
    case ingestStart(folderId: String, files: [String], goal: String?)
    case ingestCancel(folderId: String)
    case getKnowledge(folderId: String)
    case getReview(folderId: String)
    case getWeekly(folderId: String)
    case genExam(folderId: String, structureId: String, note: String?)
    case listExams(folderId: String)
    case deleteExam(examId: String)
    case getUsage
    case getGreeting(localTime: String, name: String?, folderId: String? = nil)
    case getVoices
    // v3
    case getHome(folderId: String?)
    case getBackup
    case backupNow

    public var json: JSONValue {
        func obj(_ t: String, _ fields: [String: JSONValue?] = [:]) -> JSONValue {
            var o: [String: JSONValue] = ["t": .string(t)]
            for (k, v) in fields { if let v { o[k] = v } }
            return .object(o)
        }
        func str(_ s: String?) -> JSONValue? { s.flatMap { $0.isEmpty ? nil : JSONValue.string($0) } }
        switch self {
        case let .hello(pairCode, pairToken, device, sessionId, lastSeq, settings, folderId):
            return obj("hello", ["v": .number(Double(BelugaCore.protocolVersion)), "device": .string(device),
                                 "settings": settings.json, "pairCode": str(pairCode), "pairToken": str(pairToken),
                                 "sessionId": .string(sessionId ?? ""), "lastSeq": lastSeq.map { JSONValue.number(Double($0)) },
                                 "folderId": str(folderId)])
        case .settings(let s):
            var o: [String: JSONValue] = ["t": "settings"]
            if case .object(let so) = s.json { o.merge(so) { a, _ in a } }
            return .object(o)
        case let .userText(text, files, draft, via):
            return obj("user_text", ["text": .string(text),
                                     "files": files.isEmpty ? nil : .array(files.map(JSONValue.string)),
                                     "draft": draft ? .bool(true) : nil, "via": str(via)])
        case let .audioStart(rate, draft):
            return obj("audio_start", ["mime": "audio/wav", "sampleRate": .number(Double(rate)), "draft": draft ? .bool(true) : nil])
        case .audioEnd: return obj("audio_end")
        case .interrupt: return obj("interrupt")
        case let .photo(requestId, jpeg, w, h):
            return obj("photo", ["jpeg": .string(jpeg.base64EncodedString()), "w": .number(Double(w)),
                                 "h": .number(Double(h)), "requestId": str(requestId)])
        case .played(let seq): return obj("played", ["seq": .number(Double(seq))])
        case .listSessions(let folderId): return obj("list_sessions", ["folderId": str(folderId)])
        case .openSession(let id): return obj("open_session", ["sessionId": .string(id)])
        case .getProgress: return obj("get_progress")
        case .listFolders: return obj("list_folders")
        case let .createFolder(name, about, subject, clientId):
            return obj("create_folder", ["name": .string(name), "about": about.map(JSONValue.string),
                                         "subject": str(subject), "clientId": str(clientId)])
        case let .renameFolder(id, name): return obj("rename_folder", ["id": .string(id), "name": .string(name)])
        case let .updateFolder(id, name, about):
            return obj("update_folder", ["id": .string(id), "name": name.map(JSONValue.string), "about": about.map(JSONValue.string)])
        case .deleteFolder(let id): return obj("delete_folder", ["id": .string(id)])
        case let .moveSession(sessionId, folderId):
            return obj("move_session", ["sessionId": .string(sessionId), "folderId": folderId.map(JSONValue.string) ?? .null])
        case let .renameSession(sessionId, title): return obj("rename_session", ["sessionId": .string(sessionId), "title": .string(title)])
        case .deleteSession(let sessionId): return obj("delete_session", ["sessionId": .string(sessionId)])
        case let .newSession(folderId, mode, kickoff):
            return obj("new_session", ["folderId": str(folderId), "mode": mode.map { JSONValue.string($0.rawValue) },
                                       "kickoff": str(kickoff)])
        case let .ingestStart(folderId, files, goal):
            return obj("ingest_start", ["folderId": .string(folderId), "files": .array(files.map(JSONValue.string)), "goal": str(goal)])
        case .ingestCancel(let folderId): return obj("ingest_cancel", ["folderId": .string(folderId)])
        case .getKnowledge(let folderId): return obj("get_knowledge", ["folderId": .string(folderId)])
        case .getReview(let folderId): return obj("get_review", ["folderId": .string(folderId)])
        case .getWeekly(let folderId): return obj("get_weekly", ["folderId": .string(folderId)])
        case let .genExam(folderId, structureId, note):
            return obj("gen_exam", ["folderId": .string(folderId), "structureId": .string(structureId), "note": str(note)])
        case .listExams(let folderId): return obj("list_exams", ["folderId": .string(folderId)])
        case .deleteExam(let examId): return obj("delete_exam", ["examId": .string(examId)])
        case .getUsage: return obj("get_usage")
        case let .getGreeting(localTime, name, folderId):
            return obj("get_greeting", ["localTime": .string(localTime), "name": str(name), "folderId": str(folderId)])
        case .getVoices: return obj("get_voices")
        case .getHome(let folderId): return obj("get_home", ["folderId": str(folderId)])
        case .getBackup: return obj("get_backup")
        case .backupNow: return obj("backup_now")
        }
    }

    public var text: String { json.jsonString }
}
