import Foundation
import Testing
@testable import BelugaCore

/// Protocol v2 frames (docs/PROTOCOL.md sections 7-15).
@Suite struct V2MessageTests {
    private func decode(_ s: String) throws -> ServerMessage { try ServerMessage.decode(s) }
    private func json(_ m: ClientMessage) throws -> JSONValue { try JSONValue.parse(m.text) }

    @Test func readyCarriesModelsModeAndFolder() throws {
        let m = try decode(#"""
        {"t":"ready","v":2,"sessionId":"s1","title":"","tts":"device","stt":"server","boardVersion":3,"folderId":"f1",
         "mode":"chat","model":"sonnet","effort":"medium","extra":{"x":1},
         "models":[{"id":"sonnet","label":"Claude Sonnet","provider":"claude","efforts":["low","medium","high"],"defaultEffort":"medium","vision":true,"tracked":true},
                   {"id":"deepseek","label":"DeepSeek","provider":"anthropic-compatible","efforts":[],"vision":false,"tracked":false},
                   {"label":"no id"}]}
        """#)
        guard case .ready(let r) = m else { Issue.record("not ready"); return }
        #expect(r.v == 2 && r.folderId == "f1" && r.mode == .chat && r.model == "sonnet" && r.effort == "medium")
        #expect(r.models.map(\.id) == ["sonnet", "deepseek"])
        #expect(r.models[0].efforts == ["low", "medium", "high"] && r.models[0].tracked)
        #expect(!r.models[1].vision && r.models[1].efforts.isEmpty)
        // v1 ready keeps working.
        guard case .ready(let old) = try decode(#"{"t":"ready","v":1,"sessionId":"a"}"#) else { Issue.record("not ready"); return }
        #expect(old.models.isEmpty && old.mode == .chat && old.folderId == nil && old.effort == nil)
    }

    @Test func chatEvents() throws {
        guard case .event(let u) = try decode(#"{"t":"user","seq":1,"turn":1,"text":"Giải giúp tớ","via":"text","files":[{"fileId":"f9","kind":"pdf","name":"de.pdf"}]}"#),
              case .user(let text, let files, let via) = u.kind else { Issue.record("not user"); return }
        #expect(text == "Giải giúp tớ" && via == "text" && files == [FileRef(fileId: "f9", kind: "pdf", name: "de.pdf")])
        #expect(!u.isBoardEvent && !u.isLiveEvent)
        #expect(try decode(#"{"t":"chat_delta","turn":1,"text":"Ta có "}"#) == .chatDelta(turn: 1, text: "Ta có "))
        guard case .event(let c) = try decode(#"{"t":"chat","seq":2,"turn":1,"md":"$x=2$"}"#) else { Issue.record("not chat"); return }
        #expect(c.kind == .chat(md: "$x=2$"))
        guard case .event(let inline) = try decode(#"{"t":"show","seq":3,"turn":1,"step":-1,"block":{"type":"math","tex":"x"}}"#) else {
            Issue.record("not show"); return
        }
        #expect(!inline.isBoardEvent && !inline.isLiveEvent)
        guard case .event(let onBoard) = try decode(#"{"t":"show","seq":4,"turn":1,"step":0,"block":{"type":"math","tex":"x"}}"#) else {
            Issue.record("not show"); return
        }
        #expect(onBoard.isBoardEvent && onBoard.isLiveEvent)
        #expect(try decode(#"{"t":"heard","text":"x bình phương","raw":"x binh","draft":true}"#)
                == .heard(text: "x bình phương", raw: "x binh", draft: true))
        #expect(try decode(#"{"t":"heard","text":"a"}"#) == .heard(text: "a", raw: nil, draft: false))
        // Events round-trip through makeRaw.
        let made = LessonEvent(seq: 5, turn: 2, kind: .user(text: "hi", files: [FileRef(fileId: "a", kind: "image", name: "p.jpg")], via: "voice"))
        #expect(LessonEvent.from(made.raw) == made)
        #expect(LessonEvent.from(LessonEvent(seq: 6, turn: 2, kind: .chat(md: "**a**")).raw)?.kind == .chat(md: "**a**"))
    }

    @Test func foldersAndSessions() throws {
        guard case .folders(let f) = try decode(#"""
        {"t":"folders","items":[{"id":"f1","name":"Toán 9","subject":"toan","sessionCount":4,"updatedAt":"2026-11-15","hasKnowledge":true,
          "ingest":{"jobId":"j","state":"reading","done":3,"total":10}},{"id":"f2","name":"Hóa"},{"name":"no id"}]}
        """#) else { Issue.record("not folders"); return }
        #expect(f.count == 2)
        #expect(f[0].subject == "toan" && f[0].sessionCount == 4 && f[0].hasKnowledge && f[0].ingest?.done == 3)
        #expect(f[1].subject == "chung" && f[1].ingest == nil && !f[1].hasKnowledge)
        #expect(try decode(#"{"t":"folder_created","id":"f3"}"#) == .folderCreated(id: "f3"))
        guard case .sessions(let s) = try decode(#"""
        {"t":"sessions","items":[{"sessionId":"a","title":"T","updatedAt":1763000000,"folderId":"f1","mode":"chat","preview":"Giải pt"},
                                 {"sessionId":"b","title":"U","updatedAt":"x","folderId":null}]}
        """#) else { Issue.record("not sessions"); return }
        #expect(s[0] == SessionItem(sessionId: "a", title: "T", updatedAt: "1763000000", folderId: "f1", mode: .chat, preview: "Giải pt"))
        #expect(s[1].folderId == nil && s[1].mode == .live)
    }

    @Test func ingestAndKnowledge() throws {
        guard case .ingest(let i) = try decode(#"{"t":"ingest","folderId":"f1","jobId":"j1","state":"reading","done":7,"total":24,"note":"Đang đọc trang 7/24","found":{"questions":40,"topics":9},"costUsd":0.012}"#) else {
            Issue.record("not ingest"); return
        }
        #expect(i.state == .reading && i.isActive && i.done == 7 && i.total == 24 && i.foundQuestions == 40 && i.foundTopics == 9)
        #expect(abs((i.fraction ?? 0) - 7.0 / 24) < 1e-9)
        #expect(i.json["found"]?["topics"]?.int == 9 && i.json["t"]?.string == "ingest")
        guard case .ingest(let odd) = try decode(#"{"t":"ingest","state":"weird"}"#) else { Issue.record("not ingest"); return }
        #expect(odd.state == .reading && odd.fraction == nil)

        guard case .knowledge(let k) = try decode(#"""
        {"t":"knowledge","folderId":"f1","data":{"v":1,"folderId":"f1","subject":"toan","grade":9,"goal":"ôn giữa kỳ 1",
          "sources":[{"fileId":"a"},{"fileId":"b"}],
          "curriculum":[{"id":"c1","title":"Căn bậc hai","topics":[{"id":"c1.t1","title":"Rút gọn","count":14,"share":0.12,"forms":[{"id":"f"}]}]}],
          "structures":[{"id":"s1","title":"Đề giữa kỳ","durationMin":90,"totalPoints":10,
             "sections":[{"id":"A","title":"Trắc nghiệm","count":12},{"id":"B","title":"Tự luận","count":4}]}],
          "schoolProgress":{"coveredTopicIds":["c1.t1"],"currentTopicId":"c1.t2","note":"Suy từ đề mới nhất"},
          "stats":{"questions":118,"byLevel":{"nb":30,"th":44,"vd":34,"vdc":10}}}}
        """#), let data = k.data else { Issue.record("not knowledge"); return }
        #expect(k.folderId == "f1" && k.ingest == nil)
        #expect(data.grade == "9" && data.goal == "ôn giữa kỳ 1" && data.sources == 2 && data.questions == 118)
        #expect(data.byLevel["vdc"] == 10 && data.chapters.first?.topics.first?.forms == 1)
        #expect(data.structures.first?.questions == 16 && data.structures.first?.sections == ["Trắc nghiệm", "Tự luận"])
        #expect(data.currentTopicId == "c1.t2" && data.coveredTopicIds == ["c1.t1"] && data.topicTitle("c1.t1") == "Rút gọn")
        #expect(data.raw["v"]?.int == 1)
        guard case .knowledge(let empty) = try decode(#"{"t":"knowledge","folderId":"f2","data":null,"ingest":{"state":"queued","total":12}}"#) else {
            Issue.record("not knowledge"); return
        }
        #expect(empty.data == nil && empty.ingest?.state == .queued && empty.ingest?.folderId == "f2")
    }

    @Test func review() throws {
        guard case .review(let r) = try decode(#"""
        {"t":"review","folderId":"f1","updatedAt":"2026-11-15",
         "topics":[{"topicId":"c1.t1","title":"Rút gọn","chapter":"Căn","state":"weak","wrong":3,"right":1,"lastSeen":"2026-11-14"},
                   {"topicId":"c1.t2","title":"Trục căn","state":"solid","wrong":0,"right":4},{"topicId":"x","state":"??"}],
         "weak":[{"topic":"Rút gọn","note":"quên điều kiện","count":3,"lastSeen":"2026-11-14","due":"2026-11-15"}],
         "due":[{"topic":"Trục căn","note":"","due":"2026-11-15"}],
         "weekly":{"from":"2026-11-09","to":"2026-11-15","md":"**Tuần này** học 3 buổi."},
         "sessions":[{"sessionId":"s1","title":"Toán · Rút gọn","updatedAt":"2026-11-14","mode":"live","steps":4}]}
        """#) else { Issue.record("not review"); return }
        #expect(r.topics.count == 3 && r.count(.weak) == 1 && r.count(.solid) == 1 && r.count(.new) == 1)
        #expect(r.weak.first?.count == 3 && r.due.first?.due == "2026-11-15")
        #expect(r.weekly?.md.hasPrefix("**Tuần") == true && r.sessions.first?.steps == 4)
        guard case .review(let bare) = try decode(#"{"t":"review","folderId":"f2","weekly":null}"#) else { Issue.record("not review"); return }
        #expect(bare.topics.isEmpty && bare.weekly == nil && bare.sessions.isEmpty)
    }

    @Test func examsUsageGreeting() throws {
        #expect(try decode(#"{"t":"exam","folderId":"f1","examId":"e1","state":"checking","note":"Đang soát đáp án"}"#)
                == .exam(ExamStatus(folderId: "f1", examId: "e1", state: .checking, note: "Đang soát đáp án")))
        guard case .exams(let fid, let items) = try decode(#"{"t":"exams","folderId":"f1","items":[{"examId":"e1","title":"Đề 1","structureId":"s1","createdAt":"2026-11-15T03:00:00Z","questions":16},{"title":"x"}]}"#) else {
            Issue.record("not exams"); return
        }
        #expect(fid == "f1" && items == [ExamItem(examId: "e1", title: "Đề 1", structureId: "s1", createdAt: "2026-11-15T03:00:00Z", questions: 16)])

        guard case .usage(let u) = try decode(#"""
        {"t":"usage","claude":{"session":{"inputTokens":1200,"outputTokens":300,"cacheReadTokens":5000,"cacheWriteTokens":100,"costUsd":0.05,"turns":3},
          "today":{"inputTokens":9000,"costUsd":0.4,"turns":12},"week":{"costUsd":2.5},
          "limit":{"status":"allowed_warning","resetsAt":"2026-11-15T05:00:00Z","type":"five_hour"}},
         "other":{"today":{"costUsd":0.01,"inputTokens":100,"outputTokens":20}},"ingest":{"today":{"costUsd":0.02},"total":{"costUsd":0.3}}}
        """#) else { Issue.record("not usage"); return }
        #expect(u.session.totalTokens == 6600 && u.session.turns == 3 && u.today.turns == 12 && u.week.costUsd == 2.5)
        #expect(u.limit?.status == "allowed_warning" && u.limit?.type == "five_hour")
        #expect(u.otherToday.outputTokens == 20 && u.ingestTotal == 0.3)
        guard case .usage(let none) = try decode(#"{"t":"usage"}"#) else { Issue.record("not usage"); return }
        #expect(none.limit == nil && none.today.totalTokens == 0)

        #expect(try decode(#"{"t":"greeting","text":"Chào cậu","sub":"Có 2 mục tới hạn","doodle":"whale_swim"}"#)
                == .greeting(Greeting(text: "Chào cậu", sub: "Có 2 mục tới hạn", doodle: .whaleSwim)))
        #expect(try decode(#"{"t":"greeting","text":"Hi","doodle":"dragon"}"#) == .greeting(Greeting(text: "Hi", doodle: .catWalk)))
    }

    @Test func fallbackGreetingsAreShortAndCover24Hours() {
        for lang in Lang.allCases {
            for hour in 0..<24 {
                for seed in 0..<5 {
                    let g = Greeting.fallback(hour: hour, seed: seed, lang: lang)
                    #expect(!g.text.isEmpty && g.text.count <= 60)
                }
            }
        }
        #expect(Greeting.fallback(hour: 2, seed: 0).doodle == .catSleep)
    }

    @Test func encodesV2ClientMessages() throws {
        let hello = try json(.hello(pairCode: nil, pairToken: "t", device: "iPhone", sessionId: nil, lastSeq: nil,
                                    settings: TutorSettings(model: "deepseek", effort: "high", mode: .live, sttFix: false), folderId: "f1"))
        #expect(hello["v"]?.int == 2 && hello["folderId"]?.string == "f1" && hello["lastSeq"] == nil)
        #expect(hello["settings"]?["model"]?.string == "deepseek" && hello["settings"]?["effort"]?.string == "high")
        #expect(hello["settings"]?["mode"]?.string == "live" && hello["settings"]?["sttFix"]?.bool == false)

        #expect(ClientMessage.userText("hi").text == #"{"t":"user_text","text":"hi"}"#)
        #expect(ClientMessage.userText("hi", files: ["a", "b"]).text == #"{"files":["a","b"],"t":"user_text","text":"hi"}"#)
        #expect(ClientMessage.userText("x", draft: true).text == #"{"draft":true,"t":"user_text","text":"x"}"#)
        #expect(ClientMessage.audioStart(sampleRate: 16000).text == #"{"mime":"audio/wav","sampleRate":16000,"t":"audio_start"}"#)
        #expect(try json(.audioStart(sampleRate: 16000, draft: true))["draft"]?.bool == true)
        #expect(ClientMessage.listSessions().text == #"{"t":"list_sessions"}"#)
        #expect(ClientMessage.listSessions(folderId: "f1").text == #"{"folderId":"f1","t":"list_sessions"}"#)
        #expect(ClientMessage.listFolders.text == #"{"t":"list_folders"}"#)
        #expect(ClientMessage.createFolder(name: "Toán 9", subject: "toan").text == #"{"name":"Toán 9","subject":"toan","t":"create_folder"}"#)
        #expect(ClientMessage.renameFolder(id: "f1", name: "A").text == #"{"id":"f1","name":"A","t":"rename_folder"}"#)
        #expect(ClientMessage.deleteFolder(id: "f1").text == #"{"id":"f1","t":"delete_folder"}"#)
        #expect(ClientMessage.moveSession(sessionId: "s", folderId: nil).text == #"{"folderId":null,"sessionId":"s","t":"move_session"}"#)
        #expect(ClientMessage.moveSession(sessionId: "s", folderId: "f").text == #"{"folderId":"f","sessionId":"s","t":"move_session"}"#)
        #expect(ClientMessage.renameSession(sessionId: "s", title: "T").text == #"{"sessionId":"s","t":"rename_session","title":"T"}"#)
        #expect(ClientMessage.deleteSession(sessionId: "s").text == #"{"sessionId":"s","t":"delete_session"}"#)
        #expect(ClientMessage.newSession(folderId: "f", mode: .chat, kickoff: "review_quiz").text
                == #"{"folderId":"f","kickoff":"review_quiz","mode":"chat","t":"new_session"}"#)
        #expect(ClientMessage.newSession(folderId: nil, mode: nil).text == #"{"t":"new_session"}"#)
        #expect(ClientMessage.ingestStart(folderId: "f", files: ["a"], goal: "ôn giữa kỳ").text
                == #"{"files":["a"],"folderId":"f","goal":"ôn giữa kỳ","t":"ingest_start"}"#)
        #expect(ClientMessage.ingestStart(folderId: "f", files: [], goal: "").text == #"{"files":[],"folderId":"f","t":"ingest_start"}"#)
        #expect(ClientMessage.ingestCancel(folderId: "f").text == #"{"folderId":"f","t":"ingest_cancel"}"#)
        #expect(ClientMessage.getKnowledge(folderId: "f").text == #"{"folderId":"f","t":"get_knowledge"}"#)
        #expect(ClientMessage.getReview(folderId: "f").text == #"{"folderId":"f","t":"get_review"}"#)
        #expect(ClientMessage.getWeekly(folderId: "f").text == #"{"folderId":"f","t":"get_weekly"}"#)
        #expect(ClientMessage.genExam(folderId: "f", structureId: "s1", note: "khó hơn").text
                == #"{"folderId":"f","note":"khó hơn","structureId":"s1","t":"gen_exam"}"#)
        #expect(ClientMessage.listExams(folderId: "f").text == #"{"folderId":"f","t":"list_exams"}"#)
        #expect(ClientMessage.deleteExam(examId: "e").text == #"{"examId":"e","t":"delete_exam"}"#)
        #expect(ClientMessage.getUsage.text == #"{"t":"get_usage"}"#)
        #expect(ClientMessage.getGreeting(localTime: "2026-11-15T01:45", name: nil).text == #"{"localTime":"2026-11-15T01:45","t":"get_greeting"}"#)
    }

    @Test func unknownFramesAreTolerated() throws {
        #expect(try decode(#"{"t":"brand_new_thing","x":1}"#) == .unknown(type: "brand_new_thing"))
        #expect(try decode(#"{"t":"user","turn":1}"#) == .unknown(type: "user"))   // no seq
        #expect(try decode(#"{"nothing":true}"#) == .unknown(type: ""))
    }
}

@Suite struct ChatTranscriptTests {
    private func f(_ s: String) -> JSONValue { (try? JSONValue.parse(s)) ?? .null }

    @Test func deltasStreamThenChatReplaces() {
        var c = ChatTranscript()
        c.apply(f(#"{"t":"user","seq":1,"turn":1,"text":"Giải x^2=4"}"#))
        #expect(c.busy)
        c.apply(f(#"{"t":"chat_delta","turn":1,"text":"Ta có "}"#))
        c.apply(f(#"{"t":"chat_delta","turn":1,"text":"$x=\\pm 2$"}"#))
        #expect(c.items.count == 2)
        #expect(c.items[1].kind == .text(md: #"Ta có $x=\pm 2$"#, streaming: true))
        c.apply(f(#"{"t":"chat","seq":2,"turn":1,"md":"Ta có $x = \\pm 2$."}"#))
        #expect(c.items[1].kind == .text(md: #"Ta có $x = \pm 2$."#, streaming: false))
        c.apply(f(#"{"t":"show","seq":3,"turn":1,"step":-1,"block":{"type":"math","tex":"x=2"}}"#))
        c.apply(f(#"{"t":"chat_delta","turn":1,"text":"Xong"}"#))
        #expect(c.items.count == 4)
        if case .block(let b) = c.items[2].kind { #expect(b["tex"]?.string == "x=2") } else { Issue.record("no block") }
        c.apply(f(#"{"t":"turn_end","seq":4,"turn":1}"#))
        #expect(!c.busy && c.items[3].kind == .text(md: "Xong", streaming: false))
    }

    @Test func duplicatesAndLiveEvents() {
        var c = ChatTranscript()
        let user = f(#"{"t":"user","seq":1,"turn":1,"text":"hi"}"#)
        c.apply(user); c.apply(user)
        #expect(c.items.count == 1)
        c.apply(f(#"{"t":"say","seq":2,"turn":1,"text":"Nhìn màn hình nhé.","audio":true}"#))
        c.apply(f(#"{"t":"say","seq":3,"turn":1,"text":"Bước một.","audio":true}"#))
        c.apply(f(#"{"t":"step","seq":4,"turn":1,"index":0,"title":"Chuyển vế"}"#))
        c.apply(f(#"{"t":"ask","seq":5,"turn":1,"kind":"next","prompt":"Tiếp theo?"}"#))
        c.apply(f(#"{"t":"done","seq":6,"turn":1}"#))
        #expect(c.items.map(\.kind) == [.user(text: "hi", files: []), .spoken("Nhìn màn hình nhé. Bước một."),
                                        .heading("Chuyển vế"), .ask("Tiếp theo?"), .done])
    }

    @Test func loadAndState() {
        var c = ChatTranscript()
        c.apply(f(#"{"t":"state","state":"thinking"}"#))
        #expect(c.busy && c.isEmpty)
        c.apply(f(#"{"t":"state","state":"idle"}"#))
        #expect(!c.busy)
        c.load([f(#"{"t":"user","seq":1,"turn":1,"text":"a"}"#), f(#"{"t":"chat","seq":2,"turn":1,"md":"b"}"#)])
        #expect(c.items.count == 2 && !c.busy)
        c.reset()
        #expect(c.isEmpty)
    }
}
