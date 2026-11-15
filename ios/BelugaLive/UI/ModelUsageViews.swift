import SwiftUI
import QuickLook
import BelugaCore

/// Model and effort for the next turns (PROTOCOL section 10). The list comes from `ready.models`.
struct ModelPickerView: View {
    let controller: LiveController
    @Environment(\.strings) private var s
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ModelPickerList(controller: controller)
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button { dismiss() } label: { Image(systemName: "checkmark") }
                            .accessibilityLabel(s("common.done"))
                    }
                }
        }
        .tint(Ink.primary)
    }
}

struct ModelPickerList: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
            List {
                Section {
                    ForEach(controller.library.models) { m in
                        Button { controller.selectModel(m.id) } label: {
                            HStack(spacing: Space.m) {
                                VStack(alignment: .leading, spacing: Space.xxs) {
                                    Text(verbatim: m.label).font(TypeScale.body).foregroundStyle(Ink.primary)
                                    Text(verbatim: s(m.provider == "claude" ? "models.provider.claude" : "models.provider.api")
                                         + (m.vision ? "" : " · " + s("models.noVision")))
                                        .font(TypeScale.caption).foregroundStyle(Ink.secondary)
                                }
                                Spacer()
                                if m.id == controller.settings.model {
                                    Image(systemName: "checkmark").font(.system(size: 15, weight: .semibold)).foregroundStyle(Ink.primary)
                                }
                            }
                            .frame(minHeight: 44)
                        }
                        .accessibilityAddTraits(m.id == controller.settings.model ? .isSelected : [])
                    }
                }

                if let m = controller.currentModel, !m.efforts.isEmpty {
                    Section {
                        Picker(s("models.effort"), selection: Binding(get: { controller.settings.effort },
                                                                     set: { controller.selectEffort($0) })) {
                            Text(verbatim: s("effort.default") + (m.defaultEffort.map { " (" + s("effort.\($0)") + ")" } ?? "")).tag("")
                            ForEach(m.efforts, id: \.self) { e in Text(verbatim: s("effort.\(e)")).tag(e) }
                        }
                        .pickerStyle(.inline)
                        .labelsHidden()
                    } header: { Text(verbatim: s("models.effort")) } footer: { Text(verbatim: s("models.effort.footer")) }
                }

                Section {
                    NavigationLink {
                        UsageView(controller: controller)
                    } label: {
                        Label(s("usage.title"), systemImage: "chart.bar.xaxis")
                    }
                }
            }
            .navigationTitle(s("models.title"))
            .navigationBarTitleDisplayMode(.inline)
    }
}

/// Tokens and reference cost for the Claude subscription, other models and exam reading.
struct UsageView: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        List {
            if let u = controller.library.usage {
                if let limit = u.limit {
                    Section {
                        LabeledContent(s("usage.limit"), value: s("usage.limit.\(Self.limitKey(limit.status))"))
                        if let used = limit.utilization {
                            LabeledContent(s("usage.used"), value: "\(Int((used * 100).rounded())) %")
                            ProgressView(value: min(1, max(0, used)))
                        }
                        if let r = limit.resetsAt, let d = LearningDate.parse(r) {
                            LabeledContent(s("usage.resets"), value: d.formatted(Date.FormatStyle(date: .omitted, time: .shortened)
                                .weekday(.abbreviated).locale(Locale(identifier: s.lang == .vi ? "vi_VN" : "en_US"))))
                        }
                    } footer: {
                        if let type = limit.type { Text(verbatim: s("usage.window", Self.windowName(type, s))) }
                    }
                }
                bucket(s("usage.session"), u.session)
                bucket(s("usage.today"), u.today)
                bucket(s("usage.week"), u.week)
                Section {
                    LabeledContent(s("usage.other"), value: Self.money(u.otherToday.costUsd))
                    LabeledContent(s("usage.ingestToday"), value: Self.money(u.ingestToday))
                    LabeledContent(s("usage.ingestTotal"), value: Self.money(u.ingestTotal))
                    LabeledContent(s("usage.examToday"), value: Self.money(u.examToday))
                    LabeledContent(s("usage.examTotal"), value: Self.money(u.examTotal))
                } header: { Text(verbatim: s("usage.paid")) } footer: { Text(verbatim: s("usage.paid.footer")) }
            } else {
                Text(verbatim: s(controller.connected ? "progress.loading" : "progress.offline"))
                    .foregroundStyle(Ink.secondary)
            }
        }
        .navigationTitle(s("usage.title"))
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { controller.requestUsage() }
        .refreshable { controller.requestUsage() }
    }

    private func bucket(_ title: String, _ b: UsageBucket) -> some View {
        Section {
            LabeledContent(s("usage.tokens"), value: Self.tokens(b.totalTokens))
            LabeledContent(s("usage.inout"), value: Self.tokens(b.inputTokens) + " / " + Self.tokens(b.outputTokens))
            LabeledContent(s("usage.cache"), value: Self.tokens(b.cacheReadTokens) + " / " + Self.tokens(b.cacheWriteTokens))
            LabeledContent(s("usage.turns"), value: "\(b.turns)")
            LabeledContent(s("usage.cost"), value: "≈ " + Self.money(b.costUsd))
        } header: { Text(verbatim: title) } footer: {
            if title == s("usage.session") { Text(verbatim: s("usage.cost.footer")) }
        }
    }

    static func windowName(_ type: String, _ s: Strings) -> String {
        switch type {
        case "five_hour": return s("usage.window.fiveHour")
        case "seven_day", "weekly": return s("usage.window.week")
        default: return type.replacingOccurrences(of: "_", with: " ")
        }
    }

    static func limitKey(_ status: String) -> String {
        switch status {
        case "allowed": return "ok"
        case "allowed_warning": return "warning"
        case "rejected": return "rejected"
        default: return "unknown"
        }
    }

    static func tokens(_ n: Int) -> String {
        if n >= 1_000_000 { return String(format: "%.1fM", Double(n) / 1_000_000) }
        if n >= 10_000 { return "\(n / 1000)k" }
        if n >= 1000 { return String(format: "%.1fk", Double(n) / 1000) }
        return "\(n)"
    }

    static func money(_ usd: Double) -> String { String(format: "$%.2f", usd) }
}

/// An old lesson on the board, read only: swipe the steps; no camera, no voice.
struct ReplayView: View {
    let controller: LiveController
    @Environment(\.strings) private var s
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: Space.m) {
                Text(verbatim: controller.title.isEmpty ? s("board.untitled") : controller.title)
                    .font(TypeScale.headline)
                    .lineLimit(1)
                Spacer()
                if controller.board.stepCount > 0 {
                    Text(verbatim: s("board.step", controller.board.currentStep + 1, controller.board.stepCount))
                        .font(TypeScale.caption.monospacedDigit())
                        .foregroundStyle(Ink.secondary)
                }
                Button { dismiss() } label: {
                    Image(systemName: "xmark").font(.system(size: 15, weight: .semibold)).frame(width: 44, height: 44)
                }
                .accessibilityLabel(s("common.done"))
            }
            .padding(.leading, Space.xl)
            .padding(.trailing, Space.s)
            .padding(.top, Space.m)
            BoardWebView(bridge: controller.board)
        }
        .background(Color(uiColor: .systemBackground))
        .onAppear { controller.board.setInsets(top: 0, bottom: 32) }
        .onDisappear { controller.board.setInsets(top: 0, bottom: 128) }
    }
}

/// QuickLook preview with the system share button (exported exam PDF, opened attachments).
struct QuickLookView: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context: Context) -> UINavigationController {
        let ql = QLPreviewController()
        ql.dataSource = context.coordinator
        return UINavigationController(rootViewController: ql)
    }

    func updateUIViewController(_ uiViewController: UINavigationController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(url: url) }

    @MainActor
    final class Coordinator: NSObject, @preconcurrency QLPreviewControllerDataSource {
        let url: URL
        init(url: URL) { self.url = url }
        func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }
        func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> any QLPreviewItem { url as NSURL }
    }
}
