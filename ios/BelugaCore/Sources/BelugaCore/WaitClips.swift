import Foundation

// Wait clips (PROTOCOL section 21): while the tutor thinks in Live or a call, the phone plays a pre-made line in
// the tutor's own voice. Never the system voice. This machine decides when; the app plays what it says through
// the same audio path as the tutor's speech.
//
// | moment                                         | what happens                                           |
// |------------------------------------------------|--------------------------------------------------------|
// | thinking starts (the owner's turn was sent)    | phase `.thinking` (`thinkingSince` set)                 |
// | > 3 s, no `say` has its audio yet               | one random `short` clip, not the last one used; `vision`|
// |                                                | instead when the turn has a photo                       |
// | > 12 s                                         | one `long` clip (once per turn)                         |
// | first `say` has its audio                       | no more clips this turn; a clip still playing may finish|
// |                                                | (at most 2.5 s more), then the say plays                |
// | no clips (device voice, not downloaded yet)    | nothing is said                                         |
// | turn end, interrupt, the owner speaks           | clip stops, turn over                                   |

/// What the thinking period is doing right now (for the pill's light and a future sound layer).
public enum ThinkingPhase: Sendable, Equatable {
    /// Not thinking.
    case idle
    /// Thinking, nothing said yet.
    case thinking
    /// A wait clip is playing.
    case filler(String)
    /// The first spoken line is ready and waits for the clip to finish.
    case holding
}

public enum WaitClipInput: Sendable, Equatable {
    /// The owner's turn went to the tutor (Live or a call).
    case thinkingStarted
    /// The turn carries a photo (`tool:"capture"`, or a photo was just sent).
    case photo
    /// The app started a clip; `duration` in seconds (nil if unknown).
    case clipStarted(id: String, duration: TimeInterval?)
    case clipFinished(id: String)
    case turnEnded
    case interrupted
    /// The owner started speaking (mic opened).
    case userSpoke
    case tick
}

public enum WaitClipAction: Sendable, Equatable {
    case play(fillerId: String)
    /// Stop the clip now.
    case stop
}

public struct WaitClips: Sendable {
    public var shortAfter: TimeInterval = 3
    public var longAfter: TimeInterval = 12
    /// The longest a ready `say` waits for a clip to finish.
    public var holdMax: TimeInterval = 2.5

    public private(set) var thinkingSince: Date?
    public private(set) var playingId: String?
    public private(set) var playingEndsAt: Date?
    public private(set) var holdUntil: Date?
    public private(set) var turnHasPhoto = false
    public private(set) var shortUsed = false
    public private(set) var longUsed = false
    /// The first `say` of the turn has its audio: no more clips until the next turn.
    public private(set) var sayReady = false
    public private(set) var lastShort: String?
    private var clips: [VoiceFiller] = []
    private var rng: UInt64

    public init(seed: UInt64 = UInt64.random(in: 1...UInt64.max)) { rng = seed == 0 ? 0x9E37_79B9_7F4A_7C15 : seed }

    /// Lines with a clip on the phone for the current voice (empty: nothing is ever played).
    public mutating func setClips(_ fillers: [VoiceFiller]) { clips = fillers }
    public var availableClips: [VoiceFiller] { clips }

    public var phase: ThinkingPhase {
        guard thinkingSince != nil || playingId != nil else { return .idle }
        if holdUntil != nil { return .holding }
        if let playingId { return .filler(playingId) }
        return sayReady ? .idle : .thinking
    }

    public mutating func handle(_ input: WaitClipInput, now: Date) -> [WaitClipAction] {
        switch input {
        case .thinkingStarted:
            let wasPlaying = playingId != nil
            resetTurn()
            thinkingSince = now
            return wasPlaying ? [.stop] : []
        case .photo:
            turnHasPhoto = true
            return []
        case let .clipStarted(id, duration):
            guard playingId == id else { return [] }
            playingEndsAt = duration.map { now.addingTimeInterval(max(0, $0)) }
            return []
        case .clipFinished(let id):
            guard playingId == id else { return [] }
            playingId = nil
            playingEndsAt = nil
            holdUntil = nil
            if sayReady { thinkingSince = nil }
            return []
        case .turnEnded, .interrupted, .userSpoke:
            let wasPlaying = playingId != nil
            resetTurn()
            return wasPlaying ? [.stop] : []
        case .tick:
            return tick(now)
        }
    }

    /// The first `say` of the turn is ready to play. Returns when it may start: now (nil), or the moment the
    /// clip still playing ends, at most `holdMax` from now.
    public mutating func sayReady(now: Date) -> Date? {
        let first = !sayReady
        sayReady = true
        guard first, playingId != nil else {
            if playingId == nil { thinkingSince = nil }
            return nil
        }
        let cap = now.addingTimeInterval(holdMax)
        let until = playingEndsAt.map { min($0, cap) } ?? cap
        guard until > now else { return nil }
        holdUntil = until
        return until
    }

    /// A random `hello` line for a call that just connected (nil without one).
    public mutating func helloClip() -> String? {
        pick(clips.filter { $0.when == .hello }, avoiding: nil)?.id
    }

    private mutating func tick(_ now: Date) -> [WaitClipAction] {
        // A held say may go once its wait is over: the clip is cut if it is still talking.
        if let hold = holdUntil, now >= hold {
            holdUntil = nil
            let wasPlaying = playingId != nil
            playingId = nil
            playingEndsAt = nil
            thinkingSince = nil
            return wasPlaying ? [.stop] : []
        }
        guard let since = thinkingSince, !sayReady, playingId == nil else { return [] }
        let elapsed = now.timeIntervalSince(since)
        if elapsed >= longAfter, !longUsed {
            longUsed = true
            shortUsed = true
            if let clip = pick(clips.filter { $0.when == .long }, avoiding: nil) { return start(clip, now) }
        }
        if elapsed >= shortAfter, !shortUsed {
            shortUsed = true
            let vision = turnHasPhoto ? pick(clips.filter { $0.when == .vision }, avoiding: nil) : nil
            if let clip = vision ?? pick(clips.filter { $0.when == .short }, avoiding: lastShort) {
                if clip.when == .short { lastShort = clip.id }
                return start(clip, now)
            }
        }
        return []
    }

    private mutating func start(_ clip: VoiceFiller, _ now: Date) -> [WaitClipAction] {
        playingId = clip.id
        playingEndsAt = nil
        return [.play(fillerId: clip.id)]
    }

    private mutating func resetTurn() {
        thinkingSince = nil
        playingId = nil
        playingEndsAt = nil
        holdUntil = nil
        turnHasPhoto = false
        shortUsed = false
        longUsed = false
        sayReady = false
    }

    /// Uniform pick, different from `avoiding` when there is another choice.
    private mutating func pick(_ list: [VoiceFiller], avoiding: String?) -> VoiceFiller? {
        guard !list.isEmpty else { return nil }
        let pool = list.count > 1 ? list.filter { $0.id != avoiding } : list
        return pool[Int(nextRandom() % UInt64(pool.count))]
    }

    /// SplitMix64: deterministic for a given seed (tests), cheap.
    private mutating func nextRandom() -> UInt64 {
        rng &+= 0x9E37_79B9_7F4A_7C15
        var z = rng
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}
