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
        self.currentCapabilities = HostCapabilities()
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

        do {
            let preprocessor = try MLModel(contentsOf: prepared["Preprocessor"]!, configuration: configuration)
            let encoder = try MLModel(contentsOf: prepared["Encoder"]!, configuration: configuration)
            let decoder = try MLModel(contentsOf: prepared["Decoder"]!, configuration: configuration)
            let joint = try MLModel(contentsOf: prepared["JointDecisionv3"]!, configuration: configuration)
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
                maxInFlight: 1
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
        let tokens = try decode(
            encoderOutput: encoderOutput,
            sequenceLength: sequenceLength,
            loaded: loaded,
            realAudioSeconds: Double(request.numSamples) / Double(request.sampleRate),
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
        currentCapabilities = HostCapabilities()
        lock.unlock()
    }

    private var loadedSourcePath: String?

    private func snapshotModel() -> LoadedModel? {
        lock.lock(); defer { lock.unlock() }
        return model
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
        guard sequenceLength > 0 else { return [] }
        let actualFrameCount = min(
            sequenceLength,
            max(1, Int(ceil(realAudioSeconds / loaded.frameSeconds)))
        )
        let hiddenShape = [NSNumber(value: 1), NSNumber(value: loaded.encoderHidden), NSNumber(value: 1)]
        let encoderStep = try MLMultiArray(shape: hiddenShape, dataType: .float32)
        let decoderProjectionArray = try MLMultiArray(shape: [1, NSNumber(value: loaded.decoderHidden), 1], dataType: .float32)
        var hidden = try MLMultiArray(shape: [NSNumber(value: loaded.decoderLayers), 1, NSNumber(value: loaded.decoderHidden)], dataType: .float32)
        var cell = try MLMultiArray(shape: hidden.shape, dataType: .float32)
        zero(hidden); zero(cell)
        let target = try MLMultiArray(shape: [1, 1], dataType: .int32)
        let targetLength = try MLMultiArray(shape: [1], dataType: .int32)
        targetLength[0] = 1

        let prime = try decoderStep(
            token: loaded.blankID, hidden: hidden, cell: cell, target: target,
            targetLength: targetLength, model: loaded.decoder
        )
        hidden = prime.hidden; cell = prime.cell
        try normalize(prime.projection, into: decoderProjectionArray, hiddenSize: loaded.decoderHidden)

        struct Emission { let id: Int; let frame: Int; let duration: Int; let confidence: Double }
        var emissions: [Emission] = []
        var time = 0
        var sameFrameEmissions = 0
        let maxSymbolsPerStep = 10
        let maxTokens = 150

        func joint(at frame: Int) throws -> (id: Int, confidence: Double, duration: Int) {
            try copyEncoderFrame(encoderOutput, frame: frame, into: encoderStep, hiddenSize: loaded.encoderHidden)
            let input = try MLDictionaryFeatureProvider(dictionary: [
                "encoder_step": MLFeatureValue(multiArray: encoderStep),
                "decoder_step": MLFeatureValue(multiArray: decoderProjectionArray),
            ])
            let output = try loaded.joint.prediction(from: input)
            guard let token = output.featureValue(for: "token_id")?.multiArrayValue,
                  let probability = output.featureValue(for: "token_prob")?.multiArrayValue,
                  let duration = output.featureValue(for: "duration")?.multiArrayValue else {
                throw EngineHostError(code: "engine", message: "Joint output is incomplete")
            }
            let tokenID = Int(token.dataPointer.bindMemory(to: Int32.self, capacity: token.count)[0])
            let score = max(0.1, min(1.0, Double(probability.dataPointer.bindMemory(to: Float.self, capacity: probability.count)[0])))
            let durationBin = Int(duration.dataPointer.bindMemory(to: Int32.self, capacity: duration.count)[0])
            return (tokenID, score, max(1, min(4, durationBin)))
        }

        var lastEmissionFrame = -1
        while time < actualFrameCount && emissions.count < maxTokens {
            try checkCancelled(isCancelled)
            var frame = time
            var decision = try joint(at: frame)
            var duration = decision.duration
            if decision.id == loaded.blankID && duration == 0 {
                duration = 1
            } else if decision.id != loaded.blankID && duration == 0
                        && frame == lastEmissionFrame && sameFrameEmissions >= 1 {
                duration = 1
            }
            time += duration
            while decision.id == loaded.blankID && time < actualFrameCount {
                try checkCancelled(isCancelled)
                frame = time
                decision = try joint(at: frame)
                duration = decision.duration
                if duration == 0 { duration = 1 }
                time += duration
            }
            if decision.id != loaded.blankID && time < actualFrameCount {
                if frame == (emissions.last?.frame ?? -1) { sameFrameEmissions += 1 } else { sameFrameEmissions = 1 }
                lastEmissionFrame = frame
                if sameFrameEmissions >= maxSymbolsPerStep { time = min(actualFrameCount, time + 1); sameFrameEmissions = 0 }
                emissions.append(Emission(id: decision.id, frame: frame, duration: duration, confidence: decision.confidence))
                let next = try decoderStepResult(
                    token: decision.id, hidden: hidden, cell: cell, target: target,
                    targetLength: targetLength, model: loaded.decoder
                )
                hidden = next.hidden; cell = next.cell
                try normalize(next.projection, into: decoderProjectionArray, hiddenSize: loaded.decoderHidden)
            }
        }

        // A full-sized request is normally an interior sliding-window chunk;
        // its caller will merge it with the next window. Only a short final
        // request gets the reference decoder's end-of-chunk flush.
        let isFinalWindow = realAudioSeconds < loaded.windowSeconds - 0.000_001
        var flushSteps = 0
        var consecutiveBlanks = 0
        while isFinalWindow && flushSteps < maxSymbolsPerStep
            && consecutiveBlanks < 5 && emissions.count < maxTokens {
            try checkCancelled(isCancelled)
            let decision = try joint(at: min(max(time, 0), actualFrameCount - 1))
            if decision.id == loaded.blankID {
                consecutiveBlanks += 1
            } else {
                consecutiveBlanks = 0
                emissions.append(Emission(
                    id: decision.id,
                    frame: min(max(time, 0), actualFrameCount - 1),
                    duration: decision.duration,
                    confidence: decision.confidence
                ))
                let next = try decoderStepResult(
                    token: decision.id, hidden: hidden, cell: cell, target: target,
                    targetLength: targetLength, model: loaded.decoder
                )
                hidden = next.hidden; cell = next.cell
                try normalize(next.projection, into: decoderProjectionArray, hiddenSize: loaded.decoderHidden)
            }
            time = min(sequenceLength, time + max(1, decision.duration))
            flushSteps += 1
        }

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

    private func decoderStepResult(
        token: Int,
        hidden: MLMultiArray,
        cell: MLMultiArray,
        target: MLMultiArray,
        targetLength: MLMultiArray,
        model: MLModel
    ) throws -> (projection: MLMultiArray, hidden: MLMultiArray, cell: MLMultiArray) {
        target[0] = NSNumber(value: token)
        let input = try MLDictionaryFeatureProvider(dictionary: [
            "targets": MLFeatureValue(multiArray: target),
            "target_length": MLFeatureValue(multiArray: targetLength),
            "h_in": MLFeatureValue(multiArray: hidden),
            "c_in": MLFeatureValue(multiArray: cell),
        ])
        let output = try model.prediction(from: input)
        guard let projection = output.featureValue(for: "decoder")?.multiArrayValue,
              let nextHidden = output.featureValue(for: "h_out")?.multiArrayValue,
              let nextCell = output.featureValue(for: "c_out")?.multiArrayValue else {
            throw EngineHostError(code: "engine", message: "Decoder output is incomplete")
        }
        return (projection, nextHidden, nextCell)
    }

    private func decoderStep(
        token: Int,
        hidden: MLMultiArray,
        cell: MLMultiArray,
        target: MLMultiArray,
        targetLength: MLMultiArray,
        model: MLModel
    ) throws -> (projection: MLMultiArray, hidden: MLMultiArray, cell: MLMultiArray) {
        try decoderStepResult(token: token, hidden: hidden, cell: cell, target: target, targetLength: targetLength, model: model)
    }

    private func copyEncoderFrame(_ source: MLMultiArray, frame: Int, into destination: MLMultiArray, hiddenSize: Int) throws {
        let shape = source.shape.map(\.intValue)
        let strides = source.strides.map(\.intValue)
        guard shape.count == 3, frame >= 0 else { throw EngineHostError(code: "engine", message: "Invalid encoder frame") }
        let hiddenAxis = shape[1] == hiddenSize ? 1 : 2
        let timeAxis = hiddenAxis == 1 ? 2 : 1
        guard frame < shape[timeAxis] else { throw EngineHostError(code: "engine", message: "Encoder frame out of bounds") }
        let sourcePointer = source.dataPointer.bindMemory(to: Float.self, capacity: source.count)
        let destinationPointer = destination.dataPointer.bindMemory(to: Float.self, capacity: destination.count)
        for hidden in 0..<hiddenSize {
            let sourceIndex = frame * strides[timeAxis] + hidden * strides[hiddenAxis]
            destinationPointer[hidden] = sourcePointer[sourceIndex]
        }
    }

    private func normalize(_ source: MLMultiArray, into destination: MLMultiArray, hiddenSize: Int) throws {
        let shape = source.shape.map(\.intValue)
        let strides = source.strides.map(\.intValue)
        guard shape.count == 3 else { throw EngineHostError(code: "engine", message: "Invalid decoder projection") }
        let hiddenAxis = shape[2] == hiddenSize ? 2 : 1
        guard shape[hiddenAxis] == hiddenSize else { throw EngineHostError(code: "engine", message: "Decoder hidden size mismatch") }
        let sourcePointer = source.dataPointer.bindMemory(to: Float.self, capacity: source.count)
        let destinationPointer = destination.dataPointer.bindMemory(to: Float.self, capacity: destination.count)
        for hidden in 0..<hiddenSize { destinationPointer[hidden] = sourcePointer[hidden * strides[hiddenAxis]] }
    }

    private func zero(_ array: MLMultiArray) {
        let pointer = array.dataPointer.bindMemory(to: Float.self, capacity: array.count)
        for index in 0..<array.count { pointer[index] = 0 }
    }

    private func checkCancelled(_ isCancelled: () -> Bool) throws {
        if isCancelled() { throw EngineHostError(code: "cancelled", message: "Transcription was cancelled") }
    }
}
