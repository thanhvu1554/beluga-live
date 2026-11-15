import SwiftUI
import UIKit
import PhotosUI
import UniformTypeIdentifiers
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
        // Older servers ignore it; the Settings row shows the current voice.
        transport?.send(.getVoices)
        requestGreeting()
        requestHome()
    }

    /// The greeting for where the owner is: a v3 server scopes its second line to the folder (section 18).
    func requestGreeting() {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd'T'HH:mm"
        let scoped = serverV3 ? folderId : nil
        greetingRequestedFor = scoped
        transport?.send(.getGreeting(localTime: f.string(from: Date()), name: nil, folderId: scoped))
    }

    /// Suggestions for the current context (a folder, or the root). The server pushes fresh ones by itself after
    /// turns and list changes; this asks when the context changes.
    func requestHome() {
        guard serverV3 || isMock else { return }
        transport?.send(.getHome(folderId: folderId))
    }

    /// The greeting's second line may name review work, which belongs to one folder: only shown where it was asked.
    var greetingSubVisible: Bool {
        GreetingScope.showsSub(greetingContext: library.greetingFolder, context: folderId, serverVersion: ready?.v ?? 2)
    }

    /// A home card was tapped.
    func act(on card: HomeCard) {
        switch card.kind {
        case .continueSession:
            guard let sid = card.sessionId else { return }
            let item = library.session(sid)
                ?? SessionItem(sessionId: sid, title: card.title, updatedAt: "", folderId: card.folderId, mode: .chat)
            open(item)
        case .due, .quick:
            guard let fid = card.folderId else { return }
            quickReview(fid)
        }
        Haptics.tap()
    }

    // MARK: Chat transcript

    /// A sequenced event goes to the transcript once (the snapshot after a reconnect may repeat it).
    func toChat(_ e: LessonEvent) {
        guard e.seq > chatSeq else { return }
        chatSeq = e.seq
        chatLog.append(e.raw)
        toChat(e.raw)
    }

    /// The chat page came back blank (its web process was killed): draw the session again, without animation.
    func redrawChat() {
        chatPage.call("chat.reset()")
        chatPage.call("chat.load(\(JSONValue.array(chatLog).jsonString))")
    }

    func toChat(_ raw: JSONValue) {
        transcript.apply(raw)
        chatPage.call("chat.apply(\(raw.jsonString))")
    }

    func loadChatHistory(_ events: [LessonEvent]) {
        let raws = events.sorted { $0.seq < $1.seq }.map(\.raw)
        transcript.load(raws)
        chatLog = raws
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
        Diagnostics.log("mode → \(new.rawValue)")
        if new == .chat {
            waitClip(.interrupted)
            if timeline.isSpeaking { interrupt() }
            stopLiveListening()
            camera?.stop()
            caption = ""
        } else {
            if composer.dictating { Task { await stopDictation() } }
            startCamera()
            if board.hasContent { boardVisible = true }
        }
        withAnimation(Motion.panel) {
            mode = new
            // Leaving live also hangs up a call (the call is a face of live mode).
            if new == .chat, inCall { inCall = false }
        }
        if new == .chat { callDidEnd() }
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

    /// A photo from the system camera (already decoded by UIKit).
    func addImage(_ image: UIImage, name: String, to basket: ChatComposer? = nil) {
        let basket = basket ?? composer
        guard basket.attachments.count < ChatComposer.maxAttachments, let data = Uploader.prepareImage(image) else { return }
        let thumb = image.preparingThumbnail(of: CGSize(width: 160, height: 160)) ?? image
        let a = Attachment(name: name, kind: "image", mime: "image/jpeg", thumbnail: thumb, data: data)
        basket.attachments.append(a)
        upload(a, in: basket)
    }

    /// An image file's bytes (photo library, Files): downsampled off the main thread, then uploaded.
    func addImage(data: Data, name: String, to basket: ChatComposer? = nil) async {
        let basket = basket ?? composer
        guard basket.attachments.count < ChatComposer.maxAttachments else { return }
        let prepared = await Task.detached(priority: .userInitiated) { ImageLoader.prepare(data) }.value
        guard let prepared else {
            Diagnostics.log("attach: image not decodable (\(data.count) bytes)")
            showNotice(strings("attach.failed"))
            return
        }
        guard basket.attachments.count < ChatComposer.maxAttachments else { return }
        let a = Attachment(name: name, kind: "image", mime: "image/jpeg", thumbnail: prepared.thumbnail, data: prepared.jpeg)
        basket.attachments.append(a)
        upload(a, in: basket)
    }

    /// Photos chosen in the PhotosPicker, loaded one at a time (each original can be tens of MB).
    func addPhotos(_ items: [PhotosPickerItem], namePrefix: String, to basket: ChatComposer) {
        Diagnostics.log("attach: \(items.count) photos picked")
        Task { [weak self] in
            for (i, item) in items.enumerated() {
                let data: Data?
                do { data = try await item.loadTransferable(type: Data.self) } catch {
                    Diagnostics.log("attach: photo \(i + 1) failed to load: \(error.localizedDescription)")
                    data = nil
                }
                guard let self else { return }
                guard let data else { continue }
                await self.addImage(data: data, name: "\(namePrefix)-\(i + 1).jpg", to: basket)
            }
        }
    }

    /// Files chosen in the Files app: PDFs as they are, images downsampled. Read off the main thread.
    func addFiles(_ urls: [URL], to basket: ChatComposer) {
        Diagnostics.log("attach: \(urls.count) files picked")
        Task { [weak self] in
            for url in urls {
                let loaded = await Task.detached(priority: .userInitiated) { () -> Data? in
                    let scoped = url.startAccessingSecurityScopedResource()
                    defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                    return try? Data(contentsOf: url)
                }.value
                guard let self else { return }
                guard let data = loaded else {
                    Diagnostics.log("attach: file unreadable \(url.lastPathComponent)")
                    self.showNotice(self.strings("attach.failed"))
                    continue
                }
                if UTType(filenameExtension: url.pathExtension)?.conforms(to: .pdf) == true {
                    self.addPDF(data, name: url.lastPathComponent, to: basket)
                } else {
                    await self.addImage(data: data, name: url.deletingPathExtension().lastPathComponent + ".jpg", to: basket)
                }
            }
        }
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
        Diagnostics.log("upload: start \(name) \(data.count / 1024) KB")
        Task { [weak self] in
            do {
                let result = try await uploader.upload(data, name: name, mime: mime) { [weak basket] p in
                    basket?.update(id) { $0.progress = p }
                }
                basket.update(id) { $0.fileId = result.fileId; $0.progress = 1 }
                Diagnostics.log("upload: done \(name)")
            } catch {
                Diagnostics.log("upload: failed \(name): \(error)")
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
        guard audio?.startListening(lang: settings.lang, serverSTT: connected && !needsDeviceSTT()) == true else {
            showNotice(strings("notice.micUnavailable"))
            return
        }
        composer.dictating = true
        Haptics.tap()
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
        let heard = await audio.stopListening()
        // In chat the engine only ran for this dictation: release the microphone (the orange dot goes off).
        if mode == .chat { audio.suspend() }
        switch heard {
        case .text(let text):
            // Device recognition: the server corrects it (when allowed and reachable), else use it as is.
            guard connected, settings.sttFix else { composer.insertDictation(text); return }
            composer.transcribing = true
            // Only a corrected line comes back as `heard`; otherwise the raw text is used after the 2.5 s budget.
            transport?.send(.userText(text, draft: true, via: "voice"))
            armDictationTimeout(fallback: text, seconds: 3.0)
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

    /// The drawer slides shut on the shared spring (whatever triggered it: a row, a new chat, an error).
    func closeDrawer() {
        guard drawerOpen else { return }
        withAnimation(Motion.smooth) { drawerOpen = false }
    }

    func newChat(in folder: String? = nil) {
        closeDrawer()
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
        closeDrawer()
        folderTab = tab
        path = [.folder(id)]
        // Draw this folder's knowledge page now (from cache, then again when fresh data arrives) so the tab
        // is ready before it is opened.
        renderKnowledge(id)
        refreshFolder(id)
    }

    func refreshFolder(_ id: String) {
        transport?.send(.getKnowledge(folderId: id))
        transport?.send(.getReview(folderId: id))
        transport?.send(.listExams(folderId: id))
        transport?.send(.listSessions())
    }

    /// The server speaks protocol v3 (free-form folders, home, sync, backup). Unknown before the first `ready`.
    var serverV3: Bool { (ready?.v ?? 0) >= 3 }

    /// A folder with the owner's own name and an optional line about it (PROTOCOL section 17). It opens once the
    /// server confirms it, after the sheet has finished going down (a push under a sheet still sliding away
    /// reads as two movements at once).
    func createFolder(name: String, about: String) {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !n.isEmpty else { return }
        let a = String(about.trimmingCharacters(in: .whitespacesAndNewlines).prefix(Folder.aboutLimit))
        let clientId = OpQueue.newClientId()
        if !connected {
            // Offline: the folder exists on the phone at once (marked as waiting) and goes out on reconnect.
            enqueue(.createFolder(clientId: clientId, name: n, about: a))
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(0.45))
                self?.openFolder(clientId)
            }
            return
        }
        createdFolderClientId = clientId
        openCreatedFolderAfter = Date().addingTimeInterval(0.45)
        openCreatedFolder = true
        if serverV3 || isMock {
            transport?.send(.createFolder(name: n, about: a.isEmpty ? nil : a, clientId: clientId))
        } else {
            // A v2 server needs a subject; "chung" teaches in general and the name still says what it is.
            transport?.send(.createFolder(name: n, subject: "chung"))
        }
    }

    /// New name and/or about. A v2 server only knows names.
    func updateFolder(_ id: String, name: String, about: String) {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !n.isEmpty, let old = library.folder(id) else { return }
        let a = String(about.trimmingCharacters(in: .whitespacesAndNewlines).prefix(Folder.aboutLimit))
        let newName = n == old.name ? nil : n
        let newAbout = a == old.about ? nil : a
        guard newName != nil || newAbout != nil else { return }
        // Shown at once; the server's `folders` confirms it.
        if let i = library.folders.firstIndex(where: { $0.id == id }) {
            if let newName { library.folders[i].name = newName }
            if let newAbout { library.folders[i].about = newAbout }
        }
        if !connected {
            enqueue(.updateFolder(id: id, name: newName, about: newAbout))
        } else if serverV3 || isMock {
            transport?.send(.updateFolder(id: id, name: newName, about: newAbout))
        } else if let newName {
            transport?.send(.renameFolder(id: id, name: newName))
        }
    }

    /// `folder_created`: open it when it is the one this phone just asked for.
    func folderCreated(id: String, clientId: String?) {
        guard openCreatedFolder, !id.isEmpty else { return }
        if let clientId, let mine = createdFolderClientId, clientId != mine { return }
        openCreatedFolder = false
        createdFolderClientId = nil
        let wait = max(0, openCreatedFolderAfter.timeIntervalSinceNow)
        Task { [weak self] in
            if wait > 0 { try? await Task.sleep(for: .seconds(wait)) }
            self?.openFolder(id)
        }
    }

    func deleteFolder(_ id: String) {
        guard connected else { showNotice(strings("offline.needsConnection")); return }
        path.removeAll { $0 == .folder(id) }
        transport?.send(.deleteFolder(id: id))
    }

    func moveSession(_ s: SessionItem, to folder: String?) {
        if connected { transport?.send(.moveSession(sessionId: s.sessionId, folderId: folder)) }
        else { enqueue(.moveSession(sessionId: s.sessionId, folderId: folder)) }
        if s.sessionId == sessionId, folderId != folder {
            folderId = folder
            requestHome()
            requestGreeting()
        }
    }

    func renameSession(_ s: SessionItem, title newTitle: String) {
        let t = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        if connected { transport?.send(.renameSession(sessionId: s.sessionId, title: t)) }
        else { enqueue(.renameSession(sessionId: s.sessionId, title: t)) }
        if s.sessionId == sessionId { title = t }
    }

    func deleteSession(_ s: SessionItem) {
        // The open session cannot be deleted (`busy_session`): leave it for a new chat first.
        if s.sessionId == sessionId { newChat(in: folderId) }
        if connected { transport?.send(.deleteSession(sessionId: s.sessionId)) }
        else { enqueue(.deleteSession(sessionId: s.sessionId)) }
    }

    // MARK: Library frames

    func handleLibrary(_ m: ServerMessage) {
        switch m {
        case .folders(let items):
            applyServerFolders(items)
            for f in items { if let i = f.ingest { library.ingest[f.id] = i } }
        case let .folderCreated(id, clientId):
            if resolveFolderWaiter(id: id, clientId: clientId) { break }
            folderCreated(id: id, clientId: clientId)
        case .sync(let hint):
            syncHint(hint)
        case .ingest(let s):
            library.ingest[s.folderId] = s
            if knowledgeFolder == s.folderId { knowledgePage.invoke("progress", s.json); renderedIngest = s.json }
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
            if !g.text.isEmpty {
                library.greeting = g
                library.greetingFolder = greetingRequestedFor
            }
        case .backup(let b):
            let old = library.backup
            library.backup = b
            if backupRunning, b.lastAt != old?.lastAt || (b.lastError != nil && b.lastError != old?.lastError) {
                backupTimeout?.cancel()
                backupRunning = false
                if b.lastError == nil { Haptics.success() } else { Haptics.warning() }
            }
        case .home(let h):
            // Kept per context: going back to the root shows its own cards at once, not the folder's.
            library.homes[h.folderId ?? ""] = h
        case .voices(let catalog):
            library.voices = catalog
            prepareWaitClips(catalog)
        default:
            break
        }
    }

    // MARK: Knowledge page

    /// Puts a folder's knowledge (with its review overlay and any running ingest) on the page. The page is only
    /// rebuilt when what it would show changed: switching folder tabs, or the same data arriving again after
    /// `refreshFolder`, used to rebuild the whole page (KaTeX included) on the main thread mid-transition.
    func renderKnowledge(_ folder: String, force: Bool = false) {
        let data = library.knowledge[folder]?.raw ?? .null
        let review = library.review[folder]?.raw ?? .null
        let ingest: JSONValue = library.ingest[folder].map { $0.isActive || $0.state == .error ? $0.json : .null } ?? .null
        let signature = JSONValue.array([.string(folder), data, review])
        if !force, knowledgeFolder == folder, renderedKnowledge == signature {
            if renderedIngest != ingest { renderedIngest = ingest; knowledgePage.invoke("progress", ingest) }
            return
        }
        knowledgeFolder = folder
        renderedKnowledge = signature
        renderedIngest = ingest
        // The native folder screen has its own tab bar: hide the page's section strip (older pages ignore this).
        knowledgePage.call("knowledge.setOptions&&knowledge.setOptions({tabs:false})")
        // null data draws the page's empty state (with its "add exams" button).
        knowledgePage.invoke("render", data, review)
        if library.ingest[folder] != nil { knowledgePage.invoke("progress", ingest) }
    }

    /// Scrolls the knowledge page to one of its sections (menu in the folder's navigation bar).
    func showKnowledgeSection(_ id: String) {
        knowledgePage.invoke("show", .string(id))
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
        case "sections":
            let items = (body["items"] as? [[String: Any]]) ?? []
            knowledgeSections = items.compactMap { item in
                guard let id = item["id"] as? String, let title = item["title"] as? String else { return nil }
                return KnowledgeSection(id: id, title: title)
            }
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
        closeDrawer()
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

    // MARK: Backup (PROTOCOL section 20)

    func requestBackup() {
        guard connected, serverV3 || isMock else { return }
        transport?.send(.getBackup)
    }

    /// Pushes a backup now; the button shows it running until the server reports the new one (or 90 s pass).
    func backupNow() {
        guard connected, !backupRunning else { return }
        Haptics.tap()
        backupRunning = true
        transport?.send(.backupNow)
        backupTimeout?.cancel()
        backupTimeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(90))
            guard let self, !Task.isCancelled else { return }
            self.backupRunning = false
        }
    }

    // MARK: Voices

    func requestVoices() {
        transport?.send(.getVoices)
    }

    /// The owner picked a voice: `settings{voice}` (the only time a voice id goes into settings); the server
    /// answers with a fresh `voices`, which also undoes the optimistic tick if it refused the id.
    func selectVoice(_ id: String) {
        guard connected, library.voices?.current != id else { return }
        Diagnostics.log("voice: select \(id)")
        var s = settings.tutor(mode: mode)
        s.voice = id
        transport?.send(.settings(s))
        library.voices?.current = id
        Haptics.tap()
    }

    /// A preview is about to play: the tutor must not talk over it, and the mic must not hear it.
    func prepareForPreview() {
        if timeline.isSpeaking { interrupt() }
        if pill.micOpen { stopLiveListening() }
        if composer.dictating { Task { await stopDictation() } }
    }

    /// `GET /voice/<id>.mp3` request with the pairing token (PROTOCOL section 16), in the query as well as the
    /// header (URLSession may drop an `Authorization` header it manages itself).
    func voicePreviewRequest(_ id: String) -> URLRequest? {
        guard let base = settings.httpBase,
              var c = URLComponents(url: base.appendingPathComponent("voice").appendingPathComponent("\(id).mp3"),
                                    resolvingAgainstBaseURL: false) else { return nil }
        c.queryItems = [URLQueryItem(name: "token", value: settings.pairToken ?? "")]
        guard let url = c.url else { return nil }
        var req = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 15)
        req.setValue("Bearer \(settings.pairToken ?? "")", forHTTPHeaderField: "Authorization")
        return req
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

    /// `-mockTour 1`: drives the chat shell through its transitions so one screen recording shows them all:
    /// home entrance, drawer out and back, a folder pushed, its tabs, back, a question typed (Live morphs into
    /// Send), the answer streaming, Live up and down, the model sheet, Settings.
    func runMockTour() {
        Task { [weak self] in
            func pause(_ s: Double) async { try? await Task.sleep(for: .seconds(s)) }
            await pause(3.0)
            guard let self else { return }
            withAnimation(Motion.smooth) { self.drawerOpen = true }
            await pause(1.8)
            self.closeDrawer()
            await pause(1.4)
            self.openFolder("f-toan", tab: .sessions)
            await pause(1.6)
            self.folderTab = .knowledge
            await pause(1.8)
            self.folderTab = .review
            await pause(1.4)
            self.folderTab = .exams
            await pause(1.4)
            self.path = []
            await pause(1.6)
            for ch in MockData.chatQuestion {
                self.composer.text.append(ch)
                try? await Task.sleep(for: .milliseconds(28))
            }
            await pause(0.8)
            self.sendChat()
            await pause(8.0)
            self.setMode(.live)
            await pause(2.6)
            self.setMode(.chat)
            await pause(1.6)
            self.showModels = true
            await pause(1.8)
            self.showModels = false
            await pause(1.4)
            self.showSettings = true
            await pause(2.0)
            self.showSettings = false
        }
    }

    private func feed(_ v: JSONValue) { handle(ServerMessage.decode(v)) }

    func applyScene(_ scene: MockScene, mock: MockTransport) {
        if scene.isCall {
            applyCallScene(scene, mock: mock)
            return
        }
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
        case .voices:
            // The voice screen with one preview playing (Phong).
            settingsPath = [.voices]
            showSettings = true
            sceneBusy += 1
            Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(900))
                guard let self else { return }
                self.voicePreview.toggle("RxhjHDfpO54FYotYtKpw", controller: self)
                self.sceneBusy -= 1
            }
        case .connection:
            settingsPath = [.connection]
            showSettings = true
        case .newfolder:
            drawerOpen = true
            showNewFolder = true
        case .datasettings:
            settingsPath = [.data]
            showSettings = true
            sync.lastSyncAt = Date().addingTimeInterval(-240)
        case .offlinedrawer:
            // Offline, with a folder made on the phone still waiting (not written to the replica: later scenes
            // must not replay it).
            drawerOpen = true
            linkDown = true
            sceneBusy += 1
            Task { [weak self] in
                // After the replica is read (it would replace the queue).
                while let self, !self.replicaLoaded { try? await Task.sleep(for: .milliseconds(50)) }
                guard let self else { return }
                self.ops.enqueue(.createFolder(clientId: OpQueue.newClientId(), name: "Lịch sử 9", about: "Ôn cuối kỳ"))
                self.refreshOverlay()
                self.sceneBusy -= 1
            }
        case .typing:
            composer.text = "Cho tớ hỏi bài này: giải phương trình x² − 5x + 6 = 0, rồi kiểm tra lại bằng Vi-ét giúp tớ. Tớ hay nhầm dấu ở bước cuối."
        case .replay:
            applyLiveScene(.replay, mock: mock)
            liveTurns.insert(1)
            showReplay = true
        default:
            break
        }
    }
}
