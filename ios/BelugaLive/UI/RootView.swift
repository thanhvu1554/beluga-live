import SwiftUI
import UIKit
import BelugaCore

/// Root: the chat shell (home, transcript, drawer, folders) with Live laid over it when the session is in live mode.
/// App-wide sheets live here so they work from either mode.
struct RootView: View {
    @Bindable var controller: LiveController
    @Environment(\.colorScheme) private var scheme
    @Environment(\.scenePhase) private var phase

    var body: some View {
        ZStack {
            ChatShell(controller: controller)
            if controller.mode == .live {
                LiveView(controller: controller)
                    .transition(.move(edge: .bottom))
                    .zIndex(1)
            }
        }
        .animation(Motion.panel, value: controller.mode)
        .onAppear { applyTheme(scheme) }
        .onChange(of: scheme) { _, new in applyTheme(new) }
        .onChange(of: phase) { _, new in
            // "phase active/background" lines also tell the next launch whether the app died in the foreground.
            Diagnostics.log("phase \(Self.name(new))")
            if new == .active { controller.appBecameActive() }
        }
        .onAppear { Diagnostics.log("phase \(Self.name(phase))") }
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.didReceiveMemoryWarningNotification)) { _ in
            Diagnostics.log("memory warning")
        }
        .sheet(isPresented: $controller.showProgress) {
            NavigationStack { LearningProgressView(controller: controller) }
                .tint(Ink.primary)
                .presentationDetents([.large])
                .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $controller.showSettings, onDismiss: {
            controller.voicePreview.stop()
            controller.settingsPath = []
        }) {
            SettingsView(controller: controller, settings: controller.settings)
                .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $controller.showUsage) {
            NavigationStack { UsageView(controller: controller) }
                .tint(Ink.primary)
                .presentationDetents([.large])
                .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $controller.showModels) {
            ModelPickerView(controller: controller)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $controller.showReplay) {
            ReplayView(controller: controller)
                .presentationDetents([.large])
                .presentationDragIndicator(.visible)
        }
        .sheet(item: $controller.preview) { file in
            QuickLookView(url: file.url)
                .ignoresSafeArea()
        }
    }

    private static func name(_ p: ScenePhase) -> String {
        switch p {
        case .active: return "active"
        case .inactive: return "inactive"
        case .background: return "background"
        @unknown default: return "unknown"
        }
    }

    private func applyTheme(_ s: ColorScheme) {
        controller.board.setTheme(s == .dark)
        controller.chatPage.setTheme(s == .dark)
        controller.knowledgePage.setTheme(s == .dark)
    }
}

/// Live mode: camera, captions, the board panel and the voice pill (protocol v1 flow).
struct LiveView: View {
    @Bindable var controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .bottom) {
                CameraLayer(controller: controller)
                    .ignoresSafeArea()

                ShutterFlash(tick: controller.shutterTick)
                    .ignoresSafeArea()

                // Always in the hierarchy (the web view stays loaded); slid off-screen when hidden.
                let boardHeight = geo.size.height * 0.6 + geo.safeAreaInsets.bottom
                BoardPanel(controller: controller)
                    .frame(height: boardHeight)
                    .offset(y: controller.boardVisible ? 0 : boardHeight + 48)
                    .animation(Motion.panel, value: controller.boardVisible)
                    .allowsHitTesting(controller.boardVisible)
                    .accessibilityHidden(!controller.boardVisible)
                    .ignoresSafeArea(edges: .bottom)

                VStack(spacing: Space.m) {
                    TopBar(controller: controller)
                    if let issue = controller.permissionIssue {
                        PermissionBanner(issue: issue)
                            .transition(.opacity)
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, Space.l)

                // Captions live in the camera area, just above the board (or the controls when the
                // board is hidden), so they never cover board content.
                if controller.captionsOn, !controller.caption.isEmpty {
                    // The board takes the bottom 60 %; the caption may grow up to what is left.
                    CaptionView(text: controller.caption,
                                maxHeight: geo.size.height * (controller.boardVisible ? 0.28 : 0.5))
                        .padding(.horizontal, Space.l)
                        .padding(.bottom, controller.boardVisible
                                 ? geo.size.height * 0.6 + Space.m
                                 : Size.pillHeight + Space.s + Space.m)
                        .transition(.opacity)
                }

                ControlBar(controller: controller)
                    .padding(.horizontal, Space.l)
                    .padding(.bottom, Space.s)
            }
        }
        .background(Color.black.ignoresSafeArea())
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
                Button { controller.setMode(.chat) } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: Size.icon, weight: .medium))
                        .frame(width: Size.control - Space.s, height: Size.control - Space.s)
                }
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .accessibilityLabel(s("live.exit"))

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

/// Calm, persistent hint when a permission was refused, with a way to the system Settings.
private struct PermissionBanner: View {
    let issue: PermissionIssue
    @Environment(\.strings) private var s

    var body: some View {
        VStack(spacing: Space.s) {
            Text(verbatim: s(issue.messageKey))
                .font(TypeScale.callout)
                .foregroundStyle(Ink.primary)
                .multilineTextAlignment(.center)
            Button(s("permission.openSettings")) {
                if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
            }
            .font(TypeScale.callout.weight(.semibold))
            .buttonStyle(.glass)
        }
        .padding(.horizontal, Space.l)
        .padding(.vertical, Space.m)
        .glassEffect(.regular, in: .rect(cornerRadius: Radius.l))
        .frame(maxWidth: 340)
    }
}

/// The spoken line as a subtitle. Short lines size to their text; a long line is never cut —
/// the card stops growing at `maxHeight` and the rest scrolls, starting at the first word.
private struct CaptionView: View {
    let text: String
    let maxHeight: CGFloat
    @State private var textHeight: CGFloat = 0

    private var scrolls: Bool { textHeight > maxHeight }
    private var shownHeight: CGFloat { textHeight > 0 ? min(textHeight, maxHeight) : 0 }

    var body: some View {
        ScrollView(.vertical) {
            Text(verbatim: text)
                .font(TypeScale.callout)
                .foregroundStyle(Ink.primary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, Space.l)
                .padding(.vertical, Space.m)
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { textHeight = $0 }
        }
        .id(text)   // a new line always starts at its first word
        .scrollIndicators(scrolls ? .visible : .hidden)
        .scrollBounceBehavior(.basedOnSize)   // short lines stay still under the finger
        .frame(maxWidth: 340)
        .frame(height: shownHeight)
        .opacity(textHeight > 0 ? 1 : 0)
        .glassEffect(.regular, in: .rect(cornerRadius: Radius.l))
    }
}
