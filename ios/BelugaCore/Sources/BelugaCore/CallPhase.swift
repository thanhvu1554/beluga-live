import Foundation

// The call screen's phases (PROTOCOL section 22). Phone side only, no new frames:
//
// 1. `dialing`   "Đang gọi Beluga…" and ringing, while the live session is prepared underneath. Lasts until the
//                session is ready, at least 1.6 s, at most 8 s (then `failed`, with "Gọi lại").
// 2. `connected` a connect sound and tap, "Đã kết nối" for 1.2 s, then the mm:ss timer; a `hello` clip, then listen.
// 3. `ended`     "Đã kết thúc · mm:ss" for 0.9 s, then the screen closes.
//
// Hanging up works in every phase. Time is passed in (`now`), so tests drive it without waiting.

public enum CallPhase: Sendable, Equatable {
    case idle
    case dialing(since: Date)
    case connected(at: Date)
    /// `duration`: how long the call was connected (nil when it never connected).
    case ended(at: Date, duration: TimeInterval?)
    case failed(at: Date)

    public var isDialing: Bool { if case .dialing = self { return true }; return false }
    public var isConnected: Bool { if case .connected = self { return true }; return false }
    public var isEnded: Bool { if case .ended = self { return true }; return false }
    public var isFailed: Bool { if case .failed = self { return true }; return false }
}

public enum CallPhaseInput: Sendable, Equatable {
    /// The call button was pressed.
    case start
    /// The live session under the call is ready (connected, live mode, audio and mic usable), or stopped being.
    case sessionReady(Bool)
    /// The red button.
    case hangUp
    /// "Gọi lại" after a failure.
    case retry
    case tick
}

public enum CallPhaseEffect: Sendable, Equatable {
    case startRinging
    case stopRinging
    /// Connect sound, haptic, then the hello clip and listening.
    case connected
    case failed
    /// Stop the tutor and the microphone now (the screen stays for the ended beat).
    case hangUp
    /// Close the call screen.
    case close
}

/// What the header says.
public enum CallLabel: Sendable, Equatable {
    case none
    case dialing
    case connected
    /// The running timer since this moment.
    case timer(since: Date)
    case ended(duration: TimeInterval?)
    case failed
}

public struct CallPhaseMachine: Sendable {
    public var minDial: TimeInterval = 1.6
    public var maxDial: TimeInterval = 8
    public var connectedLabel: TimeInterval = 1.2
    public var endedHold: TimeInterval = 0.9

    public private(set) var phase: CallPhase
    public private(set) var sessionReady = false

    public init(phase: CallPhase = .idle) { self.phase = phase }

    /// The call screen is up (any phase but idle).
    public var isActive: Bool { phase != .idle }

    public mutating func handle(_ input: CallPhaseInput, now: Date) -> [CallPhaseEffect] {
        switch input {
        case .start:
            switch phase {
            case .idle, .ended, .failed:
                phase = .dialing(since: now)
                return [.startRinging] + evaluate(now)
            case .dialing, .connected:
                return []
            }
        case .sessionReady(let ready):
            sessionReady = ready
            return evaluate(now)
        case .retry:
            guard phase.isFailed else { return [] }
            phase = .dialing(since: now)
            return [.startRinging] + evaluate(now)
        case .hangUp:
            switch phase {
            case .idle:
                return []
            case .dialing:
                phase = .ended(at: now, duration: nil)
                return [.stopRinging, .hangUp]
            case .connected(let at):
                phase = .ended(at: now, duration: max(0, now.timeIntervalSince(at)))
                return [.hangUp]
            case .ended, .failed:
                // A second tap does not wait for the beat.
                phase = .idle
                return [.hangUp, .close]
            }
        case .tick:
            return evaluate(now)
        }
    }

    private mutating func evaluate(_ now: Date) -> [CallPhaseEffect] {
        switch phase {
        case .dialing(let since):
            let elapsed = now.timeIntervalSince(since)
            if sessionReady, elapsed >= minDial {
                phase = .connected(at: now)
                return [.stopRinging, .connected]
            }
            if elapsed >= maxDial {
                phase = .failed(at: now)
                return [.stopRinging, .failed]
            }
            return []
        case .ended(let at, _):
            if now.timeIntervalSince(at) >= endedHold {
                phase = .idle
                return [.close]
            }
            return []
        default:
            return []
        }
    }

    public func label(now: Date) -> CallLabel {
        switch phase {
        case .idle: return .none
        case .dialing: return .dialing
        case .connected(let at): return now.timeIntervalSince(at) < connectedLabel ? .connected : .timer(since: at)
        case .ended(_, let d): return .ended(duration: d)
        case .failed: return .failed
        }
    }
}
