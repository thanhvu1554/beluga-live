import SwiftUI
import BelugaCore

struct SettingsView: View {
    let controller: LiveController
    @Bindable var settings: AppSettings
    @Environment(\.strings) private var s
    @Environment(\.dismiss) private var dismiss
    @State private var hostDraft = ""
    @State private var codeDraft = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent(s("settings.host")) {
                        TextField(s("settings.host.placeholder"), text: $hostDraft)
                            .multilineTextAlignment(.trailing)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .keyboardType(.URL)
                            .submitLabel(.done)
                            .onSubmit(applyConnection)
                    }
                    LabeledContent(s("settings.pairCode")) {
                        TextField("000000", text: $codeDraft)
                            .multilineTextAlignment(.trailing)
                            .keyboardType(.numberPad)
                            .font(TypeScale.body.monospacedDigit())
                    }
                    LabeledContent(s("settings.status")) {
                        HStack(spacing: Space.s) {
                            Circle()
                                .fill(controller.connected ? Ink.primary : Color.clear)
                                .stroke(Ink.secondary, lineWidth: Stroke.thin)
                                .frame(width: 8, height: 8)
                            Text(verbatim: s(controller.connected ? "settings.status.connected" : "settings.status.offline"))
                                .foregroundStyle(Ink.secondary)
                        }
                    }
                } header: {
                    Text(verbatim: s("settings.section.connection"))
                } footer: {
                    Text(verbatim: s("settings.connection.footer"))
                }

                Section {
                    Picker(s("settings.model"), selection: $settings.model) {
                        Text(verbatim: "Sonnet").tag(ModelChoice.sonnet)
                        Text(verbatim: "Opus").tag(ModelChoice.opus)
                        Text(verbatim: "Haiku").tag(ModelChoice.haiku)
                    }
                    VStack(alignment: .leading, spacing: Space.s) {
                        Text(verbatim: s("settings.rigor"))
                        Picker(s("settings.rigor"), selection: $settings.rigor) {
                            Text(verbatim: s("settings.rigor.light")).tag(Rigor.light)
                            Text(verbatim: s("settings.rigor.medium")).tag(Rigor.medium)
                            Text(verbatim: s("settings.rigor.hard")).tag(Rigor.hard)
                        }
                        .pickerStyle(.segmented)
                        .labelsHidden()
                    }
                    .padding(.vertical, Space.xs)
                } header: {
                    Text(verbatim: s("settings.section.tutor"))
                } footer: {
                    Text(verbatim: s("settings.rigor.footer"))
                }

                Section {
                    VStack(alignment: .leading, spacing: Space.s) {
                        Text(verbatim: s("settings.talkMode"))
                        Picker(s("settings.talkMode"), selection: $settings.handsFree) {
                            Text(verbatim: s("settings.talkMode.hold")).tag(false)
                            Text(verbatim: s("settings.talkMode.handsFree")).tag(true)
                        }
                        .pickerStyle(.segmented)
                        .labelsHidden()
                    }
                    .padding(.vertical, Space.xs)
                    Toggle(s("settings.captions"), isOn: $settings.captions)
                    Toggle(s("settings.haptics"), isOn: $settings.haptics)
                } header: {
                    Text(verbatim: s("settings.section.voice"))
                }

                Section {
                    Picker(s("settings.language"), selection: $settings.lang) {
                        Text(verbatim: "Tiếng Việt").tag(Lang.vi)
                        Text(verbatim: "English").tag(Lang.en)
                    }
                } header: {
                    Text(verbatim: s("settings.section.language"))
                }

                if controller.isMock {
                    Section {
                        Text(verbatim: s("settings.mock"))
                            .font(TypeScale.caption)
                            .foregroundStyle(Ink.secondary)
                    }
                }
            }
            .tint(Color(uiColor: .systemGray))
            .navigationTitle(s("settings.title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button { applyConnection(); dismiss() } label: {
                        Image(systemName: "checkmark")
                    }
                    .accessibilityLabel(s("common.done"))
                }
            }
        }
        .tint(Ink.primary)
        .onAppear {
            hostDraft = settings.host
            codeDraft = settings.pairCode
        }
        .onChange(of: settings.lang) { _, _ in controller.settingsChanged() }
        .onChange(of: settings.model) { _, _ in controller.settingsChanged() }
        .onChange(of: settings.rigor) { _, _ in controller.settingsChanged() }
        .onChange(of: settings.haptics) { _, _ in controller.settingsChanged() }
    }

    private func applyConnection() {
        let host = hostDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        let code = codeDraft.filter(\.isNumber)
        guard host != settings.host || code != settings.pairCode else { return }
        settings.host = host
        settings.pairCode = code
        if !controller.isMock { controller.reconnectNow() }
    }
}
