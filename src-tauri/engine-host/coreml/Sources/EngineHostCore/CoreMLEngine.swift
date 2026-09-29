// Portions of the TDT greedy-decoding structure are adapted from FluidAudio
// v0.17.4 (Apache-2.0); see NOTICE for attribution.
@preconcurrency import CoreML
import Foundation

public final class CoreMLEngine: EngineBackend {
    private final class LoadedModel {
        let preprocessor: MLModel
        let encoder: MLModel
        let decoder: MLModel
        let joint: MLModel
        let vocabulary: [Int: String]
        let modelID: String
        let windowSamples: Int
        let windowSeconds: Double
        let frameSeconds: Double
        let encoderFrames: Int
        let encoderHidden: Int
        let decoderLayers: Int
        let decoderHidden: Int
        let blankID: Int

        init(
            preprocessor: MLModel,
            encoder: MLModel,
            decoder: MLModel,
            joint: MLModel,
            vocabulary: [Int: String],
            modelID: String,
            windowSamples: Int,
            windowSeconds: Double,
            frameSeconds: Double,
            encoderFrames: Int,
            encoderHidden: Int,
            decoderLayers: Int,
            decoderHidden: Int,
            blankID: Int
        ) {
            self.preprocessor = preprocessor
            self.encoder = encoder
            self.decoder = decoder
            self.joint = joint
            self.vocabulary = vocabulary
            self.modelID = modelID
            self.windowSamples = windowSamples
            self.windowSeconds = windowSeconds
            self.frameSeconds = frameSeconds
            self.encoderFrames = encoderFrames
            self.encoderHidden = encoderHidden
            self.decoderLayers = decoderLayers
            self.decoderHidden = decoderHidden
            self.blankID = blankID
        }
    }

    private let lock = NSLock()
    private let cacheDirectory: URL
    private var model: LoadedModel?
    private var loading = false
    private var currentCapabilities: HostCapabilities

    public init(cacheDirectory: URL? = nil) {
        if let cacheDirectory {
            self.cacheDirectory = cacheDirectory
        } else {
            let home = FileManager.default.homeDirectoryForCurrentUser
            self.cacheDirectory = home
                .appendingPathComponent("Library/Caches/Sagascript/EngineHost", isDirectory: true)
        }
        self.currentCapabilities = HostCapabilities(maxInFlight: Self.configuredMaxInFlight)
    }

    public var capabilities: HostCapabilities {
        lock.lock(); defer { lock.unlock() }
        return currentCapabilities
    }

    public var loadedModelID: String? {
        lock.lock(); defer { lock.unlock() }
        return model?.modelID
    }

    public var isLoading: Bool {
        lock.lock(); defer { lock.unlock() }
        return loading
    }

    public func load(modelDirectory: String, modelID: String, computeUnits: String) throws -> LoadResult {
        let started = Date()
        let sourceURL = URL(fileURLWithPath: modelDirectory, isDirectory: true)
        guard FileManager.default.fileExists(atPath: sourceURL.path) else {
            throw EngineHostError(code: "model_missing", message: "Model directory does not exist: \(modelDirectory)")
        }
        let units = try makeComputeUnits(computeUnits)

        lock.lock()
        loading = true
        let alreadyLoaded = model?.modelID == modelID && modelDirectory == loadedSourcePath
        lock.unlock()
        defer {
            lock.lock(); loading = false; lock.unlock()
        }

        if alreadyLoaded, let existing = snapshotModel() {
            return LoadResult(
                modelID: existing.modelID,
                loadMilliseconds: 0,
                compiled: false,
                windowSeconds: existing.windowSeconds,
                frameSeconds: existing.frameSeconds,
                vocabularySize: existing.vocabulary.count,
                blankID: existing.blankID
            )
        }

        let prepared = try prepareComponents(
            sourceURL: sourceURL,
            modelID: modelID
        )
        let configuration = MLModelConfiguration()
        configuration.computeUnits = units
        // The mel front end has ops that only run on the CPU; requesting the
        // Neural Engine for it only adds dispatch overhead. The per-token
        // decoder/joint networks are tiny, so the same applies to them when
        // measured (see docs/engine-host-protocol.md tuning notes).
        let preprocessorConfiguration = MLModelConfiguration()
        preprocessorConfiguration.computeUnits = try tuningUnits("SAGASCRIPT_ENGINE_PREPROCESSOR_UNITS", default: .cpuOnly)
        let decoderConfiguration = MLModelConfiguration()
        decoderConfiguration.computeUnits = try tuningUnits("SAGASCRIPT_ENGINE_DECODER_UNITS", default: units)

        do {
            let preprocessor = try MLModel(contentsOf: prepared["Preprocessor"]!, configuration: preprocessorConfiguration)
            let encoder = try MLModel(contentsOf: prepared["Encoder"]!, configuration: configuration)
            let decoder = try MLModel(contentsOf: prepared["Decoder"]!, configuration: decoderConfiguration)
            let joint = try MLModel(contentsOf: prepared["JointDecisionv3"]!, configuration: decoderConfiguration)
            let vocabulary = try loadVocabulary(at: prepared.vocabularyURL)
            let loaded = try makeLoadedModel(
                preprocessor: preprocessor,
                encoder: encoder,
                decoder: decoder,
                joint: joint,
                vocabulary: vocabulary,
                modelID: modelID
            )
            lock.lock()
            model = loaded
            loadedSourcePath = modelDirectory
            let overlap = ProcessInfo.processInfo.environment["SAGASCRIPT_ENGINE_OVERLAP_S"]
                .flatMap(Double.init)
                ?? (loaded.windowSeconds <= 15 ? 2 : 6)
            currentCapabilities = HostCapabilities(
                maxWindowSeconds: loaded.windowSeconds,
                preferredWindowSeconds: loaded.windowSeconds,
                preferredOverlapSeconds: overlap,
                maxInFlight: configuredMaxInFlight
            )
            lock.unlock()

            return LoadResult(
                modelID: modelID,
                loadMilliseconds: Date().timeIntervalSince(started) * 1000,
                compiled: prepared.compiled,
                windowSeconds: loaded.windowSeconds,
                frameSeconds: loaded.frameSeconds,
                vocabularySize: loaded.vocabulary.count,
                blankID: loaded.blankID
            )
        } catch let error as EngineHostError {
            throw error
        } catch {
            throw EngineHostError(code: "model_load_failed", message: error.localizedDescription)
        }
    }

    public func transcribe(_ request: WindowRequest, isCancelled: @escaping () -> Bool) throws -> WindowResult {
        guard request.sampleRate == 16_000 else {
            throw EngineHostError(code: "bad_request", message: "sample_rate must be 16000")
        }
        guard request.format == "f32le" else {
            throw EngineHostError(code: "bad_request", message: "format must be f32le")
        }
        guard request.offsetSamples >= 0, request.numSamples > 0 else {
            throw EngineHostError(code: "bad_request", message: "offset_samples and num_samples must be valid")
        }
        guard let loaded = snapshotModel() else {
            throw EngineHostError(code: "not_loaded", message: "No CoreML model is loaded")
        }
        guard request.numSamples <= loaded.windowSamples else {
            throw EngineHostError(code: "bad_request", message: "num_samples exceeds the loaded model window")
        }
        try checkCancelled(isCancelled)

        let samples = try readSamples(request: request, paddedTo: loaded.windowSamples)
        try checkCancelled(isCancelled)

        let preprocessStart = Date()
        let audioSignal = try makeFloatArray(samples)
        // The reference pipeline rounds the declared length up to a whole
        // encoder frame (1280 samples), which reproduces its text exactly on the
        // ten fleurs clips but costs one word (Aristoteles -> Aristotels) versus
        // declaring the true length, so the true length is kept.
        let audioLength = try makeIntArray([request.numSamples])
        let preprocessorInput = try MLDictionaryFeatureProvider(dictionary: [
            "audio_signal": MLFeatureValue(multiArray: audioSignal),
            "audio_length": MLFeatureValue(multiArray: audioLength),
        ])
        let preprocessed = try loaded.preprocessor.prediction(from: preprocessorInput)
        guard let mel = preprocessed.featureValue(for: "mel")?.multiArrayValue else {
            throw EngineHostError(code: "engine", message: "Preprocessor output is missing mel")
        }
        let melLength = intValue(preprocessed, name: "mel_length", fallback: mel.shape.last?.intValue ?? 0)
        let preprocessMilliseconds = Date().timeIntervalSince(preprocessStart) * 1000
        try checkCancelled(isCancelled)

        let encodeStart = Date()
        let encoderLengthInput = try makeIntArray([melLength])
        let encoderInput = try MLDictionaryFeatureProvider(dictionary: [
            "mel": MLFeatureValue(multiArray: mel),
            "mel_length": MLFeatureValue(multiArray: encoderLengthInput),
        ])
        let encoded = try loaded.encoder.prediction(from: encoderInput)
        guard let encoderOutput = encoded.featureValue(for: "encoder")?.multiArrayValue else {
            throw EngineHostError(code: "engine", message: "Encoder output is missing encoder")
        }
        let sequenceLength = min(
            intValue(encoded, name: "encoder_length", fallback: loaded.encoderFrames),
            loaded.encoderFrames
        )
        let encodeMilliseconds = Date().timeIntervalSince(encodeStart) * 1000
        try checkCancelled(isCancelled)

        let decodeStart = Date()
        let realAudioSeconds = Double(request.numSamples) / Double(request.sampleRate)
        let tokens = try decode(
            encoderOutput: encoderOutput,
            sequenceLength: sequenceLength,
            loaded: loaded,
            realAudioSeconds: realAudioSeconds,
            isCancelled: isCancelled
        )
        let decodeMilliseconds = Date().timeIntervalSince(decodeStart) * 1000

        return WindowResult(
            tokens: tokens,
            audioSeconds: Double(request.numSamples) / Double(request.sampleRate),
            preprocessMilliseconds: preprocessMilliseconds,
            encodeMilliseconds: encodeMilliseconds,
            decodeMilliseconds: decodeMilliseconds
        )
    }

    public func unload() {
        lock.lock()
        model = nil
        loadedSourcePath = nil
        currentCapabilities = HostCapabilities(maxInFlight: configuredMaxInFlight)
        lock.unlock()
    }

    private var loadedSourcePath: String?

    private func snapshotModel() -> LoadedModel? {
        lock.lock(); defer { lock.unlock() }
        return model
    }

    private var configuredMaxInFlight: Int { Self.configuredMaxInFlight }

    /// Measured on the 15-minute Swedish benchmark: 1 -> 5.3 s, 2 -> 3.2 s,
    /// 3 -> 2.9 s, 4 -> 2.8 s, 6 -> 2.8 s. The Neural Engine encoder becomes the
    /// bottleneck at 3 (about 2.2 s of serialized encoder time), so 3 is the
    /// smallest value on the plateau. `SAGASCRIPT_ENGINE_MAX_IN_FLIGHT` overrides
    /// it for benchmarking.
    private static var configuredMaxInFlight: Int {
        ProcessInfo.processInfo.environment["SAGASCRIPT_ENGINE_MAX_IN_FLIGHT"]
            .flatMap(Int.init).map { max(1, min(8, $0)) } ?? 3
    }

    private func tuningUnits(_ variable: String, default fallback: MLComputeUnits) throws -> MLComputeUnits {
        guard let value = ProcessInfo.processInfo.environment[variable] else { return fallback }
        return try makeComputeUnits(value)
    }

    private func makeComputeUnits(_ name: String) throws -> MLComputeUnits {
        switch name {
        case "ane": return .cpuAndNeuralEngine
        case "gpu": return .cpuAndGPU
        case "cpu": return .cpuOnly
        case "all": return .all
        default:
            throw EngineHostError(code: "bad_request", message: "compute_units must be ane, gpu, cpu, or all")
        }
    }

    private struct PreparedComponents {
        let paths: [String: URL]
        let vocabularyURL: URL
        let compiled: Bool

        subscript(_ name: String) -> URL? { paths[name] }
    }

    private func prepareComponents(sourceURL: URL, modelID: String) throws -> PreparedComponents {
        let names = ["Preprocessor", "Encoder", "Decoder", "JointDecisionv3"]
        var paths: [String: URL] = [:]
        var packages: [String: URL] = [:]
        for name in names {
            if let compiled = findComponent(named: name, suffix: "mlmodelc", in: sourceURL) {
                paths[name] = compiled
            } else if let package = findComponent(named: name, suffix: "mlpackage", in: sourceURL) {
                packages[name] = package
            } else {
                throw EngineHostError(code: "model_missing", message: "Missing \(name).mlmodelc or \(name).mlpackage")
            }
        }
        guard let vocabularyURL = findVocabulary(in: sourceURL) else {
            throw EngineHostError(code: "model_missing", message: "Missing parakeet_vocab.json")
        }
        guard !packages.isEmpty else {
            return PreparedComponents(paths: paths, vocabularyURL: vocabularyURL, compiled: false)
        }

        var parts = [modelID]
        for name in names {
            if let path = paths[name] {
                parts.append("\(name):\(try hashURL(path))")
            }
            if let path = packages[name] {
                parts.append("\(name):\(try hashURL(path))")
            }
        }
        parts.append("vocab:\(try hashURL(vocabularyURL))")
        let key = sha256Data(Data(parts.sorted().joined(separator: "\n").utf8))
        let destination = cacheDirectory.appendingPathComponent(key, isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        } catch {
            throw EngineHostError(code: "model_load_failed", message: "Cannot create CoreML cache: \(error.localizedDescription)")
        }

        var didCompile = false
        for name in names {
            guard let package = packages[name] else { continue }
            let target = destination.appendingPathComponent("\(name).mlmodelc", isDirectory: true)
            if !FileManager.default.fileExists(atPath: target.path) {
                do {
                    let compiled = try MLModel.compileModel(at: package)
                    try FileManager.default.copyItem(at: compiled, to: target)
                    didCompile = true
                } catch {
                    throw EngineHostError(code: "model_load_failed", message: "CoreML compilation failed for \(name): \(error.localizedDescription)")
                }
            }
            paths[name] = target
        }
        return PreparedComponents(paths: paths, vocabularyURL: vocabularyURL, compiled: didCompile)
    }

    private func findComponent(named name: String, suffix: String, in directory: URL) -> URL? {
        guard let enumerator = FileManager.default.enumerator(
            at: directory,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) else { return nil }
        for case let url as URL in enumerator {
            guard url.lastPathComponent == "\(name).\(suffix)" else { continue }
            return url
        }
        return nil
    }

    private func findVocabulary(in directory: URL) -> URL? {
        guard let enumerator = FileManager.default.enumerator(at: directory, includingPropertiesForKeys: nil) else {
            return nil
        }
        for case let url as URL in enumerator where url.lastPathComponent == "parakeet_vocab.json" {
            return url
        }
        return nil
    }

    private func hashURL(_ url: URL) throws -> String {
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) else {
            throw EngineHostError(code: "model_missing", message: "Missing model component: \(url.path)")
        }
        if !isDirectory.boolValue { return try sha256File(url) }
        guard let enumerator = FileManager.default.enumerator(at: url, includingPropertiesForKeys: [.isDirectoryKey]) else {
            throw EngineHostError(code: "model_load_failed", message: "Cannot enumerate model component: \(url.path)")
        }
        let files = enumerator.compactMap { $0 as? URL }.filter { candidate in
            var childDirectory: ObjCBool = false
            return FileManager.default.fileExists(atPath: candidate.path, isDirectory: &childDirectory) && !childDirectory.boolValue
        }.sorted { $0.path < $1.path }
        var entries = Data()
        for file in files {
            entries.append(contentsOf: file.path.replacingOccurrences(of: url.path + "/", with: "").utf8)
            entries.append(0)
            entries.append(contentsOf: try sha256File(file).utf8)
            entries.append(0)
        }
        return sha256Data(entries)
    }

    private func loadVocabulary(at url: URL) throws -> [Int: String] {
        let data = try Data(contentsOf: url)
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw EngineHostError(code: "model_load_failed", message: "Vocabulary is not a JSON object")
        }
        var vocabulary: [Int: String] = [:]
        for (key, value) in object {
            guard let id = Int(key), let text = value as? String else {
                throw EngineHostError(code: "model_load_failed", message: "Vocabulary contains an invalid entry")
            }
            vocabulary[id] = text
        }
        guard !vocabulary.isEmpty else {
            throw EngineHostError(code: "model_load_failed", message: "Vocabulary is empty")
        }
        return vocabulary
    }

    private func makeLoadedModel(
        preprocessor: MLModel,
        encoder: MLModel,
        decoder: MLModel,
        joint: MLModel,
        vocabulary: [Int: String],
        modelID: String
    ) throws -> LoadedModel {
        guard let audioShape = shape(of: preprocessor, input: "audio_signal"), audioShape.count >= 2,
              let windowSamples = audioShape.last?.intValue, windowSamples > 0 else {
            throw EngineHostError(code: "model_load_failed", message: "Preprocessor audio_signal shape is unavailable")
        }
        guard let encoderShape = shape(of: encoder, output: "encoder"), encoderShape.count == 3 else {
            throw EngineHostError(code: "model_load_failed", message: "Encoder output shape is unavailable")
        }
        let encoderHidden = encoderShape.dropFirst().map(\.intValue).max() ?? 0
        let encoderFrames = encoderShape.dropFirst().map(\.intValue).min() ?? 0
        guard encoderHidden > 0, encoderFrames > 0 else {
            throw EngineHostError(code: "model_load_failed", message: "Encoder output shape is invalid")
        }
        guard let stateShape = shape(of: decoder, input: "h_in"), stateShape.count == 3,
              let decoderLayers = stateShape.first?.intValue,
              let decoderHidden = stateShape.last?.intValue,
              decoderLayers > 0, decoderHidden > 0 else {
            throw EngineHostError(code: "model_load_failed", message: "Decoder h_in shape is unavailable")
        }
        // Parakeet-TDT-v3 stores the 8192 regular SentencePiece entries in
        // parakeet_vocab.json; the transducer blank is the following class
        // and is intentionally absent from the vocabulary file.
        guard let maxVocabularyID = vocabulary.keys.max() else {
            throw EngineHostError(code: "model_load_failed", message: "Vocabulary is empty")
        }
        let blankID = maxVocabularyID + 1
        let windowSeconds = Double(windowSamples) / 16_000
        return LoadedModel(
            preprocessor: preprocessor,
            encoder: encoder,
            decoder: decoder,
            joint: joint,
            vocabulary: vocabulary,
            modelID: modelID,
            windowSamples: windowSamples,
            windowSeconds: windowSeconds,
            // Parakeet's conformer subsampling is eight 10 ms mel hops. The
            // output frame count is ceil-based (188 frames for a 15 s input),
            // so dividing the fixed window by that count would make every
            // seam drift by one frame over time. Derive the frame count from
            // the model, but retain the architecture's 80 ms frame clock.
            frameSeconds: 0.08,
            encoderFrames: encoderFrames,
            encoderHidden: encoderHidden,
            decoderLayers: decoderLayers,
            decoderHidden: decoderHidden,
            blankID: blankID
        )
    }

    private func shape(of model: MLModel, input name: String) -> [NSNumber]? {
        model.modelDescription.inputDescriptionsByName[name]?.multiArrayConstraint?.shape
    }

    private func shape(of model: MLModel, output name: String) -> [NSNumber]? {
        model.modelDescription.outputDescriptionsByName[name]?.multiArrayConstraint?.shape
    }

    private func makeFloatArray(_ values: [Float]) throws -> MLMultiArray {
        let result = try MLMultiArray(shape: [1, NSNumber(value: values.count)] as [NSNumber], dataType: .float32)
        let pointer = result.dataPointer.bindMemory(to: Float.self, capacity: values.count)
        values.withUnsafeBufferPointer { source in
            pointer.update(from: source.baseAddress!, count: values.count)
        }
        return result
    }

    private func makeIntArray(_ values: [Int]) throws -> MLMultiArray {
        let result = try MLMultiArray(shape: [NSNumber(value: values.count)] as [NSNumber], dataType: .int32)
        let pointer = result.dataPointer.bindMemory(to: Int32.self, capacity: values.count)
        for (index, value) in values.enumerated() { pointer[index] = Int32(value) }
        return result
    }

    private func intValue(_ provider: MLFeatureProvider, name: String, fallback: Int) -> Int {
        guard let array = provider.featureValue(for: name)?.multiArrayValue, array.count > 0 else { return fallback }
        return Int(array.dataPointer.bindMemory(to: Int32.self, capacity: array.count)[0])
    }

    private func readSamples(request: WindowRequest, paddedTo count: Int) throws -> [Float] {
        let byteOffset = request.offsetSamples.multipliedReportingOverflow(by: 4)
        let byteCount = request.numSamples.multipliedReportingOverflow(by: 4)
        guard !byteOffset.overflow, !byteCount.overflow else {
            throw EngineHostError(code: "bad_request", message: "PCM range is too large")
        }
        let handle: FileHandle
        do { handle = try FileHandle(forReadingFrom: URL(fileURLWithPath: request.pcmPath)) }
        catch { throw EngineHostError(code: "bad_request", message: "Cannot open pcm_path: \(error.localizedDescription)") }
        defer { try? handle.close() }
        do {
            handle.seek(toFileOffset: UInt64(byteOffset.partialValue))
            guard let data = try handle.read(upToCount: byteCount.partialValue), data.count == byteCount.partialValue else {
                throw EngineHostError(code: "bad_request", message: "pcm_path does not contain the requested range")
            }
            var samples = [Float](repeating: 0, count: count)
            data.withUnsafeBytes { raw in
                let bytes = raw.bindMemory(to: UInt8.self)
                for index in 0..<request.numSamples {
                    let base = index * 4
                    let bits = UInt32(bytes[base])
                        | UInt32(bytes[base + 1]) << 8
                        | UInt32(bytes[base + 2]) << 16
                        | UInt32(bytes[base + 3]) << 24
                    samples[index] = Float(bitPattern: bits)
                }
            }
            return samples
        } catch let error as EngineHostError {
            throw error
        } catch {
            throw EngineHostError(code: "bad_request", message: "Cannot read pcm_path: \(error.localizedDescription)")
        }
    }

    private func decode(
        encoderOutput: MLMultiArray,
        sequenceLength: Int,
        loaded: LoadedModel,
        realAudioSeconds: Double,
        isCancelled: @escaping () -> Bool
    ) throws -> [TranscriptionToken] {
        let actualFrameCount = min(
            sequenceLength,
            max(1, Int(ceil(realAudioSeconds / loaded.frameSeconds)))
        )
        let session = try TdtDecodeSession(
            decoder: loaded.decoder,
            joint: loaded.joint,
            blankID: loaded.blankID,
            encoderHidden: loaded.encoderHidden,
            decoderHidden: loaded.decoderHidden,
            decoderLayers: loaded.decoderLayers
        )
        // A full-sized request is normally an interior sliding-window chunk;
        // its caller will merge it with the next window. Only a short final
        // request gets the reference decoder's end-of-chunk flush.
        let isFinalWindow = realAudioSeconds < loaded.windowSeconds - 0.000_001
        let emissions = try session.decode(
            encoderOutput: encoderOutput,
            sequenceLength: sequenceLength,
            actualFrames: actualFrameCount,
            isLastChunk: isFinalWindow,
            isCancelled: isCancelled
        )
        return emissions.compactMap { emission in
            let correctedFrame = max(0, emission.frame - 1)
            let start = Double(correctedFrame) * loaded.frameSeconds
            guard start < realAudioSeconds, let text = loaded.vocabulary[emission.id], !text.isEmpty else { return nil }
            return TranscriptionToken(
                id: emission.id,
                text: text,
                start: start,
                duration: max(loaded.frameSeconds, Double(emission.duration) * loaded.frameSeconds),
                confidence: emission.confidence
            )
        }
    }

    private func checkCancelled(_ isCancelled: () -> Bool) throws {
        if isCancelled() { throw EngineHostError(code: "cancelled", message: "Transcription was cancelled") }
    }
}
