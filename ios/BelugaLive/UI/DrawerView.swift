import SwiftUI
import BelugaCore

/// Side drawer (left): new chat, progress, folders (name + subject) and loose sessions.
struct DrawerOverlay: View {
    @Bindable var controller: LiveController
    @GestureState private var dragX: CGFloat = 0

    var body: some View {
        GeometryReader { geo in
            let width = min(340, geo.size.width * 0.84)
            let open = controller.drawerOpen
            ZStack(alignment: .leading) {
                Color.black.opacity(open ? 0.32 : 0)
                    .ignoresSafeArea()
                    .allowsHitTesting(open)
                    .onTapGesture { close() }
                DrawerView(controller: controller)
                    .frame(width: width)
                    .background(Color(uiColor: .systemBackground).ignoresSafeArea())
                    .overlay(alignment: .trailing) {
                        Rectangle().fill(Ink.hairline).frame(width: Stroke.hairline).ignoresSafeArea()
                    }
                    .offset(x: open ? min(0, dragX) : -width - 8)
                    .gesture(
                        DragGesture(minimumDistance: 16)
                            .updating($dragX) { v, state, _ in
                                if abs(v.translation.width) > abs(v.translation.height) { state = v.translation.width }
                            }
                            .onEnded { v in
                                if v.translation.width < -80 || v.predictedEndTranslation.width < -200 { close() }
                            }
                    )
            }
            .animation(Motion.panel, value: open)
        }
        .allowsHitTesting(controller.drawerOpen)
    }

    private func close() { withAnimation(Motion.panel) { controller.drawerOpen = false } }
}

private struct DrawerView: View {
    @Bindable var controller: LiveController
    @Environment(\.strings) private var s
    @State private var creating = false
    @State private var renaming: SessionItem?
    @State private var draftName = ""

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Button { controller.newChat() } label: {
                        Label(s("chat.new"), systemImage: "square.and.pencil")
                    }
                    Button { controller.drawerOpen = false; controller.requestProgress(); controller.showProgress = true } label: {
                        Label(s("progress.title"), systemImage: "chart.bar")
                    }
                }
                .font(TypeScale.body.weight(.medium))

                Section {
                    if controller.library.folders.isEmpty {
                        Text(verbatim: s("folders.empty"))
                            .font(TypeScale.callout)
                            .foregroundStyle(Ink.secondary)
                    }
                    ForEach(controller.library.folders) { folder in
                        Button { controller.openFolder(folder.id) } label: { FolderRow(folder: folder) }
                    }
                } header: {
                    HStack {
                        Text(verbatim: s("folders.title"))
                        Spacer()
                        Button { creating = true } label: {
                            Image(systemName: "plus").frame(width: 44, height: 32)
                        }
                        .accessibilityLabel(s("folders.new"))
                    }
                }

                Section {
                    let loose = controller.library.sessions(in: nil)
                    if loose.isEmpty {
                        Text(verbatim: s("sessions.empty"))
                            .font(TypeScale.callout)
                            .foregroundStyle(Ink.secondary)
                    }
                    ForEach(loose) { item in
                        Button { controller.open(item) } label: { SessionRow(item: item) }
                            .contextMenu { SessionMenu(controller: controller, item: item, rename: { renaming = item; draftName = item.title }) }
                    }
                } header: {
                    Text(verbatim: s("sessions.recent"))
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle(Text(verbatim: "Beluga"))
            .navigationBarTitleDisplayMode(.inline)
        }
        .tint(Ink.primary)
        .sheet(isPresented: $creating) {
            NewFolderSheet { name, subject in controller.createFolder(name: name, subject: subject) }
                .presentationDetents([.medium])
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
        HStack(spacing: Space.m) {
            Image(systemName: Subject.symbol(folder.subject))
                .font(.system(size: 15, weight: .regular))
                .foregroundStyle(Ink.secondary)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(verbatim: folder.name)
                    .font(TypeScale.body)
                    .foregroundStyle(Ink.primary)
                    .lineLimit(1)
                Text(verbatim: Subject.name(folder.subject, lang: s.lang) + " · " + s("folders.sessionCount", folder.sessionCount))
                    .font(TypeScale.caption)
                    .foregroundStyle(Ink.secondary)
            }
            Spacer(minLength: Space.s)
            if let i = folder.ingest, i.isActive {
                ProgressView().controlSize(.small)
            }
        }
        .frame(minHeight: 44)
    }
}

struct SessionRow: View {
    let item: SessionItem
    @Environment(\.strings) private var s

    var body: some View {
        let parts = item.title.components(separatedBy: " · ")
        HStack(alignment: .firstTextBaseline, spacing: Space.m) {
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(verbatim: parts.count > 1 ? parts.dropFirst().joined(separator: " · ") : (item.title.isEmpty ? s("board.untitled") : item.title))
                    .font(TypeScale.body)
                    .foregroundStyle(Ink.primary)
                    .lineLimit(2)
                if let preview = item.preview {
                    Text(verbatim: preview)
                        .font(TypeScale.caption)
                        .foregroundStyle(Ink.secondary)
                        .lineLimit(1)
                } else if parts.count > 1 {
                    Text(verbatim: parts[0])
                        .font(TypeScale.caption)
                        .foregroundStyle(Ink.secondary)
                }
            }
            Spacer(minLength: Space.s)
            if item.mode == .live {
                Image(systemName: "waveform")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Ink.tertiary)
                    .accessibilityLabel(Text(verbatim: "Live"))
            }
            Text(verbatim: DateText.short(item.updatedAt, lang: s.lang) ?? item.updatedAt)
                .font(TypeScale.caption.monospacedDigit())
                .foregroundStyle(Ink.tertiary)
        }
        .padding(.vertical, Space.xxs)
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

/// Name + subject for a new folder.
struct NewFolderSheet: View {
    var initialName = ""
    var initialSubject = "toan"
    let done: (String, String) -> Void
    @Environment(\.strings) private var s
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var subject = "toan"

    var body: some View {
        NavigationStack {
            Form {
                TextField(s("folders.name.placeholder"), text: $name)
                Picker(s("folders.subject"), selection: $subject) {
                    ForEach(Folder.subjects, id: \.self) { id in
                        Text(verbatim: Subject.name(id, lang: s.lang)).tag(id)
                    }
                }
            }
            .navigationTitle(s("folders.new"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(s("common.cancel")) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(s("common.save")) { done(name, subject); dismiss() }
                        .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .tint(Ink.primary)
        .onAppear { name = initialName; subject = initialSubject }
    }
}
