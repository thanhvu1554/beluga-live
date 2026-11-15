import AVFoundation
import SwiftUI
import UIKit

/// Full-screen back camera + sharp still capture.
final class CameraController: NSObject, @unchecked Sendable, AVCapturePhotoCaptureDelegate {
    let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "camera.session")
    private let photoOutput = AVCapturePhotoOutput()
    private var configured = false
    private let lock = NSLock()
    private var pending: [Int64: CheckedContinuation<UIImage?, Never>] = [:]

    func start() {
        queue.async { [self] in
            if !configured { configure() }
            if configured, !session.isRunning { session.startRunning() }
        }
    }

    func stop() {
        queue.async { [self] in if session.isRunning { session.stopRunning() } }
    }

    static func requestAccess() async -> Bool {
        await AVCaptureDevice.requestAccess(for: .video)
    }

    static var accessGranted: Bool { AVCaptureDevice.authorizationStatus(for: .video) == .authorized }

    var isRunning: Bool { session.isRunning }

    private func configure() {
        guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back),
              let input = try? AVCaptureDeviceInput(device: device) else { return }
        session.beginConfiguration()
        session.sessionPreset = .photo
        if session.canAddInput(input) { session.addInput(input) }
        if session.canAddOutput(photoOutput) { session.addOutput(photoOutput) }
        photoOutput.maxPhotoQualityPrioritization = .quality
        session.commitConfiguration()
        // The app is portrait-only: ask for upright photos (the output defaults to the sensor's landscape).
        if let connection = photoOutput.connection(with: .video), connection.isVideoRotationAngleSupported(90) {
            connection.videoRotationAngle = 90
        }
        if (try? device.lockForConfiguration()) != nil {
            if device.isFocusModeSupported(.continuousAutoFocus) { device.focusMode = .continuousAutoFocus }
            if device.isAutoFocusRangeRestrictionSupported { device.autoFocusRangeRestriction = .near }
            device.unlockForConfiguration()
        }
        configured = true
    }

    /// Captures a sharp photo (quality prioritized), or nil when the camera is unavailable.
    func capture() async -> UIImage? {
        await withCheckedContinuation { (cont: CheckedContinuation<UIImage?, Never>) in
            queue.async { [self] in
                // `capturePhoto` raises an Objective-C exception (uncatchable, kills the app) without an active,
                // enabled video connection: the session can be interrupted (camera taken by another app, system
                // pressure) while `isRunning` is still true.
                guard configured, session.isRunning, !session.isInterrupted,
                      let connection = photoOutput.connection(with: .video), connection.isActive, connection.isEnabled else {
                    Diagnostics.log("camera: capture skipped (no active connection)")
                    cont.resume(returning: nil); return
                }
                // JPEG when offered (an unsupported codec also raises); otherwise the default (HEIC).
                let settings = photoOutput.availablePhotoCodecTypes.contains(.jpeg)
                    ? AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
                    : AVCapturePhotoSettings()
                // Must not exceed the output's maximum, or it raises.
                settings.photoQualityPrioritization = photoOutput.maxPhotoQualityPrioritization == .quality ? .quality : .balanced
                lock.lock(); pending[settings.uniqueID] = cont; lock.unlock()
                photoOutput.capturePhoto(with: settings, delegate: self)
            }
        }
    }

    func photoOutput(_ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto, error: Error?) {
        let image = photo.fileDataRepresentation().flatMap(UIImage.init(data:))
        lock.lock(); let cont = pending.removeValue(forKey: photo.resolvedSettings.uniqueID); lock.unlock()
        cont?.resume(returning: image)
    }

    /// Always the last callback of a capture: answers a waiter that got no photo (capture failed early).
    func photoOutput(_ output: AVCapturePhotoOutput, didFinishCaptureFor resolvedSettings: AVCaptureResolvedPhotoSettings, error: Error?) {
        lock.lock(); let cont = pending.removeValue(forKey: resolvedSettings.uniqueID); lock.unlock()
        if let error { Diagnostics.log("camera: capture error \(error.localizedDescription)") }
        cont?.resume(returning: nil)
    }
}

@MainActor
enum PhotoEncoder {
    /// JPEG with the long side scaled to ~2000 px.
    static func jpeg(_ image: UIImage, longSide: CGFloat = 2000, quality: CGFloat = 0.82) -> (Data, Int, Int)? {
        let size = image.size
        let scale = min(1, longSide / max(size.width, size.height))
        let target = CGSize(width: (size.width * scale).rounded(), height: (size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: target, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: target))
        }
        guard let data = resized.jpegData(compressionQuality: quality) else { return nil }
        return (data, Int(target.width), Int(target.height))
    }
}

/// Camera preview layer, aspect fill.
struct CameraPreview: UIViewRepresentable {
    let session: AVCaptureSession

    final class PreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
    }

    func makeUIView(context: Context) -> PreviewView {
        let v = PreviewView()
        v.previewLayer.session = session
        v.previewLayer.videoGravity = .resizeAspectFill
        v.backgroundColor = .black
        return v
    }

    func updateUIView(_ uiView: PreviewView, context: Context) {}
}
