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
    var model: String { didSet { save("model", model) } }
    var rigor: Rigor { didSet { save("rigor", rigor.rawValue) } }
    var handsFree: Bool { didSet { save("handsFree", handsFree) } }
    var captions: Bool { didSet { save("captions", captions) } }
    var haptics: Bool { didSet { save("haptics", haptics) } }
    /// Effort for the chosen model; empty = the model's default.
    var effort: String { didSet { save("effort", effort) } }
    /// Mode a new session starts in.
    var defaultMode: SessionMode { didSet { save("defaultMode", defaultMode.rawValue) } }
    /// Let the server correct speech recognition with a cheap model (PROTOCOL section 7).
    var sttFix: Bool { didSet { save("sttFix", sttFix) } }

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
        model = defaults.string(forKey: "model") ?? "sonnet"
        rigor = defaults.string(forKey: "rigor").flatMap(Rigor.init(rawValue:)) ?? .medium
        handsFree = defaults.object(forKey: "handsFree") as? Bool ?? false
        captions = defaults.object(forKey: "captions") as? Bool ?? false
        haptics = defaults.object(forKey: "haptics") as? Bool ?? true
        effort = defaults.string(forKey: "effort") ?? ""
        defaultMode = defaults.string(forKey: "defaultMode").flatMap(SessionMode.init(rawValue:)) ?? .chat
        sttFix = defaults.object(forKey: "sttFix") as? Bool ?? true
    }

    func tutor(mode: SessionMode) -> TutorSettings {
        TutorSettings(lang: lang, model: model, effort: effort.isEmpty ? nil : effort, rigor: rigor, mode: mode, sttFix: sttFix)
    }

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
    /// `http://<pc>:8787` for uploads, files and exams (PROTOCOL sections 9 and 13).
    var httpBase: URL? { hostPort.flatMap { URL(string: "http://\($0)") } }

    private func withToken(_ path: String) -> URL? {
        guard let base = httpBase,
              var c = URLComponents(url: base.appendingPathComponent(path), resolvingAgainstBaseURL: false) else { return nil }
        c.queryItems = [URLQueryItem(name: "token", value: pairToken ?? "")]
        return c.url
    }
    func fileURL(_ fileId: String) -> URL? { withToken("file/\(fileId)") }
    func examURL(_ examId: String) -> URL? { withToken("exam/\(examId)") }

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
