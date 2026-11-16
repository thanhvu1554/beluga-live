import SwiftUI
import BelugaCore

/// Replica state the screens show (PROTOCOL section 19): quiet, only what the owner needs to know.
struct SyncStatus: Equatable {
    enum Phase: Equatable { case idle, syncing(done: Int, total: Int), failed }
    var phase: Phase = .idle
    /// Last complete round with the server (from the replica's book, so it survives restarts).
    var lastSyncAt: Date?
    /// Offline edits waiting to be sent.
    var pendingOps = 0
    /// The replica has something to show.
    var hasCopy = false
}

// The local replica and its sync (PROTOCOL section 19): what the phone shows comes from its own copy first and is
// replaced by the server's answer; offline edits wait in a queue and go out in order when the server is back.
extension LiveController {

    // MARK: Start

    /// Reads the replica (off the main thread) and lays it under whatever the server has not answered yet.
    func loadReplica() {
        let fresh = isMock && UserDefaults.standard.bool(forKey: "mockFreshReplica")
        let store = replica
        Task { [weak self] in
            if fresh { await store.wipe() }
            let snap = await store.load()
            self?.applyReplica(snap)
        }
    }

    private func applyReplica(_ snap: ReplicaStore.Snapshot) {
        book = snap.book
        ops = snap.ops
        sync.lastSyncAt = snap.book.lastSyncAt
        sync.pendingOps = ops.ops.count
        sync.hasCopy = !snap.book.isEmpty || snap.folders != nil
        // Server lists that already arrived are fresher than the copy.
        if !gotServerFolders, let list = snap.folders, case .folders(let items) = ServerMessage.decode(["t": "folders", "items": .array(list)]) {
            serverFolders = items
            for f in items { if let i = f.ingest { library.ingest[f.id] = i } }
        }
        if !gotServerSessions, let list = snap.sessions, case .sessions(let items) = ServerMessage.decode(["t": "sessions", "items": .array(list)]) {
            serverSessions = items
        }
        refreshOverlay()
        for (item, v) in snap.parts.sorted(by: { $0.key < $1.key }) { applyPart(item, v, fromServer: false) }
        Diagnostics.log("replica: loaded \(snap.book.etags.count) parts, \(ops.ops.count) pending edits")
        replicaLoaded = true
        // A `ready` that came first waited for the book and the queue.
        if connected { catchUp() }
    }

    /// After `ready`: edits made offline go out first, then the replica catches up with the server.
    func catchUp() {
        guard replicaLoaded else { return }
        Task { [weak self] in
            await self?.replayOps()
            self?.syncNow()
        }
    }

    // MARK: Lists

    /// The server's folder list, with offline edits laid over it.
    func applyServerFolders(_ items: [Folder]) {
        gotServerFolders = true
        serverFolders = items
        refreshOverlay()
        saveLists(folders: true)
    }

    func applyServerSessions(_ items: [SessionItem]) {
        gotServerSessions = true
        serverSessions = items
        refreshOverlay()
        saveLists(folders: false)
    }

    func refreshOverlay() {
        library.folders = ops.overlay(folders: serverFolders)
        library.sessions = ops.overlay(sessions: serverSessions)
        sync.pendingOps = ops.ops.count
    }

    private func saveLists(folders: Bool) {
        let store = replica
        let f: [JSONValue]? = folders ? serverFolders.map(\.json) : nil
        let s: [JSONValue]? = folders ? nil : serverSessions.map(Self.sessionJSON)
        Task { await store.saveLists(folders: f, sessions: s) }
    }

    static func sessionJSON(_ s: SessionItem) -> JSONValue {
        var o: [String: JSONValue] = ["sessionId": .string(s.sessionId), "title": .string(s.title),
                                      "updatedAt": .string(s.updatedAt), "mode": .string(s.mode.rawValue)]
        if let f = s.folderId { o["folderId"] = .string(f) }
        if let p = s.preview { o["preview"] = .string(p) }
        return .object(o)
    }

    // MARK: Parts

    /// Puts a part on the screens. From the replica at launch only fills what the server has not answered yet.
    func applyPart(_ item: SyncItem, _ v: JSONValue, fromServer: Bool) {
        switch item {
        case .folder(let id):
            if let k = v["knowledge"], fromServer || !library.knowledgeLoaded.contains(id) {
                handleLibrary(ServerMessage.decode(["t": "knowledge", "folderId": .string(id), "data": k]))
            }
            if let r = v["review"], r.object != nil, fromServer || library.review[id] == nil {
                var frame = r
                if case .object(var o) = r { o["t"] = "review"; o["folderId"] = .string(id); frame = .object(o) }
                handleLibrary(ServerMessage.decode(frame))
            }
            if let exams = v["exams"]?.array, fromServer || library.exams[id] == nil {
                handleLibrary(ServerMessage.decode(["t": "exams", "folderId": .string(id), "items": .array(exams)]))
            }
            // A folder the lists do not know yet (a launch without lists.json).
            if let f = v["folder"].flatMap(Folder.from), !serverFolders.contains(where: { $0.id == f.id }), !gotServerFolders {
                serverFolders.append(f)
                refreshOverlay()
            }
        case .profile:
            if case .object(var p)? = v["progress"], fromServer || progress == nil {
                p["t"] = "progress"
                if case .progress(let report) = ServerMessage.decode(.object(p)) { progress = report }
            }
        case .session:
            break
        }
    }

    // MARK: Sync rounds

    private var syncSource: SyncSource? {
        if isMock { return transport as? MockTransport }
        guard let base = settings.httpBase, let token = settings.pairToken, !token.isEmpty else { return nil }
        return HTTPSyncSource(base: base, token: token)
    }

    /// Pulls the manifest and whatever changed since the last round (each `ready` and each return to the
    /// foreground). Parts already on the phone are asked with their etag, so an unchanged one costs a 304.
    func syncNow() {
        guard connected, serverV3 || isMock, !syncRunning, let source = syncSource else { return }
        syncRunning = true
        sync.phase = .syncing(done: 0, total: 0)
        Task { [weak self] in
            let r = await source.fetch("manifest", etag: nil)
            guard let self else { return }
            guard case .body(let data, _) = r, let m = SyncManifest.from(data) else {
                Diagnostics.log("sync: manifest failed")
                self.syncRunning = false
                self.sync.phase = .failed
                return
            }
            let plan = self.book.plan(against: m)
            self.manifestEtags = [:]
            if let p = m.profileEtag { self.manifestEtags[.profile] = p }
            for f in m.folders { self.manifestEtags[.folder(f.id)] = f.etag }
            for s in m.sessions { self.manifestEtags[.session(s.id)] = s.etag }
            self.book.serverTime = m.serverTime
            for item in plan.drop {
                await self.replica.drop(item)
                self.book.forget(item)
            }
            Diagnostics.log("sync: manifest, pull \(plan.pull.count), drop \(plan.drop.count)")
            self.pull(plan.pull, source: source)
            if self.syncQueue.isIdle { self.finishRound() }
        }
    }

    /// `sync{…}` from the server: those parts changed.
    func syncHint(_ hint: SyncHint) {
        guard connected, let source = syncSource, !hint.items.isEmpty else { return }
        syncRunning = true
        pull(hint.items, source: source)
    }

    private func pull(_ items: [SyncItem], source: SyncSource) {
        syncQueue.add(items)
        pump(source)
    }

    private func pump(_ source: SyncSource) {
        updatePhase()
        while let item = syncQueue.next() {
            let etag = book.etag(item)
            Task { [weak self] in
                let r = await source.fetch(item.path, etag: etag)
                guard let self else { return }
                let ok = await self.store(item, r)
                self.syncQueue.finish(item, ok: ok)
                self.pump(source)
                if self.syncQueue.isIdle { self.finishRound() }
            }
        }
    }

    /// File first, then the etag (an interrupted round resumes by itself), then the screens.
    private func store(_ item: SyncItem, _ r: SyncFetch) async -> Bool {
        switch r {
        case .body(let data, let tag):
            await replica.save(item, body: data)
            let etag = manifestEtags[item] ?? tag ?? ""
            if etag.isEmpty { book.forget(item) } else { book.record(item, etag: etag) }
            if let v = try? JSONValue.parse(data) { applyPart(item, v, fromServer: true) }
            sync.hasCopy = true
            return true
        case .notModified:
            if let etag = manifestEtags[item] { book.record(item, etag: etag) }
            return true
        case .failed(let why):
            Diagnostics.log("sync: \(item.key) failed: \(why)")
            return false
        }
    }

    private func updatePhase() {
        guard !syncQueue.isIdle else { return }
        sync.phase = .syncing(done: syncQueue.done, total: max(syncQueue.total, syncQueue.done))
    }

    private func finishRound() {
        syncRunning = false
        let failed = !syncQueue.failed.isEmpty
        book.lastSyncAt = Date()
        sync.lastSyncAt = book.lastSyncAt
        sync.phase = failed ? .failed : .idle
        let store = replica, b = book
        Task { await store.saveBook(b) }
    }

    /// The connection went away mid-round: nothing new starts, running pulls finish or fail by themselves.
    func pauseSync() {
        syncQueue.cancelPending()
        syncRunning = false
        if case .syncing = sync.phase { sync.phase = .idle }
    }

    // MARK: Offline edits

    func enqueue(_ kind: PendingOp.Kind) {
        ops.enqueue(kind)
        refreshOverlay()
        let store = replica, q = ops
        Task { await store.saveOps(q) }
        Diagnostics.log("replica: queued edit (\(ops.ops.count) waiting)")
    }

    /// Back online: the queued edits go out in order. A folder created offline is waited for (its real id), so
    /// later edits that point at it follow it. Anything not confirmed stays queued for the next `ready`.
    func replayOps() async {
        guard !replaying, !ops.isEmpty else { return }
        replaying = true
        defer { replaying = false }
        Diagnostics.log("replica: replaying \(ops.ops.count) edits")
        while connected, let op = ops.first {
            if case let .createFolder(cid, name, about) = op.kind {
                let message: ClientMessage = serverV3 || isMock
                    ? op.message : .createFolder(name: name, about: about.isEmpty ? nil : about, subject: "chung")
                guard let id = await waitForFolder(clientId: cid, seconds: 6, after: message) else { break }
                ops.resolve(clientId: cid, to: id)
                ops.removeFirst()
                repointFolder(from: cid, to: id)
            } else {
                transport?.send(op.message)
                ops.removeFirst()
            }
            let store = replica, q = ops
            await store.saveOps(q)
            sync.pendingOps = ops.ops.count
        }
        refreshOverlay()
        transport?.send(.listFolders)
        transport?.send(.listSessions())
    }

    /// Sends `message` once the wait for its `folder_created` is in place (the answer can come back at once).
    private func waitForFolder(clientId: String, seconds: Double, after message: ClientMessage) async -> String? {
        await withCheckedContinuation { (c: CheckedContinuation<String?, Never>) in
            folderWaiters[clientId] = c
            transport?.send(message)
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(seconds))
                self?.folderWaiters.removeValue(forKey: clientId)?.resume(returning: nil)
            }
        }
    }

    /// `folder_created` for an edit the replay is waiting on (v2 servers do not echo the client id).
    func resolveFolderWaiter(id: String, clientId: String?) -> Bool {
        let key = clientId ?? (serverV3 ? nil : folderWaiters.keys.first)
        guard let key, let waiter = folderWaiters.removeValue(forKey: key) else { return false }
        waiter.resume(returning: id)
        return true
    }

    /// The folder made offline now has its server id: the open screen keeps showing it without a navigation.
    private func repointFolder(from local: String, to id: String) {
        if path.contains(.folder(local)) {
            var t = Transaction()
            t.disablesAnimations = true
            withTransaction(t) { path = path.map { $0 == .folder(local) ? .folder(id) : $0 } }
        }
        if folderId == local { folderId = id }
        if pendingFolderId == local { pendingFolderId = id }
        if knowledgeFolder == local { knowledgeFolder = nil }
    }

    // MARK: Sessions offline

    /// A session opened without the server: its transcript from the replica (read only until the server is back,
    /// when the full snapshot replaces it).
    func showReplicaSession(_ id: String) {
        let store = replica
        Task { [weak self] in
            guard let v = await store.session(id) else { return }
            guard let self, self.sessionId == id, !self.connected else { return }
            let events = (v["events"]?.array ?? []).compactMap(LessonEvent.from)
            guard !events.isEmpty else { return }
            if let t = v["title"]?.nonEmptyString { self.title = t }
            for e in events where e.isLiveEvent { self.liveTurns.insert(e.turn) }
            self.loadChatHistory(events)
        }
    }

    // MARK: Link indicator

    /// Offline is shown only once the link has been down for a moment: a Wi-Fi hand-over never flashes it.
    /// Set without a transaction: lists (the drawer, often off screen) must not batch-animate on it; the few
    /// places that show it animate locally.
    func linkChanged(up: Bool) {
        linkDownTask?.cancel()
        if up {
            if linkDown { linkDown = false }
            return
        }
        linkDownTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1.5))
            guard let self, !Task.isCancelled, !self.connected else { return }
            self.linkDown = true
        }
    }
}
