import Foundation
import BelugaCore

/// The phone's copy of the server's data (PROTOCOL section 19), in `Application Support/replica/` (backed up with
/// the phone): the etag book, the queue of offline edits, the last folder and session lists, and one file per part
/// (`/sync/folder/<id>`, `/sync/session/<id>`, `/sync/profile` bodies as the server sent them).
/// An actor: every read and write happens off the main thread, one at a time.
actor ReplicaStore {
    struct Snapshot: Sendable {
        var book = EtagBook()
        var ops = OpQueue()
        var folders: [JSONValue]?
        var sessions: [JSONValue]?
        /// Folder parts and the profile (sessions load on demand: they can be large).
        var parts: [SyncItem: JSONValue] = [:]
    }

    let dir: URL

    init(mock: Bool) {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        // Mock runs keep their own copy, so a simulator walk-through never mixes with a real server's data.
        dir = base.appendingPathComponent(mock ? "replica-mock" : "replica", isDirectory: true)
    }

    // MARK: Files

    private func url(_ name: String) -> URL { dir.appendingPathComponent(name) }

    static func fileName(_ item: SyncItem) -> String {
        let safe = item.key.map { c -> Character in c.isLetter || c.isNumber || c == "-" || c == "_" ? c : "_" }
        return "part-" + String(safe) + ".json"
    }

    private func read(_ name: String) -> Data? { try? Data(contentsOf: url(name)) }

    private func write(_ name: String, _ data: Data) {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? data.write(to: url(name), options: .atomic)
    }

    private func remove(_ name: String) { try? FileManager.default.removeItem(at: url(name)) }

    // MARK: API

    func load() -> Snapshot {
        var s = Snapshot()
        if let d = read("book.json"), let b = try? JSONDecoder().decode(EtagBook.self, from: d) { s.book = b }
        if let d = read("ops.json"), let q = try? JSONDecoder().decode(OpQueue.self, from: d) { s.ops = q }
        if let d = read("lists.json"), let v = try? JSONValue.parse(d) {
            s.folders = v["folders"]?.array
            s.sessions = v["sessions"]?.array
        }
        for item in s.book.items {
            if case .session = item { continue }
            if let d = read(Self.fileName(item)), let v = try? JSONValue.parse(d) { s.parts[item] = v }
        }
        return s
    }

    func session(_ id: String) -> JSONValue? {
        read(Self.fileName(.session(id))).flatMap { try? JSONValue.parse($0) }
    }

    /// A part as the server sent it (the caller records its etag once this returned: file first, then etag).
    func save(_ item: SyncItem, body: Data) {
        write(Self.fileName(item), body)
    }

    func drop(_ item: SyncItem) {
        remove(Self.fileName(item))
    }

    func saveBook(_ book: EtagBook) {
        if let d = try? JSONEncoder().encode(book) { write("book.json", d) }
    }

    func saveOps(_ ops: OpQueue) {
        if let d = try? JSONEncoder().encode(ops) { write("ops.json", d) }
    }

    func saveLists(folders: [JSONValue]?, sessions: [JSONValue]?) {
        var o: [String: JSONValue] = [:]
        if let d = read("lists.json"), let v = try? JSONValue.parse(d), let old = v.object { o = old }
        if let folders { o["folders"] = .array(folders) }
        if let sessions { o["sessions"] = .array(sessions) }
        write("lists.json", Data(JSONValue.object(o).jsonString.utf8))
    }

    func wipe() {
        try? FileManager.default.removeItem(at: dir)
    }
}

/// Where sync parts come from: the server over HTTP, or the mock.
@MainActor
protocol SyncSource: AnyObject {
    func fetch(_ path: String, etag: String?) async -> SyncFetch
}

enum SyncFetch: Sendable {
    case body(Data, etag: String?)
    case notModified
    case failed(String)
}

/// `GET /sync/<path>` with the pairing token and `If-None-Match` (PROTOCOL section 19).
@MainActor
final class HTTPSyncSource: SyncSource {
    private let base: URL
    private let token: String
    private let session: URLSession = {
        let c = URLSessionConfiguration.default
        c.timeoutIntervalForRequest = 20
        c.waitsForConnectivity = false
        c.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: c)
    }()

    init(base: URL, token: String) { self.base = base; self.token = token }

    func fetch(_ path: String, etag: String?) async -> SyncFetch {
        var c = URLComponents(url: base.appendingPathComponent("sync/" + path), resolvingAgainstBaseURL: false)
        c?.queryItems = [URLQueryItem(name: "token", value: token)]
        guard let url = c?.url else { return .failed("url") }
        var req = URLRequest(url: url)
        if let etag, !etag.isEmpty { req.setValue("\"\(etag)\"", forHTTPHeaderField: "If-None-Match") }
        do {
            let (data, response) = try await session.data(for: req)
            guard let http = response as? HTTPURLResponse else { return .failed("no response") }
            let tag = (http.value(forHTTPHeaderField: "ETag") ?? "")
                .trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "W/", with: "")
                .trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            switch http.statusCode {
            case 304: return .notModified
            case 200: return .body(data, etag: tag.isEmpty ? nil : tag)
            default: return .failed("HTTP \(http.statusCode)")
            }
        } catch {
            return .failed(error.localizedDescription)
        }
    }
}
