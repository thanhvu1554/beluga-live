import AVKit
import SwiftUI
import UIKit

/// Picture in Picture for the call, with the video-call API: the PiP window shows a native view of ours (step,
/// featured block, spoken line, who is talking) hosted in an `AVPictureInPictureVideoCallViewController`. It starts
/// by itself when the app leaves the foreground during a call (`canStartPictureInPictureAutomaticallyFromInline`)
/// and a tap restores the call screen.
///
/// The source view is the call screen's stage (`PiPSourceView`); one controller per source view. When the call ends
/// automatic start is switched off first, so releasing the controller never starts PiP by accident.
@MainActor
final class CallPiP {
    enum Event: Sendable { case willStart, didStart, willStop, didStop, failed(String), restore }

    var onEvent: (@MainActor @Sendable (Event) -> Void)?

    private var controller: AVPictureInPictureController?
    private var callViewController: AVPictureInPictureVideoCallViewController?
    private weak var source: UIView?
    private let delegate = PiPDelegate()
    private var automatic = false
    private var loggedUnsupported = false

    var isActive: Bool { controller?.isPictureInPictureActive ?? false }
    var isPossible: Bool { controller?.isPictureInPicturePossible ?? false }

    /// The call screen's stage appeared: build the PiP controller around it, with `content` as the window's view.
    func prepare<Content: View>(source: UIView, content: Content) {
        guard AVPictureInPictureController.isPictureInPictureSupported() else {
            if !loggedUnsupported { Diagnostics.log("pip: not supported on this device"); loggedUnsupported = true }
            return
        }
        if self.source === source, controller != nil { return }
        release()

        let callVC = AVPictureInPictureVideoCallViewController()
        // Portrait 3:4: room for a title, a formula or a small table, and two lines of caption.
        callVC.preferredContentSize = CGSize(width: 900, height: 1200)
        let host = UIHostingController(rootView: content)
        host.view.backgroundColor = .black
        host.view.frame = callVC.view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        callVC.addChild(host)
        callVC.view.addSubview(host.view)
        host.didMove(toParent: callVC)
        callVC.view.backgroundColor = .black

        let contentSource = AVPictureInPictureController.ContentSource(activeVideoCallSourceView: source,
                                                                       contentViewController: callVC)
        let made: AVPictureInPictureController? = AVPictureInPictureController(contentSource: contentSource)
        guard let pip = made else {
            Diagnostics.log("pip: controller could not be created")
            return
        }
        delegate.onEvent = { [weak self] event in self?.onEvent?(event) }
        pip.delegate = delegate
        pip.canStartPictureInPictureAutomaticallyFromInline = automatic
        controller = pip
        callViewController = callVC
        self.source = source
        Diagnostics.log("pip: prepared (automatic \(automatic))")
        // Breadcrumb once things settled: whether PiP could start now (the first thing to check if it never shows).
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1.5))
            guard let self, let current = self.controller else { return }
            Diagnostics.log("pip: possible \(current.isPictureInPicturePossible)")
        }
    }

    /// Automatic start when the app leaves the foreground: on for the whole call, off when it ends.
    func setAutomatic(_ on: Bool) {
        automatic = on
        controller?.canStartPictureInPictureAutomaticallyFromInline = on
    }

    func stop() {
        guard let controller, controller.isPictureInPictureActive else { return }
        controller.stopPictureInPicture()
    }

    /// The stage left the screen (call ended): automatic start off before the controller goes away.
    func release(source: UIView? = nil) {
        if let source, source !== self.source { return }
        guard let controller else { return }
        controller.canStartPictureInPictureAutomaticallyFromInline = false
        if controller.isPictureInPictureActive { controller.stopPictureInPicture() }
        controller.delegate = nil
        self.controller = nil
        callViewController = nil
        self.source = nil
        Diagnostics.log("pip: released")
    }
}

/// AVKit delegate, forwarding to the main actor.
final class PiPDelegate: NSObject, AVPictureInPictureControllerDelegate, @unchecked Sendable {
    nonisolated(unsafe) var onEvent: (@MainActor @Sendable (CallPiP.Event) -> Void)?

    private func send(_ event: CallPiP.Event) {
        let handler = onEvent
        Task { @MainActor in handler?(event) }
    }

    func pictureInPictureControllerWillStartPictureInPicture(_ pictureInPictureController: AVPictureInPictureController) {
        send(.willStart)
    }

    func pictureInPictureControllerDidStartPictureInPicture(_ pictureInPictureController: AVPictureInPictureController) {
        send(.didStart)
    }

    func pictureInPictureController(_ pictureInPictureController: AVPictureInPictureController,
                                    failedToStartPictureInPictureWithError error: any Error) {
        let ns = error as NSError
        send(.failed("\(ns.domain) \(ns.code) \(ns.localizedDescription)"))
    }

    func pictureInPictureControllerWillStopPictureInPicture(_ pictureInPictureController: AVPictureInPictureController) {
        send(.willStop)
    }

    func pictureInPictureControllerDidStopPictureInPicture(_ pictureInPictureController: AVPictureInPictureController) {
        send(.didStop)
    }

    /// The call screen never went away (PiP mirrors it), so it is restored as it is.
    func pictureInPictureController(_ pictureInPictureController: AVPictureInPictureController,
                                    restoreUserInterfaceForPictureInPictureStopWithCompletionHandler completionHandler: @escaping (Bool) -> Void) {
        completionHandler(true)
        send(.restore)
    }
}

final class PiPSourceUIView: UIView {
    var onWindow: ((UIView) -> Void)?
    weak var pip: CallPiP?

    override func didMoveToWindow() {
        super.didMoveToWindow()
        // Prepared once the view is in a window (PiP animates from its frame).
        if window != nil { onWindow?(self) }
    }
}

/// The view PiP animates from and restores to: a plain UIView behind the call stage that hands itself to `CallPiP`.
struct PiPSourceView<Content: View>: UIViewRepresentable {
    let pip: CallPiP
    let content: () -> Content

    typealias SourceView = PiPSourceUIView

    func makeUIView(context: Context) -> SourceView {
        let v = SourceView()
        v.backgroundColor = .clear
        v.isUserInteractionEnabled = false
        v.pip = pip
        let pip = self.pip
        let content = self.content
        v.onWindow = { view in pip.prepare(source: view, content: content()) }
        return v
    }

    func updateUIView(_ uiView: SourceView, context: Context) {}

    static func dismantleUIView(_ uiView: SourceView, coordinator: ()) {
        uiView.onWindow = nil
        uiView.pip?.release(source: uiView)
    }
}
