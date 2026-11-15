import AVFoundation
import Speech
import Synchronization
import BelugaCore

/// Thread-safe audio level (0...1) written from audio threads, read by the UI.
final class LevelMeter: Sendable {
    private let state = Mutex<Float>(0)
    var value: Float { state.withLock { $0 } }
    func set(_ v: Float) { state.withLock { $0 = max(0, min(1, v)) } }

    /// Maps dBFS to 0...1 (-55 dB = silence, -10 dB = full).
    func set(db: Float) { set((db + 55) / 45) }

    static func rmsDB(_ buffer: AVAudioPCMBuffer) -> Float {
        let n = Int(buffer.frameLength)
        guard n > 0 else { return -100 }
        var sum: Float = 0
        if let ch = buffer.floatChannelData?[0] {
            for i in 0..<n { sum += ch[i] * ch[i] }
        } else if let ch = buffer.int16ChannelData?[0] {
            for i in 0..<n { let s = Float(ch[i]) / 32768; sum += s * s }
        } else { return -100 }
        let rms = sqrt(sum / Float(n))
        return 20 * log10(max(rms, 1e-7))
    }
}

/// True the first time `take()` is called (feeds a converter exactly one buffer).
final class OnceFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var used = false
    func take() -> Bool { lock.lock(); defer { lock.unlock() }; if used { return false }; used = true; return true }
}

/// Collects mic audio as 16 kHz mono PCM16 for server-side STT.
final class PCMCollector: @unchecked Sendable {
    private let lock = NSLock()
    private var data = Data()
    private var converter: AVAudioConverter?
    let outFormat = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: true)!

    func append(_ buffer: AVAudioPCMBuffer) {
        lock.lock(); defer { lock.unlock() }
        if converter == nil || converter?.inputFormat != buffer.format {
            converter = AVAudioConverter(from: buffer.format, to: outFormat)
        }
        guard let converter else { return }
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * outFormat.sampleRate / buffer.format.sampleRate) + 32
        guard let out = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return }
        let fed = OnceFlag()
        var error: NSError?
        converter.convert(to: out, error: &error) { _, status in
            if fed.take() { status.pointee = .haveData; return buffer }
            status.pointee = .noDataNow; return nil
        }
        if let p = out.int16ChannelData?[0], out.frameLength > 0 {
            data.append(UnsafeBufferPointer(start: p, count: Int(out.frameLength)))
        }
    }

    /// Complete WAV file (RIFF header + PCM16 mono 16 kHz).
    func wav() -> Data {
        lock.lock(); let pcm = data; lock.unlock()
        var h = Data()
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { h.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { h.append(contentsOf: $0) } }
        h.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + pcm.count))
        h.append(contentsOf: Array("WAVE".utf8)); h.append(contentsOf: Array("fmt ".utf8))
        u32(16); u16(1); u16(1); u32(16_000); u32(32_000); u16(2); u16(16)
        h.append(contentsOf: Array("data".utf8)); u32(UInt32(pcm.count))
        return h + pcm
    }
}

/// Feeds AVSpeechSynthesizer output buffers into a player node, so the voice goes through the
/// same engine (echo cancellation reference + real output level for the pill).
final class TTSFeeder: @unchecked Sendable {
    private let lock = NSLock()
    private let node: AVAudioPlayerNode
    private let format: AVAudioFormat
    private var converter: AVAudioConverter?
    private var pending = 0
    private var writingDone = false
    private var finished = false
    private var gotAudio = false
    private let onFinish: @Sendable () -> Void

    init(node: AVAudioPlayerNode, format: AVAudioFormat, onFinish: @escaping @Sendable () -> Void) {
        self.node = node; self.format = format; self.onFinish = onFinish
    }

    var hasAudio: Bool { lock.lock(); defer { lock.unlock() }; return gotAudio }

    func receive(_ buffer: AVAudioBuffer) {
        guard let pcm = buffer as? AVAudioPCMBuffer, pcm.frameLength > 0 else {
            lock.lock(); writingDone = true; let done = pending == 0; lock.unlock()
            if done { finish() }
            return
        }
        // The node was started (`play()`) on the main actor before the synthesizer began. Scheduling on a node
        // whose engine stopped meanwhile (route change, interruption) is pointless: give up on this line.
        guard node.engine?.isRunning == true else { finish(); return }
        guard let out = convert(pcm) else { return }
        lock.lock(); pending += 1; gotAudio = true; lock.unlock()
        node.scheduleBuffer(out, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            guard let self else { return }
            self.lock.lock(); self.pending -= 1; let done = self.pending == 0 && self.writingDone; self.lock.unlock()
            if done { self.finish() }
        }
    }

    func finish() {
        lock.lock(); let first = !finished; finished = true; lock.unlock()
        if first { onFinish() }
    }

    private func convert(_ pcm: AVAudioPCMBuffer) -> AVAudioPCMBuffer? {
        if pcm.format == format { return pcm }
        if converter == nil || converter?.inputFormat != pcm.format {
            converter = AVAudioConverter(from: pcm.format, to: format)
        }
        guard let converter else { return nil }
        let cap = AVAudioFrameCount(Double(pcm.frameLength) * format.sampleRate / pcm.format.sampleRate) + 64
        guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: cap) else { return nil }
        let fed = OnceFlag()
        var err: NSError?
        converter.convert(to: out, error: &err) { _, status in
            if fed.take() { status.pointee = .haveData; return pcm }
            status.pointee = .noDataNow; return nil
        }
        return out.frameLength > 0 ? out : nil
    }
}

final class RequestBox: @unchecked Sendable {
    let request: SFSpeechAudioBufferRecognitionRequest?
    init(_ r: SFSpeechAudioBufferRecognitionRequest?) { request = r }
}

/// Owns the audio engine: speech out (system voice or server mp3), mic in (STT), earcons.
///
/// AVAudioEngine reports misuse with Objective-C exceptions that Swift cannot catch, so every call that can raise
/// one is guarded here:
/// - the graph (attach, connect, output tap) is built once per engine instance: a second `installTap` on the same
///   bus raises "Tap() already exists";
/// - `AVAudioPlayerNode.play()` raises when the engine is not running, so it is only called on the main actor right
///   after `ensureRunning()` confirmed the engine runs (route changes, phone calls, Siri and backgrounding all stop
///   the engine behind our back);
/// - the input tap uses the input's current format and only when that format is valid (0 Hz / 0 channels raises);
/// - a media-services reset invalidates every node: the engine is rebuilt from scratch.
/// The microphone side (voice processing) is only set up once record permission is granted; before that the
/// engine is output-only, so speaking never triggers a permission prompt or fails for lack of one.
@MainActor
final class AudioHub {
    let inputLevel = LevelMeter()
    let outputLevel = LevelMeter()

    private var engine = AVAudioEngine()
    private var ttsNode = AVAudioPlayerNode()
    private var fileNode = AVAudioPlayerNode()
    private var earconNode = AVAudioPlayerNode()
    /// Wait clips (PROTOCOL section 21) play on their own node: the tutor's voice and a clip never stop each other
    /// by accident, and a clip can fade out on its own.
    private var fillerNode = AVAudioPlayerNode()
    private let ttsFormat = AVAudioFormat(standardFormatWithSampleRate: 22_050, channels: 1)!
    private let synth = AVSpeechSynthesizer()
    private var feeder: TTSFeeder?
    private var fillerFade: Task<Void, Never>?
    /// Graph built for the current engine instance.
    private var configured = false
    /// The current engine has the voice-processing input (built after record permission was granted).
    private var hasInput = false
    private var earcons: [Earcon: AVAudioPCMBuffer] = [:]

    private var recognizer: SFSpeechRecognizer?
    private var recognitionTask: SFSpeechRecognitionTask?
    private var requestBox: RequestBox?
    private var collector: PCMCollector?
    private(set) var listening = false
    private var transcript = ""
    private var transcriptFinal = false
    private var observers: [NSObjectProtocol] = []
    /// `suspend()` gave the session back (chat): only an explicit `ensureRunning()` from a caller wakes the engine,
    /// never a route change or the end of an interruption.
    private var suspended = false
    /// The app is in the background outside a call: nothing may start the engine (with the `audio` background mode
    /// a running engine would keep the app awake, and a play-and-record session cannot start from the background).
    private var backgroundHold = false

    enum Grant: Sendable { case granted, undetermined, denied }

    /// The engine runs (the shared audio session is set up for it).
    var isRunning: Bool { configured && engine.isRunning }

    // MARK: Lifecycle

    /// Builds the graph if needed and starts the engine. Returns whether it runs.
    @discardableResult
    func ensureRunning() -> Bool {
        guard !backgroundHold else { return false }
        suspended = false
        let wantInput = Self.microphoneGrant() == .granted
        if configured, wantInput, !hasInput {
            Diagnostics.log("audio: rebuilding engine with microphone")
            rebuild()
        }
        if !configured {
            // Session first: voice processing is set up against a play-and-record session.
            prepareSession(record: wantInput)
            configure(withInput: wantInput)
        }
        guard !engine.isRunning else { return true }
        prepareSession(record: hasInput)
        engine.prepare()
        do {
            try engine.start()
            Diagnostics.log("audio: engine started (input \(hasInput))")
        } catch {
            Diagnostics.log("audio: engine start failed \(error.localizedDescription)")
        }
        return engine.isRunning
    }

    /// Category (only when it differs: setting it re-routes audio) and activation. Only while the engine is stopped.
    private func prepareSession(record: Bool) {
        let session = AVAudioSession.sharedInstance()
        do {
            let category: AVAudioSession.Category = record ? .playAndRecord : .playback
            let options: AVAudioSession.CategoryOptions = record ? [.defaultToSpeaker, .allowBluetoothA2DP] : []
            if session.category != category || session.categoryOptions != options {
                try session.setCategory(category, mode: .default, options: options)
            }
            try session.setActive(true)
        } catch {
            Diagnostics.log("audio: session error \(error.localizedDescription)")
        }
    }

    private func configure(withInput: Bool) {
        configured = true
        hasInput = withInput
        if withInput {
            // Echo cancellation so the loudspeaker can be used while the mic is open.
            do { try engine.inputNode.setVoiceProcessingEnabled(true) } catch {
                Diagnostics.log("audio: voice processing \(error.localizedDescription)")
            }
            // The voice-processing input keeps running with the engine; keep it muted except while listening.
            engine.inputNode.isVoiceProcessingInputMuted = true
        }
        for n in [ttsNode, fileNode, earconNode, fillerNode] { engine.attach(n) }
        engine.connect(ttsNode, to: engine.mainMixerNode, format: ttsFormat)
        engine.connect(earconNode, to: engine.mainMixerNode, format: Earcon.format)
        engine.connect(fileNode, to: engine.mainMixerNode, format: nil)
        engine.connect(fillerNode, to: engine.mainMixerNode, format: nil)
        Self.installOutputTap(engine.mainMixerNode, meter: outputLevel)
        if earcons.isEmpty { earcons = Earcon.renderAll() }
        installObservers()
    }

    /// A fresh engine and nodes (after a media-services reset, or to add the microphone).
    /// `afterReset`: the old engine and nodes are dead (media services reset) and are not touched again.
    private func rebuild(afterReset: Bool = false) {
        if afterReset {
            feeder?.finish(); feeder = nil
            listening = false
            recognitionTask?.cancel()
            recognitionTask = nil; requestBox = nil; collector = nil
            inputLevel.set(0); outputLevel.set(0)
        } else {
            stopSpeaking()
            endListeningNow()
            if configured {
                engine.mainMixerNode.removeTap(onBus: 0)
                engine.stop()
            }
        }
        engine = AVAudioEngine()
        ttsNode = AVAudioPlayerNode()
        fileNode = AVAudioPlayerNode()
        earconNode = AVAudioPlayerNode()
        fillerNode = AVAudioPlayerNode()
        configured = false
        hasInput = false
    }

    /// Leaving Live or ending dictation: stop the engine and give the audio session back (the microphone
    /// indicator goes off, other apps' audio may resume). `ensureRunning()` brings it back.
    func suspend() {
        guard configured else { return }
        suspended = true
        stopSpeaking()
        endListeningNow()
        if engine.isRunning { engine.stop() }
        deactivateSession()
        Diagnostics.log("audio: suspended")
    }

    private func deactivateSession() {
        do {
            try AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        } catch {
            Diagnostics.log("audio: deactivate \(error.localizedDescription)")
        }
    }

    /// The app went to the background outside a call. Before the `audio` background mode existed iOS stopped the
    /// engine and suspended the app; this keeps it that way: speech stops, the engine stops (the microphone tap stays
    /// installed, as when the system stopped it) and the session is released until `endBackgroundHold()`.
    func holdForBackground() {
        backgroundHold = true
        guard configured else { return }
        stopSpeaking()
        if engine.isRunning { engine.stop() }
        deactivateSession()
        Diagnostics.log("audio: held for background")
    }

    /// Back in the foreground: the engine may start again (callers decide whether it should).
    func endBackgroundHold() {
        backgroundHold = false
    }

    /// Restarts the engine after route changes, interruptions and media-services resets.
    private func installObservers() {
        guard observers.isEmpty else { return }
        let nc = NotificationCenter.default
        // Registered for any engine (it is replaced on rebuild); queue .main, so assumeIsolated holds.
        observers.append(nc.addObserver(forName: .AVAudioEngineConfigurationChange, object: nil, queue: .main) { [weak self] note in
            let source = note.object.map { ObjectIdentifier($0 as AnyObject) }
            MainActor.assumeIsolated { self?.configurationChanged(source: source) }
        })
        observers.append(nc.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
            let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
            MainActor.assumeIsolated { self?.interruption(began: raw == AVAudioSession.InterruptionType.began.rawValue) }
        })
        observers.append(nc.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                Diagnostics.log("audio: media services reset")
                self?.rebuild(afterReset: true)
            }
        })
    }

    private func configurationChanged(source: ObjectIdentifier?) {
        guard configured, source == nil || source == ObjectIdentifier(engine) else { return }
        Diagnostics.log("audio: configuration change (listening \(listening), suspended \(suspended), hold \(backgroundHold))")
        // Chat (suspended) or background outside a call: leave the engine off; the next real use restarts it.
        guard !suspended, !backgroundHold else { return }
        // The engine stopped itself; a player waiting for completion may never get it.
        stopSpeaking()
        let wasListening = listening
        if wasListening { engine.inputNode.removeTap(onBus: 0) }
        guard ensureRunning() else { return }
        // Formats may have changed (Bluetooth headset, other sample rate): put the mic tap back with the new one.
        if wasListening, let box = requestBox, !installInputTap(box: box, collector: collector) {
            Diagnostics.log("audio: input tap not restored")
        }
    }

    private func interruption(began: Bool) {
        Diagnostics.log("audio: interruption \(began ? "began" : "ended")")
        if began {
            stopSpeaking()
        } else if configured, !suspended, !backgroundHold {
            // Only Live and the call restart the engine; in chat it was suspended and stays off.
            ensureRunning()
        }
    }

    /// Back in the foreground in Live: the engine was stopped while the app was away.
    func resumeIfNeeded() {
        guard configured, !engine.isRunning else { return }
        ensureRunning()
    }

    private nonisolated static func installOutputTap(_ node: AVAudioMixerNode, meter: LevelMeter) {
        node.installTap(onBus: 0, bufferSize: 1024, format: nil) { buffer, _ in
            meter.set(db: LevelMeter.rmsDB(buffer))
        }
    }

    // MARK: Speaking

    /// Speaks with the system voice; returns when playback finished or was stopped.
    func speak(_ text: String, lang: Lang) async {
        stopSpeaking()
        guard ensureRunning() else { return }
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = AVSpeechSynthesisVoice(language: lang == .vi ? "vi-VN" : "en-US")
        utterance.rate = AVSpeechUtteranceDefaultSpeechRate
        // Started here, on the main actor, while the engine is known to run; the feeder only schedules buffers.
        ttsNode.play()
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            let feeder = TTSFeeder(node: ttsNode, format: ttsFormat) { cont.resume() }
            self.feeder = feeder
            Self.write(synth, utterance, feeder)
            // Watchdog: if the synthesizer never produces audio, do not block the lesson.
            Task { @MainActor [weak feeder] in
                try? await Task.sleep(for: .seconds(4))
                if let feeder, !feeder.hasAudio { feeder.finish() }
            }
        }
        if feeder != nil { feeder = nil }
    }

    private nonisolated static func write(_ synth: AVSpeechSynthesizer, _ u: AVSpeechUtterance, _ feeder: TTSFeeder) {
        synth.write(u) { buffer in feeder.receive(buffer) }
    }

    /// Plays server mp3; returns false when it cannot (undecodable data, engine not running).
    func play(mp3 data: Data) async -> Bool {
        stopSpeaking()
        guard ensureRunning() else { return false }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("say-\(UUID().uuidString).mp3")
        guard (try? data.write(to: url)) != nil else { return false }
        defer { try? FileManager.default.removeItem(at: url) }
        guard let file = try? AVAudioFile(forReading: url) else { return false }
        let format = file.processingFormat
        // A format the mixer cannot take would raise inside `connect`.
        guard format.sampleRate > 0, (1...2).contains(format.channelCount) else {
            Diagnostics.log("audio: mp3 format rejected \(format)")
            return false
        }
        engine.disconnectNodeOutput(fileNode)
        engine.connect(fileNode, to: engine.mainMixerNode, format: format)
        guard engine.isRunning else { return false }
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            Self.schedule(file, on: fileNode) { cont.resume() }
            fileNode.play()
        }
        return true
    }

    private nonisolated static func schedule(_ file: AVAudioFile, on node: AVAudioPlayerNode, done: @escaping @Sendable () -> Void) {
        node.scheduleFile(file, at: nil, completionCallbackType: .dataPlayedBack) { _ in done() }
    }

    /// Plays a wait clip; returns when it ended or was stopped (false when it could not start).
    func playFiller(_ url: URL) async -> Bool {
        fillerNode.stop()
        guard ensureRunning(), let file = try? AVAudioFile(forReading: url) else { return false }
        let format = file.processingFormat
        guard format.sampleRate > 0, (1...2).contains(format.channelCount) else { return false }
        engine.disconnectNodeOutput(fillerNode)
        engine.connect(fillerNode, to: engine.mainMixerNode, format: format)
        guard engine.isRunning else { return false }
        fillerNode.volume = 1
        fillerFade?.cancel()
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            Self.schedule(file, on: fillerNode) { cont.resume() }
            fillerNode.play()
        }
        return true
    }

    /// Ends a wait clip. With `fade`, the voice dips out over about 100 ms instead of being cut mid-syllable.
    func stopFiller(fade: Bool = true) {
        fillerFade?.cancel()
        guard fillerNode.isPlaying else { return }
        guard fade else { fillerNode.stop(); return }
        let node = fillerNode
        fillerFade = Task { @MainActor in
            for v in [0.75, 0.5, 0.28, 0.1] as [Float] {
                node.volume = v
                try? await Task.sleep(for: .milliseconds(25))
                if Task.isCancelled { break }
            }
            node.stop()
            node.volume = 1
        }
    }

    func stopSpeaking() {
        if synth.isSpeaking { synth.stopSpeaking(at: .immediate) }
        // `stop()` also fires pending completion handlers, which ends the waits above.
        ttsNode.stop()
        fileNode.stop()
        feeder?.finish()
        feeder = nil
        outputLevel.set(0)
    }

    /// Speaks a short line without waiting (e.g. "Đang suy nghĩ…").
    func say(_ text: String, lang: Lang) {
        Task { await speak(text, lang: lang) }
    }

    // MARK: Listening

    static func microphoneGrant() -> Grant {
        switch AVAudioApplication.shared.recordPermission {
        case .granted: return .granted
        case .undetermined: return .undetermined
        default: return .denied
        }
    }

    static func speechGrant() -> Grant {
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized: return .granted
        case .notDetermined: return .undetermined
        default: return .denied
        }
    }

    /// Asks for whatever is still undetermined: the microphone first, then speech recognition if needed.
    static func requestPermissions(needSpeech: Bool) async -> Bool {
        var mic = microphoneGrant() == .granted
        if microphoneGrant() == .undetermined { mic = await AVAudioApplication.requestRecordPermission() }
        guard mic else { return false }
        guard needSpeech else { return true }
        if speechGrant() == .undetermined {
            return await withCheckedContinuation { (c: CheckedContinuation<Bool, Never>) in
                requestSpeechAuth { c.resume(returning: $0) }
            }
        }
        return speechGrant() == .granted
    }

    private nonisolated static func requestSpeechAuth(_ done: @escaping @Sendable (Bool) -> Void) {
        SFSpeechRecognizer.requestAuthorization { done($0 == .authorized) }
    }

    /// Opens the microphone. False when it cannot (engine not running, no valid input format): nothing changed.
    func startListening(lang: Lang, serverSTT: Bool) -> Bool {
        guard !listening else { return true }
        guard ensureRunning(), hasInput else {
            Diagnostics.log("audio: cannot listen (running \(engine.isRunning), input \(hasInput))")
            return false
        }
        transcript = ""; transcriptFinal = false
        let collector = serverSTT ? PCMCollector() : nil
        var request: SFSpeechAudioBufferRecognitionRequest?
        if !serverSTT {
            let r = SFSpeechAudioBufferRecognitionRequest()
            r.shouldReportPartialResults = true
            r.taskHint = .dictation
            request = r
        }
        let box = RequestBox(request)
        engine.inputNode.removeTap(onBus: 0)
        guard installInputTap(box: box, collector: collector) else {
            Diagnostics.log("audio: no valid input format")
            return false
        }
        if let request {
            let rec = SFSpeechRecognizer(locale: Locale(identifier: lang == .vi ? "vi-VN" : "en-US"))
            recognizer = rec
            if rec?.supportsOnDeviceRecognition == true { request.requiresOnDeviceRecognition = true }
            if let rec {
                recognitionTask = Self.recognize(rec, request) { [weak self] text, final in
                    Task { @MainActor in
                        self?.transcript = text
                        if final { self?.transcriptFinal = true }
                    }
                }
            }
        }
        requestBox = box
        self.collector = collector
        engine.inputNode.isVoiceProcessingInputMuted = false
        listening = true
        Diagnostics.log("audio: listening (\(serverSTT ? "server" : "device") STT)")
        return true
    }

    /// Installs the mic tap with the input's current format; false (and no tap) when that format is unusable.
    private func installInputTap(box: RequestBox, collector: PCMCollector?) -> Bool {
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else { return false }
        Self.installInputTap(input, format: format, meter: inputLevel, box: box, collector: collector)
        return true
    }

    private nonisolated static func recognize(_ rec: SFSpeechRecognizer, _ r: SFSpeechAudioBufferRecognitionRequest,
                                              _ handler: @escaping @Sendable (String, Bool) -> Void) -> SFSpeechRecognitionTask {
        let onDevice = r.requiresOnDeviceRecognition
        return rec.recognitionTask(with: r) { result, error in
            if let result { handler(result.bestTranscription.formattedString, result.isFinal) }
            // Breadcrumb: device recognition is the part most likely to refuse work in the background (PiP call).
            if let error = error as NSError? {
                Diagnostics.log("speech: error \(error.domain) \(error.code) on-device \(onDevice)")
            }
        }
    }

    private nonisolated static func installInputTap(_ node: AVAudioInputNode, format: AVAudioFormat, meter: LevelMeter,
                                                    box: RequestBox, collector: PCMCollector?) {
        node.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
            meter.set(db: LevelMeter.rmsDB(buffer))
            box.request?.append(buffer)
            collector?.append(buffer)
        }
    }

    /// Drops the microphone at once (no result).
    private func endListeningNow() {
        guard listening else { return }
        listening = false
        if hasInput {
            engine.inputNode.removeTap(onBus: 0)
            engine.inputNode.isVoiceProcessingInputMuted = true
        }
        requestBox?.request?.endAudio()
        recognitionTask?.cancel()
        recognitionTask = nil; requestBox = nil; collector = nil
        inputLevel.set(0)
    }

    enum Heard { case text(String), wav(Data), nothing }

    /// Stops the mic and returns what was heard.
    func stopListening(discard: Bool = false) async -> Heard {
        guard listening else { return .nothing }
        listening = false
        if hasInput {
            engine.inputNode.removeTap(onBus: 0)
            engine.inputNode.isVoiceProcessingInputMuted = true
        }
        inputLevel.set(0)
        if let collector {
            self.collector = nil
            requestBox = nil
            return discard ? .nothing : .wav(collector.wav())
        }
        requestBox?.request?.endAudio()
        if discard { recognitionTask?.cancel() }
        // Wait briefly for the final transcription.
        var waited = 0
        while !discard, !transcriptFinal, waited < 15 {
            try? await Task.sleep(for: .milliseconds(100)); waited += 1
        }
        recognitionTask = nil; requestBox = nil
        let text = transcript.trimmingCharacters(in: .whitespacesAndNewlines)
        return discard || text.isEmpty ? .nothing : .text(text)
    }

    /// Latest partial transcription (for captions).
    var partialTranscript: String { transcript }

    // MARK: Earcons

    func play(_ e: Earcon) {
        guard ensureRunning(), let buffer = earcons[e] else { return }
        earconNode.scheduleBuffer(buffer, at: nil, options: .interrupts, completionHandler: nil)
        if !earconNode.isPlaying { earconNode.play() }
    }
}
