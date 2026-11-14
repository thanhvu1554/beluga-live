import AVFoundation
import UIKit

/// Short, low, quiet tones synthesized at launch (no audio files).
enum Earcon: CaseIterable {
    case listen      // mic opens: soft rising fifth
    case send        // utterance sent: single low note
    case ask         // the tutor waits for an answer
    case done        // lesson finished: low resolved dyad
    case offline     // connection lost
    case shutter     // photo taken

    static let format = AVAudioFormat(standardFormatWithSampleRate: 44_100, channels: 1)!

    /// (frequency Hz, start s, duration s, gain)
    private var notes: [(Double, Double, Double, Float)] {
        switch self {
        case .listen: return [(196, 0, 0.16, 0.10), (294, 0.08, 0.22, 0.09)]
        case .send: return [(220, 0, 0.20, 0.09)]
        case .ask: return [(262, 0, 0.18, 0.08), (262, 0.20, 0.24, 0.06)]
        case .done: return [(196, 0, 0.5, 0.08), (294, 0.05, 0.5, 0.06), (392, 0.10, 0.5, 0.04)]
        case .offline: return [(165, 0, 0.30, 0.08)]
        case .shutter: return [(1200, 0, 0.025, 0.05), (800, 0.03, 0.03, 0.04)]
        }
    }

    static func renderAll() -> [Earcon: AVAudioPCMBuffer] {
        var out: [Earcon: AVAudioPCMBuffer] = [:]
        for e in allCases { if let b = e.render() { out[e] = b } }
        return out
    }

    private func render() -> AVAudioPCMBuffer? {
        let sr = Self.format.sampleRate
        let total = notes.map { $0.1 + $0.2 }.max() ?? 0.2
        let frames = AVAudioFrameCount(total * sr) + 1
        guard let buf = AVAudioPCMBuffer(pcmFormat: Self.format, frameCapacity: frames),
              let ch = buf.floatChannelData?[0] else { return nil }
        buf.frameLength = frames
        for i in 0..<Int(frames) { ch[i] = 0 }
        for (f, start, dur, gain) in notes {
            let s0 = Int(start * sr), n = Int(dur * sr)
            for i in 0..<n where s0 + i < Int(frames) {
                let t = Double(i) / sr
                let attack = min(1, t / 0.006)
                let decay = exp(-t / (dur * 0.35))
                // Sine with a touch of second harmonic: warm, not beepy.
                let v = sin(2 * .pi * f * t) + 0.18 * sin(4 * .pi * f * t)
                ch[s0 + i] += Float(v * attack * decay) * gain
            }
        }
        return buf
    }
}

@MainActor
enum Haptics {
    static var enabled = true

    static func tap() {
        guard enabled else { return }
        UIImpactFeedbackGenerator(style: .soft).impactOccurred(intensity: 0.6)
    }

    static func light() {
        guard enabled else { return }
        UIImpactFeedbackGenerator(style: .light).impactOccurred(intensity: 0.5)
    }

    static func success() {
        guard enabled else { return }
        UINotificationFeedbackGenerator().notificationOccurred(.success)
    }
}
