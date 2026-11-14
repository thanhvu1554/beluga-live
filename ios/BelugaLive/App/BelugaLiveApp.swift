import SwiftUI
import BelugaCore

@main
struct BelugaLiveApp: App {
    @State private var controller: LiveController

    init() {
        let options = LaunchOptions.current()
        // Mock runs never touch the user's saved settings.
        let settings = options.mock
            ? AppSettings(defaults: UserDefaults(suiteName: "mock") ?? .standard, persist: false)
            : AppSettings()
        if options.mock, options.scene == nil || options.scene == .settings {
            settings.host = settings.host.isEmpty ? "beluga-pc" : settings.host
        }
        _controller = State(initialValue: LiveController(settings: settings, options: options))
    }

    var body: some Scene {
        WindowGroup {
            RootView(controller: controller)
                .environment(\.strings, Strings(lang: controller.settings.lang))
                .tint(Ink.primary)
                .statusBarHidden(false)
                .task { controller.start() }
        }
    }
}
