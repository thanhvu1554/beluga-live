import Foundation

/// Hands-free mode: a simple level + silence detector.
/// Feed it RMS levels in dBFS; it reports when speech starts and when it ends.
public struct VoiceActivityDetector: Sendable {
    public enum Event: Sendable, Equatable { case speechStarted, speechEnded }

    /// Speech must be this many dB above the tracked noise floor.
    public var margin: Float = 12
    /// Absolute floor below which nothing counts as speech.
    public var minimumLevel: Float = -50
    /// Speech must last this long to count.
    public var minSpeech: TimeInterval = 0.2
    /// This much silence ends an utterance.
    public var silence: TimeInterval = 0.9

    public private(set) var noiseFloor: Float = -60
    public private(set) var inSpeech = false
    private var loudSince: TimeInterval?
    private var quietSince: TimeInterval?

    public init() {}

    public mutating func reset() {
        inSpeech = false; loudSince = nil; quietSince = nil
    }

    public mutating func feed(level db: Float, at t: TimeInterval) -> Event? {
        let loud = db > Swift.max(minimumLevel, noiseFloor + margin)
        if !loud {
            // Track the noise floor slowly upward, quickly downward.
            noiseFloor = db < noiseFloor ? db : noiseFloor + (db - noiseFloor) * 0.02
        }
        if !inSpeech {
            if loud {
                if loudSince == nil { loudSince = t }
                if let s = loudSince, t - s >= minSpeech {
                    inSpeech = true; quietSince = nil
                    return .speechStarted
                }
            } else {
                loudSince = nil
            }
        } else {
            if loud {
                quietSince = nil
            } else {
                if quietSince == nil { quietSince = t }
                if let q = quietSince, t - q >= silence {
                    inSpeech = false; loudSince = nil; quietSince = nil
                    return .speechEnded
                }
            }
        }
        return nil
    }
}
