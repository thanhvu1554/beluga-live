import Foundation

// Voice catalogue: `get_voices` → `voices{items, current, provider}`; choosing sends `settings{voice}` and the
// server answers with a fresh `voices`. Preview clips: `GET /voice/<id>.mp3` with `Authorization: Bearer <pairToken>`.

public struct VoiceInfo: Sendable, Equatable, Identifiable, Hashable {
    public enum Gender: String, Sendable { case female, male, unknown }
    public enum Accent: String, Sendable { case north, central, south, unknown }

    public var id: String
    public var name: String
    public var gender: Gender
    public var accent: Accent
    /// One short line describing the voice.
    public var desc: String
    /// The server has a preview clip for this voice.
    public var preview: Bool
    /// Ids of the wait/hello lines this voice has a clip for (v3, section 21).
    public var fillers: [String]

    public init(id: String, name: String, gender: Gender = .unknown, accent: Accent = .unknown, desc: String = "",
                preview: Bool = false, fillers: [String] = []) {
        self.id = id; self.name = name; self.gender = gender; self.accent = accent; self.desc = desc; self.preview = preview
        self.fillers = fillers
    }

    static func from(_ v: JSONValue) -> VoiceInfo? {
        guard let id = v["id"]?.nonEmptyString else { return nil }
        return VoiceInfo(id: id, name: v["name"]?.nonEmptyString ?? id,
                         gender: v["gender"]?.string.flatMap(Gender.init(rawValue:)) ?? .unknown,
                         accent: v["accent"]?.string.flatMap(Accent.init(rawValue:)) ?? .unknown,
                         desc: v["desc"]?.string ?? "", preview: v["preview"]?.bool ?? false,
                         fillers: (v["fillers"]?.strings ?? []).filter { !$0.isEmpty }.uniqued { $0 })
    }
}

/// The `voices` frame.
public struct VoiceCatalog: Sendable, Equatable {
    public var items: [VoiceInfo]
    /// Id of the voice in use (may be empty or unknown to `items`).
    public var current: String?
    /// "genvoice" | "elevenlabs" | "device" (any other value is kept as is).
    public var provider: String
    /// Every wait/hello line the server knows (v3); each voice lists the ones it has a clip for.
    public var fillers: [VoiceFiller]

    public init(items: [VoiceInfo] = [], current: String? = nil, provider: String = "", fillers: [VoiceFiller] = []) {
        self.items = items; self.current = current; self.provider = provider; self.fillers = fillers
    }

    /// Lines with a clip in the current voice (none with the device voice or an older server).
    public var currentFillers: [VoiceFiller] {
        guard !isDevice, let id = current, let voice = items.first(where: { $0.id == id }) else { return [] }
        let have = Set(voice.fillers)
        return fillers.filter { have.contains($0.id) }
    }

    /// The tutor speaks with the phone's own voice: there is nothing to choose.
    public var isDevice: Bool { provider == "device" }

    static func from(_ v: JSONValue) -> VoiceCatalog {
        VoiceCatalog(items: (v["items"]?.array ?? []).compactMap(VoiceInfo.from).uniquedByID(),
                     current: v["current"]?.nonEmptyString, provider: v["provider"]?.string ?? "",
                     fillers: (v["fillers"]?.array ?? []).compactMap(VoiceFiller.from).uniquedByID())
    }
}
