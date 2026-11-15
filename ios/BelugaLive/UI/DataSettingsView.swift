import SwiftUI
import BelugaCore

/// Settings > "Dữ liệu": the copy on this phone and its sync (PROTOCOL section 19), and the server's backup to
/// Cloudflare R2 (section 20). Plain words, one action each; nothing here is needed day to day.
struct DataSettingsView: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    private var sync: SyncStatus { controller.sync }
    private var syncing: Bool { if case .syncing = sync.phase { return true }; return false }

    var body: some View {
        Form {
            Section {
                SyncSummaryRow(controller: controller)
                if sync.pendingOps > 0 {
                    LabeledContent(s("data.pending")) {
                        Text(verbatim: "\(sync.pendingOps)").monospacedDigit().foregroundStyle(Ink.secondary)
                    }
                }
                Button {
                    Haptics.tap()
                    controller.syncNow()
                } label: {
                    Label(s("data.syncNow"), systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(!controller.connected || syncing)
            } header: {
                Text(verbatim: s("data.section.phone"))
            } footer: {
                Text(verbatim: s("data.phone.footer"))
            }

            Section {
                BackupRows(controller: controller)
            } header: {
                Text(verbatim: s("data.section.backup"))
            } footer: {
                Text(verbatim: s(controller.library.backup?.enabled == false ? "data.backup.off.footer" : "data.backup.footer"))
            }
        }
        .animation(Motion.snappy, value: sync)
        .animation(Motion.snappy, value: controller.library.backup)
        .animation(Motion.snappy, value: controller.backupRunning)
        .navigationTitle(s("data.title"))
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { controller.requestBackup() }
    }
}

/// One line about the phone's copy: synced at, syncing n of m, offline since, or a failed round.
struct SyncSummaryRow: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        let sync = controller.sync
        HStack(spacing: Space.m) {
            Group {
                switch sync.phase {
                case .syncing: ProgressView().controlSize(.small)
                default: Image(systemName: symbol)
                }
            }
            .font(.body.weight(.medium))
            .foregroundStyle(Ink.secondary)
            .frame(width: 24)
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(verbatim: title).font(.body).foregroundStyle(Ink.primary)
                if let detail {
                    Text(verbatim: detail).font(.footnote).foregroundStyle(Ink.secondary)
                }
            }
            .contentTransition(.opacity)
        }
        .padding(.vertical, Space.xxs)
        .accessibilityElement(children: .combine)
    }

    private var symbol: String {
        if controller.linkDown { return "wifi.slash" }
        if controller.sync.phase == .failed { return "exclamationmark.triangle" }
        return controller.sync.lastSyncAt == nil ? "iphone" : "checkmark.circle"
    }

    private var title: String {
        let sync = controller.sync
        if controller.linkDown { return s("sync.offline.title") }
        switch sync.phase {
        case .syncing(let done, let total): return total > 0 ? s("data.syncing.count", done, total) : s("data.syncing")
        case .failed: return s("data.sync.failed")
        case .idle: return sync.lastSyncAt == nil ? s("data.sync.never") : s("data.synced")
        }
    }

    private var detail: String? {
        guard let at = controller.sync.lastSyncAt else { return controller.serverV3 || controller.isMock ? nil : s("data.sync.needsV3") }
        return s("data.sync.at", SyncTime.text(at, lang: s.lang))
    }
}

/// The server's R2 backup: on or off, the last one (when, how big, or its error), the next one, "Sao lưu ngay".
private struct BackupRows: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        if let b = controller.library.backup {
            if b.enabled {
                LabeledContent(s("data.backup.last")) {
                    Text(verbatim: lastText(b)).foregroundStyle(Ink.secondary).multilineTextAlignment(.trailing)
                }
                if let err = b.lastError {
                    Label {
                        Text(verbatim: err).font(.footnote).foregroundStyle(Ink.primary)
                    } icon: {
                        Image(systemName: "exclamationmark.triangle").foregroundStyle(Ink.primary)
                    }
                }
                if let next = b.nextAt.flatMap(LearningDate.parse) {
                    LabeledContent(s("data.backup.next")) {
                        Text(verbatim: SyncTime.text(next, lang: s.lang)).foregroundStyle(Ink.secondary)
                    }
                }
                Button {
                    controller.backupNow()
                } label: {
                    HStack {
                        Label(s(controller.backupRunning ? "data.backup.running" : "data.backup.now"), systemImage: "externaldrive.badge.icloud")
                            .contentTransition(.opacity)
                        Spacer()
                        if controller.backupRunning { ProgressView().controlSize(.small).transition(.opacity) }
                    }
                }
                .disabled(!controller.connected || controller.backupRunning)
                .accessibilityIdentifier("data.backupNow")
            } else {
                LabeledContent(s("data.backup.state")) {
                    Text(verbatim: s("data.backup.off")).foregroundStyle(Ink.secondary)
                }
            }
        } else if controller.connected && (controller.serverV3 || controller.isMock) {
            HStack(spacing: Space.m) {
                ProgressView().controlSize(.small)
                Text(verbatim: s("data.backup.loading")).foregroundStyle(Ink.secondary)
            }
            .frame(minHeight: Size.tap)
        } else {
            Text(verbatim: s(controller.connected ? "data.sync.needsV3" : "data.backup.offline"))
                .foregroundStyle(Ink.secondary)
                .frame(minHeight: Size.tap, alignment: .leading)
        }
    }

    private func lastText(_ b: BackupStatus) -> String {
        guard let at = b.lastAt.flatMap(LearningDate.parse) else { return s("data.backup.none") }
        var parts = [SyncTime.text(at, lang: s.lang)]
        if let bytes = b.lastBytes { parts.append(ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)) }
        return parts.joined(separator: " · ")
    }
}
