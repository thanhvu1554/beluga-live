import SwiftUI
import BelugaCore

/// Settings, ordered by what the owner looks at: first a glance (is the computer there, how much of the
/// Claude limit is used), then how the tutor teaches, then voice, then the rarely touched things (the
/// computer's address and pairing code live one level down, in "Máy tính").
struct SettingsView: View {
    let controller: LiveController
    @Bindable var settings: AppSettings
    @Environment(\.strings) private var s
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack(path: Binding(get: { controller.settingsPath }, set: { controller.settingsPath = $0 })) {
            Form {
                Section {
                    StatusGlance(controller: controller)
                }

                Section {
                    NavigationLink {
                        ModelPickerList(controller: controller)
                    } label: {
                        LabeledContent(s("settings.model")) {
                            Text(verbatim: modelSummary).foregroundStyle(Ink.secondary)
                        }
                    }
                    segmented(s("settings.defaultMode"), selection: $settings.defaultMode) {
                        Text(verbatim: s("settings.mode.chat")).tag(SessionMode.chat)
                        Text(verbatim: "Live").tag(SessionMode.live)
                    }
                    segmented(s("settings.rigor"), selection: $settings.rigor) {
                        Text(verbatim: s("settings.rigor.light")).tag(Rigor.light)
                        Text(verbatim: s("settings.rigor.medium")).tag(Rigor.medium)
                        Text(verbatim: s("settings.rigor.hard")).tag(Rigor.hard)
                    }
                } header: {
                    Text(verbatim: s("settings.section.tutor"))
                } footer: {
                    Text(verbatim: s("settings.rigor.footer"))
                }

                Section {
                    NavigationLink(value: SettingsRoute.voices) {
                        LabeledContent(s("voices.row")) {
                            Text(verbatim: voiceSummary).foregroundStyle(Ink.secondary)
                        }
                    }
                    segmented(s("settings.talkMode"), selection: $settings.handsFree) {
                        Text(verbatim: s("settings.talkMode.hold")).tag(false)
                        Text(verbatim: s("settings.talkMode.handsFree")).tag(true)
                    }
                    Toggle(s("settings.sttFix"), isOn: $settings.sttFix)
                    Toggle(s("settings.captions"), isOn: $settings.captions)
                    Toggle(s("settings.haptics"), isOn: $settings.haptics)
                } header: {
                    Text(verbatim: s("settings.section.voice"))
                }

                Section {
                    NavigationLink {
                        LearningProgressView(controller: controller)
                            .onAppear { controller.requestProgress() }
                    } label: {
                        Label(s("progress.title"), systemImage: "chart.bar")
                    }
                    Picker(selection: $settings.lang) {
                        Text(verbatim: "Tiếng Việt").tag(Lang.vi)
                        Text(verbatim: "English").tag(Lang.en)
                    } label: {
                        Label(s("settings.language"), systemImage: "globe")
                    }
                }

                Section {
                    NavigationLink(value: SettingsRoute.connection) {
                        Label {
                            LabeledContent(s("settings.host")) {
                                Text(verbatim: settings.host.isEmpty ? s("settings.host.none") : settings.host)
                                    .foregroundStyle(Ink.secondary)
                                    .lineLimit(1)
                            }
                        } icon: { Image(systemName: "desktopcomputer") }
                    }
                    NavigationLink(value: SettingsRoute.data) {
                        Label {
                            LabeledContent(s("data.title")) {
                                Text(verbatim: dataSummary).foregroundStyle(Ink.secondary).lineLimit(1)
                            }
                        } icon: { Image(systemName: "externaldrive") }
                    }
                    NavigationLink(value: SettingsRoute.diagnostics) {
                        Label(s("diag.title"), systemImage: "stethoscope")
                    }
                } header: {
                    Text(verbatim: s("settings.section.connection"))
                } footer: {
                    if controller.isMock { Text(verbatim: s("settings.mock")) }
                }
            }
            .tint(Color(uiColor: .systemGray))
            .navigationDestination(for: SettingsRoute.self) { route in
                switch route {
                case .voices: VoiceSettingsView(controller: controller)
                case .diagnostics: DiagnosticsView()
                case .connection: ConnectionView(controller: controller, settings: settings)
                case .usage: UsageView(controller: controller)
                case .data: DataSettingsView(controller: controller)
                }
            }
            .navigationTitle(s("settings.title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button { dismiss() } label: {
                        Image(systemName: "checkmark")
                    }
                    .accessibilityLabel(s("common.done"))
                }
            }
        }
        .tint(Ink.primary)
        .onAppear { controller.requestUsage() }
        .onChange(of: settings.lang) { _, _ in controller.settingsChanged() }
        .onChange(of: settings.sttFix) { _, _ in controller.settingsChanged() }
        .onChange(of: settings.rigor) { _, _ in controller.settingsChanged() }
        .onChange(of: settings.haptics) { _, _ in controller.settingsChanged() }
    }

    /// A titled segmented control row (same layout for every choice of two or three).
    private func segmented<V: Hashable, C: View>(_ title: String, selection: Binding<V>, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: Space.s) {
            Text(verbatim: title)
            Picker(title, selection: selection) { content() }
                .pickerStyle(.segmented)
                .labelsHidden()
        }
        .padding(.vertical, Space.xs)
    }

    private var modelSummary: String {
        let name = controller.currentModel?.label ?? settings.model
        return settings.effort.isEmpty ? name : name + " · " + s("effort.\(settings.effort)")
    }

    /// "Dữ liệu" row: the one thing worth a glance (offline, edits waiting, syncing, or when it last synced).
    private var dataSummary: String {
        let sync = controller.sync
        if controller.linkDown { return s("sync.offline.title") }
        if sync.pendingOps > 0 { return s("sync.pending", sync.pendingOps) }
        if case .syncing = sync.phase { return s("data.syncing") }
        if let at = sync.lastSyncAt { return SyncTime.text(at, lang: s.lang) }
        return ""
    }

    private var voiceSummary: String {
        guard let catalog = controller.library.voices else { return "" }
        if catalog.isDevice { return s("voices.deviceName") }
        return controller.library.voice(catalog.current)?.name ?? ""
    }
}

/// The top of Settings: connection state in words, and the Claude limit as one bar. Tapping the bar opens the
/// full usage page.
private struct StatusGlance: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            HStack(spacing: Space.s) {
                Circle()
                    .fill(controller.connected ? Ink.primary : Color.clear)
                    .stroke(Ink.secondary, lineWidth: Stroke.thin)
                    .frame(width: 9, height: 9)
                Text(verbatim: s(controller.connected ? "settings.status.connected" : "settings.status.offline"))
                    .font(TypeScale.headline)
                Spacer(minLength: Space.s)
                if !controller.settings.host.isEmpty {
                    Text(verbatim: controller.settings.host)
                        .font(TypeScale.caption)
                        .foregroundStyle(Ink.secondary)
                        .lineLimit(1)
                }
            }
            .animation(Motion.snappy, value: controller.connected)

            NavigationLink(value: SettingsRoute.usage) {
                LimitMeter(controller: controller, compact: true)
            }
        }
        .padding(.vertical, Space.xs)
    }
}

/// Used share of the Claude plan limit: a line of words over one bar, and when it refills.
struct LimitMeter: View {
    let controller: LiveController
    var compact = false
    @Environment(\.strings) private var s

    var body: some View {
        let limit = controller.library.usage?.limit
        let used = limit?.utilization.map { min(1, max(0, $0)) }
        VStack(alignment: .leading, spacing: Space.s) {
            HStack(alignment: .firstTextBaseline) {
                Text(verbatim: s("usage.limit"))
                    .font(compact ? TypeScale.callout : TypeScale.headline)
                    .foregroundStyle(Ink.primary)
                Spacer(minLength: Space.s)
                if let used {
                    Text(verbatim: "\(Int((used * 100).rounded())) %")
                        .font((compact ? TypeScale.callout : TypeScale.display).weight(.semibold).monospacedDigit())
                        .foregroundStyle(Ink.primary)
                        .contentTransition(.numericText())
                } else if let limit {
                    Text(verbatim: s("usage.limit.\(UsageView.limitKey(limit.status))"))
                        .font(TypeScale.callout).foregroundStyle(Ink.secondary)
                }
            }
            ProgressView(value: used ?? 0)
                .tint(Ink.primary)
                .opacity(used == nil ? 0.35 : 1)
            Text(verbatim: detail(limit))
                .font(TypeScale.caption)
                .foregroundStyle(Ink.secondary)
        }
        .animation(Motion.smooth, value: used)
    }

    private func detail(_ limit: Usage.Limit?) -> String {
        guard let limit else { return s(controller.connected ? "progress.loading" : "usage.offline") }
        var parts: [String] = []
        if let type = limit.type { parts.append(s("usage.window", UsageView.windowName(type, s))) }
        if let r = limit.resetsAt, let d = LearningDate.parse(r) {
            parts.append(s("usage.resetsAt", d.formatted(Date.FormatStyle(date: .omitted, time: .shortened)
                .weekday(.abbreviated).locale(Locale(identifier: s.lang == .vi ? "vi_VN" : "en_US")))))
        }
        if limit.status == "allowed_warning" || limit.status == "rejected" {
            parts.insert(s("usage.limit.\(UsageView.limitKey(limit.status))"), at: 0)
        }
        return parts.joined(separator: ". ")
    }
}

/// "Máy tính": address and pairing code of the computer running the tutor. Applied when leaving the page or
/// pressing return; opened directly on first run and after a wrong pairing code.
struct ConnectionView: View {
    let controller: LiveController
    @Bindable var settings: AppSettings
    @Environment(\.strings) private var s
    @State private var hostDraft = ""
    @State private var codeDraft = ""
    @FocusState private var focus: Field?

    enum Field { case host, code }

    var body: some View {
        Form {
            Section {
                LabeledContent(s("settings.host")) {
                    TextField(s("settings.host.placeholder"), text: $hostDraft)
                        .multilineTextAlignment(.trailing)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .submitLabel(.next)
                        .focused($focus, equals: .host)
                        .onSubmit { focus = .code }
                }
                LabeledContent(s("settings.pairCode")) {
                    TextField("000000", text: $codeDraft)
                        .multilineTextAlignment(.trailing)
                        .keyboardType(.numberPad)
                        .font(TypeScale.body.monospacedDigit())
                        .focused($focus, equals: .code)
                }
                LabeledContent(s("settings.status")) {
                    Text(verbatim: s(controller.connected ? "settings.status.connected" : "settings.status.offline"))
                        .foregroundStyle(Ink.secondary)
                        .contentTransition(.opacity)
                }
            } footer: {
                Text(verbatim: s("settings.connection.footer"))
            }
            Section {
                Button { apply(); focus = nil } label: {
                    Text(verbatim: s("settings.connect"))
                        .font(TypeScale.body.weight(.semibold))
                        .frame(maxWidth: .infinity)
                }
                .disabled(hostDraft.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .animation(Motion.snappy, value: controller.connected)
        .navigationTitle(s("settings.host"))
        .navigationBarTitleDisplayMode(.inline)
        .onAppear {
            hostDraft = settings.host
            codeDraft = settings.pairCode
            if settings.host.isEmpty { focus = .host } else if controller.connectionStopped { focus = .code }
        }
        .onDisappear(perform: apply)
    }

    private func apply() {
        let host = hostDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        let code = codeDraft.filter(\.isNumber)
        // A stopped connection (wrong pairing code) retries even when the same code is entered again.
        guard host != settings.host || code != settings.pairCode || controller.connectionStopped else { return }
        settings.host = host
        settings.pairCode = code
        if !controller.isMock { controller.reconnectNow() }
    }
}
