import SwiftUI
import BelugaCore

struct RootView: View {
    @Bindable var controller: LiveController
    @Environment(\.colorScheme) private var scheme
    @Environment(\.strings) private var s

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .bottom) {
                CameraLayer(controller: controller)
                    .ignoresSafeArea()

                ShutterFlash(tick: controller.shutterTick)
                    .ignoresSafeArea()

                if controller.boardVisible {
                    BoardPanel(controller: controller)
                        .frame(height: geo.size.height * 0.6 + geo.safeAreaInsets.bottom)
                        .transition(.move(edge: .bottom))
                        .ignoresSafeArea(edges: .bottom)
                }

                VStack(spacing: Space.m) {
                    TopBar(controller: controller)
                    Spacer(minLength: 0)
                    if controller.captionsOn, !controller.caption.isEmpty {
                        CaptionView(text: controller.caption)
                            .transition(.opacity)
                    }
                    ControlBar(controller: controller)
                }
                .padding(.horizontal, Space.l)
                .padding(.bottom, Space.s)
            }
        }
        .onAppear { controller.board.setTheme(scheme == .dark) }
        .onChange(of: scheme) { _, new in controller.board.setTheme(new == .dark) }
        .sheet(isPresented: $controller.showSessions) {
            SessionsView(controller: controller)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $controller.showSettings) {
            SettingsView(controller: controller, settings: controller.settings)
                .presentationDragIndicator(.visible)
        }
    }
}

/// Live camera, or the bundled sample problem photo in mock mode.
private struct CameraLayer: View {
    let controller: LiveController

    var body: some View {
        if let camera = controller.camera {
            CameraPreview(session: camera.session)
        } else {
            Color.black.overlay {
                Image("mock-problem")
                    .resizable()
                    .scaledToFill()
            }
            .clipped()
        }
    }
}

/// Brief white flash when a photo is taken (no colour, no bounce).
private struct ShutterFlash: View {
    let tick: Int
    @State private var opacity = 0.0

    var body: some View {
        Color.white.opacity(opacity)
            .allowsHitTesting(false)
            .onChange(of: tick) { _, _ in
                opacity = 0.7
                withAnimation(.easeOut(duration: 0.35)) { opacity = 0 }
            }
    }
}

private struct TopBar: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        GlassEffectContainer(spacing: Space.l) {
            HStack(alignment: .center, spacing: Space.m) {
                Button { controller.openSessions() } label: {
                    Image(systemName: "list.bullet")
                        .font(.system(size: Size.icon, weight: .medium))
                        .frame(width: Size.control - Space.s, height: Size.control - Space.s)
                }
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .accessibilityLabel(s("sessions.title"))

                Spacer(minLength: 0)

                if let notice = controller.notice {
                    Text(verbatim: notice)
                        .font(TypeScale.caption)
                        .foregroundStyle(Ink.primary)
                        .lineLimit(2)
                        .padding(.horizontal, Space.m)
                        .padding(.vertical, Space.s)
                        .glassEffect(.regular, in: .capsule)
                        .transition(.opacity)
                }

                Spacer(minLength: 0)

                Button { controller.showSettings = true } label: {
                    Image(systemName: "gearshape")
                        .font(.system(size: Size.icon, weight: .medium))
                        .frame(width: Size.control - Space.s, height: Size.control - Space.s)
                }
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .accessibilityLabel(s("settings.title"))
            }
        }
        .foregroundStyle(Ink.primary)
    }
}

private struct ControlBar: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        GlassEffectContainer(spacing: Space.l) {
            HStack(spacing: Space.l) {
                Button { controller.manualCapture() } label: {
                    Image(systemName: "camera")
                        .font(.system(size: Size.icon, weight: .medium))
                        .frame(width: Size.control - Space.s, height: Size.control - Space.s)
                }
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .accessibilityLabel(s("control.capture"))

                PillView(controller: controller)

                Button { controller.showBoard(!controller.boardVisible) } label: {
                    Image(systemName: controller.boardVisible ? "chevron.down" : "rectangle.bottomhalf.inset.filled")
                        .font(.system(size: Size.icon, weight: .medium))
                        .frame(width: Size.control - Space.s, height: Size.control - Space.s)
                        .contentTransition(.symbolEffect(.replace))
                }
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .disabled(!controller.board.hasContent)
                .accessibilityLabel(s(controller.boardVisible ? "control.hideBoard" : "control.showBoard"))
            }
        }
        .foregroundStyle(Ink.primary)
    }
}

private struct CaptionView: View {
    let text: String

    var body: some View {
        Text(verbatim: text)
            .font(TypeScale.callout)
            .foregroundStyle(Ink.primary)
            .multilineTextAlignment(.center)
            .lineLimit(3)
            .padding(.horizontal, Space.l)
            .padding(.vertical, Space.m)
            .glassEffect(.regular, in: .rect(cornerRadius: Radius.l))
            .frame(maxWidth: 340)
    }
}
