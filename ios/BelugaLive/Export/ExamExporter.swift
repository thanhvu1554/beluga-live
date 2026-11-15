import UIKit
import WebKit

/// Turns the printable exam page (`GET /exam/<id>?token=`, PROTOCOL section 13) into an A4 PDF:
/// a hidden WKWebView loads it, we wait for `exam_ready` (or `dataset.ready == "1"`), then paginate it with
/// `UIPrintPageRenderer` and the web view's print formatter.
@MainActor
final class ExamExporter: NSObject, @preconcurrency WKNavigationDelegate, @preconcurrency WKScriptMessageHandler {
    enum Source { case url(URL), html(String) }
    enum ExportError: Error { case noWindow, empty }

    private var web: WKWebView?
    private var readyContinuation: CheckedContinuation<Void, Never>?
    private var isReady = false

    /// A4 in points.
    static let a4 = CGRect(x: 0, y: 0, width: 595.2, height: 841.8)
    static let margin: CGFloat = 28

    func export(_ source: Source, fileName: String) async throws -> URL {
        let config = WKWebViewConfiguration()
        config.userContentController.add(WeakExportHandler(self), name: "board")
        // Print formatters lay out against the view width: one A4 page wide.
        let web = WKWebView(frame: CGRect(x: -2000, y: 0, width: Self.a4.width, height: Self.a4.height), configuration: config)
        web.navigationDelegate = self
        web.isOpaque = true
        web.backgroundColor = .white
        self.web = web
        // Must sit in a window to lay out and paint; parked off-screen.
        guard let window = UIApplication.shared.connectedScenes.compactMap({ ($0 as? UIWindowScene)?.keyWindow }).first else {
            throw ExportError.noWindow
        }
        window.addSubview(web)
        defer { web.removeFromSuperview(); self.web = nil }

        switch source {
        case .url(let url): web.load(URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 20))
        case .html(let html): web.loadHTMLString(html, baseURL: nil)
        }
        await waitUntilReady(timeout: 25)
        // Let fonts and the last formulas paint.
        try? await Task.sleep(for: .milliseconds(400))

        let data = Self.render(web)
        guard data.count > 0 else { throw ExportError.empty }
        let safe = fileName.replacingOccurrences(of: "/", with: "-").trimmingCharacters(in: .whitespaces)
        let url = FileManager.default.temporaryDirectory.appendingPathComponent((safe.isEmpty ? "de-thi" : safe) + ".pdf")
        try data.write(to: url, options: .atomic)
        return url
    }

    private static func render(_ web: WKWebView) -> Data {
        let renderer = UIPrintPageRenderer()
        renderer.addPrintFormatter(web.viewPrintFormatter(), startingAtPageAt: 0)
        let paper = a4
        let printable = paper.insetBy(dx: margin, dy: margin)
        renderer.setValue(NSValue(cgRect: paper), forKey: "paperRect")
        renderer.setValue(NSValue(cgRect: printable), forKey: "printableRect")
        let data = NSMutableData()
        UIGraphicsBeginPDFContextToData(data, paper, nil)
        renderer.prepare(forDrawingPages: NSRange(location: 0, length: renderer.numberOfPages))
        for page in 0..<renderer.numberOfPages {
            UIGraphicsBeginPDFPage()
            renderer.drawPage(at: page, in: UIGraphicsGetPDFContextBounds())
        }
        UIGraphicsEndPDFContext()
        return data as Data
    }

    /// Waits for the page's `exam_ready` message; also polls `dataset.ready` in case the message is missed.
    private func waitUntilReady(timeout: Double) async {
        let deadline = Date().addingTimeInterval(timeout)
        let poll = Task { [weak self] in
            while !Task.isCancelled, Date() < deadline {
                try? await Task.sleep(for: .milliseconds(300))
                guard let self, let web = self.web else { return }
                let value = try? await web.evaluateJavaScript("document.documentElement.dataset.ready || ''") as? String
                if value == "1" { self.markReady(); return }
            }
            self?.markReady()
        }
        if !isReady {
            await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                if isReady { c.resume() } else { readyContinuation = c }
            }
        }
        poll.cancel()
    }

    private func markReady() {
        isReady = true
        readyContinuation?.resume()
        readyContinuation = nil
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        if (message.body as? [String: Any])?["type"] as? String == "exam_ready" { markReady() }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { markReady() }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { markReady() }
}

@MainActor
private final class WeakExportHandler: NSObject, @preconcurrency WKScriptMessageHandler {
    weak var target: (any WKScriptMessageHandler)?
    init(_ t: any WKScriptMessageHandler) { target = t }
    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        target?.userContentController(c, didReceive: m)
    }
}
