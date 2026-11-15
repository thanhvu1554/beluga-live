import SwiftUI
import PhotosUI
import UniformTypeIdentifiers
import BelugaCore

/// "+  [Nhắn cho Beluga…]  mic  Live": multi-line text, attachments (photos, camera, PDF), dictation,
/// model choice in the "+" menu, and the switch to Live. Send replaces Live once there is something to send.
struct ComposerView: View {
    let controller: LiveController
    @Bindable var composer: ChatComposer
    @Environment(\.strings) private var s
    @Environment(\.colorScheme) private var scheme
    @FocusState private var focused: Bool
    @State private var showPhotos = false
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var showFiles = false
    @State private var showCamera = false

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            if !composer.attachments.isEmpty {
                AttachmentStrip(composer: composer) { id in controller.retryUpload(id, in: composer) }
            }
            HStack(alignment: .bottom, spacing: Space.xs) {
                plusMenu
                field
                MicButton(controller: controller, composer: composer)
                if composer.hasContent {
                    sendButton
                } else {
                    liveButton
                }
            }
        }
        .padding(Space.xs + Space.xxs)
        .glassEffect(.regular, in: .rect(cornerRadius: 28))
        .animation(.easeOut(duration: Motion.quick), value: composer.hasContent)
        .animation(.easeOut(duration: Motion.quick), value: composer.attachments.count)
        .photosPicker(isPresented: $showPhotos, selection: $photoItems,
                      maxSelectionCount: max(1, ChatComposer.maxAttachments - composer.attachments.count),
                      matching: .images)
        .onChange(of: photoItems) { _, items in loadPhotos(items) }
        .fileImporter(isPresented: $showFiles, allowedContentTypes: [.pdf, .image], allowsMultipleSelection: true) { result in
            switch result {
            case .success(let urls): controller.addFiles(urls, to: composer)
            case .failure(let error): Diagnostics.log("attach: file importer failed: \(error.localizedDescription)")
            }
        }
        .fullScreenCover(isPresented: $showCamera) {
            CameraPicker { image in controller.addImage(image, name: "chup-\(Int(Date().timeIntervalSince1970)).jpg") }
                .ignoresSafeArea()
        }
        .onChange(of: composer.focusTick) { _, _ in focused = true }
    }

    private var field: some View {
        ZStack(alignment: .leading) {
            TextField(s("composer.placeholder"), text: $composer.text, axis: .vertical)
                .font(TypeScale.body)
                .lineLimit(1...6)
                .focused($focused)
                .opacity(composer.dictating || composer.transcribing ? 0 : 1)
                .accessibilityLabel(s("composer.placeholder"))
            if composer.dictating || composer.transcribing {
                HStack(spacing: Space.s) {
                    if composer.transcribing { ProgressView().controlSize(.small) }
                    Text(verbatim: s(composer.dictating ? "composer.listening" : "composer.transcribing"))
                        .font(TypeScale.body)
                        .foregroundStyle(Ink.secondary)
                        .lineLimit(1)
                }
            }
        }
        .padding(.vertical, 11)
        .padding(.horizontal, Space.xs)
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
    }

    private var plusMenu: some View {
        Menu {
            Section {
                Button { showPhotos = true } label: { Label(s("attach.photos"), systemImage: "photo.on.rectangle") }
                if UIImagePickerController.isSourceTypeAvailable(.camera) {
                    Button { showCamera = true } label: { Label(s("attach.camera"), systemImage: "camera") }
                }
                Button { showFiles = true } label: { Label(s("attach.files"), systemImage: "doc") }
            }
            Section {
                Button { controller.showModels = true } label: {
                    Label(modelLabel, systemImage: "slider.horizontal.3")
                }
            }
        } label: {
            Image(systemName: "plus")
                .font(.system(size: Size.icon, weight: .medium))
                .frame(width: 44, height: 44)
                .contentShape(.circle)
        }
        .foregroundStyle(Ink.primary)
        .accessibilityLabel(s("attach.menu"))
    }

    private var modelLabel: String {
        let name = controller.currentModel?.label ?? controller.settings.model
        let effort = controller.settings.effort
        return effort.isEmpty ? name : name + " · " + s("effort.\(effort)")
    }

    private var sendButton: some View {
        Button { controller.sendChat() } label: {
            Image(systemName: "arrow.up")
                .font(.system(size: Size.icon, weight: .semibold))
                .foregroundStyle(inverse)
                .frame(width: 30, height: 30)
        }
        .buttonStyle(.glassProminent)
        .buttonBorderShape(.circle)
        .tint(ink)
        .frame(minWidth: 44, minHeight: 44)
        .disabled(!composer.canSend)
        .accessibilityLabel(s("composer.send"))
        .transition(.scale.combined(with: .opacity))
    }

    private var liveButton: some View {
        Button { controller.setMode(.live) } label: {
            HStack(spacing: Space.xs) {
                Image(systemName: "waveform")
                    .font(.system(size: 15, weight: .semibold))
                Text(verbatim: "Live")
                    .font(TypeScale.callout.weight(.semibold))
            }
            .foregroundStyle(inverse)
            .frame(height: 30)
        }
        .buttonStyle(.glassProminent)
        .buttonBorderShape(.capsule)
        .tint(ink)
        .frame(minHeight: 44)
        .accessibilityLabel(s("live.enter"))
        .transition(.scale.combined(with: .opacity))
    }

    /// Solid ink for the primary buttons (black in light, white in dark) and the text on it.
    private var ink: Color { scheme == .dark ? .white : .black }
    private var inverse: Color { scheme == .dark ? .black : .white }

    private func loadPhotos(_ items: [PhotosPickerItem]) {
        guard !items.isEmpty else { return }
        photoItems = []
        controller.addPhotos(items, namePrefix: "anh", to: composer)
    }
}

/// Hold to talk, or tap to start and tap again to stop. The text lands in the field for editing.
private struct MicButton: View {
    let controller: LiveController
    let composer: ChatComposer
    @Environment(\.strings) private var s
    @State private var pressStart: Date?
    @State private var startedByThisPress = false

    var body: some View {
        Image(systemName: composer.dictating ? "stop.fill" : "mic")
            .font(.system(size: composer.dictating ? 14 : Size.icon, weight: .medium))
            .foregroundStyle(composer.dictating ? Color(uiColor: .systemBackground) : Ink.primary)
            .frame(width: 36, height: 36)
            .background(Circle().fill(composer.dictating ? Ink.primary : Color.clear))
            .frame(width: 44, height: 44)
            .contentShape(.circle)
            .contentTransition(.symbolEffect(.replace))
            .opacity(composer.transcribing ? 0.4 : 1)
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { _ in
                        guard pressStart == nil else { return }
                        pressStart = Date()
                        startedByThisPress = !composer.dictating
                        if startedByThisPress { controller.startDictation() }
                    }
                    .onEnded { _ in
                        let held = Date().timeIntervalSince(pressStart ?? Date())
                        pressStart = nil
                        // A tap that started dictation keeps it open; a hold, or a tap while listening, stops it.
                        if !startedByThisPress || held > 0.45 { Task { await controller.stopDictation() } }
                    }
            )
            .accessibilityElement()
            .accessibilityLabel(s(composer.dictating ? "composer.dictation.stop" : "composer.dictation"))
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { controller.toggleDictation() }
    }
}

/// Picked files as chips: thumbnail or document icon, upload ring, retry on failure, remove.
struct AttachmentStrip: View {
    let composer: ChatComposer
    let retry: (UUID) -> Void
    @Environment(\.strings) private var s

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: Space.s) {
                ForEach(composer.attachments) { a in
                    chip(a)
                }
            }
            .padding(.horizontal, Space.xs)
            .padding(.top, Space.xs)
        }
        .scrollIndicators(.hidden)
    }

    private func chip(_ a: Attachment) -> some View {
        ZStack(alignment: .topTrailing) {
            Group {
                if let thumb = a.thumbnail {
                    Image(uiImage: thumb).resizable().scaledToFill()
                } else {
                    VStack(spacing: Space.xxs) {
                        Image(systemName: "doc.text").font(.system(size: 20, weight: .regular))
                        Text(verbatim: a.name)
                            .font(TypeScale.micro)
                            .lineLimit(2)
                            .multilineTextAlignment(.center)
                    }
                    .foregroundStyle(Ink.secondary)
                    .padding(Space.xs)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Ink.hairline)
                }
            }
            .frame(width: a.thumbnail == nil ? 88 : 64, height: 64)
            .clipShape(.rect(cornerRadius: Radius.m))
            .overlay {
                if !a.isUploaded {
                    ZStack {
                        RoundedRectangle(cornerRadius: Radius.m).fill(Color.black.opacity(0.35))
                        if a.failed {
                            Button { retry(a.id) } label: {
                                Image(systemName: "arrow.clockwise").font(.system(size: 16, weight: .semibold))
                                    .frame(width: 44, height: 44)
                            }
                            .foregroundStyle(.white)
                            .accessibilityLabel(s("attach.retry"))
                        } else {
                            UploadRing(progress: a.progress)
                                .frame(width: 24, height: 24)
                        }
                    }
                }
            }
            Button { composer.remove(a.id) } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(Color(uiColor: .systemBackground))
                    .frame(width: 20, height: 20)
                    .background(Circle().fill(Ink.primary))
                    .frame(width: 32, height: 32)
                    .contentShape(.rect)
            }
            .offset(x: 10, y: -10)
            .accessibilityLabel(s("attach.remove"))
        }
        .padding(.top, Space.xs)
        .padding(.trailing, Space.xs)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text(verbatim: a.name))
    }
}

private struct UploadRing: View {
    let progress: Double
    var body: some View {
        ZStack {
            Circle().stroke(Color.white.opacity(0.35), lineWidth: 2.5)
            Circle().trim(from: 0, to: max(0.04, progress))
                .stroke(Color.white, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .animation(.easeOut(duration: Motion.quick), value: progress)
    }
}

/// The system camera, for a quick photo of a page.
struct CameraPicker: UIViewControllerRepresentable {
    let onImage: (UIImage) -> Void
    @Environment(\.dismiss) private var dismiss

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ uiViewController: UIImagePickerController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    @MainActor
    final class Coordinator: NSObject, @preconcurrency UIImagePickerControllerDelegate, @preconcurrency UINavigationControllerDelegate {
        let parent: CameraPicker
        init(_ parent: CameraPicker) { self.parent = parent }

        func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
            if let image = info[.originalImage] as? UIImage { parent.onImage(image) }
            parent.dismiss()
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) { parent.dismiss() }
    }
}
