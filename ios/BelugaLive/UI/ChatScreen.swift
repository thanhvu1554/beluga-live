import SwiftUI
import BelugaCore

/// The chat side of the app: a navigation stack (home/chat at the root, folders pushed) that slides right to
/// reveal the drawer underneath, like the Claude app. The slide follows the finger from the left edge of the
/// chat screen and from anywhere on the open drawer; letting go carries the finger's speed into the spring.
struct ChatShell: View {
    @Bindable var controller: LiveController
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Live finger translation while dragging (nil at rest).
    @State private var drag: CGFloat?
    @State private var dragStartOpen = false

    var body: some View {
        GeometryReader { geo in
            let width = min(330, geo.size.width * 0.84)
            let x = position(width: width)
            let p = max(0, min(1, x / width))
            ZStack(alignment: .leading) {
                // Underneath: the drawer, drifting in a little slower than the content (a soft parallax).
                DrawerView(controller: controller)
                    .frame(width: width)
                    .offset(x: reduceMotion ? 0 : -(1 - p) * width * 0.3)
                    .opacity(p > 0.001 ? 1 : 0)
                    .simultaneousGesture(dragGesture(width: width, opening: false))
                    .accessibilityHidden(!controller.drawerOpen)

                NavigationStack(path: $controller.path) {
                    ChatScreen(controller: controller)
                        .navigationDestination(for: Route.self) { route in
                            switch route {
                            case .folder(let id): FolderView(controller: controller, folderId: id)
                            }
                        }
                }
                .tint(Ink.primary)
                .overlay {
                    // Dims the content and takes taps and drags while the drawer is out.
                    Palette.dim
                        .opacity(p)
                        .ignoresSafeArea()
                        .allowsHitTesting(p > 0.01)
                        .onTapGesture { setOpen(false) }
                        .gesture(dragGesture(width: width, opening: false))
                        .accessibilityHidden(true)
                }
                .overlay(alignment: .leading) {
                    Rectangle().fill(Ink.hairline).frame(width: Stroke.hairline)
                        .opacity(p > 0.001 ? 1 : 0)
                        .ignoresSafeArea()
                        .allowsHitTesting(false)
                }
                .overlay(alignment: .leading) {
                    // A strip along the left edge of the chat pulls the drawer out (the web transcript would
                    // take the touch otherwise). It stays clear of the bar buttons and the composer.
                    if controller.path.isEmpty && !controller.drawerOpen && controller.mode == .chat {
                        Color.clear
                            .frame(width: 16)
                            .contentShape(.rect)
                            .padding(.top, 72)
                            .padding(.bottom, 150)
                            .gesture(dragGesture(width: width, opening: true))
                            .accessibilityHidden(true)
                    }
                }
                .offset(x: x)
                .accessibilityHidden(controller.drawerOpen)
            }
        }
    }

    /// Where the content's leading edge is: at rest 0 or `width`; under the finger it follows, with a rubber band
    /// past both ends.
    private func position(width: CGFloat) -> CGFloat {
        let base: CGFloat = controller.drawerOpen ? width : 0
        guard let drag else { return base }
        let raw = (dragStartOpen ? width : 0) + drag
        if raw < 0 { return Motion.rubberBand(raw, limit: 24) }
        if raw > width { return width + Motion.rubberBand(raw - width, limit: 24) }
        return raw
    }

    private func dragGesture(width: CGFloat, opening: Bool) -> some Gesture {
        DragGesture(minimumDistance: 12, coordinateSpace: .global)
            .onChanged { v in
                if drag == nil {
                    // Horizontal intent only; vertical drags belong to the lists and the transcript.
                    guard abs(v.translation.width) > abs(v.translation.height) * 1.2 else { return }
                    if opening, controller.drawerOpen || !controller.path.isEmpty { return }
                    if !opening, !controller.drawerOpen { return }
                    dragStartOpen = controller.drawerOpen
                }
                var t = Transaction()
                t.disablesAnimations = true
                withTransaction(t) { drag = v.translation.width }
            }
            .onEnded { v in
                guard let d = drag else { return }
                let start: CGFloat = dragStartOpen ? width : 0
                let now = start + d
                let projected = now + v.velocity.width * 0.18
                let open = projected > width / 2
                let target: CGFloat = open ? width : 0
                let anim = reduceMotion ? Motion.snappy : Motion.release(velocity: v.velocity.width, distance: target - now)
                if open != dragStartOpen { Haptics.selection() }
                withAnimation(anim) {
                    controller.drawerOpen = open
                    drag = nil
                }
            }
    }

    private func setOpen(_ open: Bool) {
        withAnimation(Motion.respecting(reduceMotion, Motion.smooth)) { controller.drawerOpen = open }
    }
}

/// Home (greeting and suggestions) when the session is empty, the transcript otherwise; the composer floats at
/// the bottom over the transcript, which scrolls on under its glass.
struct ChatScreen: View {
    @Bindable var controller: LiveController
    @Environment(\.strings) private var s
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Space the floating composer (and the keyboard under it) covers at the bottom of the web transcript.
    @State private var webBottom: CGFloat = 0
    @State private var composerTop: CGFloat = 0

    private var isEmpty: Bool { controller.transcript.isEmpty }

    var body: some View {
        ZStack {
            Color(uiColor: .systemBackground).ignoresSafeArea()
            // The web transcript stays loaded; the native one covers it until `chat.html` reports ready.
            // It runs under the composer (and the keyboard): its frame never changes while they move, so the
            // page never relayouts mid-animation; the covered height is passed to the page as an inset instead.
            WebPageView(page: controller.chatPage)
                .ignoresSafeArea(edges: .bottom)
                .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).maxY } action: { webBottom = $0 }
                .opacity(controller.chatPage.ready && !isEmpty ? 1 : 0)
            if !controller.chatPage.ready && !isEmpty {
                NativeTranscript(controller: controller)
            }
            if isEmpty {
                HomeGreeting(controller: controller)
                    .transition(.opacity)
            }
        }
        // Home and transcript cross-fade here, on the content only (never on the navigation bar's items).
        .animation(Motion.smooth, value: isEmpty)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: Space.s) {
                if let notice = controller.notice, controller.mode == .chat {
                    Text(verbatim: notice)
                        .font(TypeScale.caption)
                        .foregroundStyle(Ink.primary)
                        .padding(.horizontal, Space.m)
                        .padding(.vertical, Space.s)
                        .glassEffect(.regular, in: .capsule)
                        .transition(.opacity.combined(with: .scale(0.96, anchor: .bottom)))
                }
                if isEmpty, !controller.composer.hasContent {
                    HomeSuggestions(controller: controller)
                        .transition(.opacity.combined(with: .offset(y: 8)))
                }
                ComposerView(controller: controller, composer: controller.composer)
                    .padding(.horizontal, Space.m)
            }
            .padding(.bottom, Space.s)
            .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).minY } action: { composerTop = $0 }
        }
        .onChange(of: webBottom - composerTop) { _, covered in
            controller.chatPage.invoke("setInsets", 8, .number(Double(max(0, Int(covered.rounded())) + 8)))
        }
        .animation(Motion.snappy, value: controller.notice)
        .animation(Motion.snappy, value: controller.composer.hasContent)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    withAnimation(Motion.respecting(reduceMotion, Motion.smooth)) { controller.drawerOpen = true }
                } label: {
                    Image(systemName: "line.3.horizontal")
                }
                .accessibilityLabel(s("drawer.open"))
            }
            ToolbarItem(placement: .principal) {
                ChatTitle(controller: controller)
            }
            // The bar always holds the same items: bar buttons that come and go with the transcript made UIKit
            // rebuild the bar mid-update (the app died there on a real phone). "New chat" is just disabled on home.
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button { controller.newChat(in: controller.folderId) } label: {
                    Image(systemName: "square.and.pencil")
                }
                .disabled(isEmpty)
                .accessibilityLabel(s("chat.new"))
                Button { controller.showSettings = true } label: {
                    Image(systemName: "gearshape")
                }
                .accessibilityLabel(s("settings.title"))
            }
        }
    }
}

/// Navigation bar title. Home: the wordmark with the model under it (tap to change the model, like the
/// Claude app). In a session: its title and folder.
private struct ChatTitle: View {
    let controller: LiveController
    @Environment(\.strings) private var s

    private var home: Bool { controller.transcript.isEmpty && controller.title.isEmpty }

    var body: some View {
        // One button with the same shape in both states (only its texts change): the bar item is never
        // replaced by another kind of view while the bar updates.
        Button { controller.showModels = true } label: {
            VStack(spacing: 1) {
                Text(verbatim: home ? "Beluga" : (controller.title.isEmpty ? s("board.untitled") : controller.title))
                    .font(home ? .system(size: 17, weight: .semibold, design: .serif) : TypeScale.callout.weight(.semibold))
                    .foregroundStyle(Ink.primary)
                    .lineLimit(1)
                    .contentTransition(.opacity)
                HStack(spacing: 3) {
                    Text(verbatim: subtitle)
                        .font(TypeScale.micro)
                        .lineLimit(1)
                        .contentTransition(.opacity)
                    Image(systemName: "chevron.down")
                        .font(.system(size: 7, weight: .bold))
                        .opacity(home ? 1 : 0)
                }
                .foregroundStyle(Ink.secondary)
                .opacity(subtitle.isEmpty ? 0 : 1)
            }
            .frame(minWidth: 120, maxWidth: 240, minHeight: Size.tap)
            .contentShape(.rect)
        }
        .buttonStyle(PressScaleStyle())
        .accessibilityLabel(Text(verbatim: home ? "Beluga, " + modelLabel : (controller.title.isEmpty ? s("board.untitled") : controller.title)))
        .accessibilityHint(Text(verbatim: s("models.title")))
        .animation(Motion.snappy, value: controller.title)
        .animation(Motion.snappy, value: home)
    }

    /// Home: the model. In a session: its folder (nothing when the session is in no folder).
    private var subtitle: String {
        if home { return modelLabel }
        return controller.library.folder(controller.folderId)?.name ?? ""
    }

    private var modelLabel: String {
        let name = controller.currentModel?.label ?? controller.settings.model
        let effort = controller.settings.effort
        return effort.isEmpty ? name : name + " · " + s("effort.\(effort)")
    }
}

/// The calm home: a line doodle that draws itself, one serif line from the server (or the built-in list).
/// It sits a little above the middle of the free space (the optical centre), and enters once per greeting.
struct HomeGreeting: View {
    let controller: LiveController
    @Environment(\.strings) private var s
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false
    @State private var onScreen = false

    var body: some View {
        let g = controller.library.greeting
        GeometryReader { geo in
            VStack(spacing: Space.l) {
                DoodleView(doodle: g.doodle, active: onScreen && controller.mode == .chat && !controller.drawerOpen)
                    .frame(width: 240, height: 144)
                    .foregroundStyle(Ink.secondary)
                    .accessibilityHidden(true)
                VStack(spacing: Space.s) {
                    Text(verbatim: g.text)
                        .font(TypeScale.greeting)
                        .foregroundStyle(Ink.primary)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                        .contentTransition(.opacity)
                    if let sub = g.sub {
                        Text(verbatim: sub)
                            .font(TypeScale.callout)
                            .foregroundStyle(Ink.secondary)
                            .multilineTextAlignment(.center)
                    }
                    if let folder = controller.library.folder(controller.folderId) {
                        Label { Text(verbatim: folder.name) } icon: { Image(systemName: Subject.symbol(folder.kind)) }
                            .font(TypeScale.caption.weight(.medium))
                            .foregroundStyle(Ink.secondary)
                            .padding(.horizontal, Space.m)
                            .padding(.vertical, Space.xs + Space.xxs)
                            .background(Ink.wash, in: .capsule)
                            .padding(.top, Space.xs)
                    }
                }
                .opacity(shown ? 1 : 0)
                .offset(y: shown || reduceMotion ? 0 : 10)
            }
            .padding(.horizontal, Space.xxl)
            .frame(maxWidth: .infinity)
            // Weighted toward the composer: the greeting sits with the suggestions and the field it invites the
            // owner to use, and the air goes to the top instead of splitting the screen into two empty halves.
            .position(x: geo.size.width / 2, y: geo.size.height * 0.56)
        }
        .animation(Motion.gentle, value: g.text)
        .onAppear {
            onScreen = true
            guard !shown else { return }
            withAnimation(Motion.gentle.delay(reduceMotion ? 0 : 0.18)) { shown = true }
        }
        .onDisappear { onScreen = false }
    }
}

/// A few things the owner is likely to do next, built from what the app already knows: go on with the last
/// chat, a quick review in a folder, a photo of a problem, Live. Shown above the composer on the home screen.
struct HomeSuggestions: View {
    let controller: LiveController
    @Environment(\.strings) private var s
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false

    struct Item: Identifiable {
        let id: String
        let symbol: String
        let title: String
        let detail: String
        let action: @MainActor () -> Void
    }

    var body: some View {
        let items = suggestions
        ScrollView(.horizontal) {
            HStack(spacing: Space.s) {
                ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                    Button { item.action() } label: { card(item) }
                        .buttonStyle(PressScaleStyle())
                        .opacity(shown ? 1 : 0)
                        .offset(y: shown || reduceMotion ? 0 : 12)
                        .animation(Motion.gentle.delay(reduceMotion ? 0 : 0.28 + Double(i) * 0.05), value: shown)
                }
            }
            .padding(.horizontal, Space.m)
        }
        .scrollIndicators(.hidden)
        .scrollClipDisabled()
        .onAppear { shown = true }
    }

    private func card(_ item: Item) -> some View {
        HStack(spacing: Space.s + Space.xxs) {
            Image(systemName: item.symbol)
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(Ink.secondary)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text(verbatim: item.title)
                    .font(TypeScale.callout.weight(.semibold))
                    .foregroundStyle(Ink.primary)
                    .lineLimit(1)
                Text(verbatim: item.detail)
                    .font(TypeScale.caption)
                    .foregroundStyle(Ink.secondary)
                    .lineLimit(1)
            }
        }
        .padding(.leading, Space.m)
        .padding(.trailing, Space.l)
        .frame(minWidth: 132, maxWidth: 220, minHeight: Size.row, alignment: .leading)
        .background(Ink.wash, in: .rect(cornerRadius: Radius.card, style: .continuous))
        .contentShape(.rect(cornerRadius: Radius.card, style: .continuous))
        .accessibilityElement(children: .combine)
    }

    private var suggestions: [Item] {
        var out: [Item] = []
        let lib = controller.library
        let recent = lib.sessions
            .filter { controller.folderId == nil || $0.folderId == controller.folderId }
            .max { $0.updatedAt < $1.updatedAt }
        if let recent {
            let parts = recent.title.components(separatedBy: " · ")
            let name = parts.count > 1 ? parts.dropFirst().joined(separator: " · ") : recent.title
            out.append(Item(id: "continue", symbol: recent.mode == .live ? "waveform" : "arrow.uturn.backward",
                            title: s("home.continue"), detail: name.isEmpty ? s("board.untitled") : name) {
                controller.open(recent)
            })
        }
        let folder = controller.library.folder(controller.folderId)
            ?? lib.folders.filter(\.hasKnowledge).max { $0.updatedAt < $1.updatedAt }
        if let folder {
            out.append(Item(id: "review", symbol: "bolt", title: s("review.quick"), detail: folder.name) {
                controller.quickReview(folder.id)
            })
        }
        out.append(Item(id: "photo", symbol: "camera", title: s("home.photo"), detail: s("home.photo.detail")) {
            controller.composer.cameraTick += 1
        })
        out.append(Item(id: "live", symbol: "waveform", title: "Live", detail: s("home.live.detail")) {
            controller.setMode(.live)
        })
        out.append(Item(id: "call", symbol: "phone", title: s("call.home.title"), detail: s("call.home.detail")) {
            controller.startCall()
        })
        return out
    }
}
