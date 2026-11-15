import SwiftUI
import AVKit
import BelugaCore

/// The call screen: a calling app's face on the live session. The lesson is the "remote video" (step, content,
/// spoken line, glowing while the tutor talks), the camera is the self-view tile (ringed while the student talks),
/// and a row of round controls sits at the bottom. Dark in both appearances, like the pill and the camera chrome.
/// The stage is also PiP's source view: leaving the app turns it into the PiP window.
struct CallView: View {
    @Bindable var controller: LiveController
    @Environment(\.strings) private var s
    @Environment(\.colorScheme) private var outerScheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .bottom) {
                CallBackdrop()
                    .ignoresSafeArea()

                VStack(spacing: Space.l) {
                    header
                        .opacity(shown ? 1 : 0)
                    if let notice = controller.notice {
                        Text(verbatim: notice)
                            .font(TypeScale.caption)
                            .foregroundStyle(CallInk.primary)
                            .lineLimit(2)
                            .padding(.horizontal, Space.m)
                            .padding(.vertical, Space.s)
                            .glassEffect(.regular.tint(Palette.scrim.opacity(0.4)), in: .capsule)
                            .transition(.opacity.combined(with: .scale(0.94)))
                    }
                    if let issue = controller.permissionIssue {
                        PermissionBanner(issue: issue)
                            .transition(.opacity.combined(with: .scale(0.96, anchor: .top)))
                    }
                    CallStageCard(controller: controller)
                        .opacity(shown ? 1 : 0)
                        .scaleEffect(shown ? 1 : 0.96)
                    Color.clear.frame(height: CallControls.height)
                }
                .padding(.horizontal, Space.l)
                .padding(.top, Space.s)

                // The same board as Live, raised on demand over the stage (in the page's own appearance).
                let boardHeight = geo.size.height * 0.72 + geo.safeAreaInsets.bottom
                BoardPanel(controller: controller)
                    .environment(\.colorScheme, outerScheme)
                    .frame(height: boardHeight)
                    .offset(y: controller.boardVisible ? 0 : boardHeight + 48)
                    .allowsHitTesting(controller.boardVisible)
                    .accessibilityHidden(!controller.boardVisible)
                    .ignoresSafeArea(edges: .bottom)

                CallControls(controller: controller)
                    .padding(.horizontal, Space.l)
                    .padding(.bottom, Space.s)
                    .offset(y: shown ? 0 : 56)
                    .opacity(shown ? 1 : 0)
            }
            .animation(Motion.snappy, value: controller.notice)
            .animation(Motion.snappy, value: controller.permissionIssue)
        }
        .environment(\.colorScheme, .dark)
        .onAppear {
            // Rises with the screen, then the stage settles and the controls slide up a beat later.
            if controller.options.scene != nil { shown = true; return }
            withAnimation(Motion.respecting(reduceMotion, Motion.gentle).delay(0.12)) { shown = true }
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: Space.m) {
            VStack(alignment: .leading, spacing: Space.xs) {
                Text(verbatim: "Beluga")
                    .font(.system(size: 30, weight: .regular, design: .serif))
                    .foregroundStyle(CallInk.primary)
                HStack(spacing: Space.s) {
                    CallStatusText(controller: controller, size: 15)
                    if !controller.title.isEmpty, controller.callPhase.phase.isConnected || !controller.callPhase.isActive {
                        Text(verbatim: "·")
                        Text(verbatim: controller.title)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                }
                .font(TypeScale.callout)
                .foregroundStyle(CallInk.secondary)
            }
            Spacer(minLength: Space.s)
            SelfViewTile(controller: controller)
        }
    }
}

/// Near-black ground with a faint light from above, so the stage card reads as a lit surface.
private struct CallBackdrop: View {
    var body: some View {
        ZStack {
            CallInk.ground
            RadialGradient(colors: [Palette.glow.opacity(0.07), .clear], center: .top, startRadius: 0, endRadius: 520)
        }
    }
}

/// The tutor's tile: what the lesson is on now. Tap to stop the tutor and speak.
private struct CallStageCard: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    private var stage: CallStage { controller.callStage }
    private var phase: CallPhase { controller.callPhase.phase }
    private let shape = RoundedRectangle(cornerRadius: Radius.panel, style: .continuous)

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if phase.isDialing || phase.isFailed {
                // Before the lesson picks up the stage is the call itself: who is being called, or why it failed.
                CallDialingStage(controller: controller, failed: phase.isFailed)
                    .transition(.opacity)
            } else {
                PresenceLabel(presence: controller.callPresence, size: 15)
                    .padding(.bottom, Space.xl)
                    .transition(.opacity)
            }

            if phase.isDialing || phase.isFailed {
                EmptyView()
            } else if stage.isEmpty {
                VStack(alignment: .leading, spacing: Space.m) {
                    Text(verbatim: s("call.empty.title"))
                        .font(.system(size: 30, weight: .regular, design: .serif))
                        .foregroundStyle(CallInk.primary)
                    Text(verbatim: s("call.empty"))
                        .font(.system(size: 17))
                        .foregroundStyle(CallInk.secondary)
                }
                .transition(.opacity)
            } else {
                stepHeader
                    .id("step\(stage.stepIndex)")
                    .transition(.opacity.combined(with: .offset(y: 6)))
                    .padding(.bottom, Space.l)
                VStack(alignment: .leading, spacing: Space.l) {
                    ForEach(stage.supporting()) { block in
                        StageBlockView(block: block, metrics: .stage, dimmed: true)
                            .transition(.opacity)
                    }
                    if let block = stage.featured {
                        StageBlockView(block: block, metrics: .stage)
                            .id(block.id)
                            .transition(.asymmetric(insertion: .opacity.combined(with: .offset(y: 12)), removal: .opacity))
                    }
                }
            }

            Spacer(minLength: Space.l)

            if !stage.line.isEmpty {
                Text(verbatim: stage.line)
                    .font(.system(size: StageMetrics.stage.caption, weight: stage.prompt != nil ? .semibold : .regular))
                    .foregroundStyle(stage.prompt != nil ? CallInk.primary : CallInk.secondary)
                    .lineLimit(4)
                    .minimumScaleFactor(0.8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .id(stage.line)
                    .transition(.opacity)
            }
        }
        .padding(Space.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background {
            ZStack {
                shape.fill(CallInk.wash)
                if phase.isDialing { RingPulse().transition(.opacity) }
                CallGlow(controller: controller, role: .tutor)
                // PiP animates out of this card and back into it.
                PiPSourceView(pip: controller.pip) { CallPiPContent(controller: controller) }
            }
        }
        .clipShape(shape)
        .overlay(shape.strokeBorder(CallInk.hairline, lineWidth: Stroke.hairline))
        .contentShape(shape)
        // The call is over: the stage steps back while "Đã kết thúc" holds for a beat.
        .opacity(phase.isEnded ? 0.5 : 1)
        .onTapGesture { controller.callTapStage() }
        .animation(Motion.smooth, value: stage)
        .animation(Motion.smooth, value: phase)
        .accessibilityElement(children: .combine)
        .accessibilityHint(Text(verbatim: s("call.stage.hint")))
    }

    private var stepHeader: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            if stage.stepIndex >= 0 {
                Text(verbatim: s("call.step", stage.stepIndex + 1).uppercased())
                    .font(.system(size: StageMetrics.stage.stepLabel, weight: .semibold))
                    .tracking(1.2)
                    .foregroundStyle(CallInk.tertiary)
            }
            if !stage.stepTitle.isEmpty {
                Text(verbatim: stage.stepTitle)
                    .font(.system(size: StageMetrics.stage.title, weight: .semibold))
                    .foregroundStyle(CallInk.primary)
                    .lineLimit(2)
            }
        }
    }
}

/// The student's tile: the camera (what the tutor can photograph), ringed with light while the mic hears them.
private struct SelfViewTile: View {
    let controller: LiveController
    @Environment(\.strings) private var s
    private let shape = RoundedRectangle(cornerRadius: Radius.card - 2, style: .continuous)

    var body: some View {
        ZStack(alignment: .bottom) {
            picture
            if let key = controller.cameraNoteKey {
                Text(verbatim: s(key))
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(CallInk.primary)
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
                    .minimumScaleFactor(0.8)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(Capsule().fill(Palette.scrim.opacity(0.55)))
                    .padding(6)
                    .transition(.opacity)
            }
            ShutterFlash(tick: controller.shutterTick)
        }
        .frame(width: 88, height: 118)
        .clipShape(shape)
        .overlay(shape.strokeBorder(CallInk.hairline, lineWidth: Stroke.hairline))
        // The halo is drawn 10 pt outside the tile's edge.
        .overlay(CallGlow(controller: controller, role: .student).padding(-10).allowsHitTesting(false))
        .animation(Motion.snappy, value: controller.cameraNoteKey)
        .animation(Motion.smooth, value: controller.callCameraOn)
        .accessibilityElement()
        .accessibilityLabel(Text(verbatim: s(controller.cameraNoteKey ?? "call.camera.on")))
    }

    @ViewBuilder private var picture: some View {
        if !controller.callCameraOn {
            ZStack {
                CallInk.wash
                Image(systemName: "video.slash.fill")
                    .font(.system(size: 20, weight: .medium))
                    .foregroundStyle(CallInk.tertiary)
                    .padding(.bottom, 22)
            }
            .transition(.opacity)
        } else if let camera = controller.camera {
            CameraPreview(session: camera.session)
        } else {
            Image("mock-problem")
                .resizable()
                .scaledToFill()
                .frame(width: 88, height: 118)
                .clipped()
        }
    }
}

/// Light driven by the real audio level: rising from the stage's bottom edge while the tutor speaks (breathing while
/// it thinks), and a halo around the self-view while the student speaks.
private struct CallGlow: View {
    enum Role { case tutor, student }
    let controller: LiveController
    let role: Role
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var glow = GlowModel()

    private var state: PillState { controller.pillState }

    private var active: Bool {
        switch role {
        case .tutor: return state == .speaking || state == .thinking || state == .tool
        case .student: return state == .listening && !controller.callMuted
        }
    }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60, paused: !active)) { ctx in
            let date = ctx.date
            let audioDriven = (role == .tutor && state == .speaking) || (role == .student && state == .listening)
            let _ = glow.update(raw: active && audioDriven ? controller.level(at: date) : 0, at: date)
            Canvas { gc, size in draw(&gc, size, date) }
        }
        .allowsHitTesting(false)
        .onChange(of: state) { _, _ in glow.reset() }
    }

    private func draw(_ gc: inout GraphicsContext, _ size: CGSize, _ date: Date) {
        switch role {
        case .tutor:
            let h = size.height
            if state == .speaking {
                let level = glow.level
                guard level > 0.02 else { return }
                band(&gc, size, height: h * 0.3 * pow(level, 0.8), opacity: Double(min(1, level * 1.5)),
                     amp: reduceMotion ? 0 : min(6, 2 + 7 * level), phase: glow.phase)
            } else if state == .thinking || state == .tool {
                let since = date.timeIntervalSince(controller.pill.enteredAt)
                let breath = reduceMotion ? 0.5 : (sin(since * 2 * .pi / 2.4 - .pi / 2) + 1) / 2
                band(&gc, size, height: h * 0.1, opacity: 0.18 + 0.35 * breath, amp: 0, phase: 0)
            }
        case .student:
            let level = glow.level
            guard level > 0.02 else { return }
            let rect = CGRect(origin: .zero, size: size).insetBy(dx: 10, dy: 10)
            let path = Path(roundedRect: rect, cornerRadius: Radius.card - 2, style: .continuous)
            gc.drawLayer { layer in
                layer.opacity = Double(min(1, 0.25 + level * 1.4))
                layer.addFilter(.blur(radius: 5 + 5 * level))
                layer.stroke(path, with: .color(Palette.glow), lineWidth: 2 + 5 * level)
            }
            gc.stroke(path, with: .color(Palette.glow.opacity(Double(min(1, 0.4 + level)))), lineWidth: 1.5)
        }
    }

    private static let ramp = Gradient(stops: [
        .init(color: Palette.glow.opacity(0), location: 0),
        .init(color: Palette.glow.opacity(0.4), location: 0.6),
        .init(color: Palette.glow.opacity(0.9), location: 1),
    ])

    /// A soft band from the bottom edge, domed (louder in the middle) with a gentle ripple, like the pill's.
    private func band(_ gc: inout GraphicsContext, _ size: CGSize, height: CGFloat, opacity: Double, amp: CGFloat, phase: CGFloat) {
        guard height > 0.5, opacity > 0.01 else { return }
        let w = size.width, h = size.height
        var path = Path()
        let steps = 32
        path.move(to: CGPoint(x: -16, y: h + 16))
        for i in 0...steps {
            let u = CGFloat(i) / CGFloat(steps)
            let envelope: CGFloat = 0.6 + 0.4 * sin(.pi * u)
            let ripple = amp * (0.6 * sin(2 * .pi * 1.3 * u + phase) + 0.4 * sin(2 * .pi * 2.1 * u - 1.4 * phase))
            path.addLine(to: CGPoint(x: -16 + (w + 32) * u, y: h - height * envelope - ripple))
        }
        path.addLine(to: CGPoint(x: w + 16, y: h + 16))
        path.closeSubpath()
        gc.drawLayer { layer in
            layer.opacity = opacity * 0.55
            layer.addFilter(.blur(radius: 24))
            layer.fill(path, with: .linearGradient(Self.ramp, startPoint: CGPoint(x: 0, y: h - height - amp), endPoint: CGPoint(x: 0, y: h)))
        }
    }
}

/// Mute, audio route, camera, board, end: one row of round buttons, labelled underneath.
struct CallControls: View {
    static let height: CGFloat = Size.liveButton + 26
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        GlassEffectContainer(spacing: Space.l) {
            HStack(alignment: .top, spacing: 0) {
                CallButton(symbol: controller.callMuted ? "mic.slash.fill" : "mic.fill",
                           label: s(controller.callMuted ? "call.control.unmute" : "call.control.mute"),
                           selected: controller.callMuted) { controller.toggleCallMute() }
                CallRouteButton(route: controller.callRoute, label: s("call.control.audio"))
                CallButton(symbol: controller.callCameraOn ? "video.fill" : "video.slash.fill",
                           label: s(controller.callCameraOn ? "call.control.cameraOff" : "call.control.cameraOn"),
                           selected: !controller.callCameraOn) { controller.toggleCallCamera() }
                CallButton(symbol: controller.boardVisible ? "chevron.down" : "rectangle.bottomhalf.inset.filled",
                           label: s(controller.boardVisible ? "control.hideBoard" : "call.control.board"),
                           selected: controller.boardVisible) { controller.toggleCallBoard() }
                    .disabled(!controller.board.hasContent)
                CallEndButton(label: s("call.control.end")) { controller.endCall() }
            }
        }
        .frame(height: Self.height, alignment: .top)
    }
}

/// A round control: dark glass, or filled with light when its option is on (muted, camera off, board up).
private struct CallButton: View {
    let symbol: String
    let label: String
    let selected: Bool
    let action: () -> Void
    @Environment(\.isEnabled) private var enabled

    var body: some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(systemName: symbol)
                    .font(.system(size: 20, weight: .medium))
                    .contentTransition(.symbolEffect(.replace))
                    .foregroundStyle(selected ? Palette.scrim : CallInk.primary)
                    .frame(width: Size.liveButton, height: Size.liveButton)
                    .background(Circle().fill(selected ? Palette.glow : Palette.scrim.opacity(0.35)))
                    .contentShape(.circle)
                    .glassEffect(.regular.tint(Palette.scrim.opacity(selected ? 0 : 0.45)).interactive(), in: .circle)
                Text(verbatim: label)
                    .font(TypeScale.micro)
                    .foregroundStyle(CallInk.secondary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.75)
                    .contentTransition(.opacity)
            }
            .opacity(enabled ? 1 : 0.4)
            .frame(maxWidth: .infinity)
        }
        .buttonStyle(PressScaleStyle(scale: 0.94))
        .animation(Motion.snappy, value: selected)
        .accessibilityLabel(Text(verbatim: label))
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// Speaker / receiver / headphones: the system route picker, dressed as the other controls.
private struct CallRouteButton: View {
    let route: CallRoute
    let label: String

    var body: some View {
        VStack(spacing: 6) {
            ZStack {
                Image(systemName: route.symbol)
                    .font(.system(size: 19, weight: .medium))
                    .contentTransition(.symbolEffect(.replace))
                    .foregroundStyle(CallInk.primary)
                    .frame(width: Size.liveButton, height: Size.liveButton)
                    .background(Circle().fill(Palette.scrim.opacity(0.35)))
                    .glassEffect(.regular.tint(Palette.scrim.opacity(0.45)).interactive(), in: .circle)
                RoutePicker()
                    .frame(width: Size.liveButton, height: Size.liveButton)
            }
            Text(verbatim: label)
                .font(TypeScale.micro)
                .foregroundStyle(CallInk.secondary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity)
        .animation(Motion.snappy, value: route)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text(verbatim: label))
    }
}

/// `AVRoutePickerView` with an invisible glyph: the tap opens the system's audio route menu.
private struct RoutePicker: UIViewRepresentable {
    func makeUIView(context: Context) -> AVRoutePickerView {
        let v = AVRoutePickerView()
        v.tintColor = .clear
        v.activeTintColor = .clear
        v.prioritizesVideoDevices = false
        v.backgroundColor = .clear
        return v
    }

    func updateUIView(_ uiView: AVRoutePickerView, context: Context) {}
}

private struct CallEndButton: View {
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(systemName: "phone.down.fill")
                    .font(.system(size: 22, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: Size.liveButton, height: Size.liveButton)
                    .background(Circle().fill(Color(red: 1, green: 0.23, blue: 0.19)))
                    .contentShape(.circle)
                Text(verbatim: label)
                    .font(TypeScale.micro)
                    .foregroundStyle(CallInk.secondary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity)
        }
        .buttonStyle(PressScaleStyle(scale: 0.92))
        .accessibilityLabel(Text(verbatim: label))
    }
}

/// What the call header says under "Beluga": calling, connected, the running timer, ended with its length, or
/// that the call failed. Each change cross-fades with a small lift; the timer ticks in tabular digits.
struct CallStatusText: View {
    let controller: LiveController
    let size: CGFloat
    @Environment(\.strings) private var s

    var body: some View {
        if controller.callPhase.isActive {
            TimelineView(.periodic(from: .now, by: 0.25)) { ctx in
                let label = controller.callPhase.label(now: ctx.date)
                ZStack(alignment: .leading) {
                    Text(verbatim: text(label, now: ctx.date))
                        .font(.system(size: size, weight: .medium).monospacedDigit())
                        .id(Self.kind(label))
                        .transition(.opacity.combined(with: .offset(y: 4)))
                }
                .animation(Motion.snappy, value: Self.kind(label))
                .accessibilityIdentifier("call.status")
            }
        } else {
            // Scenes and older paths: the plain timer.
            CallTimer(start: controller.callStartedAt, size: size)
        }
    }

    private func text(_ label: CallLabel, now: Date) -> String {
        switch label {
        case .none: return ""
        case .dialing: return s("call.phase.dialing")
        case .connected: return s("call.phase.connected")
        case .timer(let since): return CallTimer.format(now.timeIntervalSince(since))
        case .ended(let d): return d.map { s("call.phase.ended", CallTimer.format($0)) } ?? s("call.phase.endedShort")
        case .failed: return s("call.phase.failed")
        }
    }

    static func kind(_ label: CallLabel) -> String {
        switch label {
        case .none: return "none"
        case .dialing: return "dialing"
        case .connected: return "connected"
        case .timer: return "timer"
        case .ended: return "ended"
        case .failed: return "failed"
        }
    }
}

/// The stage while the call rings or after it failed: a serif line like the empty stage, a quiet explanation, and
/// "Gọi lại" when it failed.
private struct CallDialingStage: View {
    let controller: LiveController
    let failed: Bool
    @Environment(\.strings) private var s

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            Text(verbatim: s(failed ? "call.phase.failed" : "call.phase.dialing"))
                .font(.system(size: 30, weight: .regular, design: .serif))
                .foregroundStyle(CallInk.primary)
                .contentTransition(.opacity)
            Text(verbatim: s(failed ? "call.failed.body" : "call.dialing.body"))
                .font(.system(size: 17))
                .foregroundStyle(CallInk.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .contentTransition(.opacity)
            if failed {
                Button { controller.retryCall() } label: {
                    Label(s("call.retry"), systemImage: "phone.fill")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(Palette.scrim)
                        .padding(.horizontal, Space.l)
                        .frame(minHeight: Size.tap)
                        .background(Capsule().fill(Palette.glow))
                        .contentShape(.capsule)
                }
                .buttonStyle(PressScaleStyle(scale: 0.95))
                .padding(.top, Space.s)
                .transition(.opacity.combined(with: .scale(0.92, anchor: .leading)))
            }
        }
        .animation(Motion.snappy, value: failed)
    }
}

/// Calling: soft rings spread from the middle of the stage, in the ring-back's rhythm. Still with Reduce Motion.
private struct RingPulse: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private let period: Double = 2.6

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60, paused: reduceMotion)) { ctx in
            Canvas { gc, size in
                let center = CGPoint(x: size.width / 2, y: size.height * 0.58)
                let base = min(size.width, size.height) * 0.16
                let t = ctx.date.timeIntervalSinceReferenceDate
                for i in 0..<3 {
                    let u = reduceMotion ? 0.35 + Double(i) * 0.2 : ((t / period) + Double(i) / 3).truncatingRemainder(dividingBy: 1)
                    let r = base * (1 + 2.2 * u)
                    let alpha = (1 - u) * 0.22
                    let rect = CGRect(x: center.x - r, y: center.y - r, width: r * 2, height: r * 2)
                    gc.stroke(Path(ellipseIn: rect), with: .color(Palette.glow.opacity(alpha)), lineWidth: 1.5)
                }
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}
