import SwiftUI
import BelugaCore

/// Plain native transcript, used until (or when) the web page `chat.html` is not ready. Markdown is shown with
/// the system's inline renderer; math stays as TeX source.
struct NativeTranscript: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: Space.l) {
                    ForEach(controller.transcript.items) { item in
                        row(item).id(item.id)
                    }
                    if controller.transcript.busy {
                        Text(verbatim: s("chat.thinking"))
                            .font(TypeScale.callout)
                            .foregroundStyle(Ink.tertiary)
                            .id(-1)
                    }
                }
                .padding(.horizontal, Space.l)
                .padding(.vertical, Space.l)
            }
            .onChange(of: controller.transcript.items.count) { _, _ in
                if let last = controller.transcript.items.last { proxy.scrollTo(last.id, anchor: .bottom) }
            }
        }
    }

    @ViewBuilder private func row(_ item: ChatTranscript.Item) -> some View {
        switch item.kind {
        case let .user(text, files):
            VStack(alignment: .trailing, spacing: Space.xs) {
                if !files.isEmpty {
                    HStack(spacing: Space.xs) {
                        ForEach(files, id: \.fileId) { f in
                            Label { Text(verbatim: f.name) } icon: { Image(systemName: f.kind == "pdf" ? "doc.text" : "photo") }
                                .font(TypeScale.caption)
                                .lineLimit(1)
                                .padding(.horizontal, Space.s)
                                .padding(.vertical, Space.xs)
                                .background(Capsule().fill(Ink.hairline))
                        }
                    }
                }
                Text(verbatim: text)
                    .font(TypeScale.body)
                    .padding(.horizontal, Space.m)
                    .padding(.vertical, Space.s)
                    .background(RoundedRectangle(cornerRadius: Radius.l).fill(Ink.hairline))
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, Space.xxxl)
        case let .text(md, _):
            Text(Self.markdown(md))
                .font(TypeScale.body)
                .textSelection(.enabled)
        case .block(let b):
            Text(verbatim: b["tex"]?.string ?? b["md"]?.string ?? b["fallback"]?.string ?? "")
                .font(.system(.body, design: .monospaced))
                .padding(Space.m)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: Radius.m).stroke(Ink.hairline))
        case .heading(let t):
            Text(verbatim: t).font(TypeScale.headline)
        case .spoken(let t):
            Text(verbatim: t).font(TypeScale.body).foregroundStyle(Ink.secondary)
        case .ask(let prompt):
            Text(verbatim: prompt).font(TypeScale.body.weight(.semibold))
        case .done:
            Text(verbatim: s("board.done")).font(TypeScale.caption).foregroundStyle(Ink.tertiary)
        }
    }

    static func markdown(_ md: String) -> AttributedString {
        (try? AttributedString(markdown: md, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(md)
    }
}
