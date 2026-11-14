import Foundation
import Testing
@testable import BelugaCore

private func say(_ seq: Int, turn: Int = 1, _ text: String = "x") -> LessonEvent {
    LessonEvent(seq: seq, turn: turn, kind: .say(text: text, audio: false))
}
private func step(_ seq: Int, turn: Int = 1, index: Int = 0) -> LessonEvent {
    LessonEvent(seq: seq, turn: turn, kind: .step(index: index, title: "s\(index)"))
}
private func show(_ seq: Int, turn: Int = 1) -> LessonEvent {
    LessonEvent(seq: seq, turn: turn, kind: .show(step: 0, block: ["type": "math", "tex": "x"]))
}
private func turnEnd(_ seq: Int, turn: Int = 1) -> LessonEvent {
    LessonEvent(seq: seq, turn: turn, kind: .turnEnd(interrupted: false))
}
private func seqs(_ actions: [TimelineAction]) -> [String] {
    actions.map {
        switch $0 {
        case .speak(let e): "speak\(e.seq)"
        case .apply(let e): "apply\(e.seq)"
        case .restore(let e): "restore\(e.seq)"
        }
    }
}

@Suite struct TimelineTests {
    @Test func boardWaitsForPrecedingSay() {
        var t = Timeline(lastPlayed: 0)
        do { let r = seqs(t.receive(say(1))); #expect(r == ["speak1"]) }
        do { let r = seqs(t.receive(step(2))); #expect(r == []) }
        do { let r = seqs(t.receive(show(3))); #expect(r == []) }
        do { let r = seqs(t.receive(say(4))); #expect(r == []) }
        do { let r = seqs(t.finishedSpeaking(seq: 1)); #expect(r == ["apply2", "apply3", "speak4"]) }
        #expect(t.lastPlayed == 3)
        do { let r = seqs(t.finishedSpeaking(seq: 4)); #expect(r == []) }
        #expect(t.lastPlayed == 4)
        #expect(t.isIdle)
    }

    @Test func boardEventsBeforeAnySayApplyImmediately() {
        var t = Timeline(lastPlayed: 0)
        do { let r = seqs(t.receive(step(1))); #expect(r == ["apply1"]) }
        do { let r = seqs(t.receive(show(2))); #expect(r == ["apply2"]) }
    }

    @Test func outOfOrderEventsAreReordered() {
        var t = Timeline(lastPlayed: 0)
        do { let r = seqs(t.receive(show(3))); #expect(r == []) }
        do { let r = seqs(t.receive(step(2))); #expect(r == []) }
        #expect(t.hasGap)
        do { let r = seqs(t.receive(say(1))); #expect(r == ["speak1"]) }
        do { let r = seqs(t.finishedSpeaking(seq: 1)); #expect(r == ["apply2", "apply3"]) }
    }

    @Test func duplicatesAreIgnored() {
        var t = Timeline(lastPlayed: 0)
        _ = t.receive(say(1))
        do { let r = seqs(t.receive(say(1))); #expect(r == []) }
        _ = t.finishedSpeaking(seq: 1)
        do { let r = seqs(t.receive(say(1))); #expect(r == []) }
        do { let r = seqs(t.receive(step(2))); #expect(r == ["apply2"]) }
        do { let r = seqs(t.receive(step(2))); #expect(r == []) }
    }

    @Test func wrongFinishIsIgnored() {
        var t = Timeline(lastPlayed: 0)
        _ = t.receive(say(1))
        _ = t.receive(step(2))
        do { let r = seqs(t.finishedSpeaking(seq: 7)); #expect(r == []) }
        #expect(t.isSpeaking)
    }

    @Test func gapCanBeSkipped() {
        var t = Timeline(lastPlayed: 0)
        _ = t.receive(step(3))
        #expect(t.hasGap)
        do { let r = seqs(t.skipGap()); #expect(r == ["apply3"]) }
        #expect(t.nextSeq == 4)
    }

    @Test func freshTimelineAnchorsOnFirstEvent() {
        var t = Timeline()
        do { let r = seqs(t.receive(say(41))); #expect(r == ["speak41"]) }
        do { let r = seqs(t.receive(step(42))); #expect(r == []) }
        do { let r = seqs(t.finishedSpeaking(seq: 41)); #expect(r == ["apply42"]) }
    }

    @Test func interruptDropsRestOfTurnButKeepsTurnEnd() {
        var t = Timeline(lastPlayed: 0)
        _ = t.receive(say(1))
        _ = t.receive(step(2))
        _ = t.receive(say(3))
        do { let r = seqs(t.interrupt()); #expect(r == []) }
        #expect(!t.isSpeaking)
        // Late events of the interrupted turn are dropped, turn_end still applies.
        do { let r = seqs(t.receive(show(4))); #expect(r == []) }
        do { let r = seqs(t.receive(turnEnd(5))); #expect(r == ["apply5"]) }
        // Next turn plays normally.
        do { let r = seqs(t.receive(say(6, turn: 2))); #expect(r == ["speak6"]) }
        #expect(t.lastPlayed == 5)
    }

    @Test func snapshotRebuildsBoardAndPlaysUnplayed() {
        var t = Timeline(lastPlayed: 3)
        let events = [say(1), step(2), show(3), say(4), show(5)]
        let actions = t.applySnapshot(events.reversed(), rebuildBoard: true)
        #expect(seqs(actions) == ["restore2", "restore3", "speak4"])
        do { let r = seqs(t.finishedSpeaking(seq: 4)); #expect(r == ["apply5"]) }
    }

    @Test func snapshotWithoutRebuildSkipsHistory() {
        var t = Timeline(lastPlayed: 3)
        let actions = t.applySnapshot([step(2), show(3), show(4)], rebuildBoard: false)
        #expect(seqs(actions) == ["apply4"])
    }

    @Test func snapshotReplaysSayCutByDisconnect() {
        var t = Timeline(lastPlayed: 0)
        _ = t.receive(say(1))
        _ = t.finishedSpeaking(seq: 1)
        _ = t.receive(say(2))
        _ = t.receive(step(3))
        // Connection dropped while speaking seq 2; server resends from lastSeq = 1.
        let actions = t.applySnapshot([say(2), step(3)], rebuildBoard: false)
        #expect(seqs(actions) == ["speak2"])
        do { let r = seqs(t.finishedSpeaking(seq: 2)); #expect(r == ["apply3"]) }
    }
}
