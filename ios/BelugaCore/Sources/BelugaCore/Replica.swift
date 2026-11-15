import Foundation

// Bookkeeping for the local replica (PROTOCOL section 19): the manifest the server publishes, which parts the
// phone must pull or drop, a bounded and resumable pull queue, and the queue of edits made while offline.
// File IO and HTTP live in the app; everything here is pure and tested.

/// One part of the replica.
public enum SyncItem: Hashable, Sendable, Codable, Comparable {
    case profile
    case folder(String)
    case session(String)

    /// Key in the etag book and file name stem.
    public var key: String {
        switch self {
        case .profile: return "profile"
        case .folder(let id): return "folder:" + id
        case .session(let id): return "session:" + id
        }
    }

    /// Path under `/sync/` on the server.
    public var path: String {
        switch self {
        case .profile: return "profile"
        case .folder(let id): return "folder/" + id
        case .session(let id): return "session/" + id
        }
    }

    public init?(key: String) {
        if key == "profile" { self = .profile; return }
        if key.hasPrefix("folder:"), key.count > 7 { self = .folder(String(key.dropFirst(7))); return }
        if key.hasPrefix("session:"), key.count > 8 { self = .session(String(key.dropFirst(8))); return }
        return nil
    }

    /// Profile first (small, used everywhere), then folders, then sessions.
    private var rank: Int {
        switch self { case .profile: return 0; case .folder: return 1; case .session: return 2 }
    }
    public static func < (a: SyncItem, b: SyncItem) -> Bool { a.rank != b.rank ? a.rank < b.rank : a.key < b.key }
}

/// `GET /sync/manifest`.
public struct SyncManifest: Sendable, Equatable {
    public struct Entry: Sendable, Equatable {
        public var id: String
        public var etag: String
        public var folderId: String?
        public init(id: String, etag: String, folderId: String? = nil) { self.id = id; self.etag = etag; self.folderId = folderId }
    }
    public var v: Int
    public var serverTime: String?
    public var profileEtag: String?
    public var folders: [Entry]
    public var sessions: [Entry]
    public var voicesEtag: String?

    public init(v: Int = 1, serverTime: String? = nil, profileEtag: String? = nil, folders: [Entry] = [], sessions: [Entry] = [],
                voicesEtag: String? = nil) {
        self.v = v; self.serverTime = serverTime; self.profileEtag = profileEtag; self.folders = folders
        self.sessions = sessions; self.voicesEtag = voicesEtag
    }

    /// nil when the body is not a manifest (an HTML error page, a JSON error).
    public static func from(_ v: JSONValue) -> SyncManifest? {
        guard v.object != nil, v["folders"]?.array != nil || v["sessions"]?.array != nil || v["profile"] != nil else { return nil }
        func etag(_ x: JSONValue?) -> String { x?["etag"]?.string ?? x?["etag"]?.dateText ?? "" }
        let folders = (v["folders"]?.array ?? []).compactMap { f -> Entry? in
            guard let id = f["id"]?.nonEmptyString else { return nil }
            return Entry(id: id, etag: etag(f))
        }.uniqued { $0.id }
        let sessions = (v["sessions"]?.array ?? []).compactMap { s -> Entry? in
            guard let id = s["sessionId"]?.nonEmptyString else { return nil }
            return Entry(id: id, etag: etag(s), folderId: s["folderId"]?.nonEmptyString)
        }.uniqued { $0.id }
        let profile = v["profile"].map { etag($0) }
        return SyncManifest(v: v["v"]?.int ?? 1, serverTime: v["serverTime"]?.dateText,
                            profileEtag: profile, folders: folders, sessions: sessions,
                            voicesEtag: v["voices"].map { etag($0) })
    }

    public static func from(_ data: Data) -> SyncManifest? {
        (try? JSONValue.parse(data)).flatMap(from)
    }
}

/// What a sync must do to make the replica match a manifest.
public struct SyncPlan: Sendable, Equatable {
    public var pull: [SyncItem]
    public var drop: [SyncItem]
    public init(pull: [SyncItem] = [], drop: [SyncItem] = []) { self.pull = pull; self.drop = drop }
    public var isEmpty: Bool { pull.isEmpty && drop.isEmpty }
}

/// The etag of every part the replica holds. An etag is only recorded after the part's file was written, so an
/// interrupted sync resumes by itself: the next plan pulls whatever is still missing or stale.
public struct EtagBook: Codable, Sendable, Equatable {
    public var etags: [String: String]
    public var lastSyncAt: Date?
    public var serverTime: String?

    public init(etags: [String: String] = [:], lastSyncAt: Date? = nil, serverTime: String? = nil) {
        self.etags = etags; self.lastSyncAt = lastSyncAt; self.serverTime = serverTime
    }

    public func etag(_ item: SyncItem) -> String? { etags[item.key] }
    public mutating func record(_ item: SyncItem, etag: String) { etags[item.key] = etag }
    public mutating func forget(_ item: SyncItem) { etags[item.key] = nil }
    public var items: [SyncItem] { etags.keys.compactMap(SyncItem.init(key:)).sorted() }
    public var isEmpty: Bool { etags.isEmpty }

    /// Pull what is new or changed (an empty server etag always counts as changed); drop what the server no
    /// longer has. The profile is never dropped.
    public func plan(against m: SyncManifest) -> SyncPlan {
        var pull: [SyncItem] = []
        var drop: [SyncItem] = []
        if let p = m.profileEtag, p.isEmpty || etag(.profile) != p { pull.append(.profile) }
        for f in m.folders where f.etag.isEmpty || etag(.folder(f.id)) != f.etag { pull.append(.folder(f.id)) }
        for s in m.sessions where s.etag.isEmpty || etag(.session(s.id)) != s.etag { pull.append(.session(s.id)) }
        let folderIds = Set(m.folders.map(\.id)), sessionIds = Set(m.sessions.map(\.id))
        for item in items {
            switch item {
            case .folder(let id) where !folderIds.contains(id): drop.append(item)
            case .session(let id) where !sessionIds.contains(id): drop.append(item)
            default: break
            }
        }
        return SyncPlan(pull: pull, drop: drop)
    }
}

/// Pulls at most `maxInFlight` parts at a time; a failed part is retried later (up to `maxAttempts`), then given
/// up for this round. Never blocks: the caller asks `next()` whenever a slot frees.
public struct SyncQueue: Sendable, Equatable {
    public let maxInFlight: Int
    public let maxAttempts: Int
    public private(set) var pending: [SyncItem] = []
    public private(set) var inFlight: Set<SyncItem> = []
    public private(set) var attempts: [SyncItem: Int] = [:]
    public private(set) var done = 0
    public private(set) var failed: [SyncItem] = []
    /// Parts added since the queue was last idle (for "n of m").
    public private(set) var total = 0

    public init(maxInFlight: Int = 3, maxAttempts: Int = 3) {
        self.maxInFlight = max(1, maxInFlight); self.maxAttempts = max(1, maxAttempts)
    }

    public var isIdle: Bool { pending.isEmpty && inFlight.isEmpty }

    /// Adds parts not already queued or running.
    public mutating func add(_ items: [SyncItem]) {
        if isIdle { done = 0; total = 0; failed = []; attempts = [:] }
        for i in items where !pending.contains(i) && !inFlight.contains(i) {
            pending.append(i)
            total += 1
            failed.removeAll { $0 == i }
        }
    }

    /// The next part to pull, if a slot is free.
    public mutating func next() -> SyncItem? {
        guard inFlight.count < maxInFlight, !pending.isEmpty else { return nil }
        let item = pending.removeFirst()
        inFlight.insert(item)
        attempts[item, default: 0] += 1
        return item
    }

    public mutating func finish(_ item: SyncItem, ok: Bool) {
        guard inFlight.remove(item) != nil else { return }
        if ok {
            done += 1
        } else if attempts[item, default: 0] < maxAttempts {
            pending.append(item)
        } else {
            done += 1
            failed.append(item)
        }
    }

    /// Stops after the running pulls: nothing new starts (the connection went away).
    public mutating func cancelPending() {
        total -= pending.count
        pending.removeAll()
    }
}

// MARK: - Offline edits

/// An edit made while the server was unreachable, replayed in order when it comes back (section 19).
public struct PendingOp: Codable, Sendable, Equatable, Identifiable {
    public enum Kind: Codable, Sendable, Equatable {
        case createFolder(clientId: String, name: String, about: String)
        case updateFolder(id: String, name: String?, about: String?)
        case renameSession(sessionId: String, title: String)
        case moveSession(sessionId: String, folderId: String?)
        case deleteSession(sessionId: String)
    }
    public var id: String
    public var kind: Kind
    public init(id: String = UUID().uuidString, kind: Kind) { self.id = id; self.kind = kind }

    /// The frame that carries this edit to the server.
    public var message: ClientMessage {
        switch kind {
        case let .createFolder(clientId, name, about):
            return .createFolder(name: name, about: about.isEmpty ? nil : about, clientId: clientId)
        case let .updateFolder(id, name, about): return .updateFolder(id: id, name: name, about: about)
        case let .renameSession(sid, title): return .renameSession(sessionId: sid, title: title)
        case let .moveSession(sid, fid): return .moveSession(sessionId: sid, folderId: fid)
        case .deleteSession(let sid): return .deleteSession(sessionId: sid)
        }
    }
}

public struct OpQueue: Codable, Sendable, Equatable {
    public private(set) var ops: [PendingOp]
    public init(ops: [PendingOp] = []) { self.ops = ops }

    public static let localPrefix = "local-"
    public static func newClientId() -> String { localPrefix + UUID().uuidString.lowercased() }
    /// A folder id the phone made up while offline (not known to the server yet).
    public static func isLocal(_ id: String?) -> Bool { id?.hasPrefix(localPrefix) == true }

    public var isEmpty: Bool { ops.isEmpty }
    public var first: PendingOp? { ops.first }
    public mutating func removeFirst() { if !ops.isEmpty { ops.removeFirst() } }

    /// Adds an edit, folding it into an earlier one when that keeps the meaning:
    /// - name/about of a folder created offline go into its create;
    /// - two renames of the same session keep the last; edits after a session's deletion are dropped.
    public mutating func enqueue(_ kind: PendingOp.Kind) {
        switch kind {
        case let .updateFolder(id, name, about):
            if let i = ops.firstIndex(where: { if case .createFolder(id, _, _) = $0.kind { return true }; return false }),
               case let .createFolder(cid, oldName, oldAbout) = ops[i].kind {
                ops[i].kind = .createFolder(clientId: cid, name: name ?? oldName, about: about ?? oldAbout)
                return
            }
            if let i = ops.lastIndex(where: { if case .updateFolder(id, _, _) = $0.kind { return true }; return false }),
               i == ops.count - 1, case let .updateFolder(_, oldName, oldAbout) = ops[i].kind {
                ops[i].kind = .updateFolder(id: id, name: name ?? oldName, about: about ?? oldAbout)
                return
            }
        case let .renameSession(sid, _):
            if deleted(sid) { return }
            if let i = ops.lastIndex(where: { if case .renameSession(sid, _) = $0.kind { return true }; return false }) {
                ops[i].kind = kind
                return
            }
        case let .moveSession(sid, _):
            if deleted(sid) { return }
        case let .deleteSession(sid):
            if deleted(sid) { return }
        case .createFolder:
            break
        }
        ops.append(PendingOp(kind: kind))
    }

    private func deleted(_ sid: String) -> Bool {
        ops.contains { if case .deleteSession(sid) = $0.kind { return true }; return false }
    }

    /// The server answered an offline create with its real id: later edits that point at the local id follow.
    public mutating func resolve(clientId: String, to id: String) {
        for i in ops.indices {
            switch ops[i].kind {
            case let .updateFolder(fid, name, about) where fid == clientId:
                ops[i].kind = .updateFolder(id: id, name: name, about: about)
            case let .moveSession(sid, fid) where fid == clientId:
                ops[i].kind = .moveSession(sessionId: sid, folderId: id)
            default: break
            }
        }
    }

    /// Folders as the owner should see them: the server's list with offline edits laid over it, and folders
    /// created offline appended (marked pending).
    public func overlay(folders: [Folder]) -> [Folder] {
        var out = folders
        for op in ops {
            switch op.kind {
            case let .createFolder(cid, name, about):
                if !out.contains(where: { $0.id == cid }) {
                    out.append(Folder(id: cid, name: name, about: about, pending: true))
                }
            case let .updateFolder(id, name, about):
                if let i = out.firstIndex(where: { $0.id == id }) {
                    if let name { out[i].name = name }
                    if let about { out[i].about = about }
                }
            default: break
            }
        }
        return out
    }

    /// Sessions with offline renames, moves and deletions applied.
    public func overlay(sessions: [SessionItem]) -> [SessionItem] {
        var out = sessions
        for op in ops {
            switch op.kind {
            case let .renameSession(sid, title):
                if let i = out.firstIndex(where: { $0.sessionId == sid }) { out[i].title = title }
            case let .moveSession(sid, fid):
                if let i = out.firstIndex(where: { $0.sessionId == sid }) { out[i].folderId = fid }
            case .deleteSession(let sid):
                out.removeAll { $0.sessionId == sid }
            default: break
            }
        }
        return out
    }
}

// MARK: - Offline exam pages

public enum OfflineExam {
    /// Points the exam page's web resources at a local copy of the board package: `{{BOARD}}` (section 19) and the
    /// server's own absolute `/board/` paths (pages saved from `GET /exam`) both become `boardPath`.
    public static func localize(_ html: String, boardPath: String) -> String {
        let base = boardPath.hasSuffix("/") ? String(boardPath.dropLast()) : boardPath
        return html
            .replacingOccurrences(of: "{{BOARD}}", with: base)
            .replacingOccurrences(of: "\"/board/", with: "\"" + base + "/")
            .replacingOccurrences(of: "'/board/", with: "'" + base + "/")
            .replacingOccurrences(of: "(/board/", with: "(" + base + "/")
    }
}
