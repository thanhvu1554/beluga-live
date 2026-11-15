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

    /// The path the app's voice preview takes: the body is sniffed as MP3 (ID3 tag or not), cached as `.mp3`,
    /// and decoded from that file with the MP3 type stated. Every real clip shape must come out as 5.6 s of audio.
    @Test func previewPathPlaysID3TaggedClips() throws {
        for name in ["voice-id3", "voice-bigtag", "voice-raw"] {
            let data = try Self.fixture(name)
            #expect(AudioSniff.isMP3(data), "\(name) is recognised as MP3")
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("preview-\(name).mp3")
            try data.write(to: url)
            defer { try? FileManager.default.removeItem(at: url) }
            let player = try AVAudioPlayer(contentsOf: url, fileTypeHint: AVFileType.mp3.rawValue)
            #expect(player.prepareToPlay())
            #expect(abs(player.duration - 5.64) < 0.1, "\(name) decodes to the whole clip")
        }
        // What must never be cached or handed to the decoder.
        #expect(!AudioSniff.isMP3(Data("<!doctype html><title>404</title>".utf8)))
        #expect(!AudioSniff.isMP3(Data(#"{"error":"unauthorized"}"#.utf8)))
        let tagOnly = try Self.fixture("voice-id3").prefix(45)
        #expect(!AudioSniff.isMP3(tagOnly), "a tag with no audio after it (a cut-off download)")
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
