import Foundation

// Folder isolation on the phone (PROTOCOL section 18): every screen that shows something "for this context"
// picks it here, so the rule "nothing of folder A ever appears in folder B" lives in one tested place.
//
// The bug this replaces: the home screen guessed its "Ôn nhanh" folder as "the folder whose knowledge changed
// last", whatever folder the owner was in, so opening English suggested Math.

/// One suggestion card of the home screen.
public struct HomeCard: Sendable, Equatable, Identifiable {
    public enum Kind: String, Sendable { case continueSession = "continue", due, quick }
    public var kind: Kind
    /// Folder the card acts on (nil: a loose session).
    public var folderId: String?
    /// The folder's own name: always set when the card belongs to a folder and the home is the root one.
    public var folderName: String?
    /// The folder's teaching kind (icon only).
    public var folderKind: String?
    public var sessionId: String?
    /// Session title (continue).
    public var title: String
    /// Due items (due).
    public var count: Int
    public var topics: [String]

    public var id: String { kind.rawValue + ":" + (sessionId ?? folderId ?? "") }

    public init(kind: Kind, folderId: String? = nil, folderName: String? = nil, folderKind: String? = nil,
                sessionId: String? = nil, title: String = "", count: Int = 0, topics: [String] = []) {
        self.kind = kind; self.folderId = folderId; self.folderName = folderName; self.folderKind = folderKind
        self.sessionId = sessionId; self.title = title; self.count = count; self.topics = topics
    }
}

public enum HomePlanner {
    /// What the phone knows about a folder (from the server lists or the replica).
    public struct LocalFolder: Sendable, Equatable {
        public var id: String
        public var name: String
        public var kind: String
        /// Topics due for review in this folder (from its own `review`).
        public var dueTopics: [String]
        public var hasKnowledge: Bool
        public init(id: String, name: String, kind: String = "chung", dueTopics: [String] = [], hasKnowledge: Bool = false) {
            self.id = id; self.name = name; self.kind = kind; self.dueTopics = dueTopics; self.hasKnowledge = hasKnowledge
        }
    }

    public struct LocalSession: Sendable, Equatable {
        public var sessionId: String
        public var title: String
        public var folderId: String?
        public var updatedAt: String
        public init(sessionId: String, title: String, folderId: String? = nil, updatedAt: String = "") {
            self.sessionId = sessionId; self.title = title; self.folderId = folderId; self.updatedAt = updatedAt
        }
    }

    public static let maxDue = 3
    public static let maxQuick = 2

    /// Cards for the home screen of `context` (a folder id, or nil for the root home).
    ///
    /// - The server's `home` frame is used only when it answers this very context; inside a folder anything it
    ///   carries about another folder is dropped (defence against a stale frame or a server bug).
    /// - Without a matching frame (an older server, offline, or the frame for the new context not here yet) the
    ///   cards come from local data, scoped the same way: inside a folder only that folder's sessions and review.
    /// - At the root every folder card carries the folder's name; a card whose folder name is unknown is dropped
    ///   rather than shown unlabelled.
    public static func cards(context rawContext: String?, home: HomeFrame?, folders: [LocalFolder],
                             sessions: [LocalSession]) -> [HomeCard] {
        let context = rawContext.flatMap { $0.isEmpty ? nil : $0 }
        let byId = Dictionary(folders.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        func inScope(_ folderId: String?) -> Bool { context == nil || folderId == context }
        func name(_ id: String?, _ given: String?) -> String? {
            guard let id else { return nil }
            if let n = byId[id]?.name, !n.isEmpty { return n }
            if let g = given, !g.isEmpty { return g }
            return nil
        }

        var out: [HomeCard] = []
        if let home, home.folderId == context {
            if let c = home.continueItem, inScope(c.folderId) {
                // Inside a folder `continue` must be a session of that folder: one without a folder is out too.
                if context == nil || c.folderId != nil {
                    out.append(HomeCard(kind: .continueSession, folderId: c.folderId, folderName: name(c.folderId, c.folderName),
                                        folderKind: c.folderId.flatMap { byId[$0]?.kind }, sessionId: c.sessionId, title: c.title))
                }
            }
            for d in home.due where inScope(d.folderId) {
                guard let n = name(d.folderId, d.folderName) else { continue }
                if out.contains(where: { $0.kind == .due && $0.folderId == d.folderId }) { continue }
                out.append(HomeCard(kind: .due, folderId: d.folderId, folderName: n, folderKind: d.kind,
                                    count: max(d.count, d.topics.count), topics: d.topics))
                if out.filter({ $0.kind == .due }).count >= maxDue { break }
            }
            for q in home.quick where inScope(q.folderId) {
                guard let n = name(q.folderId, q.folderName) else { continue }
                // A folder already offered for its due items needs no second "quick review" card.
                if out.contains(where: { ($0.kind == .due || $0.kind == .quick) && $0.folderId == q.folderId }) { continue }
                out.append(HomeCard(kind: .quick, folderId: q.folderId, folderName: n, folderKind: q.kind))
                if out.filter({ $0.kind == .quick }).count >= maxQuick { break }
            }
            return out
        }

        // Local fallback, scoped.
        let scopedSessions = sessions.filter { inScope($0.folderId) && (context == nil || $0.folderId != nil) }
        if let recent = scopedSessions.max(by: { $0.updatedAt < $1.updatedAt }) {
            out.append(HomeCard(kind: .continueSession, folderId: recent.folderId, folderName: name(recent.folderId, nil),
                                folderKind: recent.folderId.flatMap { byId[$0]?.kind }, sessionId: recent.sessionId,
                                title: recent.title))
        }
        if let context {
            guard let f = byId[context] else { return out }
            if !f.dueTopics.isEmpty {
                out.append(HomeCard(kind: .due, folderId: f.id, folderName: f.name, folderKind: f.kind,
                                    count: f.dueTopics.count, topics: f.dueTopics))
            } else if f.hasKnowledge {
                out.append(HomeCard(kind: .quick, folderId: f.id, folderName: f.name, folderKind: f.kind))
            }
        } else {
            let due = folders.filter { !$0.dueTopics.isEmpty && !$0.name.isEmpty }
                .sorted { $0.dueTopics.count != $1.dueTopics.count ? $0.dueTopics.count > $1.dueTopics.count : $0.id < $1.id }
                .prefix(maxDue)
            for f in due {
                out.append(HomeCard(kind: .due, folderId: f.id, folderName: f.name, folderKind: f.kind,
                                    count: f.dueTopics.count, topics: f.dueTopics))
            }
        }
        return out
    }
}

/// The greeting's second line may talk about review work, which belongs to one folder.
public enum GreetingScope {
    /// Whether `sub` of a greeting asked for `greetingContext` may be shown in `context`.
    /// A v2 server does not scope `sub` to a folder (it may name another subject), so inside a folder it is hidden.
    public static func showsSub(greetingContext: String?, context: String?, serverVersion: Int) -> Bool {
        let g = greetingContext.flatMap { $0.isEmpty ? nil : $0 }
        let c = context.flatMap { $0.isEmpty ? nil : $0 }
        if serverVersion < 3 { return c == nil && g == nil }
        return g == c
    }
}

/// The knowledge page is one web view shared by every folder.
public enum KnowledgeScope {
    /// The page may be shown for `wanted` only once it was last drawn for that folder and the folder's data is
    /// known (arrived from the server or the replica, empty included). Until then the screen shows a neutral
    /// placeholder, never the previous folder's page.
    public static func pageVisible(renderedFolder: String?, wanted: String, dataKnown: Bool) -> Bool {
        renderedFolder == wanted && dataKnown
    }

    /// Frames that carry a folder id are only applied to the screen showing that folder.
    public static func applies(frameFolder: String, to shown: String?) -> Bool {
        !frameFolder.isEmpty && frameFolder == shown
    }
}
