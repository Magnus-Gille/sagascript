import Foundation
import os.log

/// Service for downloading and managing GGML model files for whisper.cpp
/// Models are stored in ~/Library/Application Support/FlowDictate/Models/
actor ModelDownloadService {
    static let shared = ModelDownloadService()

    private let logger = Logger(subsystem: "com.flowdictate", category: "ModelDownload")
    private let loggingService = LoggingService.shared
    private var activeDownloads: [String: Task<URL, Error>] = [:]

    /// Directory where models are stored
    static var modelsDirectory: URL {
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return appSupport.appendingPathComponent("FlowDictate/Models", isDirectory: true)
    }

    /// Ensure the models directory exists
    private func ensureModelsDirectoryExists() throws {
        let dir = Self.modelsDirectory
        if !FileManager.default.fileExists(atPath: dir.path) {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            print("[ModelDownload] Created models directory: \(dir.path)")
        }
    }

    /// Get the local path for a GGML model
    func localPath(for model: WhisperModel) -> URL {
        Self.modelsDirectory.appendingPathComponent(model.ggmlFilename)
    }

    /// Check if a model is downloaded
    func isDownloaded(_ model: WhisperModel) -> Bool {
        guard model.isSwedishOptimized else { return true } // Standard models handled by WhisperKit
        return FileManager.default.fileExists(atPath: localPath(for: model).path)
    }

    /// Download a model if not already present
    /// Returns the local file URL
    func ensureModelAvailable(_ model: WhisperModel, progressHandler: ((Double) -> Void)? = nil) async throws -> URL {
        guard model.isSwedishOptimized else {
            throw ModelDownloadError.notSupported("Standard models are handled by WhisperKit")
        }

        let localURL = localPath(for: model)

        // Already downloaded
        if FileManager.default.fileExists(atPath: localURL.path) {
            print("[ModelDownload] Model already exists: \(localURL.path)")
            return localURL
        }

        // Check if download already in progress
        if let existingTask = activeDownloads[model.rawValue] {
            print("[ModelDownload] Download already in progress, waiting...")
            return try await existingTask.value
        }

        // Start new download
        let downloadTask = Task<URL, Error> {
            try await downloadModel(model, to: localURL, progressHandler: progressHandler)
        }

        activeDownloads[model.rawValue] = downloadTask

        do {
            let result = try await downloadTask.value
            activeDownloads.removeValue(forKey: model.rawValue)
            return result
        } catch {
            activeDownloads.removeValue(forKey: model.rawValue)
            throw error
        }
    }

    /// Download the model file
    private func downloadModel(_ model: WhisperModel, to destination: URL, progressHandler: ((Double) -> Void)?) async throws -> URL {
        guard let downloadURL = model.ggmlDownloadURL else {
            throw ModelDownloadError.noDownloadURL
        }

        try ensureModelsDirectoryExists()

        print("")
        print("┌────────────────────────────────────────────────────────────┐")
        print("│  Downloading KB-Whisper Model                              │")
        print("└────────────────────────────────────────────────────────────┘")
        print("")
        print("[ModelDownload] Model: \(model.displayName)")
        print("[ModelDownload] URL: \(downloadURL)")
        print("[ModelDownload] Destination: \(destination.path)")
        print("")

        logger.info("Starting download of \(model.rawValue) from \(downloadURL.absoluteString)")

        loggingService.info(.Transcription, "model_download_started", data: [
            "model": AnyCodable(model.rawValue),
            "url": AnyCodable(downloadURL.absoluteString)
        ])

        let startTime = CFAbsoluteTimeGetCurrent()

        // Create a download task with progress tracking
        let (tempURL, response) = try await URLSession.shared.download(from: downloadURL, delegate: nil)

        guard let httpResponse = response as? HTTPURLResponse, httpResponse.statusCode == 200 else {
            let statusCode = (response as? HTTPURLResponse)?.statusCode ?? -1
            throw ModelDownloadError.downloadFailed("HTTP \(statusCode)")
        }

        // Move to final destination
        if FileManager.default.fileExists(atPath: destination.path) {
            try FileManager.default.removeItem(at: destination)
        }
        try FileManager.default.moveItem(at: tempURL, to: destination)

        let elapsed = CFAbsoluteTimeGetCurrent() - startTime
        let fileSize = (try? FileManager.default.attributesOfItem(atPath: destination.path)[.size] as? Int64) ?? 0
        let sizeMB = Double(fileSize) / 1_000_000

        print("")
        print("[ModelDownload] ✓ Download complete in \(String(format: "%.1f", elapsed))s (\(String(format: "%.1f", sizeMB)) MB)")
        logger.info("Download complete: \(model.rawValue) in \(elapsed, format: .fixed(precision: 1))s")

        loggingService.info(.Transcription, "model_download_complete", data: [
            "model": AnyCodable(model.rawValue),
            "durationMs": AnyCodable(Int(elapsed * 1000)),
            "sizeMB": AnyCodable(sizeMB)
        ])

        return destination
    }

    /// Delete a downloaded model
    func deleteModel(_ model: WhisperModel) throws {
        let path = localPath(for: model)
        if FileManager.default.fileExists(atPath: path.path) {
            try FileManager.default.removeItem(at: path)
            print("[ModelDownload] Deleted model: \(path.path)")
        }
    }
}

// MARK: - Errors

enum ModelDownloadError: LocalizedError {
    case notSupported(String)
    case noDownloadURL
    case downloadFailed(String)

    var errorDescription: String? {
        switch self {
        case .notSupported(let reason):
            return "Model type not supported: \(reason)"
        case .noDownloadURL:
            return "No download URL available for this model"
        case .downloadFailed(let reason):
            return "Download failed: \(reason)"
        }
    }
}
