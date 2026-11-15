#if canImport(AVFoundation)
import AVFoundation
import Foundation
import Testing
@testable import BelugaCore

/// Real decoding of the voice preview clips the server stores (`server/voices/<id>.mp3`, made by ffmpeg, so they
/// start with an ID3v2.4 tag). Runs Core Audio on the CI Mac: the same decoder family as the phone.
/// Every variant is printed (`AUDIOPROBE`) so the CI log shows which ones Core Audio accepts.
@Suite struct AudioDecodeTests {
    static func fixture(_ name: String) throws -> Data {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "mp3", subdirectory: "Fixtures"))
        return try Data(contentsOf: url)
    }

    /// nil when AVAudioPlayer refuses the bytes, else the clip's duration.
    static func player(_ data: Data, hint: String?) -> (duration: TimeInterval?, error: String?) {
        do {
            let p = try hint.map { try AVAudioPlayer(data: data, fileTypeHint: $0) } ?? AVAudioPlayer(data: data)
            _ = p.prepareToPlay()
            return (p.duration, nil)
        } catch {
            return (nil, "\((error as NSError).code)")
        }
    }

    static func file(_ data: Data) -> TimeInterval? {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("probe-\(UUID().uuidString).mp3")
        defer { try? FileManager.default.removeItem(at: url) }
        guard (try? data.write(to: url)) != nil, let f = try? AVAudioFile(forReading: url) else { return nil }
        return Double(f.length) / f.processingFormat.sampleRate
    }

    @Test func probeEveryVariant() throws {
        for name in ["voice-id3", "voice-raw", "voice-bigtag"] {
            let data = try Self.fixture(name)
            let plain = Self.player(data, hint: nil)
            let hinted = Self.player(data, hint: AVFileType.mp3.rawValue)
            let stripped = Self.player(AudioSniff.strippingID3(data), hint: nil)
            print("AUDIOPROBE \(name) bytes=\(data.count) kind=\(AudioSniff.kind(data)) tag=\(AudioSniff.id3Size(data))"
                  + " player(data)=\(plain.duration.map { String(format: "%.2fs", $0) } ?? "FAIL \(plain.error ?? "")")"
                  + " player(data,hint mp3)=\(hinted.duration.map { String(format: "%.2fs", $0) } ?? "FAIL \(hinted.error ?? "")")"
                  + " player(stripped)=\(stripped.duration.map { String(format: "%.2fs", $0) } ?? "FAIL \(stripped.error ?? "")")"
                  + " file(.mp3)=\(Self.file(data).map { String(format: "%.2fs", $0) } ?? "FAIL")")
        }
    }
}
#endif
