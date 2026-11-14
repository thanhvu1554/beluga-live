import SwiftUI
import BelugaCore

/// In-app language (independent of the system language): looks keys up in the String Catalog
/// compiled into `vi.lproj` / `en.lproj`.
struct Strings: Sendable {
    let lang: Lang

    nonisolated(unsafe) private static let bundles: [Lang: Bundle] = {
        var out: [Lang: Bundle] = [:]
        for l in Lang.allCases {
            if let path = Bundle.main.path(forResource: l.rawValue, ofType: "lproj"), let b = Bundle(path: path) {
                out[l] = b
            }
        }
        return out
    }()

    func callAsFunction(_ key: String) -> String {
        let bundle = Self.bundles[lang] ?? .main
        return bundle.localizedString(forKey: key, value: key, table: "Localizable")
    }

    func callAsFunction(_ key: String, _ args: CVarArg...) -> String {
        String(format: callAsFunction(key), locale: Locale(identifier: lang.rawValue), arguments: args)
    }
}

private struct StringsKey: EnvironmentKey {
    static let defaultValue = Strings(lang: .vi)
}

extension EnvironmentValues {
    var strings: Strings {
        get { self[StringsKey.self] }
        set { self[StringsKey.self] = newValue }
    }
}
