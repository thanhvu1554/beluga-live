import AVFoundation
import SwiftUI
import BelugaCore

// Wait clips (PROTOCOL section 21): while the tutor thinks in Live or a call, a short line in the tutor's own voice,
// made once on the server and kept on the phone. `WaitClips` (BelugaCore) decides when; this plays what it says and
// stops it the moment real speech is due. Never the system voice, never in chat.
extension LiveController {

    // MARK: Clips on the phone

    /// The catalogue arrived or the voice changed: fetch the current voice's lines (cached per voice and line).
    func prepareWaitClips(_ catalog: VoiceCatalog) {
        let lines = catalog.currentFillers
        guard let voice = catalog.current, !lines.isEmpty else {
            fillerVoice = nil
            fillerFiles = [:]
            waitClips.setClips([])
            return
        }
        guard voice != fillerVoice || fillerFiles.count < lines.count else { return }
        fillerVoice = voice
        if isMock {
            // No server: every line "has" the bundled clip; playback is timed, not heard.
            fillerFiles = Dictionary(uniqueKeysWithValues: lines.map { ($0.id, URL(fileURLWithPath: "/mock/\($0.id).mp3")) })
            waitClips.setClips(lines)
            return
        }
        let requests = lines.map { ($0.id, fillerRequest(voice: voice, filler: $0.id)) }
        Task { [weak self] in
            var files: [String: URL] = [:]
            for (id, request) in requests {
                if let url = await VoicePreviewPlayer.cached([voice, id], source: .server(request)) { files[id] = url }
            }
            guard let self, self.fillerVoice == voice else { return }
            self.fillerFiles = files
            // Only lines whose clip is on the phone can ever be chosen.
            self.waitClips.setClips(lines.filter { files[$0.id] != nil })
            Diagnostics.log("wait clips: \(files.count)/\(lines.count) for \(voice)")
        }
    }

    /// `GET /voice/<voiceId>/<fillerId>.mp3` (token in the query and the header, like the preview).
    private func fillerRequest(voice: String, filler: String) -> URLRequest? {
        guard let base = settings.httpBase,
              var c = URLComponents(url: base.appendingPathComponent("voice").appendingPathComponent(voice)
                                        .appendingPathComponent(filler + ".mp3"), resolvingAgainstBaseURL: false) else { return nil }
        c.queryItems = [URLQueryItem(name: "token", value: settings.pairToken ?? "")]
        guard let url = c.url else { return nil }
        var req = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 15)
        req.setValue("Bearer \(settings.pairToken ?? "")", forHTTPHeaderField: "Authorization")
        return req
    }

    // MARK: Driving the rules

    /// Feeds the machine (Live and calls only) and carries out what it says.
    func waitClip(_ input: WaitClipInput) {
        guard mode == .live || input != .thinkingStarted else { return }
        run(waitClips.handle(input, now: Date()))
        // A photo sent just before the question makes it a turn "with a photo" (the vision line).
        if input == .thinkingStarted, let sent = photoSentAt, Date().timeIntervalSince(sent) < 30 {
            run(waitClips.handle(.photo, now: Date()))
        }
    }

    func run(_ actions: [WaitClipAction]) {
        for action in actions {
            switch action {
            case .play(let id): playWaitClip(id)
            case .stop: stopWaitClip()
            }
        }
    }

    /// The first spoken line of the turn has its audio: it waits for a clip still talking (at most 2.5 s), and
    /// any clip left is faded out before the line starts.
    func holdForWaitClip() async {
        guard let until = waitClips.sayReady(now: Date()) else {
            if fillerTask != nil { stopWaitClip() }
            return
        }
        while !Task.isCancelled, waitClips.phase == .holding, Date() < until.addingTimeInterval(0.1) {
            try? await Task.sleep(for: .milliseconds(40))
            run(waitClips.handle(.tick, now: Date()))
        }
        if fillerTask != nil { stopWaitClip() }
    }

    private func playWaitClip(_ id: String) {
        guard let line = waitClips.availableClips.first(where: { $0.id == id }) else { return }
        Diagnostics.log("wait clip: \(id)")
        // Said aloud and shown where the tutor's words go (the old "Đang suy nghĩ…" line's place).
        fillerCaption = line.text
        if captionsOn { caption = line.text }
        setStageCaption(line.text)
        fillerTask?.cancel()
        if isMock {
            let seconds = max(1.4, Double(line.text.count) * 0.07)
            run(waitClips.handle(.clipStarted(id: id, duration: seconds), now: Date()))
            fillerTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(seconds))
                guard let self, !Task.isCancelled else { return }
                self.fillerTask = nil
                self.clearWaitCaption()
                self.run(self.waitClips.handle(.clipFinished(id: id), now: Date()))
            }
            return
        }
        guard let url = fillerFiles[id], let audio else {
            run(waitClips.handle(.clipFinished(id: id), now: Date()))
            return
        }
        let duration = (try? AVAudioFile(forReading: url)).map { Double($0.length) / max(1, $0.processingFormat.sampleRate) }
        run(waitClips.handle(.clipStarted(id: id, duration: duration), now: Date()))
        fillerTask = Task { [weak self] in
            let played = await audio.playFiller(url)
            guard let self, !Task.isCancelled else { return }
            self.fillerTask = nil
            if !played { Diagnostics.log("wait clip: \(id) did not play") }
            self.clearWaitCaption()
            self.run(self.waitClips.handle(.clipFinished(id: id), now: Date()))
        }
    }

    /// Stops a clip now (real speech is due, the turn ended, the owner spoke): a short fade, never a cut.
    func stopWaitClip() {
        fillerTask?.cancel()
        fillerTask = nil
        audio?.stopFiller()
        if let id = waitClips.playingId { _ = waitClips.handle(.clipFinished(id: id), now: Date()) }
        clearWaitCaption()
    }

    /// The clip's words leave the caption unless the tutor's own line already replaced them.
    func clearWaitCaption() {
        guard let text = fillerCaption else { return }
        fillerCaption = nil
        if caption == text { caption = "" }
        if callStage.caption == text { setStageCaption("") }
    }
}
