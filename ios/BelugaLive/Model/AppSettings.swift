import Foundation
import Observation
import Security
import BelugaCore

/// User settings persisted in UserDefaults (pair token in Keychain).
@MainActor @Observable
final class AppSettings {
    static let port = 8787

    var host: String { didSet { save("host", host) } }
    var pairCode: String { didSet { save("pairCode", pairCode) } }
    var lang: Lang { didSet { save("lang", lang.rawValue) } }
    var model: ModelChoice { didSet { save("model", model.rawValue) } }
    var rigor: Rigor { didSet { save("rigor", rigor.rawValue) } }
    var handsFree: Bool { didSet { save("handsFree", handsFree) } }
    var captions: Bool { didSet { save("captions", captions) } }
    var haptics: Bool { didSet { save("haptics", haptics) } }

    private let defaults: UserDefaults
    private let persist: Bool

    init(defaults: UserDefaults = .standard, persist: Bool = true) {
        self.defaults = defaults
        self.persist = persist
        let bundled = (Bundle.main.object(forInfoDictionaryKey: "BelugaDefaultHost") as? String)?
            .trimmingCharacters(in: .whitespaces) ?? ""
        host = defaults.string(forKey: "host") ?? bundled
        pairCode = defaults.string(forKey: "pairCode") ?? ""
        lang = defaults.string(forKey: "lang").flatMap(Lang.init(rawValue:)) ?? .vi
        model = defaults.string(forKey: "model").flatMap(ModelChoice.init(rawValue:)) ?? .sonnet
        rigor = defaults.string(forKey: "rigor").flatMap(Rigor.init(rawValue:)) ?? .medium
        handsFree = defaults.object(forKey: "handsFree") as? Bool ?? false
        captions = defaults.object(forKey: "captions") as? Bool ?? false
        haptics = defaults.object(forKey: "haptics") as? Bool ?? true
    }

    var tutor: TutorSettings { TutorSettings(lang: lang, model: model, rigor: rigor) }

    /// Host without scheme/port, e.g. "my-pc.tailnet.ts.net" or "100.x.y.z".
    var normalizedHost: String {
        var h = host.trimmingCharacters(in: .whitespacesAndNewlines)
        for p in ["ws://", "http://", "wss://", "https://"] where h.lowercased().hasPrefix(p) { h.removeFirst(p.count) }
        if let slash = h.firstIndex(of: "/") { h = String(h[..<slash]) }
        return h
    }

    private var hostPort: String? {
        let h = normalizedHost
        guard !h.isEmpty else { return nil }
        return h.contains(":") && !h.hasPrefix("[") ? h : "\(h):\(Self.port)"
    }

    var webSocketURL: URL? { hostPort.flatMap { URL(string: "ws://\($0)/ws") } }
    var boardURL: URL? { hostPort.flatMap { URL(string: "http://\($0)/board/") } }

    private func save(_ key: String, _ value: Any) {
        guard persist else { return }
        defaults.set(value, forKey: key)
    }

    // MARK: Session continuity

    var lastSessionId: String? {
        get { defaults.string(forKey: "sessionId") }
        set { if persist { defaults.set(newValue, forKey: "sessionId") } }
    }

    var lastPlayedSeq: Int {
        get { defaults.integer(forKey: "lastSeq") }
        set { if persist { defaults.set(newValue, forKey: "lastSeq") } }
    }

    // MARK: Pair token (Keychain)

    var pairToken: String? {
        get { persist ? Keychain.read("pairToken") : nil }
        set {
            guard persist else { return }
            if let newValue { Keychain.write("pairToken", newValue) } else { Keychain.delete("pairToken") }
        }
    }
}

enum Keychain {
    private static let service = "BelugaLive"

    static func read(_ account: String) -> String? {
        let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                kSecAttrAccount as String: account, kSecReturnData as String: true,
                                kSecMatchLimit as String: kSecMatchLimitOne]
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let d = out as? Data else { return nil }
        return String(data: d, encoding: .utf8)
    }

    static func write(_ account: String, _ value: String) {
        delete(account)
        let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                kSecAttrAccount as String: account, kSecValueData as String: Data(value.utf8),
                                kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock]
        SecItemAdd(q as CFDictionary, nil)
    }

    static func delete(_ account: String) {
        let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                kSecAttrAccount as String: account]
        SecItemDelete(q as CFDictionary)
    }
}
