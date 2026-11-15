import Foundation
import Testing
@testable import BelugaCore

@Suite struct VoicesTests {
    private func decode(_ s: String) throws -> ServerMessage { try ServerMessage.decode(s) }
    private func json(_ m: ClientMessage) throws -> JSONValue { try JSONValue.parse(m.text) }

    @Test func decodesCatalogue() throws {
        let m = try decode(#"""
        {"t":"voices","type":"voices","current":"5yEPTrA0uAhZsBEhHjfi","provider":"genvoice","items":[
          {"id":"5yEPTrA0uAhZsBEhHjfi","name":"Chi","gender":"female","accent":"north","desc":"Nữ trẻ miền Bắc","preview":true},
          {"id":"JxmKvRaNYFidf0N27Vng","name":"Sơn","gender":"male","accent":"central","desc":"Nam trẻ miền Trung","preview":false},
          {"id":"x1","name":"","gender":"other","accent":""},
          {"id":"x1","name":"Lặp lại"},
          {"name":"Không id"}]}
        """#)
        guard case .voices(let c) = m else { Issue.record("not voices: \(m)"); return }
        #expect(c.provider == "genvoice" && !c.isDevice)
        #expect(c.current == "5yEPTrA0uAhZsBEhHjfi")
        #expect(c.items.map(\.id) == ["5yEPTrA0uAhZsBEhHjfi", "JxmKvRaNYFidf0N27Vng", "x1"])
        #expect(c.items[0].gender == .female && c.items[0].accent == .north && c.items[0].preview)
        #expect(c.items[1].gender == .male && c.items[1].accent == .central && !c.items[1].preview)
        // Unknown gender/accent and a missing name are tolerated.
        #expect(c.items[2].name == "x1" && c.items[2].gender == .unknown && c.items[2].accent == .unknown && c.items[2].desc == "")
    }

    @Test func deviceProviderHasNoItems() throws {
        guard case .voices(let c) = try decode(#"{"t":"voices","items":[],"current":"","provider":"device"}"#) else {
            Issue.record("not voices"); return
        }
        #expect(c.isDevice && c.items.isEmpty && c.current == nil)
        guard case .voices(let bare) = try decode(#"{"t":"voices"}"#) else { Issue.record("not voices"); return }
        #expect(bare.items.isEmpty && bare.provider == "")
    }

    @Test func encodesRequestAndChoice() throws {
        let get = try json(.getVoices)
        #expect(get["t"]?.string == "get_voices")
        // Settings carry `voice` only when one was picked (the server applies hello.settings.voice too).
        let plain = try json(.settings(TutorSettings(model: "sonnet")))
        #expect(plain["voice"] == nil)
        let picked = try json(.settings(TutorSettings(model: "sonnet", voice: "RxhjHDfpO54FYotYtKpw")))
        #expect(picked["t"]?.string == "settings" && picked["voice"]?.string == "RxhjHDfpO54FYotYtKpw")
        let hello = try json(.hello(pairCode: nil, pairToken: "t", device: "iPhone", sessionId: nil, lastSeq: nil,
                                    settings: TutorSettings()))
        #expect(hello["settings"]?["voice"] == nil)
    }
}

@Suite struct ServerDataSafetyTests {
    @Test func numbersNeverTrap() throws {
        // Int(Double) traps on NaN, infinity and values beyond Int: none of these may crash the decoder.
        let v = try JSONValue.parse(#"{"a":1e300,"b":-1e300,"c":"nan","d":"inf","e":"12","f":3.9,"g":1731600000000}"#)
        #expect(v["a"]?.int == Int.max && v["b"]?.int == Int.min)
        #expect(v["c"]?.integer == nil && v["d"]?.integer == nil)
        #expect(v["e"]?.integer == 12 && v["f"]?.int == 3)
        #expect(v["a"]?.dateText != nil && v["g"]?.dateText == "1731600000000")
        let ingest = ServerMessage.decode(try JSONValue.parse(#"{"t":"ingest","folderId":"f","state":"reading","done":"nan","total":1e40}"#))
        guard case .ingest(let i) = ingest else { Issue.record("not ingest"); return }
        #expect(i.done == 0 && i.total == Int.max)
        let p = LearningProgress.from(try JSONValue.parse(#"{"subjects":[{"id":"toan","weak":[{"topic":"a","count":1e300,"lastSeen":1e300}]}]}"#))
        #expect(p.subjects.first?.weakTopics.first?.count == Int.max)
    }

    @Test func repeatedIdsAreDropped() throws {
        // SwiftUI lists abort on repeated ids; the server builds these lists from hand-editable memory and LLM output.
        let review = ServerMessage.decode(try JSONValue.parse(#"""
        {"t":"review","folderId":"f","topics":[{"topicId":"c1.t1","title":"A"},{"topicId":"c1.t1","title":"A'"},{"title":"B"}],
         "weak":[{"topic":"x"},{"topic":"x"}],"due":[{"topic":"y"},{"topic":"y"}],
         "sessions":[{"sessionId":"s1"},{"sessionId":"s1"}]}
        """#))
        guard case .review(let r) = review else { Issue.record("not review"); return }
        #expect(r.topics.map(\.topicId) == ["c1.t1", "B"] && r.topics[0].title == "A")
        #expect(r.weak.count == 1 && r.due.count == 1 && r.sessions.count == 1)

        let k = Knowledge.from(try JSONValue.parse(#"""
        {"curriculum":[{"id":"c1","topics":[{"id":"t"},{"id":"t"}]},{"id":"c1","topics":[]}],
         "structures":[{"id":"s1"},{"id":"s1"}]}
        """#))
        #expect(k?.chapters.count == 1 && k?.chapters.first?.topics.count == 1 && k?.structures.count == 1)

        let folders = ServerMessage.decode(try JSONValue.parse(#"{"t":"folders","items":[{"id":"a"},{"id":"a"},{"id":"b"}]}"#))
        if case .folders(let items) = folders { #expect(items.map(\.id) == ["a", "b"]) } else { Issue.record("not folders") }
        let sessions = ServerMessage.decode(try JSONValue.parse(#"{"t":"sessions","items":[{"sessionId":"a"},{"sessionId":"a"}]}"#))
        if case .sessions(let items) = sessions { #expect(items.count == 1) } else { Issue.record("not sessions") }
        let exams = ServerMessage.decode(try JSONValue.parse(#"{"t":"exams","folderId":"f","items":[{"examId":"e"},{"examId":"e"}]}"#))
        if case .exams(_, let items) = exams { #expect(items.count == 1) } else { Issue.record("not exams") }
        let user = ServerMessage.decode(try JSONValue.parse(#"{"t":"user","seq":1,"turn":1,"text":"","files":[{"fileId":"a"},{"fileId":"a"}]}"#))
        if case .event(let e) = user, case .user(_, let files, _) = e.kind { #expect(files.count == 1) } else { Issue.record("not user") }
        let ready = ServerMessage.decode(try JSONValue.parse(#"{"t":"ready","sessionId":"s","models":[{"id":"m","efforts":["low","low"]},{"id":"m"}]}"#))
        if case .ready(let r) = ready { #expect(r.models.count == 1 && r.models[0].efforts == ["low"]) } else { Issue.record("not ready") }
        let progress = LearningProgress.from(try JSONValue.parse(#"""
        {"subjects":[{"id":"toan","recent":[{"topic":"a","date":"2026-11-14"},{"topic":"a","date":"2026-11-14"}],
                      "weak":[{"topic":"w"},{"topic":"w"}]},{"id":"toan"}]}
        """#))
        #expect(progress.subjects.count == 1 && progress.subjects[0].recent.count == 1 && progress.subjects[0].weakTopics.count == 1)
    }
}
