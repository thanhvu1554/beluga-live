import Foundation
import Testing
@testable import BelugaCore

/// Replica bookkeeping (PROTOCOL section 19): manifest parsing, the pull/drop plan, the bounded resumable queue,
/// the offline edit queue, offline exam pages.
@Suite struct ReplicaTests {
    let manifestJSON = #"""
    {"v":1,"serverTime":"2026-11-15T10:00:00Z","profile":{"etag":"p1"},
     "folders":[{"id":"f1","etag":"a"},{"id":"f2","etag":"b"},{"etag":"no id"},{"id":"f1","etag":"dup"}],
     "sessions":[{"sessionId":"s1","etag":"12-1700","folderId":"f1"},{"sessionId":"s2","etag":"3-1600"}],
     "voices":{"etag":"v9"}}
    """#

    @Test func manifestParses() throws {
        let m = try #require(SyncManifest.from(Data(manifestJSON.utf8)))
        #expect(m.profileEtag == "p1" && m.voicesEtag == "v9" && m.serverTime == "2026-11-15T10:00:00Z")
        #expect(m.folders.map(\.id) == ["f1", "f2"] && m.folders[0].etag == "a")
        #expect(m.sessions.map(\.id) == ["s1", "s2"] && m.sessions[0].folderId == "f1" && m.sessions[1].folderId == nil)
        // Not a manifest: an HTML error page, a JSON error, junk.
        #expect(SyncManifest.from(Data("<html>404</html>".utf8)) == nil)
        #expect(SyncManifest.from(Data(#"{"error":"not found"}"#.utf8)) == nil)
        #expect(SyncManifest.from(Data()) == nil)
    }

    @Test func syncItemKeysRoundTrip() {
        for item in [SyncItem.profile, .folder("f-1:x"), .session("s/1")] {
            #expect(SyncItem(key: item.key) == item)
        }
        #expect(SyncItem(key: "folder:") == nil && SyncItem(key: "junk") == nil)
        #expect(SyncItem.folder("f1").path == "folder/f1" && SyncItem.session("s1").path == "session/s1" && SyncItem.profile.path == "profile")
        #expect([SyncItem.session("a"), .folder("b"), .profile].sorted() == [.profile, .folder("b"), .session("a")])
    }

    @Test func firstSyncPullsEverything() throws {
        let m = try #require(SyncManifest.from(Data(manifestJSON.utf8)))
        let plan = EtagBook().plan(against: m)
        #expect(plan.pull == [.profile, .folder("f1"), .folder("f2"), .session("s1"), .session("s2")])
        #expect(plan.drop.isEmpty)
    }

    @Test func laterSyncPullsOnlyChangesAndDropsGoneParts() throws {
        let m = try #require(SyncManifest.from(Data(manifestJSON.utf8)))
        var book = EtagBook()
        book.record(.profile, etag: "p1")
        book.record(.folder("f1"), etag: "a")
        book.record(.folder("f2"), etag: "old")
        book.record(.folder("f-deleted"), etag: "x")
        book.record(.session("s1"), etag: "12-1700")
        book.record(.session("s-gone"), etag: "1-1")
        let plan = book.plan(against: m)
        #expect(plan.pull == [.folder("f2"), .session("s2")])
        #expect(plan.drop == [.folder("f-deleted"), .session("s-gone")])
        // Up to date: nothing to do.
        var all = book
        all.record(.folder("f2"), etag: "b"); all.record(.session("s2"), etag: "3-1600")
        all.forget(.folder("f-deleted")); all.forget(.session("s-gone"))
        #expect(all.plan(against: m).isEmpty)
    }

    @Test func emptyEtagAlwaysPulls() {
        var book = EtagBook()
        book.record(.folder("f1"), etag: "")
        let m = SyncManifest(folders: [.init(id: "f1", etag: "")])
        #expect(book.plan(against: m).pull == [.folder("f1")])
    }

    @Test func interruptedSyncResumes() throws {
        // Only parts whose file was written got an etag; the next plan asks for the rest.
        let m = try #require(SyncManifest.from(Data(manifestJSON.utf8)))
        var book = EtagBook()
        book.record(.profile, etag: "p1")
        book.record(.folder("f1"), etag: "a")
        #expect(book.plan(against: m).pull == [.folder("f2"), .session("s1"), .session("s2")])
    }

    @Test func etagBookIsCodable() throws {
        var book = EtagBook(lastSyncAt: Date(timeIntervalSince1970: 1_700_000_000), serverTime: "t")
        book.record(.folder("f1"), etag: "a")
        let back = try JSONDecoder().decode(EtagBook.self, from: JSONEncoder().encode(book))
        #expect(back == book && back.items == [.folder("f1")])
    }

    @Test func queueBoundsConcurrency() {
        var q = SyncQueue(maxInFlight: 2, maxAttempts: 2)
        q.add([.profile, .folder("a"), .folder("b"), .folder("a")])
        #expect(q.total == 3)
        let a = q.next(), b = q.next()
        #expect(a == .profile && b == .folder("a"))
        #expect(q.next() == nil)                     // both slots busy
        q.finish(.profile, ok: true)
        #expect(q.next() == .folder("b"))
        q.add([.folder("b")])                        // already running: not queued twice
        #expect(q.pending.isEmpty && q.total == 3)
        q.finish(.folder("a"), ok: false)            // retried later
        #expect(q.pending == [.folder("a")])
        #expect(q.next() == .folder("a"))
        q.finish(.folder("a"), ok: false)            // second failure: given up
        q.finish(.folder("b"), ok: true)
        #expect(q.isIdle && q.done == 3 && q.failed == [.folder("a")])
        // A new round starts its count afresh.
        q.add([.session("s")])
        #expect(q.total == 1 && q.done == 0 && q.failed.isEmpty)
    }

    @Test func queueCancelKeepsRunningPulls() {
        var q = SyncQueue(maxInFlight: 1)
        q.add([.folder("a"), .folder("b"), .folder("c")])
        _ = q.next()
        q.cancelPending()
        #expect(q.pending.isEmpty && q.inFlight == [.folder("a")] && q.total == 1)
        q.finish(.folder("a"), ok: true)
        #expect(q.isIdle && q.done == 1)
        q.finish(.folder("zzz"), ok: true)           // unknown: ignored
        #expect(q.done == 1)
    }

    // MARK: Offline edits

    @Test func offlineEditsReplayInOrderWithClientIds() throws {
        var q = OpQueue()
        let cid = OpQueue.newClientId()
        #expect(OpQueue.isLocal(cid) && !OpQueue.isLocal("f1") && !OpQueue.isLocal(nil))
        q.enqueue(.createFolder(clientId: cid, name: "Anh trên lớp", about: ""))
        q.enqueue(.moveSession(sessionId: "s1", folderId: cid))
        q.enqueue(.renameSession(sessionId: "s2", title: "A"))
        #expect(q.ops.count == 3)
        // The create carries the clientId so a replay never makes two folders.
        #expect(try JSONValue.parse(q.first!.message.text) == ["t": "create_folder", "name": "Anh trên lớp", "clientId": .string(cid)])
        // The server answered: later edits follow the real id.
        q.removeFirst()
        q.resolve(clientId: cid, to: "f9")
        #expect(q.first?.kind == .moveSession(sessionId: "s1", folderId: "f9"))
        q.removeFirst()
        #expect(q.first?.message == .renameSession(sessionId: "s2", title: "A"))
        q.removeFirst()
        #expect(q.isEmpty)
        q.removeFirst()                               // harmless when empty
        #expect(q.isEmpty)
    }

    @Test func offlineEditsFold() {
        var q = OpQueue()
        q.enqueue(.createFolder(clientId: "local-1", name: "Anh", about: ""))
        q.enqueue(.updateFolder(id: "local-1", name: "Anh 9", about: nil))
        q.enqueue(.updateFolder(id: "local-1", name: nil, about: "Global Success"))
        #expect(q.ops.map(\.kind) == [.createFolder(clientId: "local-1", name: "Anh 9", about: "Global Success")])
        q.enqueue(.updateFolder(id: "f2", name: "X", about: nil))
        q.enqueue(.updateFolder(id: "f2", name: nil, about: "y"))
        #expect(q.ops.last?.kind == .updateFolder(id: "f2", name: "X", about: "y") && q.ops.count == 2)
        q.enqueue(.renameSession(sessionId: "s1", title: "A"))
        q.enqueue(.renameSession(sessionId: "s1", title: "B"))
        #expect(q.ops.count == 3 && q.ops.last?.kind == .renameSession(sessionId: "s1", title: "B"))
        q.enqueue(.deleteSession(sessionId: "s1"))
        q.enqueue(.renameSession(sessionId: "s1", title: "C"))
        q.enqueue(.moveSession(sessionId: "s1", folderId: nil))
        q.enqueue(.deleteSession(sessionId: "s1"))
        #expect(q.ops.count == 4)
    }

    @Test func overlayShowsOfflineEdits() {
        var q = OpQueue()
        q.enqueue(.createFolder(clientId: "local-1", name: "Anh trên lớp", about: "Global Success"))
        q.enqueue(.updateFolder(id: "f1", name: "Toán 9 (mới)", about: nil))
        q.enqueue(.renameSession(sessionId: "s1", title: "Đổi tên"))
        q.enqueue(.moveSession(sessionId: "s2", folderId: "local-1"))
        q.enqueue(.deleteSession(sessionId: "s3"))
        let folders = q.overlay(folders: [Folder(id: "f1", name: "Toán 9", subject: "toan")])
        #expect(folders.map(\.name) == ["Toán 9 (mới)", "Anh trên lớp"])
        #expect(folders[1].pending && folders[1].about == "Global Success" && !folders[0].pending)
        let sessions = q.overlay(sessions: [SessionItem(sessionId: "s1", title: "Cũ", updatedAt: ""),
                                            SessionItem(sessionId: "s2", title: "Hai", updatedAt: ""),
                                            SessionItem(sessionId: "s3", title: "Ba", updatedAt: "")])
        #expect(sessions.map(\.title) == ["Đổi tên", "Hai"] && sessions[1].folderId == "local-1")
        // Once the server list has the new folder, it is not appended twice.
        #expect(q.overlay(folders: [Folder(id: "local-1", name: "Anh trên lớp")]).count == 1)
    }

    @Test func opQueueIsCodable() throws {
        var q = OpQueue()
        q.enqueue(.createFolder(clientId: "local-1", name: "A", about: "b"))
        q.enqueue(.moveSession(sessionId: "s", folderId: nil))
        q.enqueue(.updateFolder(id: "f", name: nil, about: "z"))
        let back = try JSONDecoder().decode(OpQueue.self, from: JSONEncoder().encode(q))
        #expect(back == q)
    }

    @Test func offlineExamPagePointsAtTheLocalBoard() {
        let html = #"<link href="{{BOARD}}/vendor/katex/katex.min.css"><script src="/board/vendor/katex/katex.min.js"></script><style>@font-face{src:url(/board/vendor/f.woff2)}</style>"#
        let out = OfflineExam.localize(html, boardPath: "board/")
        #expect(out == #"<link href="board/vendor/katex/katex.min.css"><script src="board/vendor/katex/katex.min.js"></script><style>@font-face{src:url(board/vendor/f.woff2)}</style>"#)
    }
}
