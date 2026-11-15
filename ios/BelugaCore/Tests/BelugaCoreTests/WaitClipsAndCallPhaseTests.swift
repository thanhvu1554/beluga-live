import Foundation
import Testing
@testable import BelugaCore

/// Wait clips (PROTOCOL section 21): the playback rules table, driven by a fake clock.
@Suite struct WaitClipsTests {
    // Times near the reference date keep full Double precision, so boundaries (exactly 3 s) are exact.
    func at(_ s: TimeInterval) -> Date { Date(timeIntervalSinceReferenceDate: s) }

    let all: [VoiceFiller] = [
        .init(id: "s1", text: "Để tớ xem nào", when: .short), .init(id: "s2", text: "Ừm, chờ tớ chút", when: .short),
        .init(id: "s3", text: "Tớ nghĩ đã nhé", when: .short),
        .init(id: "l1", text: "Bài này hơi dài, cậu chờ tớ xíu", when: .long), .init(id: "l2", text: "Sắp xong rồi", when: .long),
        .init(id: "v1", text: "Để tớ nhìn ảnh kỹ đã", when: .vision),
        .init(id: "h1", text: "A lô, tớ đây", when: .hello), .init(id: "h2", text: "Tớ nghe nè", when: .hello),
    ]

    func machine(_ clips: [VoiceFiller]? = nil, seed: UInt64 = 7) -> WaitClips {
        var m = WaitClips(seed: seed)
        m.setClips(clips ?? all)
        return m
    }

    /// Ticks every 50 ms from `from` to `to`, collecting actions.
    func run(_ m: inout WaitClips, from: TimeInterval, to: TimeInterval) -> [(TimeInterval, WaitClipAction)] {
        var out: [(TimeInterval, WaitClipAction)] = []
        let steps = Int(((to - from) / 0.05 + 1e-6).rounded(.down))
        for i in 0...max(0, steps) {
            let t = from + Double(i) * 0.05
            for a in m.handle(.tick, now: at(t)) { out.append((t, a)) }
        }
        return out
    }

    @Test func nothingBeforeThreeSeconds() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        #expect(m.phase == .thinking && m.thinkingSince == at(0))
        #expect(run(&m, from: 0, to: 2.95).isEmpty)
    }

    @Test func shortClipAtThreeSecondsOnce() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        let actions = run(&m, from: 0, to: 3.1)
        #expect(actions.count == 1)
        guard case .play(let id)? = actions.first?.1 else { Issue.record("no play"); return }
        #expect(["s1", "s2", "s3"].contains(id))
        #expect(abs(actions[0].0 - 3) < 0.06)
        #expect(m.phase == .filler(id))
        _ = m.handle(.clipStarted(id: id, duration: 1.4), now: at(3.05))
        _ = m.handle(.clipFinished(id: id), now: at(4.45))
        #expect(m.phase == .thinking)
        // No second short clip in the same turn.
        #expect(run(&m, from: 4.5, to: 11.9).isEmpty)
    }

    @Test func longClipAfterTwelveSecondsOncePerTurn() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        let first = run(&m, from: 0, to: 3.0)
        guard case .play(let s)? = first.first?.1 else { Issue.record("no short"); return }
        _ = m.handle(.clipFinished(id: s), now: at(4))
        let later = run(&m, from: 4, to: 30)
        #expect(later.count == 1)
        guard case .play(let l)? = later.first?.1 else { Issue.record("no long"); return }
        #expect(["l1", "l2"].contains(l) && abs(later[0].0 - 12) < 0.06)
        _ = m.handle(.clipFinished(id: l), now: at(14))
        #expect(run(&m, from: 14, to: 40).isEmpty)
    }

    @Test func longWaitsForAShortStillPlaying() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        guard case .play(let s)? = run(&m, from: 0, to: 3).first?.1 else { Issue.record("no short"); return }
        #expect(run(&m, from: 3.05, to: 13).isEmpty)          // still "playing": no overlap
        _ = m.handle(.clipFinished(id: s), now: at(13))
        guard case .play(let l)? = run(&m, from: 13, to: 13.1).first?.1 else { Issue.record("no long"); return }
        #expect(l.hasPrefix("l"))
    }

    @Test func visionClipWhenTheTurnHasAPhoto() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        _ = m.handle(.photo, now: at(0.5))
        guard case .play(let id)? = run(&m, from: 0, to: 3).first?.1 else { Issue.record("no play"); return }
        #expect(id == "v1")
        // A photo with no vision clip falls back to a short one.
        var n = machine(all.filter { $0.when != .vision })
        _ = n.handle(.thinkingStarted, now: at(0))
        _ = n.handle(.photo, now: at(0))
        guard case .play(let s)? = run(&n, from: 0, to: 3).first?.1 else { Issue.record("no play"); return }
        #expect(s.hasPrefix("s"))
    }

    @Test func shortClipDiffersFromTheLastOne() {
        var m = machine()
        var last: String?
        for turn in 0..<40 {
            let base = Double(turn) * 100
            _ = m.handle(.thinkingStarted, now: at(base))
            var played: String?
            for (_, action) in run(&m, from: base, to: base + 3.05) {
                if case .play(let id) = action { played = id }
            }
            guard let played else { Issue.record("turn \(turn) played nothing"); return }
            #expect(played != last)
            last = played
            _ = m.handle(.clipFinished(id: played), now: at(base + 4))
            _ = m.handle(.turnEnded, now: at(base + 5))
        }
        // With a single short clip it may repeat (there is no other).
        var one = machine([.init(id: "only", when: .short)])
        for turn in 0..<3 {
            _ = one.handle(.thinkingStarted, now: at(Double(turn) * 100))
            let a = run(&one, from: Double(turn) * 100, to: Double(turn) * 100 + 3)
            #expect(a.first?.1 == .play(fillerId: "only"))
            _ = one.handle(.turnEnded, now: at(Double(turn) * 100 + 5))
        }
    }

    @Test func sameSeedSameChoices() {
        var a = machine(seed: 42), b = machine(seed: 42)
        _ = a.handle(.thinkingStarted, now: at(0)); _ = b.handle(.thinkingStarted, now: at(0))
        #expect(run(&a, from: 0, to: 3).first?.1 == run(&b, from: 0, to: 3).first?.1)
    }

    @Test func sayReadyBeforeAnyClipPlaysAtOnceAndEndsTheTurnsClips() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        #expect(m.sayReady(now: at(2)) == nil)
        #expect(m.phase == .idle)
        #expect(run(&m, from: 2, to: 20).isEmpty)
    }

    @Test func sayReadyWaitsForTheClipAtMostTwoAndAHalfSeconds() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        guard case .play(let id)? = run(&m, from: 0, to: 3).first?.1 else { Issue.record("no play"); return }
        _ = m.handle(.clipStarted(id: id, duration: 1.2), now: at(3))
        // Ready at 3.5: the clip ends at 4.2, so the say waits until then.
        #expect(m.sayReady(now: at(3.5)) == at(4.2))
        #expect(m.phase == .holding)
        _ = m.handle(.clipFinished(id: id), now: at(4.2))
        #expect(m.phase == .idle)

        // A long clip (10 s) is cut 2.5 s after the say is ready.
        var n = machine()
        _ = n.handle(.thinkingStarted, now: at(0))
        guard case .play(let l)? = run(&n, from: 0, to: 3).first?.1 else { Issue.record("no play"); return }
        _ = n.handle(.clipStarted(id: l, duration: 10), now: at(3))
        #expect(n.sayReady(now: at(4)) == at(6.5))
        let cut = run(&n, from: 4.05, to: 6.6)
        #expect(cut.count == 1 && cut[0].1 == .stop && cut[0].0 >= 6.5 - 1e-6)
        #expect(n.phase == .idle)

        // Duration unknown: the cap applies.
        var u = machine()
        _ = u.handle(.thinkingStarted, now: at(0))
        guard case .play? = run(&u, from: 0, to: 3).first?.1 else { Issue.record("no play"); return }
        #expect(u.sayReady(now: at(3.2)) == at(5.7))
        // Only the first say of a turn waits.
        #expect(u.sayReady(now: at(3.3)) == nil)
    }

    @Test func turnEndInterruptAndSpeechStopTheClip() {
        for input in [WaitClipInput.turnEnded, .interrupted, .userSpoke] {
            var m = machine()
            _ = m.handle(.thinkingStarted, now: at(0))
            guard case .play? = run(&m, from: 0, to: 3).first?.1 else { Issue.record("no play"); return }
            #expect(m.handle(input, now: at(3.5)) == [.stop])
            #expect(m.phase == .idle && m.thinkingSince == nil)
            #expect(run(&m, from: 3.5, to: 20).isEmpty)
            // Nothing playing: nothing to stop.
            #expect(m.handle(input, now: at(21)) == [])
        }
    }

    @Test func newTurnStopsALeftoverClip() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        guard case .play? = run(&m, from: 0, to: 3).first?.1 else { Issue.record("no play"); return }
        #expect(m.handle(.thinkingStarted, now: at(3.2)) == [.stop])
        #expect(m.thinkingSince == at(3.2) && m.phase == .thinking)
    }

    @Test func noClipsMeansSilence() {
        var m = machine([])
        _ = m.handle(.thinkingStarted, now: at(0))
        #expect(run(&m, from: 0, to: 30).isEmpty)
        #expect(m.phase == .thinking)                 // the light still breathes
        #expect(m.helloClip() == nil)
    }

    @Test func staleClipEventsAreIgnored() {
        var m = machine()
        _ = m.handle(.thinkingStarted, now: at(0))
        guard case .play(let id)? = run(&m, from: 0, to: 3).first?.1 else { Issue.record("no play"); return }
        _ = m.handle(.clipFinished(id: "someone-else"), now: at(3.1))
        #expect(m.phase == .filler(id))
        _ = m.handle(.clipStarted(id: "someone-else", duration: 9), now: at(3.1))
        #expect(m.playingEndsAt == nil)
    }

    @Test func helloPicksAHelloLine() {
        var m = machine()
        let h = m.helloClip()
        #expect(h == "h1" || h == "h2")
        var none = machine(all.filter { $0.when != .hello })
        #expect(none.helloClip() == nil)
    }
}

/// The call's phases (PROTOCOL section 22), with an injected clock.
@Suite struct CallPhaseTests {
    func at(_ s: TimeInterval) -> Date { Date(timeIntervalSinceReferenceDate: s) }

    @Test func dialsAtLeastOnePointSixSeconds() {
        var m = CallPhaseMachine()
        #expect(m.handle(.start, now: at(0)) == [.startRinging])
        #expect(m.label(now: at(0)) == .dialing && m.isActive)
        // Ready almost at once: still ringing until 1.6 s.
        #expect(m.handle(.sessionReady(true), now: at(0.3)) == [])
        #expect(m.handle(.tick, now: at(1.55)) == [])
        #expect(m.handle(.tick, now: at(1.6)) == [.stopRinging, .connected])
        #expect(m.phase == .connected(at: at(1.6)))
    }

    @Test func connectsWhenReadyAfterTheMinimum() {
        var m = CallPhaseMachine()
        _ = m.handle(.start, now: at(0))
        #expect(m.handle(.tick, now: at(5)) == [])
        #expect(m.handle(.sessionReady(true), now: at(5.2)) == [.stopRinging, .connected])
    }

    @Test func failsAfterEightSeconds() {
        var m = CallPhaseMachine()
        _ = m.handle(.start, now: at(0))
        #expect(m.handle(.tick, now: at(7.99)) == [])
        #expect(m.handle(.tick, now: at(8)) == [.stopRinging, .failed])
        #expect(m.label(now: at(8)) == .failed)
        // Ready too late changes nothing; "Gọi lại" dials again.
        #expect(m.handle(.sessionReady(true), now: at(9)) == [])
        #expect(m.handle(.retry, now: at(10)) == [.startRinging])
        #expect(m.handle(.tick, now: at(11.7)) == [.stopRinging, .connected])
    }

    @Test func connectedLabelThenTimer() {
        var m = CallPhaseMachine()
        _ = m.handle(.start, now: at(0))
        _ = m.handle(.sessionReady(true), now: at(0))
        _ = m.handle(.tick, now: at(2))
        #expect(m.label(now: at(2.5)) == .connected)
        #expect(m.label(now: at(3.4)) == .timer(since: at(2)))
    }

    @Test func hangUpInEveryPhase() {
        // Dialing: ends without a duration, closes after the beat.
        var d = CallPhaseMachine()
        _ = d.handle(.start, now: at(0))
        #expect(d.handle(.hangUp, now: at(1)) == [.stopRinging, .hangUp])
        #expect(d.label(now: at(1)) == .ended(duration: nil))
        #expect(d.handle(.tick, now: at(1.85)) == [])
        #expect(d.handle(.tick, now: at(2.0)) == [.close])
        #expect(d.phase == .idle && !d.isActive)

        // Connected: ended with the talk time.
        var c = CallPhaseMachine()
        _ = c.handle(.start, now: at(0)); _ = c.handle(.sessionReady(true), now: at(0)); _ = c.handle(.tick, now: at(2))
        #expect(c.handle(.hangUp, now: at(65)) == [.hangUp])
        #expect(c.label(now: at(65.5)) == .ended(duration: 63))
        // A second tap closes at once.
        #expect(c.handle(.hangUp, now: at(65.2)) == [.hangUp, .close])

        // Failed: closes at once.
        var f = CallPhaseMachine()
        _ = f.handle(.start, now: at(0)); _ = f.handle(.tick, now: at(8))
        #expect(f.handle(.hangUp, now: at(9)) == [.hangUp, .close])

        // Idle: nothing.
        var i = CallPhaseMachine()
        #expect(i.handle(.hangUp, now: at(0)) == [])
    }

    @Test func startTwiceAndRetryOutOfPlaceAreIgnored() {
        var m = CallPhaseMachine()
        _ = m.handle(.start, now: at(0))
        #expect(m.handle(.start, now: at(1)) == [])
        #expect(m.handle(.retry, now: at(1)) == [])
        #expect(m.phase == .dialing(since: at(0)))
    }

    @Test func sessionLostWhileDialingKeepsRinging() {
        var m = CallPhaseMachine()
        _ = m.handle(.start, now: at(0))
        _ = m.handle(.sessionReady(true), now: at(0.5))
        _ = m.handle(.sessionReady(false), now: at(1.0))
        #expect(m.handle(.tick, now: at(2)) == [])
        #expect(m.handle(.tick, now: at(8)) == [.stopRinging, .failed])
    }

    @Test func startsReadyWhenTheSessionAlreadyWas() {
        // A call from Live: the session is ready before the button; still rings 1.6 s.
        var m = CallPhaseMachine()
        _ = m.handle(.sessionReady(true), now: at(0))
        #expect(m.handle(.start, now: at(0)) == [.startRinging])
        #expect(m.handle(.tick, now: at(1.6)) == [.stopRinging, .connected])
    }

    @Test func callAfterAnEndedCallStartsFresh() {
        var m = CallPhaseMachine(phase: .ended(at: at(0), duration: 10))
        #expect(m.handle(.start, now: at(0.2)) == [.startRinging])
        #expect(m.phase == .dialing(since: at(0.2)))
    }
}
