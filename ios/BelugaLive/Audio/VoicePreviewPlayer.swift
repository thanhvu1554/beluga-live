import AVFoundation
import Observation
import BelugaCore

/// Plays the stored preview clip of a tutor voice (`GET /voice/<id>.mp3`), one at a time.
///
/// - The body is checked before it is kept or played (`AudioSniff`): an error page, an empty reply or a cut-off
///   download is never cached, and a bad cached file is thrown away instead of failing every later tap.
/// - The clip is decoded from its cached `.mp3` file with the MP3 type stated, not from loose bytes.
/// - While Live's engine runs (voice processing on), the clip goes through that engine like the tutor's speech:
///   an `AVAudioPlayer` beside a voice-processing engine is "other audio" and gets ducked to a whisper, and
///   re-categorising the session under a running engine would stop it.
/// - Otherwise the session is set to playback for the clip and handed back afterwards.
/// End of playback is polled on the main actor rather than taken from `AVAudioPlayerDelegate`, whose callbacks
/// are not promised to arrive on the main thread.
@MainActor @Observable
final class VoicePreviewPlayer {
    /// Voice whose clip is playing.
    private(set) var playingId: String?
    /// Voice whose clip is downloading.
    private(set) var loadingId: String?
    /// Voice whose clip could not be fetched or played.
    private(set) var failedId: String?

    @ObservationIgnored private var player: AVAudioPlayer?
    @ObservationIgnored private var task: Task<Void, Never>?
    @ObservationIgnored private var ownsSession = false
    @ObservationIgnored private weak var engineHub: AudioHub?

    nonisolated static var cacheDir: URL {
        let base = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        return base.appendingPathComponent("voices", isDirectory: true)
    }

    /// Plays this voice's clip, or stops it when it is the one playing.
    func toggle(_ id: String, controller: LiveController) {
        if playingId == id || loadingId == id { stop(); return }
        stop()
        failedId = nil
        controller.prepareForPreview()
        Diagnostics.log("voice: preview \(id)")
        if controller.isMock, controller.options.scene != nil {
            // Screenshot scenes hold the playing state without sound.
            playingId = id
            task = Task { [weak self] in
                try? await Task.sleep(for: .seconds(120))
                guard !Task.isCancelled else { return }
                self?.playingId = nil
            }
            return
        }
        let source: ClipSource = controller.isMock ? .bundled : .server(controller.voicePreviewRequest(id))
        let hub = controller.audio
        loadingId = id
        task = Task { [weak self] in
            let file = await Self.clip(id, source: source)
            guard let self, !Task.isCancelled else { return }
            self.loadingId = nil
            guard let file else { self.fail(id); return }
            if let hub, hub.isRunning {
                // Live is on underneath: through the engine, like the tutor's own voice.
                guard let data = try? Data(contentsOf: file) else { self.fail(id); return }
                self.engineHub = hub
                self.playingId = id
                let played = await hub.play(mp3: data)
                guard !Task.isCancelled else { return }
                self.engineHub = nil
                self.playingId = nil
                if !played { self.fail(id) }
                return
            }
            guard self.start(file) else { self.fail(id); return }
            self.playingId = id
            // Poll for the end (cheap: a few checks per second while a clip plays).
            while !Task.isCancelled, self.player?.isPlaying == true {
                try? await Task.sleep(for: .milliseconds(150))
            }
            guard !Task.isCancelled else { return }
            self.finish()
        }
    }

    /// Stops any clip (leaving the screen, picking another voice).
    func stop() {
        task?.cancel()
        task = nil
        loadingId = nil
        engineHub?.stopSpeaking()
        engineHub = nil
        finish()
    }

    private func fail(_ id: String) {
        failedId = id
        Haptics.warning()
    }

    private func finish() {
        player?.stop()
        player = nil
        playingId = nil
        if ownsSession {
            ownsSession = false
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        }
    }

    private func start(_ file: URL) -> Bool {
        let session = AVAudioSession.sharedInstance()
        do {
            if session.category != .playback || session.mode != .spokenAudio {
                try session.setCategory(.playback, mode: .spokenAudio)
            }
            try session.setActive(true)
            ownsSession = true
        } catch {
            Diagnostics.log("voice: session \(error.localizedDescription)")
        }
        do {
            let p = try AVAudioPlayer(contentsOf: file, fileTypeHint: AVFileType.mp3.rawValue)
            p.prepareToPlay()
            let route = session.currentRoute.outputs.map(\.portType.rawValue).joined(separator: ",")
            guard p.play() else {
                Diagnostics.log("voice: play() refused (\(String(format: "%.1f", p.duration)) s, route \(route))")
                return false
            }
            Diagnostics.log("voice: playing \(String(format: "%.1f", p.duration)) s on \(route)")
            player = p
            return true
        } catch {
            Diagnostics.log("voice: clip not playable \((error as NSError).domain) \((error as NSError).code)")
            return false
        }
    }

    enum ClipSource: Sendable {
        case server(URLRequest?)
        /// Mock runs: the real ID3-tagged clip of one server voice, bundled.
        case bundled
    }

    /// The cached clip's file, or the clip downloaded, checked and cached. nil when there is no playable clip.
    nonisolated static func clip(_ id: String, source: ClipSource) async -> URL? {
        await cached([id], source: source)
    }

    /// A clip under `Caches/voices/<path…>.mp3` (a preview: `[voiceId]`; a wait clip: `[voiceId, fillerId]`).
    nonisolated static func cached(_ path: [String], source: ClipSource) async -> URL? {
        let parts = path.map { $0.filter { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" } }
        guard !parts.isEmpty, !parts.contains(where: \.isEmpty) else { return nil }
        let safe = parts.joined(separator: "/")
        let file = cacheDir.appendingPathComponent(safe + ".mp3")
        if let cached = try? Data(contentsOf: file) {
            if AudioSniff.isMP3(cached) { return file }
            // A body that was never a clip: drop it so the next tap downloads again.
            Diagnostics.log("voice: cached \(safe) is \(AudioSniff.kind(cached).rawValue), dropped")
            try? FileManager.default.removeItem(at: file)
        }
        let data: Data
        switch source {
        case .bundled:
            guard let url = Bundle.main.url(forResource: "mock-voice", withExtension: "mp3"),
                  let d = try? Data(contentsOf: url) else { return nil }
            data = d
        case .server(let request):
            guard let request else { return nil }
            do {
                let (d, response) = try await URLSession.shared.data(for: request)
                let http = response as? HTTPURLResponse
                guard http?.statusCode == 200 else {
                    Diagnostics.log("voice: preview HTTP \(http?.statusCode ?? 0)")
                    return nil
                }
                data = d
            } catch {
                Diagnostics.log("voice: preview download \(error.localizedDescription)")
                return nil
            }
        }
        guard AudioSniff.isMP3(data) else {
            let head = data.prefix(16).map { String(format: "%02x", $0) }.joined()
            Diagnostics.log("voice: preview body is \(AudioSniff.kind(data).rawValue), \(data.count) bytes, starts \(head)")
            return nil
        }
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard (try? data.write(to: file, options: .atomic)) != nil else { return nil }
        Diagnostics.log("voice: clip \(safe) \(data.count) bytes (\(AudioSniff.kind(data).rawValue), tag \(AudioSniff.id3Size(data)))")
        return file
    }
}

extension VoicePreviewPlayer {
    /// `-mockAudioProbe 1`: how this phone's decoder takes the bundled ID3-tagged clip, every way the app could
    /// hand it over. Shown to the UI test (and only then) as an accessibility value.
    static func probe() -> String {
        guard let url = Bundle.main.url(forResource: "mock-voice", withExtension: "mp3"),
              let data = try? Data(contentsOf: url) else { return "probe: no clip" }
        func run(_ make: () throws -> AVAudioPlayer) -> String {
            do {
                let p = try make()
                return p.prepareToPlay() ? String(format: "%.2fs", p.duration) : "noprepare"
            } catch { return "FAIL \((error as NSError).code)" }
        }
        let plain = run { try AVAudioPlayer(data: data) }
        let hinted = run { try AVAudioPlayer(data: data, fileTypeHint: AVFileType.mp3.rawValue) }
        let file = run { try AVAudioPlayer(contentsOf: url, fileTypeHint: AVFileType.mp3.rawValue) }
        return "kind=\(AudioSniff.kind(data).rawValue) tag=\(AudioSniff.id3Size(data)) data=\(plain) dataHint=\(hinted) file=\(file)"
    }
}
