import SwiftUI
import UIKit
import BelugaCore

enum MockScene: String, CaseIterable {
    case idle, listening, thinking, tool, lesson, ask, done, sessions, settings, offline
}

/// Launch options: `-mock 1` and `-mockScene <name>`.
struct LaunchOptions {
    var mock: Bool
    var scene: MockScene?

    static func current() -> LaunchOptions {
        let d = UserDefaults.standard
        return LaunchOptions(mock: d.bool(forKey: "mock"), scene: d.string(forKey: "mockScene").flatMap(MockScene.init(rawValue:)))
    }
}

/// Central coordinator: transport, timeline, pill, board, audio and camera.
@MainActor @Observable
final class LiveController {
    let settings: AppSettings
    let board = BoardBridge()
    let options: LaunchOptions

    private(set) var pill = PillMachine()
    private(set) var title = ""
    private(set) var caption = ""
    private(set) var askPrompt: String?
    private(set) var sessions: [SessionItem] = []
    private(set) var notice: String?
    private(set) var connected = false
    private(set) var shutterTick = 0
    private(set) var handsFreeActive = false
    var boardVisible = false
    var showSessions = false
    var showSettings = false
    /// Scenes may force captions on for review.
    private(set) var captionsForced = false

    var pillState: PillState { pill.state }
    var isMock: Bool { options.mock }
    var captionsOn: Bool { settings.captions || captionsForced }
    var hasHost: Bool { settings.webSocketURL != nil }

    @ObservationIgnored private var transport: Transport?
    @ObservationIgnored private var timeline = Timeline()
    @ObservationIgnored private var backoff = Backoff()
    @ObservationIgnored private var ready: Ready?
    @ObservationIgnored private var sessionId: String?
    @ObservationIgnored private var needsRebuild = false
    @ObservationIgnored private var audioCache: [Int: Data] = [:]
    @ObservationIgnored private var speakTask: Task<Void, Never>?
    @ObservationIgnored private var reconnectTask: Task<Void, Never>?
    @ObservationIgnored private var tickTask: Task<Void, Never>?
    @ObservationIgnored private var noticeTask: Task<Void, Never>?
    @ObservationIgnored private var gapSince: Date?
    @ObservationIgnored private var fatalStop = false
    @ObservationIgnored private var vad = VoiceActivityDetector()
    @ObservationIgnored private(set) var audio: AudioHub?
    @ObservationIgnored let camera: CameraController?
    @ObservationIgnored private let mockLevels = MockLevels()
    @ObservationIgnored private var mockScriptedLevel: PillState?

    init(settings: AppSettings, options: LaunchOptions) {
        self.settings = settings
        self.options = options
        if options.mock {
            camera = nil
            audio = nil
        } else {
            camera = CameraController()
            audio = AudioHub()
        }
        Haptics.enabled = settings.haptics && !options.mock
        board.setLang(settings.lang)
    }

    var strings: Strings { Strings(lang: settings.lang) }

    // MARK: Start

    func start() {
        startTicker()
        if isMock {
            let mock = MockTransport()
            if options.scene == .offline { mock.reachable = false }
            attach(mock)
            if let scene = options.scene { applyScene(scene, mock: mock) } else { connect() }
            return
        }
        sessionId = settings.lastSessionId
        Task {
            _ = await CameraController.requestAccess()
            camera?.start()
            _ = await AudioHub.requestPermissions()
        }
        if hasHost { connect() } else { showSettings = true }
    }

    private func attach(_ t: Transport) {
        transport?.disconnect()
        transport = t
        t.onEvent = { [weak self] e in self?.handle(e) }
    }

    func connect() {
        reconnectTask?.cancel()
        if !isMock {
            guard let url = settings.webSocketURL else { return }
            attach(WebSocketTransport(url: url))
        }
        transport?.connect()
        let lastSeq = (board.hasContent && !needsRebuild) ? timeline.lastPlayed : 0
        if !board.hasContent, sessionId != nil { needsRebuild = true }
        transport?.send(.hello(pairCode: settings.pairCode, pairToken: settings.pairToken,
                               device: UIDevice.current.model, sessionId: sessionId,
                               lastSeq: sessionId == nil ? nil : lastSeq, settings: settings.tutor))
    }

    /// Settings that need a new connection changed (host, pair code).
    func reconnectNow() {
        fatalStop = false
        backoff.reset()
        transport?.disconnect()
        connect()
    }

    func settingsChanged() {
        Haptics.enabled = settings.haptics && !isMock
        board.setLang(settings.lang)
        if connected { transport?.send(.settings(settings.tutor)) }
    }

    // MARK: Incoming

    private func handle(_ event: TransportEvent) {
        switch event {
        case .closed:
            let wasConnected = connected
            connected = false
            stopAudioForDisconnect()
            pill.handle(.disconnected)
            if wasConnected { cue(.offline) }
            scheduleReconnect()
        case .message(let m):
            handle(m)
        }
    }

    private func handle(_ m: ServerMessage) {
        switch m {
        case .ready(let r):
            let reconnect = r.sessionId == sessionId && board.hasContent
            ready = r
            connected = true
            fatalStop = false
            backoff.reset()
            pill.handle(.connected)
            if let token = r.pairToken, !token.isEmpty { settings.pairToken = token }
            if r.sessionId != sessionId {
                sessionId = r.sessionId
                if !needsRebuild { startFreshSession() }
            }
            settings.lastSessionId = r.sessionId
            if !r.title.isEmpty { title = r.title }
            if r.boardVersion > BoardBridge.bundledVersion, let url = settings.boardURL { board.loadRemote(url) }
            if reconnect {
                showNotice(strings("notice.reconnected"))
                // Resume a say cut by the disconnect.
                if let current = timeline.current { speak(current) }
            }
            if isMock && options.scene == nil { runMockStudent() }
        case let .state(state, tool):
            pill.handle(.server(state: state, tool: tool))
        case .heard(let text):
            if captionsOn { caption = text }
        case .event(let e):
            process(timeline.receive(e))
        case let .audio(seq, _, data):
            audioCache[seq] = data
        case .title(let text):
            title = text
        case .captureRequest(let requestId):
            Task { await takePhoto(requestId: requestId) }
        case .snapshot(let events):
            process(timeline.applySnapshot(events, rebuildBoard: needsRebuild))
            needsRebuild = false
        case .sessions(let items):
            sessions = items
        case .notice(_, let text):
            showNotice(text)
        case let .error(code, text, fatal):
            if code == "bad_pair" { settings.pairToken = nil; showNotice(strings("error.badPair")) }
            else { showNotice(text.isEmpty ? code : text) }
            if fatal { fatalStop = true; transport?.disconnect(); connected = false; pill.handle(.disconnected) }
        case .unknown:
            break
        }
    }

    private func startFreshSession() {
        timeline.reset()
        board.reset()
        boardVisible = false
        title = ""
        askPrompt = nil
        audioCache.removeAll()
    }

    private func scheduleReconnect() {
        guard !fatalStop, !(isMock && options.scene == .offline) else { return }
        let delay = backoff.next()
        reconnectTask?.cancel()
        reconnectTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self, !Task.isCancelled, !self.connected else { return }
            self.connect()
        }
    }

    // MARK: Timeline actions

    private func process(_ actions: [TimelineAction]) {
        for a in actions {
            switch a {
            case .speak(let e):
                speak(e)
            case .apply(let e):
                apply(e, live: true)
                transport?.send(.played(seq: e.seq))
            case .restore(let e):
                apply(e, live: false)
            }
        }
        settings.lastPlayedSeq = timeline.lastPlayed
    }

    private func apply(_ e: LessonEvent, live: Bool) {
        switch e.kind {
        case .step, .show:
            board.apply(e)
            if live, !boardVisible { withAnimation(Motion.panel) { boardVisible = true } }
            if !live { boardVisible = true }
        case .ask(_, let prompt):
            board.apply(e)
            if live {
                askPrompt = prompt
                pill.handle(.askReached)
                cue(.ask)
                if settings.handsFree && handsFreeActive { reopenHandsFree() }
            }
        case .done:
            board.apply(e)
            if live {
                pill.handle(.doneReached)
                cue(.done)
                Haptics.success()
                if options.scene != .done {
                    Task { [weak self] in
                        try? await Task.sleep(for: .seconds(1.4))
                        self?.pill.handle(.doneSettled)
                    }
                }
            }
        case .turnEnd:
            if live {
                pill.handle(.turnEnded)
                if settings.handsFree && handsFreeActive && askPrompt == nil { reopenHandsFree() }
            }
        case .say:
            break
        }
    }

    private func speak(_ e: LessonEvent) {
        guard case .say(let text, let hasAudio) = e.kind else { return }
        pill.handle(.sayStarted)
        if captionsOn { caption = text }
        speakTask?.cancel()
        speakTask = Task { [weak self] in
            guard let self else { return }
            await self.play(text: text, seq: e.seq, serverAudio: hasAudio)
            guard !Task.isCancelled, self.timeline.current?.seq == e.seq else { return }
            self.pill.handle(.sayFinished)
            self.transport?.send(.played(seq: e.seq))
            self.process(self.timeline.finishedSpeaking(seq: e.seq))
        }
    }

    private func play(text: String, seq: Int, serverAudio: Bool) async {
        if isMock {
            mockScriptedLevel = .speaking
            let seconds = max(1.4, Double(text.count) * 0.06)
            try? await Task.sleep(for: .seconds(seconds))
            mockScriptedLevel = nil
            return
        }
        guard let audio else { return }
        if serverAudio, ready?.serverTTS == true {
            // Wait up to 3 s for the mp3 frame, then fall back to the system voice.
            var waited = 0
            while audioCache[seq] == nil, waited < 30, !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(100)); waited += 1
            }
            if let data = audioCache.removeValue(forKey: seq), await audio.play(mp3: data) { return }
        }
        guard !Task.isCancelled else { return }
        await audio.speak(text, lang: settings.lang)
    }

    private func stopAudioForDisconnect() {
        speakTask?.cancel()
        audio?.stopSpeaking()
        if timeline.isSpeaking { pill.handle(.sayFinished) }
    }

    // MARK: Talking

    /// Pill pressed (hold-to-talk) or tapped (hands-free).
    func pillPressBegan() {
        if pill.state == .speaking { interrupt() }
        if settings.handsFree {
            handsFreeActive.toggle()
            if handsFreeActive { openMic() } else { Task { await closeMic(send: false) } }
            return
        }
        openMic()
    }

    func pillPressEnded() {
        guard !settings.handsFree else { return }
        Task { await closeMic(send: true) }
    }

    func interrupt() {
        guard timeline.isSpeaking || pill.state == .thinking || pill.state == .tool else { return }
        speakTask?.cancel()
        audio?.stopSpeaking()
        transport?.send(.interrupt)
        process(timeline.interrupt())
        transport?.send(.played(seq: timeline.lastPlayed))
        pill.handle(.interrupted)
        caption = ""
        Haptics.light()
    }

    private func openMic() {
        guard connected || isMock else { return }
        vad.reset()
        pill.handle(.micOpened)
        cue(.listen)
        Haptics.tap()
        if isMock { mockScriptedLevel = .listening; return }
        audio?.startListening(lang: settings.lang, serverSTT: ready?.serverSTT == true)
    }

    private func closeMic(send: Bool) async {
        guard pill.micOpen else { return }
        if isMock {
            mockScriptedLevel = nil
            pill.handle(.micClosed(sent: false))
            return
        }
        guard let audio else { return }
        let heard = await audio.stopListening(discard: !send)
        switch heard {
        case .text(let text):
            sendUtterance(text)
        case .wav(let wav):
            transport?.send(.audioStart(sampleRate: 16_000))
            var offset = 0
            while offset < wav.count {
                let end = min(offset + 64 * 1024, wav.count)
                transport?.sendBinary(wav.subdata(in: offset..<end))
                offset = end
            }
            transport?.send(.audioEnd)
            afterSend()
        case .nothing:
            pill.handle(.micClosed(sent: false))
        }
    }

    private func sendUtterance(_ text: String) {
        transport?.send(.userText(text))
        if captionsOn { caption = text }
        afterSend()
    }

    private func afterSend() {
        askPrompt = nil
        pill.handle(.micClosed(sent: true))
        cue(.send)
    }

    private func reopenHandsFree() {
        Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(400))
            guard let self, self.handsFreeActive, !self.timeline.isSpeaking, !self.pill.micOpen else { return }
            self.openMic()
        }
    }

    // MARK: Camera

    func manualCapture() {
        Task { await takePhoto(requestId: nil) }
    }

    private func takePhoto(requestId: String?) async {
        shutterTick += 1
        cue(.shutter)
        Haptics.light()
        let image: UIImage?
        if isMock {
            image = UIImage(named: "mock-problem")
        } else {
            image = await camera?.capture()
        }
        guard let image, let encoded = PhotoEncoder.jpeg(image) else { return }
        transport?.send(.photo(requestId: requestId, jpeg: encoded.0, w: encoded.1, h: encoded.2))
    }

    // MARK: Sessions

    func openSessions() {
        showSessions = true
        transport?.send(.listSessions)
    }

    func open(_ item: SessionItem) {
        showSessions = false
        guard item.sessionId != sessionId else { return }
        audio?.stopSpeaking(); speakTask?.cancel()
        startFreshSession()
        sessionId = item.sessionId
        title = item.title
        needsRebuild = true
        transport?.send(.openSession(sessionId: item.sessionId))
    }

    func newSession() {
        showSessions = false
        audio?.stopSpeaking(); speakTask?.cancel()
        startFreshSession()
        sessionId = nil
        settings.lastSessionId = nil
        reconnectNow()
    }

    func showBoard(_ show: Bool) {
        withAnimation(Motion.panel) { boardVisible = show }
    }

    // MARK: Feedback

    private func cue(_ e: Earcon) {
        guard !isMock else { return }
        audio?.play(e)
    }

    private func showNotice(_ text: String) {
        withAnimation(.easeOut(duration: Motion.standard)) { notice = text }
        noticeTask?.cancel()
        guard options.scene != .offline else { return }
        noticeTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            withAnimation(.easeIn(duration: Motion.standard)) { self?.notice = nil }
        }
    }

    // MARK: Levels for the pill (real audio, or scripted in mock mode)

    func level(at date: Date) -> Float {
        if isMock {
            switch mockScriptedLevel {
            case .listening: return mockLevels.voice(at: date.timeIntervalSinceReferenceDate, rate: 1.0)
            case .speaking: return mockLevels.voice(at: date.timeIntervalSinceReferenceDate, rate: 1.3)
            default: return 0
            }
        }
        switch pill.state {
        case .listening: return audio?.inputLevel.value ?? 0
        case .speaking: return audio?.outputLevel.value ?? 0
        default: return 0
        }
    }

    // MARK: Ticker: thinking announcement, gap recovery, hands-free VAD

    private func startTicker() {
        tickTask?.cancel()
        tickTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(50))
                self?.tick()
            }
        }
    }

    private func tick() {
        let now = Date()
        if pill.shouldAnnounceThinking(now: now) {
            let line = strings("pill.thinkingAloud")
            if captionsOn { caption = line }
            if !isMock { audio?.say(line, lang: settings.lang) }
        }
        if timeline.hasGap {
            if gapSince == nil { gapSince = now }
            if let g = gapSince, now.timeIntervalSince(g) > 1.2 { gapSince = nil; process(timeline.skipGap()) }
        } else {
            gapSince = nil
        }
        if settings.handsFree, handsFreeActive, pill.micOpen, let audio {
            let db = audio.inputLevel.value * 45 - 55
            if vad.feed(level: db, at: now.timeIntervalSinceReferenceDate) == .speechEnded {
                Task { await closeMic(send: true) }
            }
        }
    }

    // MARK: Mock

    /// Fake student: speaks the scripted line, later answers the `ask`.
    private func runMockStudent() {
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(1.2))
            await self?.mockSpeakNextLine()
        }
    }

    private func mockSpeakNextLine() async {
        guard let mock = transport as? MockTransport, let line = mock.nextUserLine else { return }
        pill.handle(.micOpened)
        mockScriptedLevel = .listening
        if captionsOn { caption = line }
        try? await Task.sleep(for: .seconds(1.8))
        mockScriptedLevel = nil
        sendUtterance(line)
        // Answer the next ask after a pause.
        Task { [weak self] in
            while let self, !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(300))
                if self.pill.state == .asking {
                    try? await Task.sleep(for: .seconds(2.5))
                    await self.mockSpeakNextLine()
                    return
                }
            }
        }
    }

    private func applyScene(_ scene: MockScene, mock: MockTransport) {
        captionsForced = false
        if scene != .offline {
            pill.handle(.connected)
            connected = true
        } else {
            showNotice(strings("notice.offline"))
        }
        let events = mock.script.allEvents
        func restore(upTo seq: Int) {
            for e in events where e.seq <= seq && e.isBoardEvent { board.apply(e) }
            boardVisible = true
        }
        let lessonTitle = "Toán · Bất phương trình bậc hai"
        switch scene {
        case .idle, .offline:
            break
        case .listening:
            pill.handle(.micOpened)
            mockScriptedLevel = .listening
        case .thinking:
            pill.handle(.userSent)
        case .tool:
            pill.handle(.server(state: "tool", tool: "capture"))
        case .lesson:
            title = lessonTitle
            restore(upTo: 12)
            captionsForced = true
            caption = "Hệ số a dương, nên đồ thị là parabol quay lên."
            pill.handle(.sayStarted)
            mockScriptedLevel = .speaking
        case .ask:
            title = lessonTitle
            restore(upTo: 14)
            askPrompt = "Vậy f(x) dương trên những khoảng nào?"
            pill.handle(.askReached)
        case .done:
            title = lessonTitle
            restore(upTo: 22)
            pill.handle(.doneReached)
        case .sessions:
            if case .sessions(let items) = ServerMessage.decode(mock.script.sessions) { sessions = items }
            showSessions = true
        case .settings:
            showSettings = true
        }
    }
}

/// Scripted, speech-like level envelope for mock mode (syllables and short pauses).
final class MockLevels {
    func voice(at t: Double, rate: Double) -> Float {
        let tt = t * rate
        let syllable = abs(sin(tt * 6.8)) * (0.55 + 0.45 * sin(tt * 1.9 + 1.3))
        let phrase = sin(tt * 0.9) > -0.75 ? 1.0 : 0.15
        let jitter = 0.08 * sin(tt * 23.0)
        return Float(max(0, min(1, (0.15 + 0.75 * syllable) * phrase + jitter)))
    }
}
