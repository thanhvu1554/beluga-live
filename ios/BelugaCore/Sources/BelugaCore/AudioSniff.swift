import Foundation

/// Looks at the first bytes of a downloaded clip, so a bad body (an HTML or JSON error page, an empty reply) is
/// never cached or handed to the decoder, and the ID3v2 tag that can confuse `AVAudioPlayer(data:)` can be skipped.
public enum AudioSniff {
    public enum Kind: String, Sendable, Equatable {
        /// MP3 starting with an ID3v2 tag.
        case id3
        /// MP3 starting with a frame sync.
        case mpeg
        case empty
        /// Text (HTML, JSON, plain).
        case text
        case unknown
    }

    public static func kind(_ data: Data) -> Kind {
        let b = [UInt8](data.prefix(4))
        guard !b.isEmpty else { return .empty }
        if b.count >= 3, b[0] == 0x49, b[1] == 0x44, b[2] == 0x33 { return .id3 }          // "ID3"
        if b.count >= 2, b[0] == 0xFF, b[1] & 0xE0 == 0xE0 { return .mpeg }
        let printable: ClosedRange<UInt8> = 0x09...0x7E
        if b.allSatisfy({ printable.contains($0) }) { return .text }
        return .unknown
    }

    /// An MP3 the player can be given.
    public static func isMP3(_ data: Data) -> Bool {
        switch kind(data) {
        case .id3: return data.count > id3Size(data)
        case .mpeg: return true
        default: return false
        }
    }

    /// Total size of a leading ID3v2 tag (header, body and optional footer); 0 without one.
    public static func id3Size(_ data: Data) -> Int {
        let b = [UInt8](data.prefix(10))
        guard b.count == 10, b[0] == 0x49, b[1] == 0x44, b[2] == 0x33 else { return 0 }
        // Size is "synchsafe": four 7-bit bytes.
        guard b[6] & 0x80 == 0, b[7] & 0x80 == 0, b[8] & 0x80 == 0, b[9] & 0x80 == 0 else { return 0 }
        let body = Int(b[6]) << 21 | Int(b[7]) << 14 | Int(b[8]) << 7 | Int(b[9])
        let footer = b[5] & 0x10 != 0 ? 10 : 0
        return 10 + body + footer
    }

    /// The audio frames after a leading ID3v2 tag (the data itself when there is none).
    public static func strippingID3(_ data: Data) -> Data {
        let n = id3Size(data)
        guard n > 0, n < data.count else { return data }
        return data.subdata(in: data.startIndex.advanced(by: n)..<data.endIndex)
    }
}
