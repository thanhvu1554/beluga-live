import SwiftUI
import UIKit
import BelugaCore

/// One picked file on its way to the server.
struct Attachment: Identifiable, Equatable {
    let id = UUID()
    var name: String
    /// "image" | "pdf"
    var kind: String
    var mime: String
    var thumbnail: UIImage?
    var data: Data
    var progress: Double = 0
    var fileId: String?
    var failed = false

    var isUploaded: Bool { fileId != nil }
    static func == (a: Attachment, b: Attachment) -> Bool {
        a.id == b.id && a.progress == b.progress && a.fileId == b.fileId && a.failed == b.failed
    }
}

/// State of the chat composer: draft text, attachments and dictation.
@MainActor @Observable
final class ChatComposer {
    var text = ""
    var attachments: [Attachment] = []
    /// The microphone is open for dictation.
    var dictating = false
    /// Dictation ended; waiting for the (corrected) text from the server.
    var transcribing = false
    /// Asks the text field to take focus (e.g. after tapping a question in the transcript).
    var focusTick = 0

    static let maxAttachments = 30

    var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
    var uploading: Bool { attachments.contains { !$0.isUploaded && !$0.failed } }
    var hasContent: Bool { !trimmed.isEmpty || !attachments.isEmpty }
    var canSend: Bool { hasContent && !uploading && !dictating && !transcribing }
    var fileIds: [String] { attachments.compactMap(\.fileId) }

    func clear() {
        text = ""
        attachments.removeAll()
    }

    /// Dictated text lands in the field for editing (appended to what is already typed).
    func insertDictation(_ dictated: String) {
        let t = dictated.trimmingCharacters(in: .whitespacesAndNewlines)
        transcribing = false
        guard !t.isEmpty else { return }
        text = trimmed.isEmpty ? t : trimmed + " " + t
        focusTick += 1
    }

    func update(_ id: UUID, _ change: (inout Attachment) -> Void) {
        guard let i = attachments.firstIndex(where: { $0.id == id }) else { return }
        change(&attachments[i])
    }

    func remove(_ id: UUID) {
        attachments.removeAll { $0.id == id }
    }
}
