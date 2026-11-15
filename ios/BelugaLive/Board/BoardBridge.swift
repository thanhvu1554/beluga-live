import SwiftUI
import WebKit
import BelugaCore

/// Owns the board WKWebView and speaks the JS contract in PROTOCOL section 3.
@MainActor @Observable
final class BoardBridge: NSObject, @preconcurrency WKScriptMessageHandler {
    private(set) var ready = false
    private(set) var stepCount = 0
    private(set) var currentStep = 0
    private(set) var hasContent = false

    let webView: WKWebView
    @ObservationIgnored private var queue: [String] = []
    @ObservationIgnored private var theme = "light"
    @ObservationIgnored private var lang = "vi"

    /// Version of the board bundled with the app (`board/version.json`).
    static let bundledVersion: Int = {
        guard let url = Bundle.main.url(forResource: "version", withExtension: "json", subdirectory: "board"),
              let data = try? Data(contentsOf: url), let v = try? JSONValue.parse(data) else { return 0 }
        return v["version"]?.int ?? 0
    }()

    override init() {
        let config = WKWebViewConfiguration()
        let controller = WKUserContentController()
        config.userContentController = controller
        config.suppressesIncrementalRendering = false
        webView = WKWebView(frame: .zero, configuration: config)
        super.init()
        controller.add(WeakHandler(self), name: "board")
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.backgroundColor = .clear
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        webView.scrollView.bounces = false
        #if DEBUG
        webView.isInspectable = true
        #endif
        loadBundled()
    }

    func loadBundled() {
        guard let index = Bundle.main.url(forResource: "index", withExtension: "html", subdirectory: "board") else { return }
        ready = false
        webView.loadFileURL(index, allowingReadAccessTo: index.deletingLastPathComponent())
    }

    /// Server has a newer board: load it from `http://<pc>:8787/board/`.
    func loadRemote(_ url: URL) {
        ready = false
        webView.load(URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 8))
    }

    func reset() {
        stepCount = 0; currentStep = 0; hasContent = false
        call("board.reset()")
    }

    /// `restoring` draws the event without animation (history, snapshots, mock scenes).
    func apply(_ event: LessonEvent, restoring: Bool = false) {
        // The position comes from the page's `stepChanged` (it may stay on an older step the user is reading).
        if case .step(let index, _) = event.kind { stepCount = max(stepCount, index + 1) }
        hasContent = true
        call(restoring ? "board.restore([\(event.raw.jsonString)])" : "board.apply(\(event.raw.jsonString))")
    }

    /// Space (px) kept clear for native chrome; the live panel keeps the default 128 at the bottom.
    func setInsets(top: Int, bottom: Int) {
        call("board.setInsets&&board.setInsets(\(top),\(bottom))")
    }

    func goto(_ index: Int) {
        call("board.goto(\(index))")
    }

    func setTheme(_ dark: Bool) {
        theme = dark ? "dark" : "light"
        call("board.setTheme(\"\(theme)\")")
    }

    func setLang(_ l: Lang) {
        lang = l.rawValue
        call("board.setLang(\"\(lang)\")")
    }

    private func call(_ js: String) {
        guard ready else { queue.append(js); return }
        webView.evaluateJavaScript(js, completionHandler: nil)
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        let body = message.body as? [String: Any]
        let type = body?["type"] as? String
        let step = body?["index"] as? Int ?? body?["step"] as? Int
        let count = body?["count"] as? Int
        do {
            switch type {
            case "ready":
                ready = true
                let pending = queue
                queue.removeAll()
                // The app draws its own step ticks in the panel header.
                webView.evaluateJavaScript("board.setTheme(\"\(theme)\");board.setLang(\"\(lang)\");board.setOptions&&board.setOptions({dots:false})", completionHandler: nil)
                for js in pending { webView.evaluateJavaScript(js, completionHandler: nil) }
            case "stepChanged":
                if let step { currentStep = step }
                if let count { stepCount = count }
            default: break
            }
        }
    }
}

/// Avoids the retain cycle WKUserContentController creates with its handlers.
@MainActor
private final class WeakHandler: NSObject, @preconcurrency WKScriptMessageHandler {
    weak var target: (any WKScriptMessageHandler)?
    init(_ t: any WKScriptMessageHandler) { target = t }
    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        target?.userContentController(c, didReceive: m)
    }
}

struct BoardWebView: UIViewRepresentable {
    let bridge: BoardBridge
    func makeUIView(context: Context) -> WKWebView { bridge.webView }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}
