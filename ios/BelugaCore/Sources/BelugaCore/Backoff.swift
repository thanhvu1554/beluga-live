import Foundation

/// Exponential reconnect backoff with jitter: 0.5s, 1s, 2s, 4s, ... capped at `max`.
public struct Backoff: Sendable {
    public var base: TimeInterval
    public var factor: Double
    public var max: TimeInterval
    /// Fraction of the delay randomized (+/-).
    public var jitter: Double
    public private(set) var attempt = 0

    public init(base: TimeInterval = 0.5, factor: Double = 2, max: TimeInterval = 15, jitter: Double = 0.2) {
        self.base = base; self.factor = factor; self.max = max; self.jitter = jitter
    }

    /// Delay before the next attempt. `random` in 0...1 (injected for tests).
    public mutating func next(random: Double = Double.random(in: 0...1)) -> TimeInterval {
        let raw = Swift.min(max, base * pow(factor, Double(attempt)))
        attempt += 1
        let spread = raw * jitter * (random * 2 - 1)
        return Swift.max(0, Swift.min(max, raw + spread))
    }

    public mutating func reset() { attempt = 0 }
}
