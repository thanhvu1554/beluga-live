import SwiftUI
import UIKit
import AVFoundation
import BelugaCore

/// Where the call's sound goes (the route button's glyph).
enum CallRoute: String, Sendable {
    case speaker, receiver, headphones, bluetooth, airplay

    var symbol: String {
        switch self {
        case .speaker: return "speaker.wave.2.fill"
        case .receiver: return "iphone"
        case .headphones: return "headphones"
        case .bluetooth: return "airpodspro"
        case .airplay: return "airplayaudio"
        }
    }

    static func current() -> CallRoute {
        guard let port = AVAudioSession.sharedInstance().currentRoute.outputs.first?.portType else { return .speaker }
        switch port {
        case .builtInSpeaker: return .speaker
        case .builtInReceiver: return .receiver
        case .headphones, .usbAudio, .lineOut: return .headphones
        case .bluetoothA2DP, .bluetoothHFP, .bluetoothLE: return .bluetooth
        case .airPlay, .HDMI: return .airplay
        default: return .speaker
        }
    }
}

/// Why a photo cannot be taken during a call.
enum CaptureBlocker: String, Sendable {
    /// The owner turned the camera off in the call.
    case cameraOff
    /// The app is in PiP and the capture session has no multitasking camera access.
    case background
    /// No camera or no permission, or the capture failed.
    case unavailable
}

// Call mode (a calling-app face of live mode): the same session, timeline, board, audio hub and camera as Live,
// with hands-free listening, a native stage and Picture in Picture so the lesson goes on in other apps.
extension LiveController {

    /// Hands-free listening: the user's setting in Live, always in a call.
    var handsFreeOn: Bool { settings.handsFree || inCall }

    var callPresence: CallPresence { CallPresence.from(pill.state, muted: callMuted) }

    /// A short note for the self-view (and the PiP window) when the camera cannot serve the tutor.
    var cameraNoteKey: String? {
        if !callCameraOn { return "call.camera.off" }
        if camera == nil, !isMock { return "call.camera.none" }
        if appInBackground, !cameraMultitasking { return "call.camera.paused" }
        if !cameraMultitasking { return "call.camera.foregroundOnly" }
        return nil
    }

    // MARK: Stage

    func applyToStage(_ e: LessonEvent) {
        withAnimation(Motion.smooth) { callStage.apply(e) }
    }

    func setStageCaption(_ text: String) {
        guard callStage.caption != text else { return }
        withAnimation(Motion.snappy) { callStage.caption = text }
    }

    // MARK: Start and end

    /// "Gọi": the call screen rises over the chat, the session goes live, the microphone opens hands-free.
    func startCall() {
        guard !inCall else { return }
        Diagnostics.log("call: start (mode \(mode.rawValue), connected \(connected))")
        if composer.dictating { Task { await stopDictation() } }
        callMuted = false
        callCameraOn = true
        // The timer starts when the call connects (section 22).
        callStartedAt = nil
        callRoute = isMock ? .speaker : CallRoute.current()
        if isMock { cameraMultitasking = true }
        let wasLive = mode == .live
        withAnimation(Motion.smooth) {
            boardVisible = false
            inCall = true
            mode = .live
        }
        Haptics.tap()
        if !wasLive {
            startCamera()
            if connected { transport?.send(.settings(settings.tutor(mode: .live))) }
        }
        // PiP needs a running audio session; the engine also carries the tutor's voice and the microphone.
        audio?.ensureRunning()
        pip.setAutomatic(true)
        handsFreeActive = true
        if !timeline.isSpeaking, !pill.micOpen { _ = openMicForCall() }
        if !connected, !isMock { reconnectNow() }
        runCall(callPhase.handle(.start, now: Date()))
    }

    /// The red button. Works in every phase: while dialing it just stops; once connected the screen shows
    /// "Đã kết thúc · mm:ss" for a beat, then closes (a second tap closes at once).
    func endCall() {
        guard inCall else { return }
        Diagnostics.log("call: end after \(Int(Date().timeIntervalSince(callStartedAt ?? Date()))) s")
        Haptics.light()
        if callPhase.isActive { runCall(callPhase.handle(.hangUp, now: Date())) } else { setMode(.chat) }
    }

    /// "Gọi lại" after a failed dial.
    func retryCall() {
        guard inCall else { return }
        Haptics.tap()
        if !connected { reconnectNow() }
        handsFreeActive = !callMuted
        if handsFreeActive, !pill.micOpen { _ = openMicForCall() }
        runCall(callPhase.handle(.retry, now: Date()))
    }

    /// Called whenever live mode ends (red button, new chat, a session change).
    func callDidEnd() {
        pip.setAutomatic(false)
        pip.stop()
        callStartedAt = nil
        callMuted = false
        ringTask?.cancel(); ringTask = nil
        helloTask?.cancel(); helloTask = nil
        if callPhase.isActive { callPhase = CallPhaseMachine() }
        withAnimation(Motion.snappy) { callStage.caption = "" }
    }

    // MARK: Phases (PROTOCOL section 22)

    /// The live session under the call can take the call: connected, in live mode, audio running.
    private var callSessionReady: Bool {
        connected && mode == .live && (isMock || audio?.isRunning == true)
    }

    /// Ticks the phase machine; the observed value is only replaced when something changed.
    func tickCall(_ now: Date) {
        var m = callPhase
        var effects = m.sessionReady != callSessionReady ? m.handle(.sessionReady(callSessionReady), now: now) : []
        effects += m.handle(.tick, now: now)
        if m.phase != callPhase.phase || m.sessionReady != callPhase.sessionReady {
            withAnimation(Motion.smooth) { callPhase = m }
        }
        runCall(effects)
    }

    func runCall(_ effects: [CallPhaseEffect]) {
        for effect in effects {
            switch effect {
            case .startRinging: startRinging()
            case .stopRinging: ringTask?.cancel(); ringTask = nil
            case .connected: callConnected()
            case .failed: callFailed()
            case .hangUp: callHangUp()
            case .close: setMode(.chat)
            }
        }
    }

    /// Ring-back while dialing: two soft pulses every 2.6 s (synthesized, quiet).
    private func startRinging() {
        ringTask?.cancel()
        ringTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            while let self, !Task.isCancelled, self.callPhase.phase.isDialing {
                self.cue(.ring)
                try? await Task.sleep(for: .seconds(2.6))
            }
        }
    }

    /// Connected: a short rising tone and a tap, "Đã kết nối", the timer starts, Beluga picks up with a hello line
    /// in its own voice (if the phone has one), then listens.
    private func callConnected() {
        Diagnostics.log("call: connected")
        cue(.connect)
        Haptics.success()
        callStartedAt = Date()
        guard let id = waitClips.helloClip(), let line = waitClips.availableClips.first(where: { $0.id == id }) else { return }
        helloTask?.cancel()
        helloTask = Task { [weak self] in
            guard let self else { return }
            // The microphone waits while Beluga says hello, so the line is never heard back as the student.
            let reopen = self.handsFreeActive && !self.callMuted
            self.handsFreeActive = false
            await self.closeMic(send: false)
            self.fillerCaption = line.text
            self.setStageCaption(line.text)
            if self.isMock {
                try? await Task.sleep(for: .seconds(max(1.2, Double(line.text.count) * 0.07)))
            } else if let url = self.fillerFiles[id], let audio = self.audio {
                _ = await audio.playFiller(url)
            }
            guard !Task.isCancelled, self.callPhase.phase.isConnected else { return }
            self.clearWaitCaption()
            if reopen, !self.callMuted {
                self.handsFreeActive = true
                if !self.timeline.isSpeaking, !self.pill.micOpen { _ = self.openMicForCall() }
            }
        }
    }

    private func callFailed() {
        Diagnostics.log("call: dial failed (connected \(connected), engine \(audio?.isRunning ?? false))")
        Haptics.warning()
        handsFreeActive = false
        Task { await closeMic(send: false) }
    }

    /// The tutor and the microphone stop now; the screen stays for the "ended" beat.
    private func callHangUp() {
        helloTask?.cancel(); helloTask = nil
        ringTask?.cancel(); ringTask = nil
        audio?.stopFiller(fade: false)
        if timeline.isSpeaking || pill.state == .thinking || pill.state == .tool { interrupt() }
        handsFreeActive = false
        Task { await closeMic(send: false) }
    }

    // MARK: Controls

    func toggleCallMute() {
        callMuted.toggle()
        Diagnostics.log("call: mic \(callMuted ? "muted" : "on")")
        Haptics.tap()
        if callMuted {
            handsFreeActive = false
            Task { await closeMic(send: false) }
        } else {
            handsFreeActive = true
            if !timeline.isSpeaking, !pill.micOpen { _ = openMicForCall() }
        }
    }

    func toggleCallCamera() {
        callCameraOn.toggle()
        Diagnostics.log("call: camera \(callCameraOn ? "on" : "off")")
        Haptics.tap()
        if callCameraOn { startCamera() } else { camera?.stop() }
    }

    func toggleCallBoard() {
        guard board.hasContent else { return }
        showBoard(!boardVisible)
    }

    /// A tap on the tutor's tile: stop the tutor talking (and keep listening), like the pill in hands-free Live.
    func callTapStage() {
        guard inCall else { return }
        if pill.state == .speaking || pill.state == .thinking || pill.state == .tool { interrupt() }
        guard !callMuted else { return }
        handsFreeActive = true
        if !pill.micOpen { _ = openMicForCall() }
    }

    private func openMicForCall() -> Bool {
        guard !pill.micOpen else { return true }
        return openMic()
    }

    // MARK: Background

    /// The scene went to the background. In a call nothing stops: PiP takes over and audio, microphone, speech
    /// and the WebSocket keep running (`audio` background mode). Outside a call the app behaves as before the
    /// call existed: the sentence pauses and the engine is released, so iOS suspends the app.
    func appEnteredBackground() {
        appInBackground = true
        guard !isMock else { return }
        if inCall {
            Diagnostics.log("call: background (pip \(pip.isActive), possible \(pip.isPossible), camera multitasking \(cameraMultitasking), mic \(pill.micOpen), connected \(connected))")
            return
        }
        if mode == .live, timeline.isSpeaking {
            // Paused, not finished: `appBecameActive` says the sentence again.
            speakTask?.cancel()
        }
        audio?.holdForBackground()
    }

    // MARK: Capture during a call

    func callCaptureBlocker() -> CaptureBlocker? {
        if !callCameraOn { return .cameraOff }
        if isMock { return nil }
        guard let camera, CameraController.accessGranted else { return .unavailable }
        if appInBackground, !cameraMultitasking || camera.isInterrupted { return .background }
        return nil
    }

    /// Answers a `capture_request` that cannot be served. PROTOCOL has no failure frame for `photo`, so the
    /// answer is a small picture that says why in words: the tutor reads it at once instead of timing out.
    func answerCaptureFailure(requestId: String?, reason: CaptureBlocker) {
        let key: String
        switch reason {
        case .cameraOff: key = "call.photoFailed.off"
        case .background: key = "call.photoFailed.background"
        case .unavailable: key = "call.photoFailed.unavailable"
        }
        if !appInBackground { showNotice(strings(key + ".notice")) }
        guard let requestId else { return }
        guard let card = PhotoEncoder.messageCard(strings(key)) else { return }
        transport?.send(.photo(requestId: requestId, jpeg: card.0, w: card.1, h: card.2))
    }

    // MARK: Observers

    func installCallObservers() {
        guard callObservers.isEmpty, !isMock else { return }
        let nc = NotificationCenter.default
        callObservers.append(nc.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                let route = CallRoute.current()
                if route != self.callRoute {
                    if self.inCall { Diagnostics.log("call: route \(route.rawValue)") }
                    withAnimation(Motion.snappy) { self.callRoute = route }
                }
            }
        })
        if let camera {
            callObservers.append(nc.addObserver(forName: AVCaptureSession.wasInterruptedNotification, object: camera.session, queue: .main) { [weak self] note in
                let reason = (note.userInfo?[AVCaptureSessionInterruptionReasonKey] as? NSNumber)?.intValue ?? -1
                MainActor.assumeIsolated {
                    guard let self else { return }
                    Diagnostics.log("camera: interrupted reason \(reason) (call \(self.inCall), background \(self.appInBackground))")
                    self.cameraInterrupted = true
                }
            })
            callObservers.append(nc.addObserver(forName: AVCaptureSession.interruptionEndedNotification, object: camera.session, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    if self.inCall { Diagnostics.log("camera: interruption ended") }
                    self.cameraInterrupted = false
                }
            })
            callObservers.append(nc.addObserver(forName: AVCaptureSession.didStartRunningNotification, object: camera.session, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.cameraMultitasking = camera.multitaskingEnabled
                    self.cameraInterrupted = false
                }
            })
        }
        pip.onEvent = { [weak self] event in self?.pipEvent(event) }
    }

    private func pipEvent(_ event: CallPiP.Event) {
        switch event {
        case .willStart:
            Diagnostics.log("pip: will start")
        case .didStart:
            Diagnostics.log("pip: started (mic \(pill.micOpen), engine \(audio?.isRunning ?? false))")
            pipActive = true
        case .willStop:
            Diagnostics.log("pip: will stop")
        case .didStop:
            Diagnostics.log("pip: stopped (call \(inCall), background \(appInBackground))")
            pipActive = false
        case .failed(let reason):
            Diagnostics.log("pip: failed to start: \(reason)")
            pipActive = false
        case .restore:
            Diagnostics.log("pip: restore to the call screen")
        }
    }

    // MARK: Mock

    /// Call scenes: the same mock lesson, seen through the call screen.
    func applyCallScene(_ scene: MockScene, mock: MockTransport) {
        pill.handle(.connected)
        connected = true
        title = "Toán · Bất phương trình bậc hai"
        let events = mock.script.allEvents
        func restore(upTo seq: Int) {
            for e in events where e.seq <= seq && e.isBoardEvent {
                board.apply(e, restoring: true)
                callStage.apply(e)
            }
        }
        inCall = true
        callStartedAt = Date().addingTimeInterval(-754)
        cameraMultitasking = true
        switch scene {
        case .call:
            restore(upTo: 8)
            callStage.caption = "Hệ số a dương, nên đồ thị là parabol quay lên."
            pill.handle(.sayStarted)
            mockScriptedLevel = .speaking
        case .calllisten:
            restore(upTo: 12)
            callStage.caption = "Đến đây cậu thử nghĩ một chút."
            callStage.apply(LessonEvent(seq: 14, turn: 1, kind: .ask(kind: "next", prompt: "Vậy f(x) dương trên những khoảng nào?")))
            pill.handle(.askReached)
            pill.handle(.micOpened)
            mockScriptedLevel = .listening
        case .callnocam:
            restore(upTo: 19)
            callStage.caption = "Đúng rồi, f dương ở hai khoảng nằm ngoài hai nghiệm."
            cameraMultitasking = false
            callMuted = true
            pill.handle(.server(state: "thinking", tool: nil))
        case .callboard:
            restore(upTo: 12)
            callStage.caption = "Đến đây cậu thử nghĩ một chút."
            boardVisible = true
            pill.handle(.sayStarted)
            mockScriptedLevel = .speaking
        case .calldial:
            callStartedAt = nil
            _ = callPhase.handle(.start, now: Date())
        case .callfailed:
            callStartedAt = nil
            callPhase = CallPhaseMachine(phase: .failed(at: Date()))
        default:
            break
        }
    }

    /// `-mockCall 1` (screen recording): chat, then the call rises, the mock lesson plays in it (the student's
    /// lines are scripted), the board comes up and goes down, the call ends.
    func runMockCallTour() {
        Task { [weak self] in
            func pause(_ s: Double) async { try? await Task.sleep(for: .seconds(s)) }
            await pause(2.5)
            guard let self else { return }
            self.startCall()
            // Dialing, connected, the hello line: then the scripted student talks.
            await pause(4.5)
            // The scripted student speaks at the asks; hands-free reopening would hold the mic instead.
            self.handsFreeActive = false
            await self.closeMic(send: false)
            self.runMockStudent()
            await pause(24)
            self.toggleCallBoard()
            await pause(3.5)
            self.toggleCallBoard()
            await pause(2.5)
            self.endCall()
            await pause(1.4)
        }
    }
}

extension PhotoEncoder {
    /// A plain white card with a message in large black type, as a JPEG (the failure answer to `capture_request`).
    static func messageCard(_ text: String) -> (Data, Int, Int)? {
        let size = CGSize(width: 960, height: 540)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let image = UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            UIColor.white.setFill()
            ctx.fill(CGRect(origin: .zero, size: size))
            let style = NSMutableParagraphStyle()
            style.alignment = .center
            style.lineBreakMode = .byWordWrapping
            let attrs: [NSAttributedString.Key: Any] = [
                .font: UIFont.systemFont(ofSize: 40, weight: .semibold),
                .foregroundColor: UIColor.black,
                .paragraphStyle: style,
            ]
            (text as NSString).draw(in: CGRect(x: 60, y: 120, width: size.width - 120, height: size.height - 200), withAttributes: attrs)
        }
        guard let data = image.jpegData(compressionQuality: 0.8) else { return nil }
        return (data, Int(size.width), Int(size.height))
    }
}
