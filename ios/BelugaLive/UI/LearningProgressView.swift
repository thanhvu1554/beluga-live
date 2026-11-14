import SwiftUI
import BelugaCore

/// Read-only learning progress: what is shaky, what is solid, what was studied, what teaching style works.
/// Flat typography on the system background, no cards.
struct LearningProgressView: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Space.xxl) {
                if let progress = controller.progress {
                    if progress.isEmpty {
                        Message(title: s("progress.empty.title"), text: s("progress.empty.body"))
                    } else {
                        ForEach(progress.subjects.filter { !$0.isEmpty }) { subject in
                            SubjectSection(subject: subject)
                        }
                        if !progress.style.isEmpty {
                            StyleSection(lines: progress.style)
                        }
                        if let updated = progress.updatedAt.flatMap({ DateText.short($0, lang: s.lang) }) {
                            Text(verbatim: s("progress.updated", updated))
                                .font(TypeScale.caption)
                                .foregroundStyle(Ink.tertiary)
                        }
                    }
                } else {
                    Message(title: nil, text: s(controller.connected ? "progress.loading" : "progress.offline"))
                }
            }
            .padding(.horizontal, Space.xl)
            .padding(.vertical, Space.l)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Color(uiColor: .systemBackground))
        .navigationTitle(s("progress.title"))
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { controller.requestProgress() }
    }
}

private struct Message: View {
    let title: String?
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            if let title {
                Text(verbatim: title).font(TypeScale.headline).foregroundStyle(Ink.primary)
            }
            Text(verbatim: text).font(TypeScale.callout).foregroundStyle(Ink.secondary)
        }
        .padding(.top, Space.xl)
    }
}

/// Small uppercase label above a group of rows.
private struct GroupLabel: View {
    let text: String
    var body: some View {
        Text(verbatim: text)
            .font(TypeScale.micro)
            .tracking(0.8)
            .textCase(.uppercase)
            .foregroundStyle(Ink.tertiary)
            .padding(.bottom, Space.xs)
    }
}

private struct Rule: View {
    var body: some View {
        Rectangle().fill(Ink.hairline).frame(height: Stroke.hairline)
    }
}

private struct SubjectSection: View {
    let subject: SubjectProgress
    @Environment(\.strings) private var s

    var body: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            Text(verbatim: subject.name)
                .font(TypeScale.title)
                .foregroundStyle(Ink.primary)

            if !subject.weakTopics.isEmpty {
                group(s("progress.weak")) {
                    ForEach(subject.weakTopics) { WeakRow(item: $0) }
                }
            }
            if !subject.mastered.isEmpty {
                group(s("progress.mastered")) {
                    ForEach(subject.mastered) { DatedRow(item: $0) }
                }
            }
            if !subject.recent.isEmpty {
                group(s("progress.recent")) {
                    ForEach(subject.recent) { DatedRow(item: $0) }
                }
            }
        }
    }

    private func group<Content: View>(_ label: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            GroupLabel(text: label)
            Rule()
            content()
        }
    }
}

private struct WeakRow: View {
    let item: WeakTopic
    @Environment(\.strings) private var s

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(verbatim: item.topic)
                    .font(TypeScale.body)
                    .foregroundStyle(Ink.primary)
                if !item.note.isEmpty {
                    Text(verbatim: item.note)
                        .font(TypeScale.callout)
                        .foregroundStyle(Ink.secondary)
                }
                facts
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, Space.m)
            Rule()
        }
    }

    private var facts: some View {
        HStack(spacing: Space.s) {
            if item.count > 0 {
                Text(verbatim: item.count == 1 ? s("progress.count.one") : s("progress.count", item.count))
            }
            if LearningDate.isDue(item.due) {
                if item.count > 0 { Text(verbatim: "·") }
                Text(verbatim: s("progress.dueNow")).fontWeight(.semibold).foregroundStyle(Ink.primary)
            } else if let due = item.due.flatMap({ DateText.short($0, lang: s.lang) }) {
                if item.count > 0 { Text(verbatim: "·") }
                Text(verbatim: s("progress.review", due))
            }
        }
        .font(TypeScale.caption.monospacedDigit())
        .foregroundStyle(Ink.secondary)
        .padding(.top, Space.xxs)
    }
}

private struct DatedRow: View {
    let item: DatedTopic
    @Environment(\.strings) private var s

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: Space.m) {
                Text(verbatim: item.topic)
                    .font(TypeScale.body)
                    .foregroundStyle(Ink.primary)
                Spacer(minLength: Space.s)
                if let date = item.date.flatMap({ DateText.short($0, lang: s.lang) }) {
                    Text(verbatim: date)
                        .font(TypeScale.caption.monospacedDigit())
                        .foregroundStyle(Ink.secondary)
                }
            }
            .padding(.vertical, Space.m)
            Rule()
        }
    }
}

private struct StyleSection: View {
    let lines: [String]
    @Environment(\.strings) private var s

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(verbatim: s("progress.style"))
                .font(TypeScale.headline)
                .foregroundStyle(Ink.primary)
                .padding(.bottom, Space.m)
            Rule()
            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                VStack(spacing: 0) {
                    Text(verbatim: line)
                        .font(TypeScale.body)
                        .foregroundStyle(Ink.primary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, Space.m)
                    Rule()
                }
            }
        }
    }
}

/// Short calendar dates for server date texts ("12 thg 11"); unreadable text is returned as is.
enum DateText {
    static func short(_ raw: String, lang: Lang) -> String? {
        guard !raw.isEmpty else { return nil }
        guard let date = LearningDate.parse(raw) else { return raw }
        let f = DateFormatter()
        f.locale = Locale(identifier: lang == .vi ? "vi_VN" : "en_US")
        f.setLocalizedDateFormatFromTemplate("dMMM")
        return f.string(from: date)
    }
}
