import Foundation

/// A loosely typed JSON value. Board blocks are passed through to the web board untouched,
/// so the app never needs to know every block type.
public enum JSONValue: Sendable, Equatable, Codable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let b = try? c.decode(Bool.self) { self = .bool(b) }
        else if let n = try? c.decode(Double.self) { self = .number(n) }
        else if let s = try? c.decode(String.self) { self = .string(s) }
        else if let a = try? c.decode([JSONValue].self) { self = .array(a) }
        else { self = .object(try c.decode([String: JSONValue].self)) }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let b): try c.encode(b)
        case .number(let n):
            if n.rounded() == n, abs(n) < 1e15 { try c.encode(Int64(n)) } else { try c.encode(n) }
        case .string(let s): try c.encode(s)
        case .array(let a): try c.encode(a)
        case .object(let o): try c.encode(o)
        }
    }

    public subscript(key: String) -> JSONValue? {
        if case .object(let o) = self { return o[key] }
        return nil
    }

    public var string: String? { if case .string(let s) = self { return s }; return nil }
    public var double: Double? { if case .number(let n) = self { return n }; return nil }
    /// Whole number; nil for NaN, infinity or values outside `Int` (converting those with `Int(_:)` traps).
    public var int: Int? { double.flatMap(JSONValue.clampedInt) }
    /// Number (or number text) as `Int`, never trapping: see `int`.
    public var integer: Int? { numeric.flatMap(JSONValue.clampedInt) }

    /// `Int(d)` without the runtime trap on NaN, infinity or out-of-range values.
    public static func clampedInt(_ d: Double) -> Int? {
        guard d.isFinite else { return nil }
        if d >= 9.0e18 { return Int.max }
        if d <= -9.0e18 { return Int.min }
        return Int(d)
    }
    public var bool: Bool? { if case .bool(let b) = self { return b }; return nil }
    public var array: [JSONValue]? { if case .array(let a) = self { return a }; return nil }
    public var object: [String: JSONValue]? { if case .object(let o) = self { return o }; return nil }
    public var isNull: Bool { self == .null }
    /// A string that is not empty (nil otherwise).
    public var nonEmptyString: String? { string.flatMap { $0.isEmpty ? nil : $0 } }
    /// Number, or a number written as text.
    public var numeric: Double? { double ?? string.flatMap { Double($0) } }
    /// A date sent as text or as epoch seconds/milliseconds, kept as text.
    public var dateText: String? {
        if let s = nonEmptyString { return s }
        if let n = double, let i = JSONValue.clampedInt(n) { return String(i) }
        return nil
    }
    /// String array; non-string items are dropped.
    public var strings: [String] { (array ?? []).compactMap(\.string) }

    public static func parse(_ data: Data) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: data)
    }

    public static func parse(_ text: String) throws -> JSONValue {
        try parse(Data(text.utf8))
    }

    /// Compact JSON text (keys sorted for stable output).
    public var jsonString: String {
        let enc = JSONEncoder()
        enc.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        guard let data = try? enc.encode(self) else { return "null" }
        return String(decoding: data, as: UTF8.self)
    }
}

extension JSONValue: ExpressibleByStringLiteral, ExpressibleByIntegerLiteral, ExpressibleByBooleanLiteral,
    ExpressibleByArrayLiteral, ExpressibleByDictionaryLiteral, ExpressibleByFloatLiteral {
    public init(stringLiteral value: String) { self = .string(value) }
    public init(integerLiteral value: Int) { self = .number(Double(value)) }
    public init(floatLiteral value: Double) { self = .number(value) }
    public init(booleanLiteral value: Bool) { self = .bool(value) }
    public init(arrayLiteral elements: JSONValue...) { self = .array(elements) }
    public init(dictionaryLiteral elements: (String, JSONValue)...) {
        self = .object(Dictionary(elements, uniquingKeysWith: { _, b in b }))
    }
}

extension Array {
    /// Keeps the first element for each id. Server lists feed SwiftUI `ForEach`/`List`, where a repeated id is
    /// undefined behaviour and can abort the app ("Duplicate keys ... were found in a Dictionary").
    public func uniqued<ID: Hashable>(by id: (Element) -> ID) -> [Element] {
        var seen = Set<ID>()
        return filter { seen.insert(id($0)).inserted }
    }
}

extension Array where Element: Identifiable {
    /// `uniqued(by: \.id)`.
    public func uniquedByID() -> [Element] { uniqued { $0.id } }
}
