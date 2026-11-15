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
        .offset(y: drag >= 0 ? drag : Motion.rubberBand(drag, limit: 28))
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
        .padding(.bottom, Space.s)
        .contentShape(.rect)
        // Only a vertical drag on the header collapses the panel; horizontal swipes belong to the board's
        // step paging (web scroll-snap) and are never taken here.
        // The panel follows the finger (rubber band upwards); a flick or a long pull closes it, carrying the
        // finger's speed into the spring, otherwise it springs home.
        .gesture(
            DragGesture(minimumDistance: 12, coordinateSpace: .global)
                .onChanged { v in
                    guard abs(v.translation.height) > abs(v.translation.width) else { return }
                    var t = Transaction()
                    t.disablesAnimations = true
                    withTransaction(t) { drag = v.translation.height }
                }
                .onEnded { v in
                    let vertical = abs(v.translation.height) > abs(v.translation.width)
                    let close = vertical && (v.translation.height > 120 || v.translation.height + v.velocity.height * 0.2 > 240)
                    if close {
                        withAnimation(Motion.release(velocity: v.velocity.height, distance: 600)) {
                            controller.boardVisible = false
                            drag = 0
                        }
                    } else {
                        withAnimation(Motion.release(velocity: v.velocity.height, distance: -max(1, drag))) { drag = 0 }
                    }
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
        .animation(Motion.snappy, value: current)
        .animation(Motion.snappy, value: count)
        .accessibilityElement()
        .accessibilityLabel(s("board.step", current + 1, count))
    }
}
