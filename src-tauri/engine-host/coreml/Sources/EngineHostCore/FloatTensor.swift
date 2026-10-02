@preconcurrency import CoreML
import Foundation

/// Floating-point element types the engine reads and writes. Core ML packages
/// converted with a float16 interface and with a float32 interface (fp16 compute
/// inside) are both accepted, so an interface dtype difference can never break
/// decoding.
enum TensorPrecision: String {
    case float32
    case float16

    init?(_ dataType: MLMultiArrayDataType) {
        switch dataType {
        case .float32: self = .float32
        case .float16: self = .float16
        default: return nil
        }
    }

    var dataType: MLMultiArrayDataType { self == .float32 ? .float32 : .float16 }
}

func dataTypeName(_ dataType: MLMultiArrayDataType) -> String {
    switch dataType {
    case .float16: return "float16"
    case .float32: return "float32"
    case .float64: return "float64"
    case .int32: return "int32"
    default: return "dataType(\(dataType.rawValue))"
    }
}

/// "shape [1, 1024, 188] dtype float16", for error messages.
func describe(_ array: MLMultiArray) -> String {
    "shape \(array.shape.map(\.intValue)) dtype \(dataTypeName(array.dataType))"
}

@inline(__always)
func halfToFloat(_ bits: UInt16) -> Float {
    let sign = UInt32(bits & 0x8000) << 16
    let exponent = UInt32(bits >> 10) & 0x1F
    let mantissa = UInt32(bits & 0x3FF)
    if exponent == 0 {
        if mantissa == 0 { return Float(bitPattern: sign) }
        let magnitude = Float(mantissa) * 0x1p-24  // subnormal half
        return sign == 0 ? magnitude : -magnitude
    }
    if exponent == 31 {
        return Float(bitPattern: sign | 0x7F80_0000 | (mantissa << 13))
    }
    return Float(bitPattern: sign | ((exponent + 112) << 23) | (mantissa << 13))
}

/// Round-to-nearest-even float32 -> float16 bit pattern.
@inline(__always)
func floatToHalf(_ value: Float) -> UInt16 {
    let bits = value.bitPattern
    let sign = UInt16((bits >> 16) & 0x8000)
    let exponent = Int((bits >> 23) & 0xFF)
    let mantissa = bits & 0x7F_FFFF
    if exponent == 0xFF {  // inf / NaN
        return sign | 0x7C00 | (mantissa != 0 ? 0x200 : 0)
    }
    let half = exponent - 127 + 15
    if half >= 31 { return sign | 0x7C00 }  // overflow -> inf
    if half <= 0 {
        if half < -10 { return sign }  // underflow -> signed zero
        let full = mantissa | 0x80_0000
        let shift = UInt32(14 - half)
        var result = full >> shift
        let remainder = full & ((1 << shift) - 1)
        let halfway = UInt32(1) << (shift - 1)
        if remainder > halfway || (remainder == halfway && (result & 1) == 1) { result += 1 }
        return sign | UInt16(result)
    }
    var result = (UInt32(half) << 10) | (mantissa >> 13)
    let remainder = mantissa & 0x1FFF
    if remainder > 0x1000 || (remainder == 0x1000 && (result & 1) == 1) { result += 1 }
    return sign | UInt16(result)  // a carry into the exponent is correct (rounds up / to inf)
}

/// A writable float vector living in a (possibly padded, strided) MLMultiArray.
struct FloatVectorDestination {
    let raw: UnsafeMutableRawPointer
    let precision: TensorPrecision
    let stride: Int

    init(_ array: MLMultiArray, precision: TensorPrecision, axis: Int) {
        raw = array.dataPointer
        self.precision = precision
        stride = array.strides[axis].intValue
    }

    @inline(__always)
    func store(_ index: Int, _ value: Float) {
        switch precision {
        case .float32: raw.assumingMemoryBound(to: Float.self)[index * stride] = value
        case .float16: raw.assumingMemoryBound(to: UInt16.self)[index * stride] = floatToHalf(value)
        }
    }
}

/// Reads element `offset` (in elements, from the array's data pointer).
@inline(__always)
func loadFloat(_ raw: UnsafeRawPointer, _ precision: TensorPrecision, _ offset: Int) -> Float {
    switch precision {
    case .float32: return raw.assumingMemoryBound(to: Float.self)[offset]
    case .float16: return halfToFloat(raw.assumingMemoryBound(to: UInt16.self)[offset])
    }
}

/// Returns `array` unchanged when it already has `precision`; otherwise a
/// densely packed copy converted to it (used between two models whose
/// interface dtypes differ).
func converted(_ array: MLMultiArray, to precision: TensorPrecision, feature: String) throws -> MLMultiArray {
    if array.dataType == precision.dataType { return array }
    guard let source = TensorPrecision(array.dataType) else {
        throw EngineHostError(
            code: "engine",
            message: "\(feature): cannot convert \(describe(array)) to \(precision.rawValue)"
        )
    }
    let shape = array.shape.map(\.intValue)
    let strides = array.strides.map(\.intValue)
    let result = try MLMultiArray(shape: array.shape, dataType: precision.dataType)
    let total = shape.reduce(1, *)
    let raw = UnsafeRawPointer(array.dataPointer)
    let destination = result.dataPointer
    var index = [Int](repeating: 0, count: shape.count)
    for linear in 0..<total {
        var offset = 0
        for axis in 0..<shape.count { offset += index[axis] * strides[axis] }
        let value = loadFloat(raw, source, offset)
        switch precision {
        case .float32: destination.assumingMemoryBound(to: Float.self)[linear] = value
        case .float16: destination.assumingMemoryBound(to: UInt16.self)[linear] = floatToHalf(value)
        }
        var axis = shape.count - 1
        while axis >= 0 {
            index[axis] += 1
            if index[axis] < shape[axis] { break }
            index[axis] = 0
            axis -= 1
        }
    }
    return result
}

func constraint(_ model: MLModel, input name: String?, output: String?) -> MLMultiArrayConstraint? {
    if let name { return model.modelDescription.inputDescriptionsByName[name]?.multiArrayConstraint }
    if let output { return model.modelDescription.outputDescriptionsByName[output]?.multiArrayConstraint }
    return nil
}

private func floatPrecision(_ model: MLModel, modelName: String, kind: String, name: String, input: Bool) throws -> TensorPrecision {
    let found = input ? constraint(model, input: name, output: nil) : constraint(model, input: nil, output: name)
    guard let found else {
        throw EngineHostError(code: "model_load_failed", message: "\(modelName) \(kind) '\(name)' is missing or is not a multi-array")
    }
    guard let precision = TensorPrecision(found.dataType) else {
        throw EngineHostError(
            code: "model_load_failed",
            message: "\(modelName) \(kind) '\(name)' has shape \(found.shape.map(\.intValue)) dtype \(dataTypeName(found.dataType)); expected float16 or float32"
        )
    }
    return precision
}

func floatPrecision(of model: MLModel, input name: String, model modelName: String) throws -> TensorPrecision {
    try floatPrecision(model, modelName: modelName, kind: "input", name: name, input: true)
}

func floatPrecision(of model: MLModel, output name: String, model modelName: String) throws -> TensorPrecision {
    try floatPrecision(model, modelName: modelName, kind: "output", name: name, input: false)
}

func requireInt32(of model: MLModel, output name: String, model modelName: String) throws {
    guard let found = constraint(model, input: nil, output: name), found.dataType == .int32 else {
        throw EngineHostError(code: "model_load_failed", message: "\(modelName) output '\(name)' is missing or is not int32")
    }
}
