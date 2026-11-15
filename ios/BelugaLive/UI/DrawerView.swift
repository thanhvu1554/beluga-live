import SwiftUI
import BelugaCore

/// Side drawer (left), revealed under the sliding chat (`ChatShell`): new chat, progress, folders (the owner's
/// own names) and loose sessions.
struct DrawerView: View {
    @Bindable var controller: LiveController
    @Environment(\.strings) private var s
    @State private var renaming: SessionItem?
    @State private var draftName = ""

    var body: some View {
        NavigationStack {
            List {
                if controller.linkDown || controller.sync.pendingOps > 0 {
                    Section { SyncStateRow(controller: controller) }
                }
                Section {
                    Button { controller.newChat() } label: {
                        RowLabel(symbol: "square.and.pencil", title: s("chat.new"), emphasized: true)
                    }
                    Button {
                        withAnimation(Motion.smooth) { controller.drawerOpen = false }
                        controller.requestProgress()
                        controller.showProgress = true
                    } label: {
                        RowLabel(symbol: "chart.bar", title: s("progress.title"), emphasized: true)
                    }
                }

                Section {
                    if controller.library.folders.isEmpty {
                        StateBlock(title: s("folders.empty"))
                    }
                    ForEach(controller.library.folders) { folder in
                        Button { controller.openFolder(folder.id) } label: { FolderRow(folder: folder) }
                    }
                } header: {
                    HStack {
                        Text(verbatim: s("folders.title"))
                        Spacer()
                        Button { controller.showNewFolder = true } label: {
                            Image(systemName: "plus").frame(width: Size.tap, height: 32)
                        }
                        .accessibilityLabel(s("folders.new"))
                    }
                }

                Section {
                    let loose = controller.library.sessions(in: nil)
                    if loose.isEmpty {
                        StateBlock(title: s("sessions.empty"))
                    }
                    ForEach(loose) { item in
                        Button { controller.open(item) } label: { SessionRow(item: item, current: item.sessionId == controller.sessionId) }
                            .contextMenu { SessionMenu(controller: controller, item: item, rename: { renaming = item; draftName = item.title }) }
                    }
                } header: {
                    Text(verbatim: s("sessions.recent"))
                }
            }
            .listStyle(.insetGrouped)
            // No implicit animation on server data: this list sits hidden under the chat most of the time, and an
            // animated batch update of a collection view that is not on screen is a known way to die inside UIKit.
            .navigationTitle(Text(verbatim: "Beluga"))
            .navigationBarTitleDisplayMode(.inline)
        }
        .tint(Ink.primary)
        .sheet(isPresented: $controller.showNewFolder) {
            FolderEditorSheet(purpose: .create) { name, about in controller.createFolder(name: name, about: about) }
                .presentationDetents([.medium, .large])
        }
        .alert(s("sessions.rename"), isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField(s("sessions.rename"), text: $draftName)
            Button(s("common.cancel"), role: .cancel) { renaming = nil }
            Button(s("common.save")) { if let r = renaming { controller.renameSession(r, title: draftName) }; renaming = nil }
        }
    }
}

struct FolderRow: View {
    let folder: Folder
    @Environment(\.strings) private var s

    var body: some View {
        // The folder's own name and what the owner wrote about it; its teaching kind only picks the glyph.
        RowLabel(symbol: Subject.symbol(folder.kind), title: folder.name,
                 subtitle: ([s("folders.sessionCount", folder.sessionCount)] + (folder.about.isEmpty ? [] : [folder.about]))
                    .joined(separator: " · ")) {
            if let i = folder.ingest, i.isActive {
                ProgressView().controlSize(.small)
            } else if folder.pending {
                Image(systemName: "clock")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(Ink.tertiary)
                    .accessibilityLabel(Text(verbatim: s("sync.pending.item")))
            }
        }
    }
}

struct SessionRow: View {
    let item: SessionItem
    var current = false
    @Environment(\.strings) private var s

    var body: some View {
        let parts = item.title.components(separatedBy: " · ")
        let title = parts.count > 1 ? parts.dropFirst().joined(separator: " · ") : (item.title.isEmpty ? s("board.untitled") : item.title)
        let subtitle = item.preview ?? (parts.count > 1 ? parts[0] : nil)
        RowLabel(title: title, subtitle: subtitle, emphasized: current) {
            HStack(spacing: Space.s) {
                if item.mode == .live {
                    Image(systemName: "waveform")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Ink.tertiary)
                        .accessibilityLabel(Text(verbatim: "Live"))
                }
                RowMeta(text: DateText.short(item.updatedAt, lang: s.lang) ?? item.updatedAt)
            }
        }
    }
}

/// Rename, move to a folder, delete.
struct SessionMenu: View {
    let controller: LiveController
    let item: SessionItem
    let rename: () -> Void
    @Environment(\.strings) private var s

    var body: some View {
        Button { rename() } label: { Label(s("sessions.rename"), systemImage: "pencil") }
        Menu {
            Button { controller.moveSession(item, to: nil) } label: { Text(verbatim: s("folders.none")) }
            ForEach(controller.library.folders) { f in
                Button { controller.moveSession(item, to: f.id) } label: { Text(verbatim: f.name) }
            }
        } label: { Label(s("sessions.move"), systemImage: "folder") }
        Button(role: .destructive) { controller.deleteSession(item) } label: { Label(s("common.delete"), systemImage: "trash") }
    }
}

/// One quiet line at the top of the drawer while the server is out of reach or edits wait to go out: what the
/// screens show is the phone's own copy, and since when.
struct SyncStateRow: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        let pending = controller.sync.pendingOps
        HStack(alignment: .firstTextBaseline, spacing: Space.m) {
            Image(systemName: controller.linkDown ? "wifi.slash" : "arrow.triangle.2.circlepath")
                .font(.subheadline.weight(.medium))
                .foregroundStyle(Ink.secondary)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(verbatim: controller.linkDown ? s("sync.offline.title") : s("sync.sending"))
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Ink.primary)
                Text(verbatim: detail(pending: pending))
                    .font(.footnote)
                    .foregroundStyle(Ink.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, Space.xxs)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("sync.state")
    }

    private func detail(pending: Int) -> String {
        var parts: [String] = []
        if controller.linkDown {
            if let at = controller.sync.lastSyncAt {
                parts.append(s("sync.offline.since", SyncTime.text(at, lang: s.lang)))
            } else {
                parts.append(s(controller.sync.hasCopy ? "sync.offline.copy" : "sync.offline.never"))
            }
        }
        if pending > 0 { parts.append(s("sync.pending", pending)) }
        return parts.joined(separator: " · ")
    }
}

/// "21:05" today, "15/11 21:05" before.
enum SyncTime {
    static func text(_ date: Date, lang: Lang) -> String {
        let locale = Locale(identifier: lang.rawValue)
        if Calendar.current.isDateInToday(date) {
            return date.formatted(Date.FormatStyle(date: .omitted, time: .shortened).locale(locale))
        }
        return date.formatted(Date.FormatStyle().day().month(.twoDigits).hour().minute().locale(locale))
    }
}
