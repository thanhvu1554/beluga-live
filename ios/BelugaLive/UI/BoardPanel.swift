import SwiftUI

/// The lesson board sheet: ~60% of the screen, camera still visible above.
struct BoardPanel: View {
    let controller: LiveController
    @Environment(\.strings) private var s
    @State private var drag: CGFloat = 0

    var body: some View {
        VStack(spacing: 0) {
            header
            BoardWebView(bridge: controller.board)
        }
        .background(Color(uiColor: .systemBackground))
        .clipShape(UnevenRoundedRectangle(topLeadingRadius: Radius.panel, topTrailingRadius: Radius.panel))
        .overlay(alignment: .top) {
            UnevenRoundedRectangle(topLeadingRadius: Radius.panel, topTrailingRadius: Radius.panel)
                .stroke(Ink.hairline, lineWidth: Stroke.hairline)
                .allowsHitTesting(false)
        }
        .offset(y: max(0, drag))
    }

    private var header: some View {
        VStack(spacing: Space.s) {
            Capsule()
                .fill(Ink.faint)
                .frame(width: 36, height: 5)
                .padding(.top, Space.s)
            HStack(alignment: .firstTextBaseline, spacing: Space.s) {
                Text(verbatim: controller.title.isEmpty ? s("board.untitled") : controller.title)
                    .font(TypeScale.caption.weight(.semibold))
                    .foregroundStyle(Ink.secondary)
                    .lineLimit(1)
                Spacer(minLength: Space.s)
                if controller.board.stepCount > 0 {
                    StepTicks(count: controller.board.stepCount, current: controller.board.currentStep) {
                        controller.board.goto($0)
                    }
                }
            }
            .padding(.horizontal, Space.xl)
        }
        .padding(.bottom, Space.xs)
        .contentShape(.rect)
        .gesture(
            DragGesture()
                .onChanged { drag = $0.translation.height }
                .onEnded { v in
                    if v.translation.height > 120 || v.predictedEndTranslation.height > 240 {
                        controller.showBoard(false)
                    }
                    withAnimation(Motion.panel) { drag = 0 }
                }
        )
    }
}

/// Step position as short ticks; the current one is longer and darker.
private struct StepTicks: View {
    let count: Int
    let current: Int
    let select: (Int) -> Void
    @Environment(\.strings) private var s

    var body: some View {
        HStack(spacing: Space.xs) {
            ForEach(0..<count, id: \.self) { i in
                Capsule()
                    .fill(i == current ? Ink.primary : Ink.faint)
                    .frame(width: i == current ? 16 : 6, height: 3)
                    .contentShape(.rect.inset(by: -Space.s))
                    .onTapGesture { select(i) }
            }
        }
        .animation(.easeOut(duration: Motion.quick), value: current)
        .accessibilityElement()
        .accessibilityLabel(s("board.step", current + 1, count))
    }
}
