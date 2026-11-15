import SwiftUI
import BelugaCore

/// The chat side of the app: a navigation stack (home/chat at the root, folders pushed) with the drawer over it.
struct ChatShell: View {
    @Bindable var controller: LiveController

    var body: some View {
        ZStack(alignment: .leading) {
            NavigationStack(path: $controller.path) {
                ChatScreen(controller: controller)
                    .navigationDestination(for: Route.self) { route in
                        switch route {
                        case .folder(let id): FolderView(controller: controller, folderId: id)
                        }
                    }
            }
            .tint(Ink.primary)
            DrawerOverlay(controller: controller)
        }
    }
}

/// Home (greeting) when the session is empty, the transcript otherwise; the composer sits at the bottom.
struct ChatScreen: View {
    @Bindable var controller: LiveController
    @Environment(\.strings) private var s

    private var isEmpty: Bool { controller.transcript.isEmpty }

    var body: some View {
        ZStack {
            Color(uiColor: .systemBackground).ignoresSafeArea()
            // The web transcript stays loaded; the native one covers it until `chat.html` reports ready.
            WebPageView(page: controller.chatPage)
                .opacity(controller.chatPage.ready && !isEmpty ? 1 : 0)
            if !controller.chatPage.ready && !isEmpty {
                NativeTranscript(controller: controller)
            }
            if isEmpty {
                HomeGreeting(controller: controller)
                    .transition(.opacity)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: Space.s) {
                if let notice = controller.notice, controller.mode == .chat {
                    Text(verbatim: notice)
                        .font(TypeScale.caption)
                        .foregroundStyle(Ink.primary)
                        .padding(.horizontal, Space.m)
                        .padding(.vertical, Space.s)
                        .glassEffect(.regular, in: .capsule)
                        .transition(.opacity)
                }
                ComposerView(controller: controller, composer: controller.composer)
            }
            .padding(.horizontal, Space.m)
            .padding(.bottom, Space.s)
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { withAnimation(Motion.panel) { controller.drawerOpen = true } } label: {
                    Image(systemName: "line.3.horizontal")
                }
                .accessibilityLabel(s("drawer.open"))
            }
            ToolbarItem(placement: .principal) {
                ChatTitle(controller: controller)
            }
            ToolbarItemGroup(placement: .topBarTrailing) {
                if !isEmpty {
                    Button { controller.newChat(in: controller.folderId) } label: {
                        Image(systemName: "square.and.pencil")
                    }
                    .accessibilityLabel(s("chat.new"))
                }
                Button { controller.showSettings = true } label: {
                    Image(systemName: "gearshape")
                }
                .accessibilityLabel(s("settings.title"))
            }
        }
        .animation(.easeOut(duration: Motion.standard), value: isEmpty)
    }
}

/// Session title (and its folder) in the navigation bar; the small whale mark on the home screen.
private struct ChatTitle: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        if controller.transcript.isEmpty && controller.title.isEmpty {
            BelugaMark()
                .frame(width: 30, height: 20)
                .foregroundStyle(Ink.primary)
                .accessibilityLabel(Text(verbatim: "Beluga"))
        } else {
            VStack(spacing: 0) {
                Text(verbatim: controller.title.isEmpty ? s("board.untitled") : controller.title)
                    .font(TypeScale.callout.weight(.semibold))
                    .lineLimit(1)
                if let folder = controller.library.folder(controller.folderId) {
                    Text(verbatim: folder.name)
                        .font(TypeScale.micro)
                        .foregroundStyle(Ink.secondary)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: 240)
        }
    }
}

/// The calm home: a line doodle, one serif line from the server (or the built-in list), an optional hint.
struct HomeGreeting: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    var body: some View {
        let g = controller.library.greeting
        VStack(spacing: Space.l) {
            DoodleView(doodle: g.doodle)
                .frame(width: 168, height: 96)
                .foregroundStyle(Ink.secondary)
                .accessibilityHidden(true)
            Text(verbatim: g.text)
                .font(.system(.largeTitle, design: .serif))
                .foregroundStyle(Ink.primary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            if let sub = g.sub {
                Text(verbatim: sub)
                    .font(TypeScale.callout)
                    .foregroundStyle(Ink.secondary)
                    .multilineTextAlignment(.center)
            }
            if let folder = controller.library.folder(controller.folderId) {
                Label { Text(verbatim: folder.name) } icon: { Image(systemName: "folder") }
                    .font(TypeScale.caption)
                    .foregroundStyle(Ink.tertiary)
                    .padding(.top, Space.s)
            }
        }
        .padding(.horizontal, Space.xxl)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.bottom, Space.xxxl)
    }
}
