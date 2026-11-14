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
        guard let out = convert(pcm) else { return }
        lock.lock(); pending += 1; gotAudio = true; lock.unlock()
        node.scheduleBuffer(out, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            guard let self else { return }
            self.lock.lock(); self.pending -= 1; let done = self.pending == 0 && self.writingDone; self.lock.unlock()
            if done { self.finish() }
        }
        if !node.isPlaying { node.play() }
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
@MainActor
final class AudioHub {
    let inputLevel = LevelMeter()
    let outputLevel = LevelMeter()

    private let engine = AVAudioEngine()
    private let ttsNode = AVAudioPlayerNode()
    private let fileNode = AVAudioPlayerNode()
    private let earconNode = AVAudioPlayerNode()
    private let ttsFormat = AVAudioFormat(standardFormatWithSampleRate: 22_050, channels: 1)!
    private let synth = AVSpeechSynthesizer()
    private var feeder: TTSFeeder?
    private var started = false
    private var earcons: [Earcon: AVAudioPCMBuffer] = [:]

    private var recognizer: SFSpeechRecognizer?
    private var recognitionTask: SFSpeechRecognitionTask?
    private var requestBox: RequestBox?
    private var collector: PCMCollector?
    private(set) var listening = false
    private var transcript = ""
    private var transcriptFinal = false
    private var observers: [NSObjectProtocol] = []

    enum Grant: Sendable { case granted, undetermined, denied }

    // MARK: Lifecycle

    func start() {
        guard !started else { return }
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker, .allowBluetoothA2DP])
            try session.setActive(true)
        } catch {
            print("audio session error: \(error)")
        }
        // Echo cancellation so the loudspeaker can be used while the mic is open.
        do { try engine.inputNode.setVoiceProcessingEnabled(true) } catch { print("voice processing: \(error)") }
        // The voice-processing input keeps running with the engine; keep it muted except while listening.
        engine.inputNode.isVoiceProcessingInputMuted = true
        for n in [ttsNode, fileNode, earconNode] { engine.attach(n) }
        engine.connect(ttsNode, to: engine.mainMixerNode, format: ttsFormat)
        engine.connect(earconNode, to: engine.mainMixerNode, format: Earcon.format)
        engine.connect(fileNode, to: engine.mainMixerNode, format: nil)
        Self.installOutputTap(engine.mainMixerNode, meter: outputLevel)
        earcons = Earcon.renderAll()
        engine.prepare()
        do { try engine.start(); started = true } catch { print("engine start: \(error)") }
        installObservers()
    }

    /// Restarts the engine after route changes (headphones, Bluetooth) and phone-call interruptions.
    private func installObservers() {
        guard observers.isEmpty else { return }
        let nc = NotificationCenter.default
        observers.append(nc.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.recoverEngine() }
        })
        observers.append(nc.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
            let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
            MainActor.assumeIsolated {
                if raw == AVAudioSession.InterruptionType.ended.rawValue {
                    try? AVAudioSession.sharedInstance().setActive(true)
                    self?.recoverEngine()
                }
            }
        })
    }

    private func recoverEngine() {
        guard started, !engine.isRunning else { return }
        do { try engine.start() } catch { print("engine restart: \(error)") }
    }

    private nonisolated static func installOutputTap(_ node: AVAudioMixerNode, meter: LevelMeter) {
        node.installTap(onBus: 0, bufferSize: 1024, format: nil) { buffer, _ in
            meter.set(db: LevelMeter.rmsDB(buffer))
        }
    }

    // MARK: Speaking

    /// Speaks with the system voice; returns when playback finished or was stopped.
    func speak(_ text: String, lang: Lang) async {
        start()
        stopSpeaking()
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = AVSpeechSynthesisVoice(language: lang == .vi ? "vi-VN" : "en-US")
        utterance.rate = AVSpeechUtteranceDefaultSpeechRate
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

    /// Plays server mp3; returns false when the data cannot be decoded.
    func play(mp3 data: Data) async -> Bool {
        start()
        stopSpeaking()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("say-\(UUID().uuidString).mp3")
        guard (try? data.write(to: url)) != nil, let file = try? AVAudioFile(forReading: url) else { return false }
        defer { try? FileManager.default.removeItem(at: url) }
        engine.disconnectNodeOutput(fileNode)
        engine.connect(fileNode, to: engine.mainMixerNode, format: file.processingFormat)
        if !engine.isRunning { try? engine.start() }
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            Self.schedule(file, on: fileNode) { cont.resume() }
            fileNode.play()
        }
        return true
    }

    private nonisolated static func schedule(_ file: AVAudioFile, on node: AVAudioPlayerNode, done: @escaping @Sendable () -> Void) {
        node.scheduleFile(file, at: nil, completionCallbackType: .dataPlayedBack) { _ in done() }
    }

    func stopSpeaking() {
        if synth.isSpeaking { synth.stopSpeaking(at: .immediate) }
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

    func startListening(lang: Lang, serverSTT: Bool) {
        start()
        guard !listening else { return }
        transcript = ""; transcriptFinal = false
        let collector = serverSTT ? PCMCollector() : nil
        var request: SFSpeechAudioBufferRecognitionRequest?
        if !serverSTT {
            let rec = SFSpeechRecognizer(locale: Locale(identifier: lang == .vi ? "vi-VN" : "en-US"))
            recognizer = rec
            let r = SFSpeechAudioBufferRecognitionRequest()
            r.shouldReportPartialResults = true
            if rec?.supportsOnDeviceRecognition == true { r.requiresOnDeviceRecognition = true }
            r.taskHint = .dictation
            request = r
            if let rec {
                recognitionTask = Self.recognize(rec, r) { [weak self] text, final in
                    Task { @MainActor in
                        self?.transcript = text
                        if final { self?.transcriptFinal = true }
                    }
                }
            }
        }
        let box = RequestBox(request)
        requestBox = box
        self.collector = collector
        let input = engine.inputNode
        input.removeTap(onBus: 0)
        input.isVoiceProcessingInputMuted = false
        Self.installInputTap(input, meter: inputLevel, box: box, collector: collector)
        if !engine.isRunning { try? engine.start() }
        listening = true
    }

    private nonisolated static func recognize(_ rec: SFSpeechRecognizer, _ r: SFSpeechAudioBufferRecognitionRequest,
                                              _ handler: @escaping @Sendable (String, Bool) -> Void) -> SFSpeechRecognitionTask {
        rec.recognitionTask(with: r) { result, _ in
            if let result { handler(result.bestTranscription.formattedString, result.isFinal) }
        }
    }

    private nonisolated static func installInputTap(_ node: AVAudioInputNode, meter: LevelMeter, box: RequestBox, collector: PCMCollector?) {
        let format = node.outputFormat(forBus: 0)
        node.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
            meter.set(db: LevelMeter.rmsDB(buffer))
            box.request?.append(buffer)
            collector?.append(buffer)
        }
    }

    enum Heard { case text(String), wav(Data), nothing }

    /// Stops the mic and returns what was heard.
    func stopListening(discard: Bool = false) async -> Heard {
        guard listening else { return .nothing }
        listening = false
        engine.inputNode.removeTap(onBus: 0)
        engine.inputNode.isVoiceProcessingInputMuted = true
        inputLevel.set(0)
        if let collector {
            self.collector = nil
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
        start()
        guard started, let buffer = earcons[e] else { return }
        earconNode.scheduleBuffer(buffer, at: nil, options: .interrupts, completionHandler: nil)
        if !earconNode.isPlaying { earconNode.play() }
    }
}
