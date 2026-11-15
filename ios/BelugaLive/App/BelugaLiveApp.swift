import SwiftUI
import BelugaCore

@main
struct BelugaLiveApp: App {
    @State private var controller: LiveController

    init() {
        let options = LaunchOptions.current()
        // Real runs only: screenshot runs are killed by the script, which would look like a crash next time.
        if !options.mock {
            #if DEBUG
            Diagnostics.shared.start(redirectStderr: false)
            #else
            Diagnostics.shared.start(redirectStderr: true)
            #endif
        }
        // Mock runs never touch the user's saved settings.
        let settings = options.mock
            ? AppSettings(defaults: UserDefaults(suiteName: "mock") ?? .standard, persist: false)
            : AppSettings()
        if options.mock, options.scene == nil || options.scene == .settings || options.scene == .connection {
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
