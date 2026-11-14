import SwiftUI
import BelugaCore

/// Rolling history of real audio levels so the bars show what was actually heard/played.
@MainActor
final class LevelHistory {
    private(set) var values: [Float]
    private var last: Date = .distantPast
    private var smoothed: Float = 0

    init(count: Int) { values = Array(repeating: 0, count: count) }

    func push(_ level: Float, at date: Date) {
        guard date.timeIntervalSince(last) >= 1.0 / 30 else { return }
        last = date
        // Fast attack, slower release: reads as voice, not jitter.
        smoothed = level > smoothed ? smoothed + (level - smoothed) * 0.6 : smoothed + (level - smoothed) * 0.25
        values.removeFirst()
        values.append(smoothed)
    }

    func clear() {
        smoothed = 0
        for i in values.indices { values[i] = 0 }
    }
}

/// The voice pill: one glass capsule whose single drawing changes per state.
struct PillView: View {
    let controller: LiveController
    @Environment(\.strings) private var s
    @State private var history = LevelHistory(count: 23)
    @State private var pressing = false

    private var state: PillState { controller.pillState }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60, paused: !animates)) { context in
            let date = context.date
            let _ = history.push(controller.level(at: date), at: date)
            HStack(spacing: Space.s) {
                if let symbol {
                    Image(systemName: symbol)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(Ink.secondary)
                        .contentTransition(.symbolEffect(.replace))
                }
                Canvas { ctx, size in
                    draw(in: &ctx, size: size, date: date)
                }
                .frame(maxWidth: .infinity)
            }
            .padding(.horizontal, Space.l)
            .frame(width: Size.pillWidth, height: Size.pillHeight)
        }
        .scaleEffect(pressing ? 0.96 : 1)
        .animation(.spring(response: 0.25, dampingFraction: 0.8), value: pressing)
        .glassEffect(.regular.interactive(), in: .capsule)
        .contentShape(.capsule)
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { _ in
                    guard !pressing else { return }
                    pressing = true
                    controller.pillPressBegan()
                }
                .onEnded { _ in
                    pressing = false
                    controller.pillPressEnded()
                }
        )
        .onChange(of: state) { _, _ in history.clear() }
        .accessibilityElement()
        .accessibilityLabel(s("pill.a11y.\(state.rawValue)"))
        .accessibilityHint(s(controller.settings.handsFree ? "pill.hint.handsFree" : "pill.hint.hold"))
        .accessibilityAddTraits(.isButton)
    }

    private var animates: Bool {
        switch state {
        case .idle, .offline, .asking: return false
        default: return true
        }
    }

    private var symbol: String? {
        switch state {
        case .asking: return "mic"
        case .offline: return "wifi.slash"
        case .done: return "checkmark"
        default: return nil
        }
    }

    private func draw(in ctx: inout GraphicsContext, size: CGSize, date: Date) {
        let midY = size.height / 2
        let w = size.width
        let ink = GraphicsContext.Shading.color(.primary)
        let faint = GraphicsContext.Shading.color(.primary.opacity(0.18))
        let since = date.timeIntervalSince(controller.pill.enteredAt)

        func line(from x0: CGFloat, to x1: CGFloat, shading: GraphicsContext.Shading, width: CGFloat = Stroke.regular, dash: [CGFloat] = []) {
            var p = Path()
            p.move(to: CGPoint(x: x0, y: midY))
            p.addLine(to: CGPoint(x: x1, y: midY))
            ctx.stroke(p, with: shading, style: StrokeStyle(lineWidth: width, lineCap: .round, dash: dash))
        }

        switch state {
        case .idle:
            line(from: w * 0.3, to: w * 0.7, shading: .color(.primary.opacity(0.55)))

        case .asking:
            // Still, open: a line with a gap, inviting an answer.
            line(from: 0, to: w * 0.4, shading: .color(.primary.opacity(0.7)))
            line(from: w * 0.6, to: w, shading: .color(.primary.opacity(0.7)))

        case .offline:
            line(from: w * 0.15, to: w * 0.85, shading: .color(.primary.opacity(0.4)), width: Stroke.thin, dash: [3, 5])

        case .thinking:
            // Slow breathing: length and weight follow a 2.4 s cycle.
            let phase = (sin(since * 2 * .pi / 2.4 - .pi / 2) + 1) / 2
            let half = w * (0.12 + 0.16 * phase)
            line(from: w / 2 - half, to: w / 2 + half,
                 shading: .color(.primary.opacity(0.35 + 0.5 * phase)), width: Stroke.regular + CGFloat(phase))

        case .tool:
            // Sweep across a faint baseline.
            line(from: 0, to: w, shading: faint, width: Stroke.thin)
            let t = (since.truncatingRemainder(dividingBy: 1.1)) / 1.1
            let eased = 0.5 - cos(t * .pi) / 2
            let seg = w * 0.22
            let x = -seg + (w + seg) * eased
            line(from: max(0, x), to: min(w, x + seg), shading: ink, width: Stroke.bold)

        case .listening, .speaking:
            let values = history.values
            let n = values.count
            let gap = w / CGFloat(n)
            let barW: CGFloat = state == .listening ? Stroke.bold : Stroke.regular
            for (i, v) in values.enumerated() {
                let x = gap * (CGFloat(i) + 0.5)
                let level: CGFloat
                if state == .speaking {
                    // Output: symmetric around the centre, newest in the middle.
                    let k = abs(i - n / 2)
                    let idx = max(0, n - 1 - k * 2)
                    level = CGFloat(values[min(n - 1, idx)])
                } else {
                    // Mic: scrolling history, newest on the right.
                    level = CGFloat(v)
                }
                let h = max(barW, (size.height - Space.s) * level)
                var p = Path()
                p.move(to: CGPoint(x: x, y: midY - h / 2))
                p.addLine(to: CGPoint(x: x, y: midY + h / 2))
                ctx.stroke(p, with: .color(.primary.opacity(state == .listening ? 0.9 : 0.75)),
                           style: StrokeStyle(lineWidth: barW, lineCap: .round))
            }

        case .done:
            // Close to a point, then reopen to the idle line.
            let t = min(1, since / 1.2)
            let closing = t < 0.5 ? 1 - t * 2 : (t - 0.5) * 2
            let half = max(1.5, w * 0.2 * closing)
            line(from: w / 2 - half, to: w / 2 + half, shading: ink, width: Stroke.bold)
        }
    }
}
