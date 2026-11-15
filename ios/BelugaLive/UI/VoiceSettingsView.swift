import SwiftUI
import BelugaCore

/// Screens pushed inside the Settings sheet.
enum SettingsRoute: Hashable { case voices, diagnostics, connection, usage, data }

/// "Giọng nói": the tutor voices the server offers, a preview clip per voice, a tick on the one in use.
struct VoiceSettingsView: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        List {
            content
        }
        .navigationTitle(s("voices.title"))
        .navigationBarTitleDisplayMode(.inline)
        .overlay(alignment: .bottom) {
            if controller.isMock, UserDefaults.standard.bool(forKey: "mockAudioProbe") {
                // Test hook only: the decoder probe, readable by the UI test, invisible to the eye.
                Text(verbatim: VoicePreviewPlayer.probe())
                    .font(.caption2)
                    .opacity(0.02)
                    .accessibilityIdentifier("audio.probe")
                    .allowsHitTesting(false)
            }
        }
        .onAppear { controller.requestVoices() }
        .onDisappear { controller.voicePreview.stop() }
    }

    @ViewBuilder private var content: some View {
        if let catalog = controller.library.voices {
            if catalog.isDevice {
                Section { StateText(title: s("voices.device.title"), text: s("voices.device.body")) }
            } else if catalog.items.isEmpty {
                Section { StateText(title: s("voices.empty.title"), text: s("voices.empty.body")) }
            } else {
                Section {
                    ForEach(catalog.items) { voice in
                        VoiceRow(controller: controller, voice: voice, isCurrent: voice.id == catalog.current)
                    }
                } header: {
                    Text(verbatim: s("voices.header", Self.providerName(catalog.provider)))
                } footer: {
                    Text(verbatim: s(controller.connected ? "voices.footer" : "voices.offline.footer"))
                }
            }
        } else if controller.connected {
            Section {
                HStack(spacing: Space.m) {
                    ProgressView()
                    Text(verbatim: s("voices.loading")).font(TypeScale.callout).foregroundStyle(Ink.secondary)
                }
                .frame(minHeight: 44)
            }
        } else {
            Section { StateText(title: s("voices.offline.title"), text: s("voices.offline.body")) }
        }
    }

    static func providerName(_ id: String) -> String {
        switch id {
        case "genvoice": return "GenVoice"
        case "elevenlabs": return "ElevenLabs"
        default: return id
        }
    }
}

private struct StateText: View {
    let title: String
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            Text(verbatim: title).font(TypeScale.headline).foregroundStyle(Ink.primary)
            Text(verbatim: text).font(TypeScale.callout).foregroundStyle(Ink.secondary)
        }
        .padding(.vertical, Space.xs)
    }
}

/// Name, kind and one line; tap to choose. The round button on the right plays or stops the preview clip.
private struct VoiceRow: View {
    let controller: LiveController
    let voice: VoiceInfo
    let isCurrent: Bool
    @Environment(\.strings) private var s

    private var player: VoicePreviewPlayer { controller.voicePreview }
    private var playing: Bool { player.playingId == voice.id }
    private var loading: Bool { player.loadingId == voice.id }

    var body: some View {
        HStack(spacing: Space.s) {
            Button { controller.selectVoice(voice.id) } label: {
                HStack(spacing: Space.m) {
                    VStack(alignment: .leading, spacing: Space.xxs) {
                        HStack(alignment: .firstTextBaseline, spacing: Space.s) {
                            Text(verbatim: voice.name)
                                .font(TypeScale.body.weight(isCurrent ? .semibold : .regular))
                                .foregroundStyle(Ink.primary)
                            if !meta.isEmpty {
                                Text(verbatim: meta).font(TypeScale.caption).foregroundStyle(Ink.tertiary)
                            }
                        }
                        if !voice.desc.isEmpty {
                            Text(verbatim: voice.desc)
                                .font(TypeScale.caption)
                                .foregroundStyle(Ink.secondary)
                                .lineLimit(2)
                        }
                        if player.failedId == voice.id {
                            Text(verbatim: s("voices.preview.failed"))
                                .font(TypeScale.caption.weight(.medium))
                                .foregroundStyle(Ink.primary)
                        }
                    }
                    Spacer(minLength: Space.s)
                    if isCurrent {
                        Image(systemName: "checkmark")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(Ink.primary)
                    }
                }
                .frame(minHeight: 52)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled(!controller.connected)
            .accessibilityAddTraits(isCurrent ? .isSelected : [])
            .accessibilityHint(Text(verbatim: s("voices.choose.hint")))

            if voice.preview {
                previewButton
            } else {
                Color.clear.frame(width: 44, height: 44).accessibilityHidden(true)
            }
        }
    }

    private var meta: String {
        var parts: [String] = []
        if voice.gender != .unknown { parts.append(s("voices.gender.\(voice.gender.rawValue)")) }
        if voice.accent != .unknown { parts.append(s("voices.accent.\(voice.accent.rawValue)")) }
        return parts.joined(separator: " · ")
    }

    private var previewButton: some View {
        Button { player.toggle(voice.id, controller: controller) } label: {
            ZStack {
                Circle().fill(playing ? Ink.primary : Color.clear)
                Circle().stroke(playing ? Color.clear : Ink.faint, lineWidth: Stroke.thin)
                if loading {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: playing ? "stop.fill" : "play.fill")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(playing ? Color(uiColor: .systemBackground) : Ink.primary)
                        .contentTransition(.symbolEffect(.replace))
                }
            }
            .frame(width: 34, height: 34)
            .frame(width: 44, height: 44)
            .contentShape(.circle)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(verbatim: s(playing || loading ? "voices.preview.stop" : "voices.preview.play", voice.name)))
    }
}

/// Settings > Báo lỗi: the last crash (or system kill) with what led to it, ready to send.
struct DiagnosticsView: View {
    @Environment(\.strings) private var s
    @State private var report: Diagnostics.Report?
    @State private var loaded = false

    var body: some View {
        List {
            Section {
                if let report {
                    VStack(alignment: .leading, spacing: Space.xs) {
                        Text(verbatim: s("diag.last")).font(TypeScale.headline).foregroundStyle(Ink.primary)
                        if let date = report.date {
                            Text(verbatim: date.formatted(Date.FormatStyle(date: .abbreviated, time: .shortened)
                                .locale(Locale(identifier: s.lang.rawValue))))
                                .font(TypeScale.caption).foregroundStyle(Ink.secondary)
                        }
                    }
                    .padding(.vertical, Space.xs)
                    ShareLink(item: report.text) {
                        Label(s("diag.share"), systemImage: "square.and.arrow.up")
                    }
                    Button(role: .destructive) {
                        Diagnostics.shared.clearReports()
                        self.report = nil
                    } label: {
                        Label(s("diag.clear"), systemImage: "trash")
                    }
                } else if loaded {
                    Text(verbatim: s("diag.none")).font(TypeScale.callout).foregroundStyle(Ink.secondary)
                }
            } footer: {
                Text(verbatim: s("diag.footer"))
            }

            if let report {
                Section(s("diag.content")) {
                    Text(verbatim: String(report.text.prefix(30_000)))
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(Ink.secondary)
                        .textSelection(.enabled)
                }
            }

            Section {
                ShareLink(item: Diagnostics.shared.currentLog()) {
                    Label(s("diag.shareSession"), systemImage: "doc.text")
                }
            } footer: {
                Text(verbatim: s("diag.session.footer"))
            }
        }
        .navigationTitle(s("diag.title"))
        .navigationBarTitleDisplayMode(.inline)
        .onAppear {
            report = Diagnostics.shared.lastReport()
            loaded = true
        }
    }
}
