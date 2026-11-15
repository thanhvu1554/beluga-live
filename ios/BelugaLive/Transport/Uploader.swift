import Foundation
import UIKit
import ImageIO
import BelugaCore

/// What `POST /upload` answers (PROTOCOL section 9).
struct UploadedFile: Sendable, Equatable {
    var fileId: String
    var kind: String
    var name: String
}

enum UploadError: Error { case noServer, badResponse(Int), badBody }

/// Raw-body upload with progress: `POST /upload?name=` + `Authorization: Bearer <pairToken>`.
/// In mock mode it pretends, with a believable progress curve.
@MainActor
struct Uploader {
    var base: URL?
    var token: String?
    var mock: Bool

    func upload(_ data: Data, name: String, mime: String,
                progress: @escaping @MainActor @Sendable (Double) -> Void) async throws -> UploadedFile {
        let kind = mime == "application/pdf" ? "pdf" : "image"
        if mock {
            let steps = 8
            for i in 1...steps {
                try await Task.sleep(for: .milliseconds(90))
                progress(Double(i) / Double(steps))
            }
            return UploadedFile(fileId: "mock-file-\(UUID().uuidString.prefix(8))", kind: kind, name: name)
        }
        guard let base, var c = URLComponents(url: base.appendingPathComponent("upload"), resolvingAgainstBaseURL: false) else {
            throw UploadError.noServer
        }
        c.queryItems = [URLQueryItem(name: "name", value: name)]
        guard let url = c.url else { throw UploadError.noServer }
        var req = URLRequest(url: url, timeoutInterval: 120)
        req.httpMethod = "POST"
        req.setValue(mime, forHTTPHeaderField: "Content-Type")
        req.setValue("Bearer \(token ?? "")", forHTTPHeaderField: "Authorization")
        let delegate = UploadProgress { p in Task { @MainActor in progress(p) } }
        let (body, response) = try await URLSession.shared.upload(for: req, from: data, delegate: delegate)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else { throw UploadError.badResponse(status) }
        guard let v = try? JSONValue.parse(body), let id = v["fileId"]?.string else { throw UploadError.badBody }
        progress(1)
        return UploadedFile(fileId: id, kind: v["kind"]?.string ?? kind, name: v["name"]?.string ?? name)
    }

    /// Photos are sent as JPEG, long side at most 2000 px, quality 0.8 (PROTOCOL section 9).
    static func prepareImage(_ image: UIImage) -> Data? {
        PhotoEncoder.jpeg(image, longSide: 2000, quality: 0.8)?.0
    }
}

/// Picked photos and image files, decoded straight to at most 2000 px with ImageIO, off the main thread.
/// Decoding a 24-48 MP photo whole costs 100-200 MB and most of a second; thirty of those in a row on the main
/// thread (the old path: `UIImage(data:)` then a full draw) froze the app and could push it past its memory limit.
enum ImageLoader {
    struct Prepared: @unchecked Sendable {
        let jpeg: Data
        let thumbnail: UIImage?
    }

    nonisolated static func prepare(_ data: Data, longSide: Int = 2000, quality: CGFloat = 0.8) -> Prepared? {
        autoreleasepool {
            guard let source = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary),
                  let image = downsample(source, to: longSide),
                  let jpeg = UIImage(cgImage: image).jpegData(compressionQuality: quality) else { return nil }
            let thumb = downsample(source, to: 240).map { UIImage(cgImage: $0) }
            return Prepared(jpeg: jpeg, thumbnail: thumb)
        }
    }

    private nonisolated static func downsample(_ source: CGImageSource, to maxPixel: Int) -> CGImage? {
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,   // EXIF orientation applied
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixel,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    }
}

private final class UploadProgress: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    let report: @Sendable (Double) -> Void
    init(_ report: @escaping @Sendable (Double) -> Void) { self.report = report }

    func urlSession(_ session: URLSession, task: URLSessionTask, didSendBodyData bytesSent: Int64,
                    totalBytesSent: Int64, totalBytesExpectedToSend: Int64) {
        guard totalBytesExpectedToSend > 0 else { return }
        report(min(0.99, Double(totalBytesSent) / Double(totalBytesExpectedToSend)))
    }
}
