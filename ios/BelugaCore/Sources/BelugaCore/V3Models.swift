import Foundation

// Models for protocol v3 frames (docs/PROTOCOL.md sections 17-22). Tolerant like the v2 parsers: missing or
// mistyped fields become defaults, unknown fields are ignored, and an older server simply never sends them.

// MARK: - Home suggestions (section 18)

/// `home{folderId?, continue?, due, quick}`: what the server suggests for one context (a folder, or the root).
public struct HomeFrame: Sendable, Equatable {
    public struct Continue: Sendable, Equatable {
        public var sessionId: String
        public var title: String
        public var folderId: String?
        public var folderName: String?
        public var updatedAt: String
        public init(sessionId: String, title: String, folderId: String? = nil, folderName: String? = nil, updatedAt: String = "") {
            self.sessionId = sessionId; self.title = title; self.folderId = folderId; self.folderName = folderName
            self.updatedAt = updatedAt
        }
    }
    public struct Due: Sendable, Equatable, Identifiable {
        public var folderId: String
        public var folderName: String?
        public var kind: String
        public var count: Int
        public var topics: [String]
        public var id: String { folderId }
        public init(folderId: String, folderName: String? = nil, kind: String = "chung", count: Int = 0, topics: [String] = []) {
            self.folderId = folderId; self.folderName = folderName; self.kind = kind; self.count = count; self.topics = topics
        }
    }
    public struct Quick: Sendable, Equatable, Identifiable {
        public var folderId: String
        public var folderName: String?
        public var kind: String
        public var id: String { folderId }
        public init(folderId: String, folderName: String? = nil, kind: String = "chung") {
            self.folderId = folderId; self.folderName = folderName; self.kind = kind
        }
    }

    /// The context this frame answers (nil = the root home).
    public var folderId: String?
    public var continueItem: Continue?
    public var due: [Due]
    public var quick: [Quick]

    public init(folderId: String? = nil, continueItem: Continue? = nil, due: [Due] = [], quick: [Quick] = []) {
        self.folderId = folderId; self.continueItem = continueItem; self.due = due; self.quick = quick
    }

    static func from(_ v: JSONValue) -> HomeFrame {
        var cont: Continue?
        if let c = v["continue"], let sid = c["sessionId"]?.nonEmptyString {
            cont = Continue(sessionId: sid, title: c["title"]?.string ?? "", folderId: c["folderId"]?.nonEmptyString,
                            folderName: c["folderName"]?.nonEmptyString, updatedAt: c["updatedAt"]?.dateText ?? "")
        }
        let due = (v["due"]?.array ?? []).compactMap { d -> Due? in
            guard let fid = d["folderId"]?.nonEmptyString else { return nil }
            return Due(folderId: fid, folderName: d["folderName"]?.nonEmptyString, kind: d["kind"]?.nonEmptyString ?? "chung",
                       count: d["count"]?.integer ?? 0, topics: d["topics"]?.strings ?? [])
        }.uniquedByID()
        let quick = (v["quick"]?.array ?? []).compactMap { q -> Quick? in
            guard let fid = q["folderId"]?.nonEmptyString else { return nil }
            return Quick(folderId: fid, folderName: q["folderName"]?.nonEmptyString, kind: q["kind"]?.nonEmptyString ?? "chung")
        }.uniquedByID()
        return HomeFrame(folderId: v["folderId"]?.nonEmptyString, continueItem: cont, due: due, quick: quick)
    }
}

// MARK: - Sync hint and backup (sections 19, 20)

/// `sync{folders?, sessions?, profile?}`: these parts changed on the server; pull them again.
public struct SyncHint: Sendable, Equatable {
    public var folders: [String]
    public var sessions: [String]
    public var profile: Bool
    public init(folders: [String] = [], sessions: [String] = [], profile: Bool = false) {
        self.folders = folders; self.sessions = sessions; self.profile = profile
    }
    static func from(_ v: JSONValue) -> SyncHint {
        SyncHint(folders: v["folders"]?.strings.filter { !$0.isEmpty } ?? [],
                 sessions: v["sessions"]?.strings.filter { !$0.isEmpty } ?? [],
                 profile: v["profile"]?.bool ?? false)
    }
    public var items: [SyncItem] {
        (profile ? [SyncItem.profile] : []) + folders.map(SyncItem.folder) + sessions.map(SyncItem.session)
    }
}

/// `backup{enabled, provider, lastAt?, lastBytes?, lastError?, nextAt?}`.
public struct BackupStatus: Sendable, Equatable {
    public var enabled: Bool
    public var provider: String
    public var lastAt: String?
    public var lastBytes: Int?
    public var lastError: String?
    public var nextAt: String?
    public init(enabled: Bool, provider: String = "r2", lastAt: String? = nil, lastBytes: Int? = nil,
                lastError: String? = nil, nextAt: String? = nil) {
        self.enabled = enabled; self.provider = provider; self.lastAt = lastAt; self.lastBytes = lastBytes
        self.lastError = lastError; self.nextAt = nextAt
    }
    static func from(_ v: JSONValue) -> BackupStatus {
        BackupStatus(enabled: v["enabled"]?.bool ?? false, provider: v["provider"]?.nonEmptyString ?? "r2",
                     lastAt: v["lastAt"]?.dateText, lastBytes: v["lastBytes"]?.integer,
                     lastError: v["lastError"]?.nonEmptyString, nextAt: v["nextAt"]?.dateText)
    }
}

// MARK: - Wait and hello clips (section 21)

/// A pre-made line in the tutor's own voice, played while it thinks (`short`, `long`, `vision`) or when a call
/// connects (`hello`).
public struct VoiceFiller: Sendable, Equatable, Identifiable, Hashable {
    public enum When: String, Sendable { case short, long, vision, hello, other }
    public var id: String
    public var text: String
    public var when: When
    public init(id: String, text: String = "", when: When) { self.id = id; self.text = text; self.when = when }
    static func from(_ v: JSONValue) -> VoiceFiller? {
        guard let id = v["id"]?.nonEmptyString else { return nil }
        return VoiceFiller(id: id, text: v["text"]?.string ?? "",
                           when: v["when"]?.string.flatMap(When.init(rawValue:)) ?? .other)
    }
}
