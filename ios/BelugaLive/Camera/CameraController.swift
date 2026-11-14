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

    private func configure() {
        guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back),
              let input = try? AVCaptureDeviceInput(device: device) else { return }
        session.beginConfiguration()
        session.sessionPreset = .photo
        if session.canAddInput(input) { session.addInput(input) }
        if session.canAddOutput(photoOutput) { session.addOutput(photoOutput) }
        photoOutput.maxPhotoQualityPrioritization = .quality
        session.commitConfiguration()
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
                guard configured, session.isRunning else { cont.resume(returning: nil); return }
                let settings = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
                settings.photoQualityPrioritization = .quality
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
