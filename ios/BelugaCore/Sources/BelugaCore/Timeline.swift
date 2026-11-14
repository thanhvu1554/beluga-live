import Foundation

/// What is known about the server audio of one `say`.
public enum SayAudio: Sendable, Equatable {
    case mp3(Data)
    /// `audio_skip`: speak with the system voice right away.
    case skip
}

/// What the app must do next as the timeline advances.
public enum TimelineAction: Sendable, Equatable {
    /// Start speaking this `say`. Call `Timeline.finishedSpeaking(seq:)` when playback ends.
    case speak(LessonEvent)
    /// Apply a non-speech event now (board events, turn_end).
    case apply(LessonEvent)
    /// Rebuild the board from history without speaking (after a reconnect / reopen).
    case restore(LessonEvent)
}

/// Orders lesson events by `seq` and paces them against speech:
/// `say` events play one at a time; any event after a `say` waits until that `say` has finished.
/// That is how the board stays in sync with the voice.
public struct Timeline: Sendable {
    /// Highest seq fully played/applied.
    public private(set) var lastPlayed: Int
    /// Next seq the timeline is waiting for.
    public private(set) var nextSeq: Int
    /// The `say` currently being spoken.
    public private(set) var current: LessonEvent?
    /// Events received ahead of `nextSeq`.
    public private(set) var buffer: [Int: LessonEvent] = [:]
    /// Events of turns up to this one are dropped (interrupted), except `turn_end`.
    public private(set) var droppedTurn: Int = -1
    /// Highest turn seen.
    public private(set) var latestTurn: Int = -1
    /// Server TTS audio by `say` seq. It may arrive before its turn to play and out of order.
    public private(set) var audio: [Int: SayAudio] = [:]
    /// Says that gave up waiting and used the system voice: their late audio is dropped.
    private var abandonedAudio: Set<Int> = []

    /// False until the first event fixes where the sequence starts (fresh session).
    public private(set) var anchored: Bool

    /// `lastPlayed == nil`: fresh session, the first event received starts the sequence.
    public init(lastPlayed: Int? = nil) {
        self.lastPlayed = lastPlayed ?? 0
        self.nextSeq = (lastPlayed ?? 0) + 1
        self.anchored = lastPlayed != nil
    }

    public var isSpeaking: Bool { current != nil }
    public var isIdle: Bool { current == nil && buffer.isEmpty }
    /// True when the head of the queue is missing while later events are waiting.
    public var hasGap: Bool { current == nil && !buffer.isEmpty && buffer[nextSeq] == nil }

    // MARK: Server audio

    /// Store an `audio` frame. Returns false when it is no longer wanted (already played,
    /// interrupted, or its say already fell back to the system voice).
    @discardableResult
    public mutating func receiveAudio(seq: Int, data: Data) -> Bool {
        guard anchored ? seq > lastPlayed : true, !abandonedAudio.contains(seq) else { return false }
        audio[seq] = .mp3(data)
        return true
    }

    /// `audio_skip`: the server has no audio for this say.
    public mutating func receiveAudioSkip(seq: Int) {
        guard anchored ? seq > lastPlayed : true, !abandonedAudio.contains(seq) else { return }
        audio[seq] = .skip
    }

    /// Take the audio outcome for a say that is now due, if it is known yet
    /// (nil = keep waiting, up to `ready.audioWaitMs`).
    public mutating func takeAudio(seq: Int) -> SayAudio? {
        audio.removeValue(forKey: seq)
    }

    /// The wait for this say's audio timed out: speak it with the system voice, ignore late audio.
    public mutating func abandonAudio(seq: Int) {
        audio.removeValue(forKey: seq)
        abandonedAudio.insert(seq)
    }

    private mutating func pruneAudio() {
        audio = audio.filter { $0.key > lastPlayed }
        abandonedAudio = abandonedAudio.filter { $0 > lastPlayed - 64 }
    }

    /// Forget everything (new session).
    public mutating func reset(lastPlayed: Int? = nil) {
        self = Timeline(lastPlayed: lastPlayed)
    }

    /// Feed one event (may arrive out of order or duplicated).
    public mutating func receive(_ event: LessonEvent) -> [TimelineAction] {
        latestTurn = max(latestTurn, event.turn)
        if !anchored {
            anchored = true
            nextSeq = event.seq
            lastPlayed = event.seq - 1
        }
        guard event.seq >= nextSeq, buffer[event.seq] == nil else { return [] }
        buffer[event.seq] = event
        return pump()
    }

    /// The `say` with this seq finished playing.
    public mutating func finishedSpeaking(seq: Int) -> [TimelineAction] {
        guard let current, current.seq == seq else { return [] }
        lastPlayed = max(lastPlayed, seq)
        self.current = nil
        let actions = pump()
        pruneAudio()
        return actions
    }

    /// Give up waiting for a missing seq (call after a short timeout while `hasGap`).
    public mutating func skipGap() -> [TimelineAction] {
        guard hasGap, let first = buffer.keys.min() else { return [] }
        nextSeq = first
        return pump()
    }

    /// The user interrupted: stop the current `say` and drop the rest of the turn(s) seen so far,
    /// including events of those turns that are still in flight.
    public mutating func interrupt() -> [TimelineAction] {
        droppedTurn = max(droppedTurn, latestTurn, current?.turn ?? -1)
        if let current {
            lastPlayed = max(lastPlayed, current.seq)
            self.current = nil
        }
        let actions = pump()
        pruneAudio()
        return actions
    }

    /// Apply a `snapshot` sent after `ready` on reconnect.
    /// Events at or below `lastPlayed` were already played: when `rebuildBoard` is true their
    /// board events are restored silently, otherwise they are ignored. Later events play normally.
    public mutating func applySnapshot(_ events: [LessonEvent], rebuildBoard: Bool) -> [TimelineAction] {
        var actions: [TimelineAction] = []
        // Anything queued locally but not yet played is superseded by the snapshot.
        buffer.removeAll()
        if let current {
            // Will be replayed if the snapshot contains it.
            self.current = nil
            nextSeq = current.seq
            lastPlayed = min(lastPlayed, current.seq - 1)
        }
        anchored = true
        let sorted = events.sorted { $0.seq < $1.seq }
        for e in sorted where e.seq <= lastPlayed {
            latestTurn = max(latestTurn, e.turn)
            if rebuildBoard, e.isBoardEvent { actions.append(.restore(e)) }
        }
        nextSeq = lastPlayed + 1
        for e in sorted where e.seq > lastPlayed {
            actions += receive(e)
        }
        return actions
    }

    private mutating func pump() -> [TimelineAction] {
        var actions: [TimelineAction] = []
        while current == nil, let e = buffer.removeValue(forKey: nextSeq) {
            nextSeq += 1
            if e.turn <= droppedTurn, !e.isTurnEnd {
                lastPlayed = max(lastPlayed, e.seq)
                continue
            }
            if e.isSay {
                current = e
                actions.append(.speak(e))
            } else {
                lastPlayed = max(lastPlayed, e.seq)
                actions.append(.apply(e))
            }
        }
        return actions
    }
}
