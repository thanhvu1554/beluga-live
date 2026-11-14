import Foundation

// Wire model for the `progress` frame (docs/PROTOCOL.md section 6). Decoding is tolerant:
// missing fields become empty values and dates may arrive as text or as epoch numbers,
// because the server writes them from hand-editable memory files.

/// A string that may arrive as JSON text or as a number (epoch seconds or milliseconds).
struct FlexibleText: Decodable, Sendable {
    let value: String?

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { value = nil }
        else if let s = try? c.decode(String.self) { value = s.isEmpty ? nil : s }
        else if let n = try? c.decode(Double.self) { value = String(Int64(n)) }
        else { value = nil }
    }
}

public struct WeakTopic: Sendable, Equatable, Codable, Identifiable {
    public var topic: String
    public var note: String
    /// How many times the learner got this wrong.
    public var count: Int
    public var lastSeen: String?
    /// When the next review is due (date text as sent by the server).
    public var due: String?
    public var id: String { topic }

    public init(topic: String, note: String = "", count: Int = 0, lastSeen: String? = nil, due: String? = nil) {
        self.topic = topic; self.note = note; self.count = count; self.lastSeen = lastSeen; self.due = due
    }

    enum CodingKeys: String, CodingKey { case topic, note, count, lastSeen, due }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        topic = (try? c.decode(String.self, forKey: .topic)) ?? ""
        note = (try? c.decode(String.self, forKey: .note)) ?? ""
        count = (try? c.decode(Int.self, forKey: .count)) ?? Int((try? c.decode(Double.self, forKey: .count)) ?? 0)
        lastSeen = (try? c.decode(FlexibleText.self, forKey: .lastSeen))?.value
        due = (try? c.decode(FlexibleText.self, forKey: .due))?.value
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(topic, forKey: .topic)
        try c.encode(note, forKey: .note)
        try c.encode(count, forKey: .count)
        try c.encodeIfPresent(lastSeen, forKey: .lastSeen)
        try c.encodeIfPresent(due, forKey: .due)
    }
}

/// A topic with a date: mastered or recently studied.
public struct DatedTopic: Sendable, Equatable, Codable, Identifiable {
    public var topic: String
    public var date: String?
    public var id: String { topic + "|" + (date ?? "") }

    public init(topic: String, date: String? = nil) { self.topic = topic; self.date = date }

    enum CodingKeys: String, CodingKey { case topic, date }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        topic = (try? c.decode(String.self, forKey: .topic)) ?? ""
        date = (try? c.decode(FlexibleText.self, forKey: .date))?.value
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(topic, forKey: .topic)
        try c.encodeIfPresent(date, forKey: .date)
    }
}

public struct SubjectProgress: Sendable, Equatable, Codable, Identifiable {
    public var id: String
    public var name: String
    public var weakTopics: [WeakTopic]
    public var mastered: [DatedTopic]
    public var recent: [DatedTopic]

    public init(id: String, name: String, weakTopics: [WeakTopic] = [], mastered: [DatedTopic] = [], recent: [DatedTopic] = []) {
        self.id = id; self.name = name; self.weakTopics = weakTopics; self.mastered = mastered; self.recent = recent
    }

    enum CodingKeys: String, CodingKey {
        case id, name, mastered, recent
        case weakTopics = "weak"
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = (try? c.decode(String.self, forKey: .id)) ?? ""
        name = (try? c.decode(String.self, forKey: .name)) ?? id
        weakTopics = (try? c.decode([WeakTopic].self, forKey: .weakTopics)) ?? []
        mastered = (try? c.decode([DatedTopic].self, forKey: .mastered)) ?? []
        recent = (try? c.decode([DatedTopic].self, forKey: .recent)) ?? []
    }

    /// True when there is nothing to show for this subject.
    public var isEmpty: Bool { weakTopics.isEmpty && mastered.isEmpty && recent.isEmpty }
}

/// The whole progress report. (Named to avoid clashing with Foundation.Progress.)
public struct LearningProgress: Sendable, Equatable, Codable {
    public var subjects: [SubjectProgress]
    /// What teaching style works for this learner (from `profile.md`).
    public var style: [String]
    public var updatedAt: String?

    public init(subjects: [SubjectProgress] = [], style: [String] = [], updatedAt: String? = nil) {
        self.subjects = subjects; self.style = style; self.updatedAt = updatedAt
    }

    enum CodingKeys: String, CodingKey { case subjects, style, updatedAt }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        subjects = (try? c.decode([SubjectProgress].self, forKey: .subjects)) ?? []
        style = (try? c.decode([String].self, forKey: .style)) ?? []
        updatedAt = (try? c.decode(FlexibleText.self, forKey: .updatedAt))?.value
    }

    /// Nothing recorded yet: no subject has any entry and there is no style note.
    public var isEmpty: Bool { subjects.allSatisfy(\.isEmpty) && style.isEmpty }

    /// Builds the model from a raw frame; anything unreadable gives an empty report.
    public static func from(_ v: JSONValue) -> LearningProgress {
        guard let data = try? JSONEncoder().encode(v),
              let decoded = try? JSONDecoder().decode(LearningProgress.self, from: data) else { return LearningProgress() }
        return decoded
    }
}

/// Parses the date texts that appear in server frames and memory notes.
public enum LearningDate {
    /// Accepts `2026-11-14`, ISO 8601 (with or without fractional seconds), and epoch seconds or milliseconds.
    public static func parse(_ raw: String) -> Date? {
        let text = raw.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return nil }
        if let n = Double(text) { return Date(timeIntervalSince1970: n > 1e11 ? n / 1000 : n) }
        let iso = ISO8601DateFormatter()
        if let d = iso.date(from: text) { return d }
        iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = iso.date(from: text) { return d }
        iso.formatOptions = [.withFullDate]
        return iso.date(from: String(text.prefix(10)))
    }

    /// True when `raw` is a date on or before `now` (a review that is due).
    public static func isDue(_ raw: String?, now: Date = Date()) -> Bool {
        guard let raw, let date = parse(raw) else { return false }
        return date <= now
    }
}
