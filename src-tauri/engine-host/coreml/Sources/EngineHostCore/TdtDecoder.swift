// The TDT greedy-decoding loop below follows FluidAudio v0.17.4 (Apache-2.0)
// step for step (TdtDecoderV3 / TdtModelInference); see NOTICE for attribution.
@preconcurrency import CoreML
import Foundation

struct TdtEmission {
    let id: Int
    let frame: Int
    let duration: Int
    let confidence: Double
}

/// Allocates a 64-byte aligned float/int array whose innermost dimension is
/// padded to the ANE tile size, so Core ML can hand it to the Neural Engine
/// without re-laying it out.
func makeAlignedArray(shape: [Int], dataType: MLMultiArrayDataType) throws -> MLMultiArray {
    let elementSize = dataType == .float16 ? 2 : (dataType == .double ? 8 : 4)
    var strides = [Int](repeating: 1, count: shape.count)
    var running = 1
    for index in stride(from: shape.count - 1, through: 0, by: -1) {
        strides[index] = running
        let dimension = shape[index]
        if index == shape.count - 1 && dimension % 16 != 0 {
            running *= ((dimension + 15) / 16) * 16
        } else {
            running *= dimension
        }
    }
    let elements = strides[0] * shape[0]
    let bytes = max(64, ((elements * elementSize + 63) / 64) * 64)
    var raw: UnsafeMutableRawPointer?
    guard posix_memalign(&raw, 64, bytes) == 0, let pointer = raw else {
        throw EngineHostError(code: "engine", message: "Cannot allocate aligned tensor")
    }
    memset(pointer, 0, bytes)
    return try MLMultiArray(
        dataPointer: pointer,
        shape: shape.map { NSNumber(value: $0) },
        dataType: dataType,
        strides: strides.map { NSNumber(value: $0) },
        deallocator: { Darwin.free($0) }
    )
}

/// A feature provider over fixed arrays; avoids rebuilding a dictionary
/// provider for every decoder/joint step.
final class PreparedFeatureProvider: NSObject, MLFeatureProvider {
    private let values: [String: MLFeatureValue]
    let featureNames: Set<String>

    init(arrays: [String: MLMultiArray]) {
        values = arrays.mapValues { MLFeatureValue(multiArray: $0) }
        featureNames = Set(arrays.keys)
        super.init()
    }

    func featureValue(for featureName: String) -> MLFeatureValue? { values[featureName] }
}

/// Stride-aware view over the encoder output that copies single frames.
/// Accepts float32 and float16 encoder outputs.
struct EncoderFrames {
    let count: Int
    private let base: UnsafeRawPointer
    private let precision: TensorPrecision
    private let hiddenSize: Int
    private let hiddenStride: Int
    private let timeStride: Int

    init(_ array: MLMultiArray, validLength: Int, hiddenSize: Int) throws {
        let shape = array.shape.map(\.intValue)
        guard shape.count == 3, shape[0] == 1, let precision = TensorPrecision(array.dataType) else {
            throw EngineHostError(
                code: "engine",
                message: "Encoder output 'encoder': unexpected layout, \(describe(array)); expected [1, \(hiddenSize), frames] float32 or float16"
            )
        }
        let hiddenAxis = shape[1] == hiddenSize ? 1 : 2
        guard shape[hiddenAxis] == hiddenSize else {
            throw EngineHostError(
                code: "engine",
                message: "Encoder output 'encoder': hidden size mismatch, \(describe(array)); expected \(hiddenSize)"
            )
        }
        let timeAxis = hiddenAxis == 1 ? 2 : 1
        let strides = array.strides.map(\.intValue)
        guard strides[hiddenAxis] > 0, strides[timeAxis] > 0 else {
            throw EngineHostError(
                code: "engine",
                message: "Encoder output 'encoder': unexpected strides \(strides), \(describe(array))"
            )
        }
        self.hiddenSize = hiddenSize
        self.hiddenStride = strides[hiddenAxis]
        self.timeStride = strides[timeAxis]
        self.count = min(validLength, shape[timeAxis])
        self.precision = precision
        self.base = UnsafeRawPointer(array.dataPointer)
    }

    func copyFrame(_ index: Int, to destination: FloatVectorDestination) {
        let offset = index * timeStride
        if precision == .float32 && destination.precision == .float32 && hiddenStride == 1 && destination.stride == 1 {
            destination.raw.assumingMemoryBound(to: Float.self)
                .update(from: base.assumingMemoryBound(to: Float.self) + offset, count: hiddenSize)
            return
        }
        for hidden in 0..<hiddenSize {
            destination.store(hidden, loadFloat(base, precision, offset + hidden * hiddenStride))
        }
    }

    /// Float32 convenience used by tests.
    func copyFrame(_ index: Int, to destination: UnsafeMutablePointer<Float>, stride destinationStride: Int) {
        for hidden in 0..<hiddenSize {
            destination[hidden * destinationStride] = loadFloat(base, precision, index * timeStride + hidden * hiddenStride)
        }
    }
}

/// One decoding session per window; owns all per-step buffers so the hot
/// loop performs no allocations.
final class TdtDecodeSession {
    private let decoder: MLModel
    private let joint: MLModel
    private let blankID: Int
    private let encoderHidden: Int
    private let decoderHidden: Int
    private let maxSymbolsPerStep = 10
    private let maxTokensPerChunk = 150
    private let consecutiveBlankLimit = 5

    private let target: MLMultiArray
    private let targetLength: MLMultiArray
    private let hidden: MLMultiArray
    private let cell: MLMultiArray
    private let encoderStep: MLMultiArray
    private let decoderStep: MLMultiArray
    private let tokenIDBacking: MLMultiArray
    private let tokenProbBacking: MLMultiArray
    private let durationBacking: MLMultiArray
    private let decoderInput: PreparedFeatureProvider
    private let jointInput: PreparedFeatureProvider
    private let decoderOptions = MLPredictionOptions()
    private let jointOptions = MLPredictionOptions()
    private let encoderStepDestination: FloatVectorDestination
    private let decoderStepDestination: FloatVectorDestination
    private let tokenProbPrecision: TensorPrecision
    private var bias: BiasState?
    private let topKIDs: MLMultiArray?
    private let topKLogits: MLMultiArray?
    private let topKLogitsPrecision: TensorPrecision?

    init(decoder: MLModel, joint: MLModel, blankID: Int, encoderHidden: Int, decoderHidden: Int, decoderLayers: Int, bias: BoostTrie? = nil) throws {
        self.decoder = decoder
        self.joint = joint
        self.blankID = blankID
        self.encoderHidden = encoderHidden
        self.decoderHidden = decoderHidden
        // Interface dtypes are read from the models, so float16 and float32
        // packages both work without conversion.
        let statePrecision = try floatPrecision(of: decoder, input: "h_in", model: "Decoder")
        for (kind, name, precision) in [
            ("input", "c_in", try floatPrecision(of: decoder, input: "c_in", model: "Decoder")),
            ("output", "h_out", try floatPrecision(of: decoder, output: "h_out", model: "Decoder")),
            ("output", "c_out", try floatPrecision(of: decoder, output: "c_out", model: "Decoder")),
        ] where precision != statePrecision {
            throw EngineHostError(
                code: "model_load_failed",
                message: "Decoder \(kind) '\(name)' is \(precision.rawValue) but h_in is \(statePrecision.rawValue)"
            )
        }
        let encoderStepPrecision = try floatPrecision(of: joint, input: "encoder_step", model: "JointDecisionv3")
        let decoderStepPrecision = try floatPrecision(of: joint, input: "decoder_step", model: "JointDecisionv3")
        tokenProbPrecision = try floatPrecision(of: joint, output: "token_prob", model: "JointDecisionv3")
        try requireInt32(of: joint, output: "token_id", model: "JointDecisionv3")
        try requireInt32(of: joint, output: "duration", model: "JointDecisionv3")
        target = try MLMultiArray(shape: [1, 1], dataType: .int32)
        targetLength = try MLMultiArray(shape: [1], dataType: .int32)
        targetLength[0] = 1
        hidden = try makeAlignedArray(shape: [decoderLayers, 1, decoderHidden], dataType: statePrecision.dataType)
        cell = try makeAlignedArray(shape: [decoderLayers, 1, decoderHidden], dataType: statePrecision.dataType)
        encoderStep = try makeAlignedArray(shape: [1, encoderHidden, 1], dataType: encoderStepPrecision.dataType)
        decoderStep = try makeAlignedArray(shape: [1, decoderHidden, 1], dataType: decoderStepPrecision.dataType)
        tokenIDBacking = try MLMultiArray(shape: [1, 1, 1], dataType: .int32)
        tokenProbBacking = try MLMultiArray(shape: [1, 1, 1], dataType: tokenProbPrecision.dataType)
        durationBacking = try MLMultiArray(shape: [1, 1, 1], dataType: .int32)
        decoderInput = PreparedFeatureProvider(arrays: [
            "targets": target, "target_length": targetLength, "h_in": hidden, "c_in": cell,
        ])
        jointInput = PreparedFeatureProvider(arrays: ["encoder_step": encoderStep, "decoder_step": decoderStep])
        decoderOptions.outputBackings = ["h_out": hidden, "c_out": cell]
        var jointBackings: [String: MLMultiArray] = [
            "token_id": tokenIDBacking, "token_prob": tokenProbBacking, "duration": durationBacking,
        ]
        if let bias, !bias.isEmpty,
           let idsConstraint = constraint(joint, input: nil, output: "top_k_ids"), idsConstraint.dataType == .int32,
           let logitsPrecision = try? floatPrecision(of: joint, output: "top_k_logits", model: "JointDecisionv3"),
           let count = idsConstraint.shape.last?.intValue, count > 0 {
            let ids = try MLMultiArray(shape: [1, 1, 1, NSNumber(value: count)], dataType: .int32)
            let logits = try MLMultiArray(shape: [1, 1, 1, NSNumber(value: count)], dataType: logitsPrecision.dataType)
            jointBackings["top_k_ids"] = ids
            jointBackings["top_k_logits"] = logits
            topKIDs = ids; topKLogits = logits; topKLogitsPrecision = logitsPrecision
            let factor = Float(ProcessInfo.processInfo.environment["SAGASCRIPT_BOOST_START_FACTOR"] ?? "") ?? 0.25
            self.bias = BiasState(trie: bias, startFactor: factor)
        } else {
            topKIDs = nil; topKLogits = nil; topKLogitsPrecision = nil
            self.bias = nil
        }
        jointOptions.outputBackings = jointBackings
        encoderStepDestination = FloatVectorDestination(encoderStep, precision: encoderStepPrecision, axis: 1)
        decoderStepDestination = FloatVectorDestination(decoderStep, precision: decoderStepPrecision, axis: 1)
    }

    private struct Decision {
        let token: Int
        let probability: Float
        let duration: Int
    }

    /// Runs the prediction network on one token and refreshes `decoderStep`.
    /// The LSTM state is updated in place through the output backings.
    private func runDecoder(token: Int) throws {
        target[0] = NSNumber(value: token)
        let output = try decoder.prediction(from: decoderInput, options: decoderOptions)
        guard let projection = output.featureValue(for: "decoder")?.multiArrayValue else {
            throw EngineHostError(
                code: "engine",
                message: "Decoder output 'decoder' is missing (outputs: \(output.featureNames.sorted()))"
            )
        }
        try normalize(projection)
    }

    private func normalize(_ source: MLMultiArray) throws {
        let shape = source.shape.map(\.intValue)
        let strides = source.strides.map(\.intValue)
        guard shape.count == 3, let precision = TensorPrecision(source.dataType) else {
            throw EngineHostError(
                code: "engine",
                message: "Decoder output 'decoder': invalid projection, \(describe(source)); expected rank 3 float32 or float16"
            )
        }
        let hiddenAxis = shape[2] == decoderHidden ? 2 : 1
        guard shape[hiddenAxis] == decoderHidden else {
            throw EngineHostError(
                code: "engine",
                message: "Decoder output 'decoder': hidden size mismatch, \(describe(source)); expected \(decoderHidden)"
            )
        }
        let raw = UnsafeRawPointer(source.dataPointer)
        let sourceStride = strides[hiddenAxis]
        for index in 0..<decoderHidden {
            decoderStepDestination.store(index, loadFloat(raw, precision, index * sourceStride))
        }
    }

    private func runJoint(frames: EncoderFrames, frame: Int) throws -> Decision {
        frames.copyFrame(frame, to: encoderStepDestination)
        _ = try joint.prediction(from: jointInput, options: jointOptions)
        var token = Int(tokenIDBacking.dataPointer.bindMemory(to: Int32.self, capacity: 1)[0])
        var probability = loadFloat(UnsafeRawPointer(tokenProbBacking.dataPointer), tokenProbPrecision, 0)
        if bias != nil, let topKIDs, let topKLogits, let precision = topKLogitsPrecision {
            let count = topKIDs.count
            let idPointer = topKIDs.dataPointer.bindMemory(to: Int32.self, capacity: count)
            let raw = UnsafeRawPointer(topKLogits.dataPointer)
            let ids = (0..<count).map { Int(idPointer[$0]) }
            let logits = (0..<count).map { loadFloat(raw, precision, $0) }
            if let pick = bias!.choose(ids: ids, logits: logits, frame: frame) {
                token = pick.id
                probability = pick.probability
            }
        }
        return Decision(
            token: token,
            probability: probability,
            duration: Int(durationBacking.dataPointer.bindMemory(to: Int32.self, capacity: 1)[0])
        )
    }

    private func confidence(_ probability: Float) -> Double {
        guard probability.isFinite else { return 0.1 }
        return max(0.1, min(1.0, Double(probability)))
    }

    /// `durationBins` of Parakeet-TDT-v3 are `[0, 1, 2, 3, 4]`, i.e. the model's
    /// duration class is the frame advance.
    private func mapDuration(_ bin: Int) throws -> Int {
        guard bin >= 0 && bin <= 4 else {
            throw EngineHostError(code: "engine", message: "Duration bin index out of range: \(bin)")
        }
        return bin
    }

    func decode(
        encoderOutput: MLMultiArray,
        sequenceLength: Int,
        actualFrames: Int,
        isLastChunk: Bool,
        isCancelled: () -> Bool
    ) throws -> [TdtEmission] {
        guard sequenceLength > 1 else { return [] }
        let frames = try EncoderFrames(encoderOutput, validLength: sequenceLength, hiddenSize: encoderHidden)
        let effectiveLength = min(sequenceLength, actualFrames)
        let lastTimestep = effectiveLength - 1
        var timeIndices = 0
        guard timeIndices < effectiveLength else { return [] }
        var safeTimeIndices = min(timeIndices, lastTimestep)
        var timeIndicesCurrentLabels = timeIndices
        var activeMask = true

        try runDecoder(token: blankID)  // blank doubles as start-of-sequence

        var emissions: [TdtEmission] = []
        var lastEmissionTimestamp = -1
        var emissionsAtThisTimestamp = 0
        var tokensProcessed = 0

        func cancelCheck() throws {
            if isCancelled() { throw EngineHostError(code: "cancelled", message: "Transcription was cancelled") }
        }

        while activeMask {
            try cancelCheck()
            let decision = try runJoint(frames: frames, frame: safeTimeIndices)
            var label = decision.token
            var score = confidence(decision.probability)
            var duration = try mapDuration(decision.duration)
            var blankMask = label == blankID

            let currentTimeIndex = timeIndices
            if !blankMask && duration == 0 && currentTimeIndex == lastEmissionTimestamp && emissionsAtThisTimestamp >= 1 {
                duration = 1
            }
            if blankMask && duration == 0 { duration = 1 }

            timeIndicesCurrentLabels = timeIndices
            timeIndices += duration
            safeTimeIndices = min(timeIndices, lastTimestep)
            activeMask = timeIndices < effectiveLength
            var advanceMask = activeMask && blankMask

            // Blank runs reuse the decoder projection: silence does not change the LSTM context.
            while advanceMask {
                try cancelCheck()
                timeIndicesCurrentLabels = timeIndices
                let inner = try runJoint(frames: frames, frame: safeTimeIndices)
                label = inner.token
                score = confidence(inner.probability)
                duration = try mapDuration(inner.duration)
                blankMask = label == blankID
                if blankMask && duration == 0 { duration = 1 }
                timeIndices += duration
                safeTimeIndices = min(timeIndices, lastTimestep)
                activeMask = timeIndices < effectiveLength
                advanceMask = activeMask && blankMask
            }

            if activeMask && label != blankID {
                tokensProcessed += 1
                if tokensProcessed > maxTokensPerChunk { break }
                emissions.append(TdtEmission(
                    id: label, frame: timeIndicesCurrentLabels, duration: duration, confidence: score
                ))
                try runDecoder(token: label)
                bias?.emitted(token: label, frame: timeIndicesCurrentLabels)

                if timeIndicesCurrentLabels == lastEmissionTimestamp {
                    emissionsAtThisTimestamp += 1
                } else {
                    lastEmissionTimestamp = timeIndicesCurrentLabels
                    emissionsAtThisTimestamp = 1
                }
                if emissionsAtThisTimestamp >= maxSymbolsPerStep {
                    timeIndices = min(timeIndices + 1, lastTimestep)
                    safeTimeIndices = min(timeIndices, lastTimestep)
                    emissionsAtThisTimestamp = 0
                    lastEmissionTimestamp = -1
                }
            }
            activeMask = timeIndices < effectiveLength
        }

        // End-of-audio flush: only the final (short) window of a transcription.
        if isLastChunk {
            var additionalSteps = 0
            var consecutiveBlanks = 0
            var finalTimeIndices = timeIndices
            while additionalSteps < maxSymbolsPerStep && consecutiveBlanks < consecutiveBlankLimit {
                try cancelCheck()
                let variations = [
                    min(finalTimeIndices, frames.count - 1),
                    min(effectiveLength - 1, frames.count - 1),
                    min(max(0, effectiveLength - 2), frames.count - 1),
                ]
                let decision = try runJoint(frames: frames, frame: variations[additionalSteps % variations.count])
                let duration = try mapDuration(decision.duration)
                if decision.token == blankID {
                    consecutiveBlanks += 1
                } else {
                    consecutiveBlanks = 0
                    emissions.append(TdtEmission(
                        id: decision.token,
                        frame: min(finalTimeIndices, effectiveLength - 1),
                        duration: duration,
                        confidence: confidence(decision.probability)
                    ))
                    try runDecoder(token: decision.token)
                }
                finalTimeIndices = min(finalTimeIndices + max(1, duration), effectiveLength)
                additionalSteps += 1
            }
        }
        return emissions
    }
}
