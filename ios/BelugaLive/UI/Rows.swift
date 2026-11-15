import SwiftUI

/// The one list row of the app (drawer, folder tabs, model list, review lessons, exams): an optional glyph in a
/// fixed 24 pt column, a body title, an optional caption line, and trailing detail. Every list therefore has the
/// same type sizes and the same row heights (44 pt with one line, 52 pt with two).
struct RowLabel<Trailing: View>: View {
    var symbol: String?
    let title: String
    var subtitle: String?
    var titleLines = 1
    var emphasized = false
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        HStack(spacing: Space.m) {
            if let symbol {
                Image(systemName: symbol)
                    .font(.system(size: 16, weight: .regular))
                    .foregroundStyle(Ink.secondary)
                    .frame(width: 24)
            }
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(verbatim: title)
                    .font(emphasized ? TypeScale.body.weight(.semibold) : TypeScale.body)
                    .foregroundStyle(Ink.primary)
                    .lineLimit(titleLines)
                if let subtitle, !subtitle.isEmpty {
                    Text(verbatim: subtitle)
                        .font(TypeScale.caption)
                        .foregroundStyle(Ink.secondary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: Space.s)
            trailing()
        }
        .frame(minHeight: subtitle == nil ? Size.tap : Size.row)
        .contentShape(.rect)
    }
}

extension RowLabel where Trailing == EmptyView {
    init(symbol: String? = nil, title: String, subtitle: String? = nil, titleLines: Int = 1, emphasized: Bool = false) {
        self.init(symbol: symbol, title: title, subtitle: subtitle, titleLines: titleLines, emphasized: emphasized) { EmptyView() }
    }
}

/// Small trailing text in a row (dates, counts): caption, tabular digits, tertiary ink.
struct RowMeta: View {
    let text: String
    var body: some View {
        Text(verbatim: text)
            .font(TypeScale.caption.monospacedDigit())
            .foregroundStyle(Ink.tertiary)
    }
}

/// An empty or waiting state inside a list section: a headline and one line saying how to fill it.
struct StateBlock: View {
    let title: String
    var text: String?
    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            Text(verbatim: title).font(TypeScale.headline).foregroundStyle(Ink.primary)
            if let text { Text(verbatim: text).font(TypeScale.callout).foregroundStyle(Ink.secondary) }
        }
        .padding(.vertical, Space.xs)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
