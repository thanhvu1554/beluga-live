import Foundation
import Observation
import BelugaCore

/// Everything the server tells about folders, sessions, knowledge, review, exams, usage and the greeting
/// (PROTOCOL sections 8-15). Filled by `LiveController` from server frames; read by the screens.
@MainActor @Observable
final class Library {
    var folders: [Folder] = []
    /// All sessions (the app always asks for the full list and filters by folder itself).
    var sessions: [SessionItem] = []
    var knowledge: [String: Knowledge] = [:]
    /// Folders whose `knowledge` frame arrived (null data included).
    var knowledgeLoaded: Set<String> = []
    var ingest: [String: IngestStatus] = [:]
    var review: [String: FolderReview] = [:]
    var exams: [String: [ExamItem]] = [:]
    var examJobs: [String: ExamStatus] = [:]
    var usage: Usage?
    var greeting: Greeting
    var models: [ModelInfo] = ModelInfo.fallback
    /// Tutor voices (nil until the server answered `get_voices`).
    var voices: VoiceCatalog?

    func voice(_ id: String?) -> VoiceInfo? {
        guard let id else { return nil }
        return voices?.items.first { $0.id == id }
    }

    init(lang: Lang = .vi) {
        let hour = Calendar.current.component(.hour, from: Date())
        greeting = Greeting.fallback(hour: hour, seed: Int.random(in: 0..<100), lang: lang)
    }

    func folder(_ id: String?) -> Folder? {
        guard let id else { return nil }
        return folders.first { $0.id == id }
    }

    func sessions(in folderId: String?) -> [SessionItem] {
        sessions.filter { $0.folderId == folderId }
    }

    func model(_ id: String) -> ModelInfo? { models.first { $0.id == id } }

    func apply(knowledge k: FolderKnowledge) {
        knowledgeLoaded.insert(k.folderId)
        knowledge[k.folderId] = k.data
        if let i = k.ingest { ingest[k.folderId] = i }
    }

    func apply(exam s: ExamStatus) {
        examJobs[s.folderId] = s
    }
}

/// Subject ids (tutor skills) with their display names.
enum Subject {
    static func name(_ id: String, lang: Lang) -> String {
        let vi: [String: String] = ["toan": "Toán", "ly": "Vật lý", "hoa": "Hóa học", "van": "Ngữ văn",
                                    "anh-ssat": "Tiếng Anh · SSAT", "duc": "Tiếng Đức", "tin-cpp": "Tin học · C++", "chung": "Chung"]
        let en: [String: String] = ["toan": "Math", "ly": "Physics", "hoa": "Chemistry", "van": "Literature",
                                    "anh-ssat": "English · SSAT", "duc": "German", "tin-cpp": "CS · C++", "chung": "General"]
        return (lang == .vi ? vi : en)[id] ?? id
    }

    static func symbol(_ id: String) -> String {
        switch id {
        case "toan": return "function"
        case "ly": return "atom"
        case "hoa": return "flask"
        case "van": return "text.book.closed"
        case "anh-ssat", "duc": return "character.book.closed"
        case "ngoai-ngu": return "globe"
        case "tin-cpp": return "chevron.left.forwardslash.chevron.right"
        default: return "folder"
        }
    }
}
