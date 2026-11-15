import Foundation
import Testing
@testable import BelugaCore

/// Folder isolation (PROTOCOL section 18): whatever the server or the local data hold about two folders, a screen
/// in one of them never selects anything of the other. Math is the "loud" folder here (more due items, the most
/// recent session, knowledge), so any leak toward it would be the old bug: English suggesting Math.
@Suite struct FolderScopeTests {
    let math = HomePlanner.LocalFolder(id: "f-toan", name: "Toán 9 · Giữa kỳ 1", kind: "toan",
                                       dueTopics: ["Rút gọn căn", "Điều kiện xác định", "Hệ số góc"], hasKnowledge: true)
    let english = HomePlanner.LocalFolder(id: "f-anh", name: "Tiếng Anh trên lớp", kind: "ngoai-ngu", dueTopics: [], hasKnowledge: false)
    var folders: [HomePlanner.LocalFolder] { [math, english] }
    let sessions = [
        HomePlanner.LocalSession(sessionId: "s-math-new", title: "Toán · Căn thức", folderId: "f-toan", updatedAt: "2026-11-15T09:00:00Z"),
        HomePlanner.LocalSession(sessionId: "s-eng", title: "Anh · Bị động", folderId: "f-anh", updatedAt: "2026-11-14T09:00:00Z"),
        HomePlanner.LocalSession(sessionId: "s-loose", title: "Sử · 1945", folderId: nil, updatedAt: "2026-11-15T10:00:00Z"),
    ]

    /// A frame for the root that mentions both folders.
    var rootFrame: HomeFrame {
        HomeFrame(folderId: nil,
                  continueItem: .init(sessionId: "s-loose", title: "Sử · 1945"),
                  due: [.init(folderId: "f-toan", folderName: "Toán 9 · Giữa kỳ 1", kind: "toan", count: 3, topics: ["Rút gọn căn"]),
                        .init(folderId: "f-anh", folderName: "Tiếng Anh trên lớp", kind: "ngoai-ngu", count: 1, topics: ["Bị động"])],
                  quick: [.init(folderId: "f-toan", folderName: "Toán 9 · Giữa kỳ 1"), .init(folderId: "f-anh", folderName: "Tiếng Anh trên lớp")])
    }

    private func assertOnly(_ cards: [HomeCard], folder: String) {
        for c in cards {
            #expect(c.folderId == folder)
            #expect(c.sessionId != "s-math-new" || folder == "f-toan")
            #expect(c.sessionId != "s-loose")
        }
    }

    @Test func englishFromItsOwnFrame() {
        let frame = HomeFrame(folderId: "f-anh",
                              continueItem: .init(sessionId: "s-eng", title: "Anh · Bị động", folderId: "f-anh", folderName: "Tiếng Anh trên lớp"),
                              due: [.init(folderId: "f-anh", folderName: "Tiếng Anh trên lớp", count: 1, topics: ["Bị động"])])
        let cards = HomePlanner.cards(context: "f-anh", home: frame, folders: folders, sessions: sessions)
        #expect(cards.map(\.kind) == [.continueSession, .due])
        assertOnly(cards, folder: "f-anh")
    }

    @Test func englishDropsAnythingOfMathEvenIfTheServerSentIt() {
        // A buggy or stale server frame for this context that still carries Math.
        var frame = rootFrame
        frame.folderId = "f-anh"
        frame.continueItem = .init(sessionId: "s-math-new", title: "Toán · Căn thức", folderId: "f-toan", folderName: "Toán 9 · Giữa kỳ 1")
        let cards = HomePlanner.cards(context: "f-anh", home: frame, folders: folders, sessions: sessions)
        assertOnly(cards, folder: "f-anh")
        #expect(cards.map(\.kind) == [.due])
    }

    @Test func englishIgnoresTheRootFrameAndFallsBackScoped() {
        // The frame on hand answers the root (the new one for English has not arrived): it is not used.
        let cards = HomePlanner.cards(context: "f-anh", home: rootFrame, folders: folders, sessions: sessions)
        assertOnly(cards, folder: "f-anh")
        #expect(cards.map(\.sessionId) == ["s-eng"])
    }

    @Test func englishWithoutAnyFrameNeverSuggestsMath() {
        // The old guess picked "the folder with knowledge updated last" = Math. Now: only English, and English has
        // no knowledge and nothing due, so no review card at all.
        let cards = HomePlanner.cards(context: "f-anh", home: nil, folders: folders, sessions: sessions)
        assertOnly(cards, folder: "f-anh")
        #expect(!cards.contains { $0.kind == .quick || $0.kind == .due })
    }

    @Test func mathFolderStaysInMath() {
        let cards = HomePlanner.cards(context: "f-toan", home: nil, folders: folders, sessions: sessions)
        assertOnly(cards, folder: "f-toan")
        #expect(cards.map(\.kind) == [.continueSession, .due])
        #expect(cards[1].count == 3)
        let fromRoot = HomePlanner.cards(context: "f-toan", home: rootFrame, folders: folders, sessions: sessions)
        assertOnly(fromRoot, folder: "f-toan")
    }

    @Test func folderWithNothingShowsNothing() {
        let empty = HomePlanner.LocalFolder(id: "f-new", name: "Mới")
        let cards = HomePlanner.cards(context: "f-new", home: nil, folders: folders + [empty], sessions: sessions)
        #expect(cards.isEmpty)
        let fromFrame = HomePlanner.cards(context: "f-new", home: HomeFrame(folderId: "f-new"), folders: folders + [empty], sessions: sessions)
        #expect(fromFrame.isEmpty)
    }

    @Test func rootLabelsEveryFolderCard() {
        let cards = HomePlanner.cards(context: nil, home: rootFrame, folders: folders, sessions: sessions)
        #expect(cards.map(\.kind) == [.continueSession, .due, .due])
        for c in cards where c.folderId != nil { #expect(c.folderName?.isEmpty == false) }
        // Quick cards for folders already offered as due are not repeated.
        #expect(!cards.contains { $0.kind == .quick })
        #expect(cards.first(where: { $0.folderId == "f-anh" })?.folderName == "Tiếng Anh trên lớp")
    }

    @Test func rootDropsCardsItCannotLabel() {
        let frame = HomeFrame(due: [.init(folderId: "f-gone", folderName: nil, count: 2)],
                              quick: [.init(folderId: "f-gone2", folderName: "")])
        #expect(HomePlanner.cards(context: nil, home: frame, folders: folders, sessions: sessions).isEmpty)
        // The local name wins over the server's (the owner may have just renamed it).
        let named = HomeFrame(due: [.init(folderId: "f-anh", folderName: "Tên cũ", count: 1)])
        #expect(HomePlanner.cards(context: nil, home: named, folders: folders, sessions: sessions).first?.folderName == "Tiếng Anh trên lớp")
    }

    @Test func rootFallbackSortsDueAndCaps() {
        var many = folders
        for i in 0..<5 {
            many.append(.init(id: "f\(i)", name: "F\(i)", dueTopics: Array(repeating: "x", count: i + 1)))
        }
        let cards = HomePlanner.cards(context: nil, home: nil, folders: many, sessions: sessions)
        let due = cards.filter { $0.kind == .due }
        #expect(due.count == HomePlanner.maxDue)
        // Most due first; a tie is broken by id, so the order is stable.
        #expect(due.map(\.folderId) == ["f4", "f3", "f-toan"])
        // Continue at the root is simply the latest session, here a loose one (no folder label needed).
        #expect(cards.first?.sessionId == "s-loose" && cards.first?.folderName == nil)
    }

    @Test func emptyContextMeansRoot() {
        let a = HomePlanner.cards(context: "", home: rootFrame, folders: folders, sessions: sessions)
        let b = HomePlanner.cards(context: nil, home: rootFrame, folders: folders, sessions: sessions)
        #expect(a == b)
    }

    @Test func greetingSubIsScoped() {
        // v3: the sub belongs to the context it was asked for.
        #expect(GreetingScope.showsSub(greetingContext: "f-anh", context: "f-anh", serverVersion: 3))
        #expect(!GreetingScope.showsSub(greetingContext: nil, context: "f-anh", serverVersion: 3))
        #expect(!GreetingScope.showsSub(greetingContext: "f-toan", context: "f-anh", serverVersion: 3))
        #expect(GreetingScope.showsSub(greetingContext: nil, context: nil, serverVersion: 3))
        // v2: never inside a folder (its sub may name another subject).
        #expect(!GreetingScope.showsSub(greetingContext: "f-anh", context: "f-anh", serverVersion: 2))
        #expect(GreetingScope.showsSub(greetingContext: nil, context: nil, serverVersion: 2))
    }

    @Test func knowledgePageNeverShowsTheOtherFolder() {
        // Math was drawn last; English is opened: hidden until English is drawn and its data is known.
        #expect(!KnowledgeScope.pageVisible(renderedFolder: "f-toan", wanted: "f-anh", dataKnown: true))
        #expect(!KnowledgeScope.pageVisible(renderedFolder: "f-anh", wanted: "f-anh", dataKnown: false))
        #expect(KnowledgeScope.pageVisible(renderedFolder: "f-anh", wanted: "f-anh", dataKnown: true))
        #expect(!KnowledgeScope.pageVisible(renderedFolder: nil, wanted: "f-anh", dataKnown: true))
        #expect(KnowledgeScope.applies(frameFolder: "f-anh", to: "f-anh"))
        #expect(!KnowledgeScope.applies(frameFolder: "f-toan", to: "f-anh"))
        #expect(!KnowledgeScope.applies(frameFolder: "", to: nil))
    }
}
