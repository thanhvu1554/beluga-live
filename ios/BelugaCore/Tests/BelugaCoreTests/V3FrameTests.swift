import Foundation
import Testing
@testable import BelugaCore

/// Protocol v3 frames (docs/PROTOCOL.md sections 17-21), both directions, and their tolerance of older servers.
@Suite struct V3FrameTests {
    private func decode(_ s: String) throws -> ServerMessage { try ServerMessage.decode(s) }
    private func json(_ m: ClientMessage) throws -> JSONValue { try JSONValue.parse(m.text) }

    @Test func helloSaysVersionThree() throws {
        let hello = try json(.hello(pairCode: nil, pairToken: "t", device: "iPhone", sessionId: nil, lastSeq: nil,
                                    settings: TutorSettings()))
        #expect(hello["v"]?.int == 3)
    }

    @Test func foldersCarryAboutAndKind() throws {
        guard case .folders(let f) = try decode(#"""
        {"t":"folders","items":[
          {"id":"f1","name":"Tiếng Anh trên lớp","about":"Anh 9 theo sách Global Success","kind":"ngoai-ngu","subject":"ngoai-ngu","sessionCount":2},
          {"id":"f2","name":"Toán 9","subject":"toan"},
          {"id":"f3","name":"Không gì"}]}
        """#) else { Issue.record("not folders"); return }
        #expect(f[0].name == "Tiếng Anh trên lớp" && f[0].about == "Anh 9 theo sách Global Success" && f[0].kind == "ngoai-ngu")
        // A v2 server: no kind, no about; the kind follows the subject.
        #expect(f[1].kind == "toan" && f[1].about.isEmpty && f[1].subject == "toan")
        #expect(f[2].kind == "chung" && f[2].subject == "chung" && !f[2].pending)
        #expect(Folder.kinds.contains("ngoai-ngu") && Folder.kinds.count == 9)
        // The wire shape round-trips (the replica stores lists that way).
        #expect(Folder.from(f[0].json) == f[0])
        var p = Folder(id: "local-1", name: "Mới", pending: true)
        p.about = "x"
        #expect(Folder.from(p.json) == p)
    }

    @Test func folderCreatedEchoesClientId() throws {
        #expect(try decode(#"{"t":"folder_created","id":"f9","clientId":"local-a"}"#) == .folderCreated(id: "f9", clientId: "local-a"))
        #expect(try decode(#"{"t":"folder_created","id":"f9","clientId":""}"#) == .folderCreated(id: "f9", clientId: nil))
    }

    @Test func folderFramesToServer() throws {
        #expect(try json(.createFolder(name: "Tiếng Anh trên lớp", about: "Global Success", clientId: "local-1"))
                == ["t": "create_folder", "name": "Tiếng Anh trên lớp", "about": "Global Success", "clientId": "local-1"])
        // No subject and no about unless given: a v3 server picks the kind from the name.
        #expect(try json(.createFolder(name: "Anh")) == ["t": "create_folder", "name": "Anh"])
        #expect(try json(.updateFolder(id: "f1", name: "Anh 9", about: nil)) == ["t": "update_folder", "id": "f1", "name": "Anh 9"])
        // An empty about clears it (sent explicitly).
        #expect(try json(.updateFolder(id: "f1", name: nil, about: "")) == ["t": "update_folder", "id": "f1", "about": ""])
    }

    @Test func homeFrame() throws {
        guard case .home(let h) = try decode(#"""
        {"t":"home","folderId":"f1",
         "continue":{"sessionId":"s1","title":"Anh · Thì hiện tại","folderId":"f1","folderName":"Tiếng Anh trên lớp","updatedAt":"2026-11-15T01:00:00Z"},
         "due":[{"folderId":"f1","folderName":"Tiếng Anh trên lớp","kind":"ngoai-ngu","count":2,"topics":["Câu bị động","Câu điều kiện"]},
                {"folderName":"no id"},{"folderId":"f1","count":9}],
         "quick":[{"folderId":"f1","folderName":"Tiếng Anh trên lớp","kind":"ngoai-ngu"}]}
        """#) else { Issue.record("not home"); return }
        #expect(h.folderId == "f1" && h.continueItem?.sessionId == "s1" && h.continueItem?.folderName == "Tiếng Anh trên lớp")
        #expect(h.due.count == 1 && h.due[0].count == 2 && h.due[0].topics == ["Câu bị động", "Câu điều kiện"])
        #expect(h.quick.map(\.folderId) == ["f1"])
        guard case .home(let empty) = try decode(#"{"t":"home","continue":{"title":"no id"},"due":"oops"}"#) else { Issue.record("not home"); return }
        #expect(empty == HomeFrame())
        #expect(try json(.getHome(folderId: "f1")) == ["t": "get_home", "folderId": "f1"])
        #expect(try json(.getHome(folderId: nil)) == ["t": "get_home"])
        #expect(try json(.getGreeting(localTime: "2026-11-15T20:00", name: nil, folderId: "f1"))
                == ["t": "get_greeting", "localTime": "2026-11-15T20:00", "folderId": "f1"])
    }

    @Test func syncAndBackupFrames() throws {
        guard case .sync(let s) = try decode(#"{"t":"sync","folders":["f1",""],"sessions":["s1"],"profile":true}"#) else {
            Issue.record("not sync"); return
        }
        #expect(s.folders == ["f1"] && s.sessions == ["s1"] && s.profile)
        #expect(s.items == [.profile, .folder("f1"), .session("s1")])
        guard case .sync(let none) = try decode(#"{"t":"sync"}"#) else { Issue.record("not sync"); return }
        #expect(none.items.isEmpty)

        guard case .backup(let b) = try decode(#"{"t":"backup","enabled":true,"provider":"r2","lastAt":"2026-11-15T03:00:00Z","lastBytes":482133,"nextAt":"2026-11-16T03:00:00Z"}"#) else {
            Issue.record("not backup"); return
        }
        #expect(b.enabled && b.lastBytes == 482133 && b.lastError == nil && b.nextAt == "2026-11-16T03:00:00Z")
        guard case .backup(let off) = try decode(#"{"t":"backup","enabled":false}"#) else { Issue.record("not backup"); return }
        #expect(!off.enabled && off.lastAt == nil && off.provider == "r2")
        #expect(try json(.getBackup) == ["t": "get_backup"])
        #expect(try json(.backupNow) == ["t": "backup_now"])
    }

    @Test func voicesCarryFillers() throws {
        guard case .voices(let c) = try decode(#"""
        {"t":"voices","current":"v1","provider":"genvoice",
         "fillers":[{"id":"s1","text":"Để tớ xem nào","when":"short"},{"id":"l1","text":"Bài này hơi dài","when":"long"},
                    {"id":"h1","text":"A lô, tớ đây","when":"hello"},{"id":"x","when":"weird"},{"text":"no id"}],
         "items":[{"id":"v1","name":"Chi","preview":true,"fillers":["s1","h1","s1",""]},{"id":"v2","name":"Phong","fillers":["l1"]}]}
        """#) else { Issue.record("not voices"); return }
        #expect(c.fillers.map(\.id) == ["s1", "l1", "h1", "x"])
        #expect(c.fillers[3].when == .other)
        #expect(c.items[0].fillers == ["s1", "h1"])
        // Only the current voice's clips count.
        #expect(c.currentFillers.map(\.id) == ["s1", "h1"])
        var device = c
        device.provider = "device"
        #expect(device.currentFillers.isEmpty)
        // A v2 catalogue has none.
        guard case .voices(let old) = try decode(#"{"t":"voices","current":"v1","items":[{"id":"v1","name":"Chi"}]}"#) else { return }
        #expect(old.fillers.isEmpty && old.currentFillers.isEmpty)
    }

    @Test func audioSniff() {
        let id3 = Data([0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05]) + Data([1, 2, 3, 4, 5]) + Data([0xFF, 0xFB, 0x90, 0x64])
        #expect(AudioSniff.kind(id3) == .id3)
        #expect(AudioSniff.id3Size(id3) == 15)
        #expect(AudioSniff.strippingID3(id3) == Data([0xFF, 0xFB, 0x90, 0x64]))
        #expect(AudioSniff.isMP3(id3))
        #expect(AudioSniff.kind(Data([0xFF, 0xFB, 0x90])) == .mpeg)
        #expect(AudioSniff.kind(Data()) == .empty && !AudioSniff.isMP3(Data()))
        #expect(AudioSniff.kind(Data("<!doctype html>".utf8)) == .text)
        #expect(AudioSniff.kind(Data(#"{"error":"x"}"#.utf8)) == .text && !AudioSniff.isMP3(Data("{}".utf8)))
        // A tag with nothing after it is not a playable clip.
        #expect(!AudioSniff.isMP3(Data([0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])))
        // Footer flag adds 10 bytes; a size byte with the top bit set is not a valid tag.
        #expect(AudioSniff.id3Size(Data([0x49, 0x44, 0x33, 0x04, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01])) == 21)
        #expect(AudioSniff.id3Size(Data([0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x80, 0x00, 0x00, 0x01])) == 0)
        #expect(AudioSniff.strippingID3(Data([1, 2])) == Data([1, 2]))
    }
}
