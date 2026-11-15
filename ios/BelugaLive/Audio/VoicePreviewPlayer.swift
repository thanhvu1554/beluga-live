import AVFoundation
import Observation

/// Plays the stored preview clip of a tutor voice (`GET /voice/<id>.mp3`), one at a time.
/// Clips are cached in Caches/voices after the first download. Playback uses a plain `AVAudioPlayer`:
/// when Live's engine runs it shares that audio session as is (no category change, which would stop the engine);
/// otherwise the session is set to playback for the clip and handed back afterwards.
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

    private nonisolated static var cacheDir: URL {
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
        if controller.isMock {
            // No server in mock runs: show the playing state for the length of a real clip.
            playingId = id
            let seconds: Double = controller.options.scene == nil ? 4 : 120   // screenshots hold the playing state
            task = Task { [weak self] in
                try? await Task.sleep(for: .seconds(seconds))
                guard !Task.isCancelled else { return }
                self?.playingId = nil
            }
            return
        }
        let request = controller.voicePreviewRequest(id)
        let engineRunning = controller.audio?.isRunning ?? false
        loadingId = id
        task = Task { [weak self] in
            let data = await Self.clip(id, request: request)
            guard let self, !Task.isCancelled else { return }
            self.loadingId = nil
            guard let data, self.start(data, engineRunning: engineRunning) else {
                self.failedId = id
                return
            }
            self.playingId = id
            // Poll for the end (cheap: a few checks per second while a clip plays).
            while !Task.isCancelled, self.player?.isPlaying == true {
                try? await Task.sleep(for: .milliseconds(200))
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
        finish()
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

    private func start(_ data: Data, engineRunning: Bool) -> Bool {
        let session = AVAudioSession.sharedInstance()
        if !engineRunning {
            do {
                try session.setCategory(.playback, mode: .spokenAudio)
                try session.setActive(true)
                ownsSession = true
            } catch {
                Diagnostics.log("voice: session \(error.localizedDescription)")
            }
        }
        do {
            let p = try AVAudioPlayer(data: data)
            p.prepareToPlay()
            guard p.play() else { return false }
            player = p
            return true
        } catch {
            Diagnostics.log("voice: clip not playable \(error.localizedDescription)")
            return false
        }
    }

    /// Cached clip, or downloaded (and cached) with the pairing token.
    private nonisolated static func clip(_ id: String, request: URLRequest?) async -> Data? {
        let safe = id.filter { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }
        let file = cacheDir.appendingPathComponent(safe + ".mp3")
        if !safe.isEmpty, let cached = try? Data(contentsOf: file), !cached.isEmpty { return cached }
        guard let request else { return nil }
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200, !data.isEmpty else {
                Diagnostics.log("voice: preview HTTP \((response as? HTTPURLResponse)?.statusCode ?? 0)")
                return nil
            }
            if !safe.isEmpty {
                try? FileManager.default.createDirectory(at: cacheDir, withIntermediateDirectories: true)
                try? data.write(to: file, options: .atomic)
            }
            return data
        } catch {
            Diagnostics.log("voice: preview download \(error.localizedDescription)")
            return nil
        }
    }
}
