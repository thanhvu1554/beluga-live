import SwiftUI
import BelugaCore

/// Smooths a raw audio level into the value that drives the glow: fast attack (~60 ms), slow release
/// (~250 ms). It also keeps the phase of the soft ripples, which only advances while there is sound,
/// so a silent pill is completely still.
@MainActor
final class GlowModel {
    private(set) var level: CGFloat = 0
    private(set) var phase: CGFloat = 0
    private var last: Date?

    func update(raw: Float, at date: Date) {
        let dt = last.map { min(0.1, max(0, date.timeIntervalSince($0))) } ?? 0
        last = date
        let target = CGFloat(raw)
        let tau = target > level ? 0.06 : 0.25
        let alpha = dt > 0 ? CGFloat(1 - exp(-dt / tau)) : 0
        level += (target - level) * alpha
        if target == 0, level < 0.01 { level = 0 }
        if level > 0.03 { phase += CGFloat(dt) * (1.2 + 5 * level) }
    }

    func reset() {
        level = 0
        last = nil
    }
}

/// The voice pill. Outer layer: system Liquid Glass (dark, in both appearances so the light reads on any
/// camera image). Inner layer: a soft glow rising from the bottom edge, clipped to the capsule. Height and
/// brightness follow the real, smoothed audio level; thinking, tool and asking have their own quiet shapes.
struct PillView: View {
    let controller: LiveController
    @Environment(\.strings) private var s
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var glow = GlowModel()
    @State private var pressing = false

    private var state: PillState { controller.pillState }

    /// One soft-edged vertical ramp, reused every frame (opacity is applied on the layer).
    private static let ramp = Gradient(stops: [
        .init(color: Palette.glow.opacity(0), location: 0),
        .init(color: Palette.glow.opacity(0.45), location: 0.55),
        .init(color: Palette.glow.opacity(1), location: 1),
    ])

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60, paused: !animates)) { context in
            let date = context.date
            let _ = glow.update(raw: audioDriven ? controller.level(at: date) : 0, at: date)
            ZStack {
                // Dark base so the light reads on any camera image, in both appearances.
                Capsule().fill(Palette.scrim.opacity(0.5))

                Canvas { ctx, size in
                    draw(in: &ctx, size: size, date: date)
                }
                .allowsHitTesting(false)

                outline

                // What the pill is doing, or what to do with it: a glyph and a few words, centred. While sound
                // drives the glow (listening, speaking) the light says it all and the words step aside.
                if symbol != nil || label != nil {
                    HStack(spacing: Space.s) {
                        if let symbol {
                            Image(systemName: symbol)
                                .font(.system(size: 14, weight: .semibold))
                                .contentTransition(.symbolEffect(.replace))
                        }
                        if let label {
                            Text(verbatim: label)
                                .font(TypeScale.callout.weight(.medium))
                                .lineLimit(1)
                                .minimumScaleFactor(0.8)
                                .contentTransition(.opacity)
                        }
                    }
                    .foregroundStyle(Palette.glow.opacity(state == .idle ? 0.72 : 0.9))
                    .padding(.horizontal, Space.l)
                    .transition(.opacity.combined(with: .scale(0.92)))
                }
            }
            .frame(width: Size.pillWidth, height: Size.pillHeight)
            .clipShape(.capsule)
            .animation(Motion.snappy, value: state)
        }
        .scaleEffect(pressing ? 0.96 : 1)
        .animation(Motion.snappy, value: pressing)
        .glassEffect(.regular.tint(Palette.scrim.opacity(0.6)).interactive(), in: .capsule)
        .environment(\.colorScheme, .dark)
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
        .onChange(of: state) { _, _ in glow.reset() }
        .accessibilityElement()
        .accessibilityLabel(s("pill.a11y.\(state.rawValue)"))
        .accessibilityHint(s(controller.settings.handsFree ? "pill.hint.handsFree" : "pill.hint.hold"))
        .accessibilityAddTraits(.isButton)
    }

    /// Edge of the capsule: always there; dashed when offline, brighter while the pill is live.
    @ViewBuilder private var outline: some View {
        switch state {
        case .offline:
            Capsule().strokeBorder(Palette.glow.opacity(0.5),
                                   style: StrokeStyle(lineWidth: Stroke.thin, dash: [4, 5]))
        case .idle:
            Capsule().strokeBorder(Palette.glow.opacity(0.28), lineWidth: Stroke.hairline)
        default:
            Capsule().strokeBorder(Palette.glow.opacity(0.18), lineWidth: Stroke.hairline)
        }
    }

    /// Levels come from the microphone or the playing audio; everything else is drawn from the clock.
    private var audioDriven: Bool { state == .listening || state == .speaking }

    private var animates: Bool {
        switch state {
        case .idle, .offline, .asking: return false
        case .listening, .speaking: return true
        case .thinking, .tool, .done: return true
        }
    }

    private var symbol: String? {
        switch state {
        case .idle: return controller.settings.handsFree ? "hand.tap" : "mic"
        case .asking: return "mic"
        case .offline: return "wifi.slash"
        case .done: return "checkmark"
        default: return nil
        }
    }

    private var label: String? {
        switch state {
        case .idle: return s(controller.settings.handsFree ? "pill.idle.handsFree" : "pill.idle.hold")
        case .asking: return s("pill.a11y.asking")
        case .offline: return s("pill.idle.offline")
        case .done: return s("pill.a11y.done")
        default: return nil
        }
    }

    // MARK: Drawing

    private func draw(in ctx: inout GraphicsContext, size: CGSize, date: Date) {
        let since = date.timeIntervalSince(controller.pill.enteredAt)
        let h = size.height

        switch state {
        case .idle, .offline:
            break

        case .listening, .speaking:
            let level = glow.level
            guard level > 0.02 else { break }
            // Height and brightness follow the level; the top edge ripples only while there is sound.
            let height = h * 0.95 * pow(level, 0.8)
            let amp: CGFloat = reduceMotion ? 0 : min(5, 1.5 + 6 * level)
            band(&ctx, size, height: height, opacity: Double(min(1, level * 1.7)),
                 amp: amp, phase: glow.phase, dome: state == .speaking)

        case .thinking:
            // The only state that moves without sound: a slow breathing band (2.4 s).
            let breath = reduceMotion ? 0.5 : (sin(since * 2 * .pi / 2.4 - .pi / 2) + 1) / 2
            band(&ctx, size, height: h * 0.36, opacity: 0.22 + 0.5 * breath)

        case .tool:
            // A streak sweeping across once per 1.4 s cycle.
            let t = reduceMotion ? 0.5 : since.truncatingRemainder(dividingBy: 1.4) / 1.4
            let eased = 0.5 - cos(t * .pi) / 2
            let x = -size.width * 0.2 + size.width * 1.4 * eased
            let blob = Path(ellipseIn: CGRect(x: x - size.width * 0.25, y: h * 0.62, width: size.width * 0.5, height: h * 0.6))
            ctx.drawLayer { layer in
                layer.opacity = 0.8
                layer.addFilter(.blur(radius: 10))
                layer.fill(blob, with: .color(Palette.glow))
            }

        case .asking:
            // Still, faintly lit from below: an invitation to speak.
            band(&ctx, size, height: h * 0.3, opacity: 0.45)

        case .done:
            // The glow closes down once.
            let t = min(1, since / 0.9)
            band(&ctx, size, height: h * 0.55 * (1 - t), opacity: 0.8 * (1 - t))
        }
    }

    /// A glow band rising from the bottom edge: soft top edge, brightest at the base.
    private func band(_ ctx: inout GraphicsContext, _ size: CGSize, height: CGFloat, opacity: Double,
                      amp: CGFloat = 0, phase: CGFloat = 0, dome: Bool = false) {
        guard height > 0.5, opacity > 0.01 else { return }
        let w = size.width, h = size.height
        var path = Path()
        let steps = 24
        path.move(to: CGPoint(x: -12, y: h + 12))
        for i in 0...steps {
            let u = CGFloat(i) / CGFloat(steps)
            // Speaking is a dome (louder in the middle); listening is flat.
            let envelope: CGFloat = dome ? 0.55 + 0.45 * sin(.pi * u) : 1
            let ripple = amp * (0.6 * sin(2 * .pi * 1.3 * u + phase) + 0.4 * sin(2 * .pi * 2.1 * u - 1.4 * phase))
            path.addLine(to: CGPoint(x: -12 + (w + 24) * u, y: h - height * envelope - ripple))
        }
        path.addLine(to: CGPoint(x: w + 12, y: h + 12))
        path.closeSubpath()

        let top = CGPoint(x: 0, y: h - height - amp)
        let bottom = CGPoint(x: 0, y: h)
        ctx.drawLayer { layer in
            layer.opacity = opacity
            layer.addFilter(.blur(radius: 7))
            layer.fill(path, with: .linearGradient(Self.ramp, startPoint: top, endPoint: bottom))
        }
        // A tighter, brighter core at the base.
        let coreTop = CGPoint(x: 0, y: h - height * 0.5)
        ctx.drawLayer { layer in
            layer.opacity = opacity * 0.85
            layer.addFilter(.blur(radius: 3))
            layer.fill(path, with: .linearGradient(Self.ramp, startPoint: coreTop, endPoint: bottom))
        }
    }
}
