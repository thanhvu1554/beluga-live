import SwiftUI
import UIKit
import BelugaCore

// Protocol v2 (PROTOCOL sections 7-15): chat mode, attachments, dictation, folders, knowledge ingest,
// review, exams, models and usage, greeting, and the mock scenes for those screens.
extension LiveController {

    // MARK: Ready

    func readyV2(_ r: Ready) {
        if !r.models.isEmpty {
            library.models = r.models
            if library.model(settings.model) == nil, let m = r.model ?? r.models.first?.id { settings.model = m; settings.effort = "" }
        }
        // The app's mode is the truth; tell a server that disagrees.
        if r.v >= 2, r.mode != mode { transport?.send(.settings(settings.tutor(mode: mode))) }
        if let base = settings.httpBase {
            chatPage.invoke("setFileBase", .string(base.absoluteString), .string(settings.pairToken ?? ""))
        }
        // Server has the web pages this app was built without (or newer ones).
        if r.boardVersion >= 3, let url = settings.boardURL {
            if !chatPage.available { chatPage.loadRemote(board: url) }
            if !knowledgePage.available { knowledgePage.loadRemote(board: url) }
        }
        transport?.send(.listFolders)
        transport?.send(.listSessions())
        requestGreeting()
    }

    func requestGreeting() {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd'T'HH:mm"
        transport?.send(.getGreeting(localTime: f.string(from: Date()), name: nil))
    }

    // MARK: Chat transcript

    /// A sequenced event goes to the transcript once (the snapshot after a reconnect may repeat it).
    func toChat(_ e: LessonEvent) {
        guard e.seq > chatSeq else { return }
        chatSeq = e.seq
        toChat(e.raw)
    }

    func toChat(_ raw: JSONValue) {
        transcript.apply(raw)
        chatPage.call("chat.apply(\(raw.jsonString))")
    }

    func loadChatHistory(_ events: [LessonEvent]) {
        let raws = events.sorted { $0.seq < $1.seq }.map(\.raw)
        transcript.load(raws)
        chatSeq = events.map(\.seq).max() ?? 0
        chatPage.clearQueue()
        chatPage.call("chat.reset()")
        chatPage.call("chat.load(\(JSONValue.array(raws).jsonString))")
    }

    func chatPageMessage(_ body: [String: Any]) {
        switch body["type"] as? String {
        case "action":
            let action = body["action"] as? String
            if action == "copy", let text = body["text"] as? String {
                UIPasteboard.general.string = text
                showNotice(strings("chat.copied"))
            } else if action == "retry" {
                retryLastTurn()
            }
        case "ask_tap":
            composer.focusTick += 1
        case "open_file":
            if let id = body["fileId"] as? String {
                Task { await openFile(id, name: body["name"] as? String ?? id) }
            }
        case "open_url":
            if let s = body["url"] as? String, let url = URL(string: s) { UIApplication.shared.open(url) }
        default:
            break
        }
    }

    /// An attachment tapped in the transcript: fetched from `GET /file/<id>` and shown in QuickLook.
    func openFile(_ fileId: String, name: String) async {
        guard let url = settings.fileURL(fileId), !isMock else { return }
        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            guard (response as? HTTPURLResponse)?.statusCode ?? 200 < 300 else { throw UploadError.badBody }
            let local = FileManager.default.temporaryDirectory.appendingPathComponent(name.isEmpty ? fileId : name)
            try data.write(to: local, options: .atomic)
            preview = PreviewFile(url: local)
        } catch {
            showNotice(strings("attach.openFailed"))
        }
    }

    /// Sends the last user line again.
    func retryLastTurn() {
        for item in transcript.items.reversed() {
            if case .user(let text, let files) = item.kind {
                transport?.send(.userText(text, files: files.map(\.fileId)))
                return
            }
        }
    }

    // MARK: Mode

    func setMode(_ new: SessionMode) {
        guard new != mode else { return }
        if new == .chat {
            if timeline.isSpeaking { interrupt() }
            stopLiveListening()
            camera?.stop()
            caption = ""
        } else {
            if composer.dictating { Task { await stopDictation() } }
            startCamera()
            if board.hasContent { boardVisible = true }
        }
        withAnimation(Motion.panel) { mode = new }
        Haptics.tap()
        if connected { transport?.send(.settings(settings.tutor(mode: new))) }
    }

    // MARK: Sending

    func sendChat() {
        guard composer.canSend else { return }
        guard connected || isMock else { showNotice(strings("chat.offline")); return }
        transport?.send(.userText(composer.trimmed, files: composer.fileIds))
        composer.clear()
        Haptics.tap()
    }

    // MARK: Attachments

    var uploader: Uploader { Uploader(base: settings.httpBase, token: settings.pairToken, mock: isMock) }

    func addImage(_ image: UIImage, name: String, to basket: ChatComposer? = nil) {
        let basket = basket ?? composer
        guard basket.attachments.count < ChatComposer.maxAttachments, let data = Uploader.prepareImage(image) else { return }
        let thumb = image.preparingThumbnail(of: CGSize(width: 160, height: 160)) ?? image
        let a = Attachment(name: name, kind: "image", mime: "image/jpeg", thumbnail: thumb, data: data)
        basket.attachments.append(a)
        upload(a, in: basket)
    }

    func addPDF(_ data: Data, name: String, to basket: ChatComposer? = nil) {
        let basket = basket ?? composer
        guard basket.attachments.count < ChatComposer.maxAttachments else { return }
        guard data.count <= 30 * 1024 * 1024 else { showNotice(strings("attach.tooBig")); return }
        let a = Attachment(name: name, kind: "pdf", mime: "application/pdf", thumbnail: nil, data: data)
        basket.attachments.append(a)
        upload(a, in: basket)
    }

    func retryUpload(_ id: UUID, in basket: ChatComposer) {
        guard let a = basket.attachments.first(where: { $0.id == id }) else { return }
        basket.update(id) { $0.failed = false; $0.progress = 0 }
        upload(a, in: basket)
    }

    private func upload(_ a: Attachment, in basket: ChatComposer) {
        let uploader = self.uploader
        let id = a.id, data = a.data, name = a.name, mime = a.mime
        Task { [weak self] in
            do {
                let result = try await uploader.upload(data, name: name, mime: mime) { [weak basket] p in
                    basket?.update(id) { $0.progress = p }
                }
                basket.update(id) { $0.fileId = result.fileId; $0.progress = 1 }
            } catch {
                basket.update(id) { $0.failed = true }
                self?.showNotice(self?.strings("attach.failed") ?? "")
            }
        }
    }

    // MARK: Dictation (chat)

    func toggleDictation() {
        if composer.dictating { Task { await stopDictation() } } else { startDictation() }
    }

    func startDictation() {
        guard !composer.dictating, !composer.transcribing else { return }
        if isMock { composer.dictating = true; Haptics.tap(); return }
        guard canListen() else { return }
        composer.dictating = true
        Haptics.tap()
        audio?.startListening(lang: settings.lang, serverSTT: connected && !needsDeviceSTT())
    }

    /// Dictation goes to the composer, never straight to the tutor (`draft`, PROTOCOL section 7).
    func stopDictation() async {
        guard composer.dictating else { return }
        composer.dictating = false
        if isMock {
            composer.transcribing = true
            transport?.send(.userText("Cho tớ thêm một bài tương tự nhé", draft: true))
            return
        }
        guard let audio else { return }
        switch await audio.stopListening() {
        case .text(let text):
            // Device recognition: the server corrects it (when allowed and reachable), else use it as is.
            guard connected, settings.sttFix else { composer.insertDictation(text); return }
            composer.transcribing = true
            transport?.send(.userText(text, draft: true))
            armDictationTimeout(fallback: text, seconds: 3.5)
        case .wav(let wav):
            composer.transcribing = true
            sendWav(wav, draft: true)
            armDictationTimeout(fallback: nil, seconds: 15)
        case .nothing:
            break
        }
    }

    private func armDictationTimeout(fallback: String?, seconds: Double) {
        dictationTimeout?.cancel()
        dictationTimeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(seconds))
            guard let self, !Task.isCancelled, self.composer.transcribing else { return }
            if let fallback { self.composer.insertDictation(fallback) } else {
                self.composer.transcribing = false
                self.showNotice(self.strings("notice.sttFailed"))
            }
        }
    }

    // MARK: Sessions and folders

    func newChat(in folder: String? = nil) {
        drawerOpen = false
        path = []
        audio?.stopSpeaking(); speakTask?.cancel()
        sessionId = nil
        settings.lastSessionId = nil
        startFreshSession()
        folderId = folder
        if mode == .live { setMode(.chat) }
        if connected {
            transport?.send(.newSession(folderId: folder, mode: .chat))
        } else {
            pendingFolderId = folder
            reconnectNow()
        }
    }

    func openFolder(_ id: String, tab: FolderTab = .sessions) {
        drawerOpen = false
        folderTab = tab
        path = [.folder(id)]
        refreshFolder(id)
    }

    func refreshFolder(_ id: String) {
        transport?.send(.getKnowledge(folderId: id))
        transport?.send(.getReview(folderId: id))
        transport?.send(.listExams(folderId: id))
        transport?.send(.listSessions())
    }

    func createFolder(name: String, subject: String) {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !n.isEmpty else { return }
        openCreatedFolder = true
        transport?.send(.createFolder(name: n, subject: subject))
    }

    func renameFolder(_ id: String, name: String) {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !n.isEmpty else { return }
        transport?.send(.renameFolder(id: id, name: n))
    }

    func deleteFolder(_ id: String) {
        path.removeAll { $0 == .folder(id) }
        transport?.send(.deleteFolder(id: id))
    }

    func moveSession(_ s: SessionItem, to folder: String?) {
        transport?.send(.moveSession(sessionId: s.sessionId, folderId: folder))
        if s.sessionId == sessionId { folderId = folder }
    }

    func renameSession(_ s: SessionItem, title newTitle: String) {
        let t = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        transport?.send(.renameSession(sessionId: s.sessionId, title: t))
        if s.sessionId == sessionId { title = t }
    }

    func deleteSession(_ s: SessionItem) {
        transport?.send(.deleteSession(sessionId: s.sessionId))
        if s.sessionId == sessionId { newChat(in: folderId) }
    }

    // MARK: Library frames

    func handleLibrary(_ m: ServerMessage) {
        switch m {
        case .folders(let items):
            library.folders = items
            for f in items { if let i = f.ingest { library.ingest[f.id] = i } }
        case .folderCreated(let id):
            if openCreatedFolder, !id.isEmpty { openCreatedFolder = false; openFolder(id) }
        case .ingest(let s):
            library.ingest[s.folderId] = s
            if knowledgeFolder == s.folderId { knowledgePage.invoke("progress", s.json) }
            if s.state == .done {
                transport?.send(.getKnowledge(folderId: s.folderId))
                transport?.send(.getReview(folderId: s.folderId))
                Haptics.success()
            }
        case .knowledge(let k):
            library.apply(knowledge: k)
            if knowledgeFolder == k.folderId { renderKnowledge(k.folderId) }
        case .review(let r):
            library.review[r.folderId] = r
            weeklyLoading.remove(r.folderId)
            if knowledgeFolder == r.folderId { renderKnowledge(r.folderId) }
        case .exam(let s):
            library.apply(exam: s)
            if s.state == .done {
                transport?.send(.listExams(folderId: s.folderId))
                Haptics.success()
                let title = s.title ?? strings("exams.untitled")
                Task { await exportExam(s.examId, title: title) }
            } else if s.state == .error {
                showNotice(s.note.isEmpty ? strings("exams.failed") : s.note)
            }
        case let .exams(fid, items):
            library.exams[fid] = items
        case .usage(let u):
            library.usage = u
        case .greeting(let g):
            if !g.text.isEmpty { library.greeting = g }
        default:
            break
        }
    }

    // MARK: Knowledge page

    /// Puts a folder's knowledge (with its review overlay and any running ingest) on the page.
    func renderKnowledge(_ folder: String) {
        knowledgeFolder = folder
        // null data draws the page's empty state (with its "add exams" button).
        knowledgePage.invoke("render", library.knowledge[folder]?.raw ?? .null, library.review[folder]?.raw ?? .null)
        if let i = library.ingest[folder] { knowledgePage.invoke("progress", i.isActive || i.state == .error ? i.json : .null) }
    }

    func knowledgePageMessage(_ body: [String: Any]) {
        guard let folder = knowledgeFolder else { return }
        switch body["type"] as? String {
        case "gen_exam":
            if let sid = body["structureId"] as? String { genExamRequest = GenExamRequest(folderId: folder, structureId: sid) }
        case "add_exams":
            showIngest = folder
        case "ingest_cancel":
            cancelIngest(folder: folder)
        case "open_topic":
            let title = body["title"] as? String ?? ""
            newChat(in: folder)
            composer.text = strings("knowledge.topicPrompt", title)
            composer.focusTick += 1
        default:
            break
        }
    }

    // MARK: Ingest ("Thêm đề")

    func startIngest(folder: String, goal: String) {
        let files = ingestBasket.fileIds
        guard !files.isEmpty, !ingestBasket.uploading else { return }
        transport?.send(.ingestStart(folderId: folder, files: files, goal: goal))
        library.ingest[folder] = IngestStatus(folderId: folder, state: .queued, total: files.count, note: strings("ingest.queued"))
        ingestBasket.clear()
        showIngest = nil
        folderTab = .knowledge
    }

    func cancelIngest(folder: String) {
        transport?.send(.ingestCancel(folderId: folder))
    }

    // MARK: Exams

    func genExam(_ request: GenExamRequest, note: String) {
        transport?.send(.genExam(folderId: request.folderId, structureId: request.structureId, note: note))
        library.examJobs[request.folderId] = ExamStatus(folderId: request.folderId, examId: "", state: .writing,
                                                        note: strings("exams.writing"))
        genExamRequest = nil
        folderTab = .exams
    }

    /// Loads the printable exam page in a hidden web view, renders an A4 PDF and opens it in QuickLook.
    func exportExam(_ examId: String, title: String) async {
        guard exporting == nil else { return }
        let source: ExamExporter.Source
        if isMock { source = .html(MockData.examHTML(title: title)) }
        else if let url = settings.examURL(examId) { source = .url(url) }
        else { return }
        exporting = examId
        defer { exporting = nil }
        do {
            let url = try await ExamExporter().export(source, fileName: title)
            preview = PreviewFile(url: url)
        } catch {
            showNotice(strings("exams.exportFailed"))
        }
    }

    func deleteExam(_ exam: ExamItem) {
        transport?.send(.deleteExam(examId: exam.examId))
        for (fid, list) in library.exams { library.exams[fid] = list.filter { $0.examId != exam.examId } }
    }

    // MARK: Review

    func requestWeekly(_ folder: String) {
        weeklyLoading.insert(folder)
        transport?.send(.getWeekly(folderId: folder))
    }

    /// "Ôn nhanh": a new chat in the folder that the server opens with a quiz on due and weak topics.
    func quickReview(_ folder: String) {
        newChatSilently(folder: folder)
        transport?.send(.newSession(folderId: folder, mode: .chat, kickoff: "review_quiz"))
    }

    private func newChatSilently(folder: String) {
        path = []
        drawerOpen = false
        sessionId = nil
        startFreshSession()
        folderId = folder
        if mode == .live { setMode(.chat) }
    }

    /// Replays an old lesson on the board, read only, with step paging.
    func replay(_ lesson: FolderReview.Lesson, folder: String) {
        open(SessionItem(sessionId: lesson.sessionId, title: lesson.title, updatedAt: lesson.updatedAt,
                         folderId: folder, mode: lesson.mode), keepPath: true)
        showReplay = true
    }

    // MARK: Models and usage

    var currentModel: ModelInfo? { library.model(settings.model) }

    func selectModel(_ id: String) {
        guard id != settings.model else { return }
        settings.model = id
        settings.effort = ""
        settingsChanged()
    }

    func selectEffort(_ effort: String) {
        settings.effort = effort
        settingsChanged()
    }

    func requestUsage() {
        transport?.send(.getUsage)
    }

    // MARK: Mock

    /// `-mockChat 1`: types and sends a question, as a student would.
    func runMockChat() {
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(1.0))
            guard let self else { return }
            self.composer.text = MockData.chatQuestion
            try? await Task.sleep(for: .seconds(0.8))
            self.sendChat()
        }
    }

    private func feed(_ v: JSONValue) { handle(ServerMessage.decode(v)) }

    func applyScene(_ scene: MockScene, mock: MockTransport) {
        if scene.isLive {
            applyLiveScene(scene, mock: mock)
            return
        }
        // Everything else starts connected, in a fresh chat session, with folders and sessions loaded.
        connect()
        switch scene {
        case .home, .settings, .progress:
            if scene == .settings { showSettings = true }
            if scene == .progress {
                if case .progress(let report) = ServerMessage.decode(mock.script.progress) { progress = report }
                showProgress = true
            }
        case .chat:
            feed(["t": "user", "seq": 1, "turn": 1, "text": .string(MockData.chatQuestion), "via": "text"])
            feed(["t": "chat", "seq": 2, "turn": 1, "md": .string(MockData.chatAnswer)])
            feed(["t": "show", "seq": 3, "turn": 1, "step": -1, "block": MockData.chatBlock])
            feed(["t": "chat_delta", "turn": 1, "text": "**Kiểm tra lại** bằng Vi-ét: $x_1 + x_2 = 5$ và"])
            title = "Toán · Phương trình bậc hai"
        case .chatfiles:
            feed(["t": "user", "seq": 1, "turn": 1, "text": "Bài 3 với bài 5 trong ảnh này làm sao vậy?", "via": "text",
                  "files": [["fileId": "a1", "kind": "image", "name": "trang-12.jpg"],
                            ["fileId": "a2", "kind": "pdf", "name": "de-on-tap.pdf"]]])
            feed(["t": "chat", "seq": 2, "turn": 1, "md": "Tớ xem rồi. **Bài 3** là bất phương trình bậc hai: tìm nghiệm $x = 2$, $x = 3$ rồi xét dấu. **Bài 5** cần điều kiện $x \\ne 1$ trước khi quy đồng."])
            feed(["t": "turn_end", "seq": 3, "turn": 1])
            title = "Toán · Bài tập trang 12"
            composer.text = "Còn bài 4 thì sao?"
            let photo = UIImage(named: "mock-problem")
            let thumb = photo?.preparingThumbnail(of: CGSize(width: 160, height: 160))
            composer.attachments = [
                Attachment(name: "trang-13.jpg", kind: "image", mime: "image/jpeg", thumbnail: thumb, data: Data(), progress: 1, fileId: "a3"),
                Attachment(name: "trang-14.jpg", kind: "image", mime: "image/jpeg", thumbnail: thumb, data: Data(), progress: 0.45),
                Attachment(name: "de-kiem-tra.pdf", kind: "pdf", mime: "application/pdf", thumbnail: nil, data: Data(), progress: 1, fileId: "a4"),
            ]
        case .drawer, .sessions:
            drawerOpen = true
        case .folder:
            openFolder("f-toan", tab: .sessions)
        case .knowledge:
            openFolder("f-toan", tab: .knowledge)
        case .ingest:
            openFolder("f-hoa", tab: .knowledge)
            feed(["t": "ingest", "folderId": "f-hoa", "jobId": "j1", "state": "reading", "done": 7, "total": 24,
                  "note": "Đang đọc trang 7/24: Đề ôn giữa kỳ số 3", "found": ["questions": 41, "topics": 6]])
        case .folderreview:
            openFolder("f-toan", tab: .review)
        case .exams:
            openFolder("f-toan", tab: .exams)
            feed(["t": "exam", "folderId": "f-toan", "examId": "e-3", "state": "writing",
                  "note": "Đang viết đề theo ma trận: 12 câu trắc nghiệm, 5 câu tự luận"])
        case .models:
            showModels = true
        case .usage:
            requestUsage()
            showUsage = true
        case .exampdf:
            openFolder("f-toan", tab: .exams)
            sceneBusy += 1
            Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(600))
                await self?.exportExam("e-2", title: "Đề giữa kỳ 1 · Lần 2")
                try? await Task.sleep(for: .seconds(1.5))
                self?.sceneBusy -= 1
            }
        case .replay:
            applyLiveScene(.replay, mock: mock)
            liveTurns.insert(1)
            showReplay = true
        default:
            break
        }
    }
}
