import CryptoKit
import Foundation

/// Streaming SHA-256 used to key compiled model caches. CryptoKit is hardware
/// accelerated; the model packages hash to several hundred megabytes, so a
/// scalar implementation would dominate a warm load.
internal func sha256File(_ url: URL) throws -> String {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    var digest = CryptoKit.SHA256()
    while let chunk = try handle.read(upToCount: 4 * 1024 * 1024), !chunk.isEmpty {
        digest.update(data: chunk)
    }
    return hex(digest.finalize())
}

internal func sha256Data(_ data: Data) -> String {
    hex(CryptoKit.SHA256.hash(data: data))
}

private func hex(_ digest: CryptoKit.SHA256.Digest) -> String {
    digest.map { String(format: "%02x", $0) }.joined()
}
