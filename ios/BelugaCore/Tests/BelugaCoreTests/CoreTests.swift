import Foundation
import Testing
@testable import BelugaCore

@Suite struct MessageTests {
    @Test func decodesReady() throws {
        let m = try ServerMessage.decode(#"{"t":"ready","v":1,"sessionId":"abc","title":"","tts":"server","stt":"device","boardVersion":3,"pairToken":"tok"}"#)
        guard case .ready(let r) = m else { Issue.record("not ready"); return }
        #expect(r.sessionId == "abc")
        #expect(r.serverTTS)
        #expect(!r.serverSTT)
        #expect(r.boardVersion == 3)
        #expect(r.pairToken == "tok")
        #expect(r.audioWaitMs == 3000)
        let m2 = try ServerMessage.decode(#"{"t":"ready","v":1,"sessionId":"a","tts":"server","stt":"server","boardVersion":1,"audioWaitMs":6000}"#)
        if case .ready(let r2) = m2 { #expect(r2.audioWaitMs == 6000 && r2.serverSTT) } else { Issue.record("not ready") }
    }

    @Test func decodesShowWithRawBlock() throws {
        let m = try ServerMessage.decode(#"{"t":"show","seq":7,"turn":2,"step":1,"block":{"type":"plot","fn":["x^2"],"x":[-1,4]}}"#)
        guard case .event(let e) = m, case .show(let step, let block) = e.kind else { Issue.record("not show"); return }
        #expect(e.seq == 7 && e.turn == 2 && step == 1)
        #expect(block["type"]?.string == "plot")
        #expect(block["x"]?.array?.count == 2)
        #expect(e.raw["t"]?.string == "show")
    }

    @Test func decodesOtherFrames() throws {
        #expect(try ServerMessage.decode(#"{"t":"state","state":"tool","tool":"capture"}"#) == .state(state: "tool", tool: "capture"))
        #expect(try ServerMessage.decode(#"{"t":"capture_request","requestId":"r1"}"#) == .captureRequest(requestId: "r1"))
        #expect(try ServerMessage.decode(#"{"t":"error","code":"bad_pair","text":"no","fatal":true}"#) == .error(code: "bad_pair", text: "no", fatal: true))
        #expect(try ServerMessage.decode(#"{"t":"whatever"}"#) == .unknown(type: "whatever"))
        let audio = try ServerMessage.decode(#"{"t":"audio","seq":3,"mime":"audio/mpeg","data":"AAEC"}"#)
        #expect(audio == .audio(seq: 3, mime: "audio/mpeg", data: Data([0, 1, 2])))
        let snap = try ServerMessage.decode(#"{"t":"snapshot","events":[{"t":"say","seq":1,"turn":1,"text":"a","audio":false},{"t":"done","seq":2,"turn":1}]}"#)
        guard case .snapshot(let ev) = snap else { Issue.record("not snapshot"); return }
        #expect(ev.map(\.seq) == [1, 2])
        let sessions = try ServerMessage.decode(#"{"t":"sessions","items":[{"sessionId":"s","title":"T","updatedAt":"2026-11-14"}]}"#)
        #expect(sessions == .sessions([SessionItem(sessionId: "s", title: "T", updatedAt: "2026-11-14")]))
    }

    @Test func encodesClientMessages() throws {
        let hello = ClientMessage.hello(pairCode: "123456", pairToken: nil, device: "iPhone",
                                        sessionId: nil, lastSeq: 4, settings: TutorSettings())
        let v = try JSONValue.parse(hello.text)
        #expect(v["t"]?.string == "hello")
        #expect(v["v"]?.int == 1)
        #expect(v["pairCode"]?.string == "123456")
        #expect(v["pairToken"] == nil)
        #expect(v["sessionId"]?.string == "")
        #expect(v["lastSeq"]?.int == 4)
        #expect(v["settings"]?["model"]?.string == "sonnet")
        #expect(v["settings"]?["rigor"]?.string == "medium")
        #expect(ClientMessage.played(seq: 9).text == #"{"seq":9,"t":"played"}"#)
        #expect(ClientMessage.interrupt.text == #"{"t":"interrupt"}"#)
        let s = try JSONValue.parse(ClientMessage.settings(TutorSettings(lang: .en, model: .opus, rigor: .hard)).text)
        #expect(s["lang"]?.string == "en" && s["model"]?.string == "opus" && s["rigor"]?.string == "hard")
    }
}

@Suite struct PillTests {
    @Test func basicFlow() {
        let t0 = Date(timeIntervalSince1970: 0)
        var p = PillMachine(now: t0)
        #expect(p.state == .offline)
        p.handle(.connected, now: t0); #expect(p.state == .idle)
        p.handle(.micOpened, now: t0); #expect(p.state == .listening)
        p.handle(.micClosed(sent: true), now: t0); #expect(p.state == .thinking)
        p.handle(.server(state: "tool", tool: "capture"), now: t0); #expect(p.state == .tool)
        p.handle(.sayStarted, now: t0); #expect(p.state == .speaking)
        p.handle(.sayFinished, now: t0); #expect(p.state == .idle)
        p.handle(.askReached, now: t0); #expect(p.state == .asking)
        p.handle(.micOpened, now: t0); #expect(p.state == .listening)
        p.handle(.micClosed(sent: false), now: t0); #expect(p.state == .asking)
        p.handle(.doneReached, now: t0); #expect(p.state == .done)
        p.handle(.doneSettled, now: t0); #expect(p.state == .idle)
        p.handle(.disconnected, now: t0); #expect(p.state == .offline)
    }

    @Test func emptyUtteranceReturnsToAsking() {
        var p = PillMachine()
        p.handle(.connected); p.handle(.askReached)
        p.handle(.micOpened); #expect(p.state == .listening)
        p.handle(.micClosed(sent: true)); #expect(p.state == .thinking)
        p.handle(.utteranceRejected); #expect(p.state == .asking)
        p.handle(.micOpened); p.handle(.micClosed(sent: true))
        p.handle(.sayStarted); p.handle(.sayFinished)
        p.handle(.utteranceRejected); #expect(p.state == .idle)
    }

    @Test func offlineWins() {
        var p = PillMachine()
        p.handle(.connected); p.handle(.sayStarted)
        p.handle(.disconnected)
        #expect(p.state == .offline)
        p.handle(.connected)
        #expect(p.state == .idle)
    }

    @Test func thinkingAnnouncedOnceAfterFourSeconds() {
        let t0 = Date(timeIntervalSince1970: 100)
        var p = PillMachine(now: t0)
        p.handle(.connected, now: t0)
        p.handle(.userSent, now: t0)
        let a1 = p.shouldAnnounceThinking(now: t0.addingTimeInterval(3.9))
        #expect(!a1)
        let a2 = p.shouldAnnounceThinking(now: t0.addingTimeInterval(4.1))
        #expect(a2)
        let a3 = p.shouldAnnounceThinking(now: t0.addingTimeInterval(9))
        #expect(!a3)
    }

    @Test func unchangedStateReportsFalse() {
        var p = PillMachine()
        let changed = p.handle(.connected)
        #expect(changed)
        let unchanged = p.handle(.sayFinished)
        #expect(!unchanged)
    }
}

@Suite struct BackoffTests {
    @Test func growsAndCaps() {
        var b = Backoff(base: 0.5, factor: 2, max: 4, jitter: 0)
        let seq = [b.next(), b.next(), b.next(), b.next(), b.next()]
        #expect(seq == [0.5, 1, 2, 4, 4])
        b.reset()
        let first = b.next()
        #expect(first == 0.5)
    }

    @Test func jitterStaysInRange() {
        var b = Backoff(base: 1, factor: 2, max: 10, jitter: 0.2)
        let lo = b.next(random: 0)
        #expect(abs(lo - 0.8) < 1e-9)
        let hi = b.next(random: 1)
        #expect(abs(hi - 2.4) < 1e-9)
    }
}

@Suite struct VADTests {
    @Test func detectsUtterance() {
        var v = VoiceActivityDetector()
        var events: [VoiceActivityDetector.Event] = []
        var t: TimeInterval = 0
        func run(_ db: Float, _ seconds: TimeInterval) {
            let end = t + seconds
            while t < end { if let e = v.feed(level: db, at: t) { events.append(e) }; t += 0.05 }
        }
        run(-60, 1)       // background
        run(-20, 1)       // speech
        run(-60, 0.5)     // short pause, still speaking
        run(-20, 0.5)
        run(-60, 1.5)     // long silence ends it
        #expect(events == [.speechStarted, .speechEnded])
    }

    @Test func ignoresClicks() {
        var v = VoiceActivityDetector()
        var t: TimeInterval = 0
        var events: [VoiceActivityDetector.Event] = []
        for i in 0..<60 {
            let db: Float = (i % 10 == 0) ? -15 : -60
            if let e = v.feed(level: db, at: t) { events.append(e) }
            t += 0.05
        }
        #expect(events.isEmpty)
    }
}
