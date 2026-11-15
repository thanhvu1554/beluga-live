import SwiftUI
import BelugaCore

/// Name and one optional line about a folder (PROTOCOL section 17), to create one or to edit it.
/// The name is the owner's own words and the only text ever shown for the folder; the server reads name and
/// "about" to pick how to teach (its `kind` only chooses the icon). No subject picker: a folder is whatever its
/// name says ("Tiếng Anh trên lớp", "Ôn thi vào 10 chuyên Toán").
struct FolderEditorSheet: View {
    enum Purpose { case create, edit }

    let purpose: Purpose
    var initialName = ""
    var initialAbout = ""
    /// Called with the trimmed name and about when the owner saves.
    let done: (String, String) -> Void

    @Environment(\.strings) private var s
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var about = ""
    @State private var loaded = false
    @FocusState private var focus: Field?

    private enum Field: Hashable { case name, about }

    static let nameLimit = 60

    private var trimmedName: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var trimmedAbout: String { about.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var dirty: Bool { trimmedName != initialName || trimmedAbout != initialAbout }
    private var canSave: Bool { !trimmedName.isEmpty && (purpose == .create || dirty) }
    /// The counter shows up only when the limit is near, so a short line never carries a number under it.
    private var showCount: Bool { about.count >= Folder.aboutLimit - 60 }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(s("folders.name.example"), text: $name)
                        .font(.body)
                        .focused($focus, equals: .name)
                        .submitLabel(.next)
                        .onSubmit { focus = .about }
                        .textInputAutocapitalization(.sentences)
                        .accessibilityIdentifier("folder.name")
                        .onChange(of: name) { _, new in
                            if new.count > Self.nameLimit { name = String(new.prefix(Self.nameLimit)) }
                        }
                } header: {
                    Text(verbatim: s("folders.name"))
                }

                Section {
                    TextField(s("folders.about.placeholder"), text: $about, axis: .vertical)
                        .font(.body)
                        .lineLimit(3...8)
                        .focused($focus, equals: .about)
                        .textInputAutocapitalization(.sentences)
                        .accessibilityIdentifier("folder.about")
                        .onChange(of: about) { _, new in
                            // A return key would end the "one line" the server expects: it saves instead.
                            if new.contains("\n") {
                                about = new.replacingOccurrences(of: "\n", with: " ")
                                if canSave { save() }
                                return
                            }
                            if new.count > Folder.aboutLimit { about = String(new.prefix(Folder.aboutLimit)) }
                        }
                } header: {
                    Text(verbatim: s("folders.about"))
                } footer: {
                    HStack(alignment: .firstTextBaseline, spacing: Space.s) {
                        Text(verbatim: s("folders.about.footer"))
                        Spacer(minLength: 0)
                        Text(verbatim: "\(about.count)/\(Folder.aboutLimit)")
                            .font(.footnote.monospacedDigit())
                            .foregroundStyle(about.count >= Folder.aboutLimit ? Ink.primary : Ink.tertiary)
                            .opacity(showCount ? 1 : 0)
                            .accessibilityHidden(!showCount)
                    }
                    .animation(Motion.snappy, value: showCount)
                }
            }
            .navigationTitle(s(purpose == .create ? "folders.new" : "folders.edit"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(s("common.cancel")) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(s(purpose == .create ? "folders.create" : "common.save")) { save() }
                        .fontWeight(.semibold)
                        .disabled(!canSave)
                        .accessibilityIdentifier("folder.save")
                }
            }
        }
        .tint(Ink.primary)
        // Typed words are not thrown away by a stray swipe; Cancel still closes.
        .interactiveDismissDisabled(dirty)
        .onAppear {
            guard !loaded else { return }
            loaded = true
            name = initialName
            about = initialAbout
            // Straight to the keyboard: the sheet and the keyboard rise together.
            focus = purpose == .create ? .name : nil
        }
        .task {
            // A sheet can swallow a focus set in its first layout pass; ask again once it is on screen.
            try? await Task.sleep(for: .milliseconds(120))
            if purpose == .create, focus == nil { focus = .name }
        }
    }

    private func save() {
        guard canSave else { return }
        focus = nil
        Haptics.success()
        done(trimmedName, trimmedAbout)
        dismiss()
    }
}
