import SwiftUI
import PhotosUI
import BelugaCore

/// A folder: sessions, knowledge (exam matrix), review and generated exams (PROTOCOL sections 8, 11-13).
struct FolderView: View {
    @Bindable var controller: LiveController
    let folderId: String
    @Environment(\.strings) private var s
    @State private var renaming = false
    @State private var confirmDelete = false
    @State private var draftName = ""
    @State private var examNote = ""

    private var folder: Folder? { controller.library.folder(folderId) }

    var body: some View {
        VStack(spacing: 0) {
            Picker(s("folder.tabs"), selection: $controller.folderTab) {
                ForEach(FolderTab.allCases, id: \.self) { tab in
                    Text(verbatim: s("folder.tab.\(tab.rawValue)")).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, Space.l)
            .padding(.vertical, Space.s)

            Group {
                switch controller.folderTab {
                case .sessions: FolderSessions(controller: controller, folderId: folderId)
                case .knowledge: KnowledgeTab(controller: controller, folderId: folderId)
                case .review: ReviewTab(controller: controller, folderId: folderId)
                case .exams: ExamsTab(controller: controller, folderId: folderId)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(Color(uiColor: .systemGroupedBackground))
        .navigationTitle(Text(verbatim: folder?.name ?? ""))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Menu {
                    Button { draftName = folder?.name ?? ""; renaming = true } label: { Label(s("folders.rename"), systemImage: "pencil") }
                    Button { controller.showIngest = folderId } label: { Label(s("ingest.add"), systemImage: "doc.badge.plus") }
                    Button(role: .destructive) { confirmDelete = true } label: { Label(s("folders.delete"), systemImage: "trash") }
                } label: {
                    Image(systemName: "ellipsis")
                }
                .accessibilityLabel(s("common.more"))
                Button { controller.newChat(in: folderId) } label: { Image(systemName: "square.and.pencil") }
                    .accessibilityLabel(s("chat.new"))
            }
        }
        .onAppear { controller.refreshFolder(folderId) }
        .alert(s("folders.rename"), isPresented: $renaming) {
            TextField(s("folders.name.placeholder"), text: $draftName)
            Button(s("common.cancel"), role: .cancel) {}
            Button(s("common.save")) { controller.renameFolder(folderId, name: draftName) }
        }
        .confirmationDialog(s("folders.delete.confirm"), isPresented: $confirmDelete, titleVisibility: .visible) {
            Button(s("folders.delete"), role: .destructive) { controller.deleteFolder(folderId) }
        } message: {
            Text(verbatim: s("folders.delete.message"))
        }
        .sheet(isPresented: Binding(get: { controller.showIngest == folderId }, set: { if !$0 { controller.showIngest = nil } })) {
            IngestSheet(controller: controller, basket: controller.ingestBasket, folderId: folderId)
                .presentationDetents([.medium, .large])
        }
        .alert(s("exams.gen.title"), isPresented: Binding(get: { controller.genExamRequest != nil },
                                                         set: { if !$0 { controller.genExamRequest = nil } })) {
            TextField(s("exams.gen.note"), text: $examNote)
            Button(s("common.cancel"), role: .cancel) { controller.genExamRequest = nil }
            Button(s("exams.gen")) {
                if let r = controller.genExamRequest { controller.genExam(r, note: examNote) }
                examNote = ""
            }
        } message: {
            Text(verbatim: s("exams.gen.message"))
        }
    }
}

// MARK: Sessions tab

private struct FolderSessions: View {
    let controller: LiveController
    let folderId: String
    @Environment(\.strings) private var s
    @State private var renaming: SessionItem?
    @State private var draft = ""

    var body: some View {
        let items = controller.library.sessions(in: folderId)
        List {
            Section {
                Button { controller.newChat(in: folderId) } label: {
                    Label(s("chat.new"), systemImage: "square.and.pencil")
                        .font(TypeScale.body.weight(.medium))
                }
            }
            Section {
                if items.isEmpty {
                    Text(verbatim: s("folder.sessions.empty"))
                        .font(TypeScale.callout)
                        .foregroundStyle(Ink.secondary)
                }
                ForEach(items) { item in
                    Button { controller.open(item) } label: { SessionRow(item: item) }
                        .contextMenu { SessionMenu(controller: controller, item: item, rename: { renaming = item; draft = item.title }) }
                        .swipeActions {
                            Button(role: .destructive) { controller.deleteSession(item) } label: { Label(s("common.delete"), systemImage: "trash") }
                        }
                }
            }
        }
        .alert(s("sessions.rename"), isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField(s("sessions.rename"), text: $draft)
            Button(s("common.cancel"), role: .cancel) { renaming = nil }
            Button(s("common.save")) { if let r = renaming { controller.renameSession(r, title: draft) }; renaming = nil }
        }
    }
}

// MARK: Knowledge tab

private struct KnowledgeTab: View {
    let controller: LiveController
    let folderId: String
    @Environment(\.strings) private var s

    var body: some View {
        ZStack {
            WebPageView(page: controller.knowledgePage)
                .opacity(controller.knowledgePage.ready ? 1 : 0)
            if !controller.knowledgePage.ready {
                NativeKnowledge(controller: controller, folderId: folderId)
            }
        }
        .onAppear { controller.renderKnowledge(folderId) }
        .onChange(of: controller.knowledgePage.ready) { _, _ in controller.renderKnowledge(folderId) }
    }
}

/// Fallback when `knowledge.html` is not available: ingest progress, stats, curriculum and exam formats.
private struct NativeKnowledge: View {
    let controller: LiveController
    let folderId: String
    @Environment(\.strings) private var s

    var body: some View {
        List {
            if let ingest = controller.library.ingest[folderId], ingest.isActive || ingest.state == .error {
                Section { IngestProgressRow(controller: controller, status: ingest) }
            }
            if let k = controller.library.knowledge[folderId] {
                Section {
                    LabeledContent(s("knowledge.questions"), value: "\(k.questions)")
                    LabeledContent(s("knowledge.sources"), value: "\(k.sources)")
                    if !k.byLevel.isEmpty {
                        LabeledContent(s("knowledge.levels"),
                                       value: ["nb", "th", "vd", "vdc"].map { "\($0.uppercased()) \(k.byLevel[$0] ?? 0)" }.joined(separator: " · "))
                    }
                    if let current = k.currentTopicId.flatMap(k.topicTitle) {
                        LabeledContent(s("knowledge.current"), value: current)
                    }
                }
                Section(s("knowledge.structures")) {
                    ForEach(k.structures) { st in
                        HStack {
                            VStack(alignment: .leading, spacing: Space.xxs) {
                                Text(verbatim: st.title).font(TypeScale.body)
                                Text(verbatim: s("knowledge.structure.meta", st.questions, st.durationMin ?? 0))
                                    .font(TypeScale.caption).foregroundStyle(Ink.secondary)
                            }
                            Spacer()
                            Button(s("exams.gen")) {
                                controller.genExamRequest = GenExamRequest(folderId: folderId, structureId: st.id)
                            }
                            .buttonStyle(.bordered)
                        }
                    }
                }
                ForEach(k.chapters) { chapter in
                    Section(chapter.title) {
                        ForEach(chapter.topics) { topic in
                            LabeledContent(topic.title, value: "\(topic.count)")
                        }
                    }
                }
            } else if controller.library.knowledgeLoaded.contains(folderId) || !controller.connected {
                Section {
                    VStack(alignment: .leading, spacing: Space.s) {
                        Text(verbatim: s("knowledge.empty.title")).font(TypeScale.headline)
                        Text(verbatim: s("knowledge.empty.body")).font(TypeScale.callout).foregroundStyle(Ink.secondary)
                    }
                    .padding(.vertical, Space.xs)
                    Button { controller.showIngest = folderId } label: { Label(s("ingest.add"), systemImage: "doc.badge.plus") }
                }
            } else {
                ProgressView()
            }
        }
    }
}

struct IngestProgressRow: View {
    let controller: LiveController
    let status: IngestStatus
    @Environment(\.strings) private var s

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            HStack {
                Text(verbatim: s("ingest.state.\(status.state.rawValue)")).font(TypeScale.headline)
                Spacer()
                if status.total > 0 {
                    Text(verbatim: "\(status.done)/\(status.total)").font(TypeScale.caption.monospacedDigit()).foregroundStyle(Ink.secondary)
                }
            }
            if let f = status.fraction { ProgressView(value: f) } else { ProgressView().frame(maxWidth: .infinity, alignment: .leading) }
            if !status.note.isEmpty {
                Text(verbatim: status.note).font(TypeScale.callout).foregroundStyle(Ink.secondary)
            }
            if let q = status.foundQuestions {
                Text(verbatim: s("ingest.found", q, status.foundTopics ?? 0)).font(TypeScale.caption).foregroundStyle(Ink.tertiary)
            }
            if status.isActive {
                Button(s("ingest.cancel"), role: .destructive) { controller.cancelIngest(folder: status.folderId) }
                    .font(TypeScale.callout)
            }
        }
        .padding(.vertical, Space.xs)
    }
}

/// "Thêm đề": pick 10-30 photos or PDFs, they upload at once, then the server reads them.
struct IngestSheet: View {
    let controller: LiveController
    @Bindable var basket: ChatComposer
    let folderId: String
    @Environment(\.strings) private var s
    @Environment(\.dismiss) private var dismiss
    @State private var goal = ""
    @State private var showPhotos = false
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var showFiles = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(s("ingest.goal.placeholder"), text: $goal)
                } header: { Text(verbatim: s("ingest.goal")) } footer: { Text(verbatim: s("ingest.footer")) }
                Section {
                    Button { showPhotos = true } label: { Label(s("ingest.pickPhotos"), systemImage: "photo.on.rectangle") }
                    Button { showFiles = true } label: { Label(s("ingest.pickFiles"), systemImage: "doc") }
                }
                if !basket.attachments.isEmpty {
                    Section {
                        AttachmentStrip(composer: basket) { id in controller.retryUpload(id, in: basket) }
                        let done = basket.attachments.filter(\.isUploaded).count
                        ProgressView(value: Double(done), total: Double(basket.attachments.count)) {
                            Text(verbatim: s("ingest.uploaded", done, basket.attachments.count)).font(TypeScale.caption)
                        }
                    }
                }
            }
            .navigationTitle(s("ingest.add"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(s("common.cancel")) { basket.clear(); dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(s("ingest.start")) { controller.startIngest(folder: folderId, goal: goal) }
                        .disabled(basket.fileIds.isEmpty || basket.uploading)
                }
            }
            .photosPicker(isPresented: $showPhotos, selection: $photoItems,
                          maxSelectionCount: max(1, ChatComposer.maxAttachments - basket.attachments.count), matching: .images)
            .onChange(of: photoItems) { _, items in
                guard !items.isEmpty else { return }
                photoItems = []
                controller.addPhotos(items, namePrefix: "de-\(basket.attachments.count + 1)", to: basket)
            }
            .fileImporter(isPresented: $showFiles, allowedContentTypes: [.pdf, .image], allowsMultipleSelection: true) { result in
                switch result {
                case .success(let urls): controller.addFiles(urls, to: basket)
                case .failure(let error): Diagnostics.log("ingest: file importer failed: \(error.localizedDescription)")
                }
            }
        }
        .tint(Ink.primary)
        .onAppear { Diagnostics.log("ingest: sheet open (\(basket.attachments.count) files)") }
        .onDisappear { Diagnostics.log("ingest: sheet closed") }
        .onChange(of: showPhotos) { _, open in if open { Diagnostics.log("ingest: photo picker") } }
        .onChange(of: showFiles) { _, open in if open { Diagnostics.log("ingest: file importer") } }
    }
}

// MARK: Review tab

private struct ReviewTab: View {
    let controller: LiveController
    let folderId: String
    @Environment(\.strings) private var s

    var body: some View {
        List {
            Section {
                Button { controller.quickReview(folderId) } label: {
                    Label(s("review.quick"), systemImage: "bolt")
                        .font(TypeScale.body.weight(.semibold))
                }
            } footer: { Text(verbatim: s("review.quick.footer")) }

            if let r = controller.library.review[folderId] {
                if !r.topics.isEmpty {
                    Section(s("review.progress")) {
                        TopicBar(review: r)
                        ForEach(r.topics) { t in
                            HStack {
                                VStack(alignment: .leading, spacing: Space.xxs) {
                                    Text(verbatim: t.title).font(TypeScale.body)
                                    if !t.chapter.isEmpty { Text(verbatim: t.chapter).font(TypeScale.caption).foregroundStyle(Ink.tertiary) }
                                }
                                Spacer()
                                Text(verbatim: s("review.state.\(t.state.rawValue)"))
                                    .font(TypeScale.caption.weight(t.state == .weak ? .semibold : .regular))
                                    .foregroundStyle(t.state == .new ? Ink.tertiary : Ink.primary)
                            }
                        }
                    }
                }
                if !r.due.isEmpty {
                    Section(s("review.due")) {
                        ForEach(r.due) { d in TopicLine(topic: d.topic, note: d.note, trailing: DateText.short(d.due ?? "", lang: s.lang)) }
                    }
                }
                if !r.weak.isEmpty {
                    Section(s("progress.weak")) {
                        ForEach(r.weak) { w in TopicLine(topic: w.topic, note: w.note, trailing: s("progress.count", w.count)) }
                    }
                }
                Section(s("review.weekly")) {
                    if let w = r.weekly {
                        Text(NativeTranscript.markdown(w.md)).font(TypeScale.callout)
                    }
                    Button {
                        controller.requestWeekly(folderId)
                    } label: {
                        HStack {
                            Label(s(r.weekly == nil ? "review.weekly.make" : "review.weekly.again"), systemImage: "text.alignleft")
                            if controller.weeklyLoading.contains(folderId) { Spacer(); ProgressView() }
                        }
                    }
                    .disabled(controller.weeklyLoading.contains(folderId))
                }
                if !r.sessions.isEmpty {
                    Section(s("review.lessons")) {
                        ForEach(r.sessions) { lesson in
                            Button { controller.replay(lesson, folder: folderId) } label: {
                                HStack {
                                    VStack(alignment: .leading, spacing: Space.xxs) {
                                        Text(verbatim: lesson.title).font(TypeScale.body).foregroundStyle(Ink.primary)
                                        Text(verbatim: lesson.steps > 0 ? s("review.steps", lesson.steps) : s("review.chat"))
                                            .font(TypeScale.caption).foregroundStyle(Ink.secondary)
                                    }
                                    Spacer()
                                    Image(systemName: lesson.mode == .live ? "play.rectangle" : "text.bubble")
                                        .foregroundStyle(Ink.secondary)
                                }
                            }
                        }
                    }
                }
            } else {
                ProgressView()
            }
        }
    }
}

private struct TopicLine: View {
    let topic: String
    let note: String
    let trailing: String?
    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(verbatim: topic).font(TypeScale.body)
                if !note.isEmpty { Text(verbatim: note).font(TypeScale.caption).foregroundStyle(Ink.secondary) }
            }
            Spacer()
            if let trailing { Text(verbatim: trailing).font(TypeScale.caption.monospacedDigit()).foregroundStyle(Ink.tertiary) }
        }
    }
}

/// Topic states as one bar: solid, learning, weak, new (darker = further along).
private struct TopicBar: View {
    let review: FolderReview
    @Environment(\.strings) private var s

    var body: some View {
        let order: [FolderReview.Topic.State] = [.solid, .learning, .weak, .new]
        let total = max(1, review.topics.count)
        VStack(alignment: .leading, spacing: Space.s) {
            GeometryReader { geo in
                HStack(spacing: 2) {
                    ForEach(order, id: \.self) { state in
                        let n = review.count(state)
                        if n > 0 {
                            Rectangle().fill(Ink.primary.opacity(Self.alpha(state)))
                                .frame(width: max(4, (geo.size.width - 6) * CGFloat(n) / CGFloat(total)))
                        }
                    }
                }
            }
            .frame(height: 8)
            .clipShape(.capsule)
            HStack(spacing: Space.m) {
                ForEach(order, id: \.self) { state in
                    HStack(spacing: Space.xs) {
                        Circle().fill(Ink.primary.opacity(Self.alpha(state))).frame(width: 8, height: 8)
                        Text(verbatim: "\(s("review.state.\(state.rawValue)")) \(review.count(state))")
                            .font(TypeScale.caption).foregroundStyle(Ink.secondary)
                    }
                }
            }
        }
        .padding(.vertical, Space.xs)
        .accessibilityElement(children: .combine)
    }

    static func alpha(_ state: FolderReview.Topic.State) -> Double {
        switch state {
        case .solid: return 1
        case .learning: return 0.55
        case .weak: return 0.3
        case .new: return 0.1
        }
    }
}

// MARK: Exams tab

private struct ExamsTab: View {
    let controller: LiveController
    let folderId: String
    @Environment(\.strings) private var s

    var body: some View {
        List {
            if let job = controller.library.examJobs[folderId], job.isActive {
                Section {
                    VStack(alignment: .leading, spacing: Space.s) {
                        HStack {
                            Text(verbatim: s("exams.state.\(job.state.rawValue)")).font(TypeScale.headline)
                            Spacer()
                            ProgressView()
                        }
                        if !job.note.isEmpty { Text(verbatim: job.note).font(TypeScale.callout).foregroundStyle(Ink.secondary) }
                    }
                    .padding(.vertical, Space.xs)
                }
            }
            if let k = controller.library.knowledge[folderId], !k.structures.isEmpty {
                Section {
                    Menu {
                        ForEach(k.structures) { st in
                            Button(st.title) { controller.genExamRequest = GenExamRequest(folderId: folderId, structureId: st.id) }
                        }
                    } label: {
                        Label(s("exams.new"), systemImage: "plus")
                            .font(TypeScale.body.weight(.semibold))
                    }
                }
            }
            Section {
                let items = controller.library.exams[folderId] ?? []
                if items.isEmpty {
                    Text(verbatim: s("exams.empty")).font(TypeScale.callout).foregroundStyle(Ink.secondary)
                }
                ForEach(items) { exam in
                    Button { Task { await controller.exportExam(exam.examId, title: exam.title) } } label: {
                        HStack {
                            Image(systemName: "doc.richtext").foregroundStyle(Ink.secondary).frame(width: 24)
                            VStack(alignment: .leading, spacing: Space.xxs) {
                                Text(verbatim: exam.title).font(TypeScale.body).foregroundStyle(Ink.primary)
                                Text(verbatim: s("exams.meta", exam.questions, DateText.short(exam.createdAt, lang: s.lang) ?? ""))
                                    .font(TypeScale.caption).foregroundStyle(Ink.secondary)
                            }
                            Spacer()
                            if controller.exporting == exam.examId { ProgressView() }
                            else { Image(systemName: "square.and.arrow.up").foregroundStyle(Ink.tertiary) }
                        }
                        .frame(minHeight: 44)
                    }
                    .disabled(controller.exporting != nil)
                    .swipeActions {
                        Button(role: .destructive) { controller.deleteExam(exam) } label: { Label(s("common.delete"), systemImage: "trash") }
                    }
                }
            } header: { Text(verbatim: s("exams.list")) } footer: { Text(verbatim: s("exams.footer")) }
        }
    }
}
