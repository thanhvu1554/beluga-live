import Foundation
import Darwin
import MetricKit
import os
import ObjectiveC

// Crash diagnostics for a phone we cannot attach a debugger to.
//
// - Breadcrumbs: short lines (time, free memory, what happened) appended to `session.log` in Application Support
//   with a raw `write(2)`, so they survive any kind of death.
// - Uncaught Objective-C exceptions (AVAudioEngine, AVCapture, UIKit) and fatal signals (Swift runtime traps such as
//   a force unwrap or an actor-isolation assertion, SIGABRT, SIGSEGV...) add a "!!!" line and a backtrace.
// - Release builds send stderr to `stderr.log`: Swift's own "Fatal error: ..." text lands there.
// - On the next launch the previous session is examined. A crash, or a session whose last state was "foreground"
//   without a crash line (killed by the system: memory or watchdog), becomes `last-crash.txt`, shown in
//   Settings > Báo lỗi with a share button.
// - MetricKit diagnostics (crash, hang) that iOS delivers on a later launch are kept in `metrickit.txt`.

/// File descriptor of `session.log`, read by the signal handler (async-signal-safe access only).
nonisolated(unsafe) private var crashFD: Int32 = -1
/// Pre-built "!!! SIGNAL <name>" lines, indexed by signal number (no allocation inside the handler).
nonisolated(unsafe) private var signalLines = [UnsafeMutablePointer<CChar>?](repeating: nil, count: 32)

final class Diagnostics: @unchecked Sendable {
    static let shared = Diagnostics()

    private let lock = NSLock()
    private let dir: URL
    private let formatter: DateFormatter
    private var metrics: MetricsSubscriber?

    private init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        dir = base.appendingPathComponent("Diagnostics", isDirectory: true)
        formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "HH:mm:ss.SSS"
    }

    private var sessionURL: URL { dir.appendingPathComponent("session.log") }
    private var stderrURL: URL { dir.appendingPathComponent("stderr.log") }
    private var previousURL: URL { dir.appendingPathComponent("previous.log") }
    private var previousStderrURL: URL { dir.appendingPathComponent("previous-stderr.log") }
    var reportURL: URL { dir.appendingPathComponent("last-crash.txt") }
    var metricsURL: URL { dir.appendingPathComponent("metrickit.txt") }

    // MARK: Start

    /// Call once, first thing at launch.
    func start(redirectStderr: Bool) {
        let fm = FileManager.default
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
        // Last run's files become "previous"; a fresh log starts.
        for (from, to) in [(sessionURL, previousURL), (stderrURL, previousStderrURL)] {
            try? fm.removeItem(at: to)
            if fm.fileExists(atPath: from.path) { try? fm.moveItem(at: from, to: to) }
        }
        examinePreviousSession()

        crashFD = open(sessionURL.path, O_WRONLY | O_CREAT | O_APPEND, 0o644)
        if redirectStderr {
            let errFD = open(stderrURL.path, O_WRONLY | O_CREAT | O_APPEND, 0o644)
            if errFD >= 0 { dup2(errFD, STDERR_FILENO); close(errFD) }
        }
        installHandlers()
        let info = Bundle.main.infoDictionary
        let version = "\(info?["CFBundleShortVersionString"] as? String ?? "?") (\(info?["CFBundleVersion"] as? String ?? "?"))"
        log("launch \(version) \(Self.machine()) \(ProcessInfo.processInfo.operatingSystemVersionString)")

        let subscriber = MetricsSubscriber(url: metricsURL)
        metrics = subscriber
        MXMetricManager.shared.add(subscriber)
    }

    // MARK: Breadcrumbs

    /// One line in the session log. Thread-safe, cheap, never throws.
    func log(_ message: String) {
        let mb = os_proc_available_memory() / 1_048_576
        lock.lock()
        defer { lock.unlock() }
        let line = "\(formatter.string(from: Date())) [\(mb)MB free] \(message)\n"
        guard crashFD >= 0 else { return }
        line.withCString { p in _ = write(crashFD, p, strlen(p)) }
    }

    /// Breadcrumb shorthand.
    static func log(_ message: String) { shared.log(message) }

    // MARK: Crash capture

    private func installHandlers() {
        NSSetUncaughtExceptionHandler { exception in
            Diagnostics.shared.record(exception: exception)
        }
        let names: [(Int32, String)] = [(SIGABRT, "SIGABRT"), (SIGSEGV, "SIGSEGV"), (SIGBUS, "SIGBUS"), (SIGILL, "SIGILL"),
                                        (SIGTRAP, "SIGTRAP (Swift runtime trap)"), (SIGFPE, "SIGFPE")]
        for (sig, name) in names {
            signalLines[Int(sig)] = strdup("!!! CRASH: signal \(name)\n")
            signal(sig) { sig in
                if crashFD >= 0, sig >= 0, sig < 32, let line = signalLines[Int(sig)] {
                    _ = write(crashFD, line, strlen(line))
                    var frames = [UnsafeMutableRawPointer?](repeating: nil, count: 64)
                    let n = backtrace(&frames, 64)
                    backtrace_symbols_fd(&frames, n, crashFD)
                }
                signal(sig, SIG_DFL)
                raise(sig)
            }
        }
    }

    private func record(exception: NSException) {
        let stack = exception.callStackSymbols.prefix(40).joined(separator: "\n")
        log("!!! CRASH: uncaught exception \(exception.name.rawValue): \(exception.reason ?? "")\n\(stack)")
        // Written as a second entry, after the raw stack is safely on disk.
        let named = FrameNamer.names(for: exception.callStackReturnAddresses.prefix(40).map { $0.uintValue })
        log("--- frames named by the Objective-C runtime ---\n" + named.joined(separator: "\n"))
    }

    // MARK: Previous session

    private func examinePreviousSession() {
        guard let text = try? String(contentsOf: previousURL, encoding: .utf8), !text.isEmpty else { return }
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let crashed = lines.contains { $0.contains("!!! CRASH") }
        // The scene phase is logged as "phase active" / "phase background": dying while active without a crash
        // line means the system killed the app (memory limit or watchdog).
        let lastPhase = lines.last { $0.contains("phase ") }
        let killedInForeground = !crashed && (lastPhase?.contains("phase active") ?? false)
        guard crashed || killedInForeground else { return }

        var report = "Beluga Live — báo lỗi\n"
        report += "Phiên: \(lines.first.map { String($0.prefix(12)) } ?? "?") → \(lines.last { !$0.isEmpty }.map { String($0.prefix(12)) } ?? "?")\n"
        if let launch = lines.first(where: { $0.contains("] launch ") }) { report += launch + "\n" }
        report += crashed
            ? "Kết luận: ứng dụng bị dừng vì lỗi (xem dòng !!! CRASH).\n"
            : "Kết luận: ứng dụng bị hệ thống tắt khi đang mở (thường do hết bộ nhớ hoặc treo quá lâu).\n"
        if let stderr = try? String(contentsOf: previousStderrURL, encoding: .utf8), !stderr.isEmpty {
            let tail = stderr.split(separator: "\n").suffix(40).joined(separator: "\n")
            report += "\n--- stderr (cuối) ---\n\(tail)\n"
        }
        // The crash block plus what led to it.
        report += "\n--- nhật ký (cuối) ---\n" + lines.suffix(200).joined(separator: "\n")
        try? report.write(to: reportURL, atomically: true, encoding: .utf8)
    }

    // MARK: Reading (Settings)

    struct Report: Sendable {
        var text: String
        var date: Date?
    }

    /// The last crash report, MetricKit diagnostics appended; nil when nothing was recorded.
    func lastReport() -> Report? {
        let fm = FileManager.default
        var parts: [String] = []
        var date: Date?
        if let t = try? String(contentsOf: reportURL, encoding: .utf8) {
            parts.append(t)
            date = (try? fm.attributesOfItem(atPath: reportURL.path))?[.modificationDate] as? Date
        }
        if let m = try? String(contentsOf: metricsURL, encoding: .utf8), !m.isEmpty {
            parts.append("--- MetricKit ---\n" + m)
            if date == nil { date = (try? fm.attributesOfItem(atPath: metricsURL.path))?[.modificationDate] as? Date }
        }
        return parts.isEmpty ? nil : Report(text: parts.joined(separator: "\n\n"), date: date)
    }

    /// Breadcrumbs of the running session (to send even without a crash).
    func currentLog() -> String {
        (try? String(contentsOf: sessionURL, encoding: .utf8)) ?? ""
    }

    func clearReports() {
        try? FileManager.default.removeItem(at: reportURL)
        try? FileManager.default.removeItem(at: metricsURL)
    }

    private static func machine() -> String {
        var u = utsname()
        uname(&u)
        return withUnsafeBytes(of: &u.machine) { raw in
            String(decoding: raw.prefix { $0 != 0 }, as: UTF8.self)
        }
    }
}

/// MetricKit hands over crash and hang diagnostics on a later launch, on a background queue: nothing here may
/// touch the main actor.
private final class MetricsSubscriber: NSObject, MXMetricManagerSubscriber, @unchecked Sendable {
    private let url: URL
    init(url: URL) { self.url = url }

    func didReceive(_ payloads: [MXDiagnosticPayload]) {
        var text = ""
        for p in payloads {
            let crashes = p.crashDiagnostics?.count ?? 0
            let hangs = p.hangDiagnostics?.count ?? 0
            guard crashes + hangs > 0 else { continue }
            text += "\(p.timeStampBegin) – \(p.timeStampEnd): \(crashes) crash, \(hangs) hang\n"
            text += String(decoding: p.jsonRepresentation(), as: UTF8.self) + "\n\n"
        }
        guard !text.isEmpty else { return }
        Diagnostics.log("metrickit: received diagnostics")
        // Keep only the newest payloads (the JSON is large).
        let old = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
        let combined = String((text + old).prefix(400_000))
        try? combined.write(to: url, atomically: true, encoding: .utf8)
    }
}

/// Names the Objective-C method each return address falls in. On a phone the system frameworks carry no symbols,
/// so a crash inside UIKit is only "UIKitCore + 6169152"; the runtime, however, still knows where every method
/// starts. For each address this finds the method with the closest entry point at or below it in the same image.
/// A large offset (several kilobytes) means the address is probably in a C function or a block after that method.
enum FrameNamer {
    /// Pointer-authentication bits sit above the address on arm64e.
    private static func strip(_ a: UInt) -> UInt { a & 0x0000_000F_FFFF_FFFF }

    static func names(for addresses: [UInt]) -> [String] {
        var out = [String](repeating: "?", count: addresses.count)
        var pending: [String: [(index: Int, address: UInt)]] = [:]
        for (i, raw) in addresses.enumerated() {
            let a = strip(raw)
            var info = Dl_info()
            guard dladdr(UnsafeRawPointer(bitPattern: a), &info) != 0, let fname = info.dli_fname else {
                out[i] = String(format: "%2d  0x%lx", i, a)
                continue
            }
            let path = String(cString: fname)
            let image = (path as NSString).lastPathComponent
            if let sname = info.dli_sname, let saddr = info.dli_saddr {
                let symbol = String(cString: sname)
                if !symbol.isEmpty, symbol != "<redacted>" {
                    out[i] = String(format: "%2d  %@  %@ + %lu", i, image, symbol, a &- UInt(bitPattern: saddr))
                    continue
                }
            }
            out[i] = String(format: "%2d  %@  0x%lx", i, image, a)
            pending[path, default: []].append((i, a))
        }
        for (path, frames) in pending {
            let entries = methodEntries(inImage: path)
            guard !entries.isEmpty else { continue }
            let image = (path as NSString).lastPathComponent
            for f in frames {
                // Greatest entry point that is not above the address.
                var lo = 0, hi = entries.count
                while lo < hi {
                    let mid = (lo + hi) / 2
                    if entries[mid].address <= f.address { lo = mid + 1 } else { hi = mid }
                }
                guard lo > 0 else { continue }
                let e = entries[lo - 1]
                out[f.index] = String(format: "%2d  %@  %@ + %lu", f.index, image, e.name, f.address &- e.address)
            }
        }
        return out
    }

    /// Every Objective-C method of the classes defined in one image, sorted by entry address.
    private static func methodEntries(inImage path: String) -> [(address: UInt, name: String)] {
        var count: UInt32 = 0
        guard let classNames = objc_copyClassNamesForImage(path, &count) else { return [] }
        defer { free(UnsafeMutableRawPointer(mutating: classNames)) }
        var entries: [(address: UInt, name: String)] = []
        entries.reserveCapacity(Int(count) * 12)
        for k in 0..<Int(count) {
            guard let cls: AnyClass = objc_lookUpClass(classNames[k]) else { continue }
            let className = String(cString: classNames[k])
            let targets: [(AnyClass?, String)] = [(cls, "-"), (object_getClass(cls), "+")]
            for (target, sign) in targets {
                guard let target else { continue }
                var n: UInt32 = 0
                guard let methods = class_copyMethodList(target, &n) else { continue }
                for m in 0..<Int(n) {
                    let imp = method_getImplementation(methods[m])
                    let address = strip(unsafeBitCast(imp, to: UInt.self))
                    let selector = NSStringFromSelector(method_getName(methods[m]))
                    entries.append((address, sign + "[" + className + " " + selector + "]"))
                }
                free(methods)
            }
        }
        entries.sort { $0.address < $1.address }
        return entries
    }
}
