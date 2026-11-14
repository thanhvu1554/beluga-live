import SwiftUI
import BelugaCore

struct SessionsView: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Button { controller.newSession() } label: {
                        Label(s("sessions.new"), systemImage: "plus")
                            .font(TypeScale.body.weight(.medium))
                    }
                }
                Section {
                    if controller.sessions.isEmpty {
                        Text(verbatim: s("sessions.empty"))
                            .font(TypeScale.callout)
                            .foregroundStyle(Ink.secondary)
                    }
                    ForEach(controller.sessions) { item in
                        Button { controller.open(item) } label: {
                            SessionRow(item: item)
                        }
                    }
                } header: {
                    Text(verbatim: s("sessions.recent"))
                }
            }
            .navigationTitle(s("sessions.title"))
            .navigationBarTitleDisplayMode(.inline)
        }
        .tint(Ink.primary)
    }
}

private struct SessionRow: View {
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
                if parts.count > 1 {
                    Text(verbatim: parts[0])
                        .font(TypeScale.caption)
                        .foregroundStyle(Ink.secondary)
                }
            }
            Spacer(minLength: Space.s)
            Text(verbatim: Self.format(item.updatedAt, lang: s.lang))
                .font(TypeScale.caption.monospacedDigit())
                .foregroundStyle(Ink.tertiary)
        }
        .padding(.vertical, Space.xxs)
    }

    static func format(_ raw: String, lang: Lang) -> String {
        let iso = ISO8601DateFormatter()
        var date = iso.date(from: raw)
        if date == nil, let ms = Double(raw) { date = Date(timeIntervalSince1970: ms > 1e11 ? ms / 1000 : ms) }
        guard let date else { return raw }
        let f = DateFormatter()
        f.locale = Locale(identifier: lang == .vi ? "vi_VN" : "en_US")
        f.setLocalizedDateFormatFromTemplate("dMMM")
        return f.string(from: date)
    }
}
