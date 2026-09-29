import Foundation

/// Small Foundation-only SHA-256 implementation used to key compiled model caches.
internal struct SHA256 {
    private var state: [UInt32] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ]
    private var buffer: [UInt8] = []
    private var bitCount: UInt64 = 0

    private static let roundConstants: [UInt32] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
        0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
        0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ]

    mutating func update(_ data: Data) {
        bitCount += UInt64(data.count) * 8
        buffer.append(contentsOf: data)
        while buffer.count >= 64 {
            process(Array(buffer.prefix(64)))
            buffer.removeFirst(64)
        }
    }

    mutating func finalize() -> String {
        var tail = buffer
        tail.append(0x80)
        while tail.count % 64 != 56 { tail.append(0) }
        tail.append(contentsOf: withUnsafeBytes(of: bitCount.bigEndian, Array.init))
        while !tail.isEmpty {
            process(Array(tail.prefix(64)))
            tail.removeFirst(64)
        }
        return state.map { String(format: "%08x", $0) }.joined()
    }

    private mutating func process(_ chunk: [UInt8]) {
        var words = [UInt32](repeating: 0, count: 64)
        for index in 0..<16 {
            let base = index * 4
            words[index] = UInt32(chunk[base]) << 24
                | UInt32(chunk[base + 1]) << 16
                | UInt32(chunk[base + 2]) << 8
                | UInt32(chunk[base + 3])
        }
        for index in 16..<64 {
            let x = words[index - 15]
            let y = words[index - 2]
            let s0 = x.rotateRight(7) ^ x.rotateRight(18) ^ (x >> 3)
            let s1 = y.rotateRight(17) ^ y.rotateRight(19) ^ (y >> 10)
            words[index] = words[index - 16] &+ s0 &+ words[index - 7] &+ s1
        }

        var a = state[0], b = state[1], c = state[2], d = state[3]
        var e = state[4], f = state[5], g = state[6], h = state[7]
        for index in 0..<64 {
            let s1 = e.rotateRight(6) ^ e.rotateRight(11) ^ e.rotateRight(25)
            let choose = (e & f) ^ ((~e) & g)
            let temp1 = h &+ s1 &+ choose &+ Self.roundConstants[index] &+ words[index]
            let s0 = a.rotateRight(2) ^ a.rotateRight(13) ^ a.rotateRight(22)
            let majority = (a & b) ^ (a & c) ^ (b & c)
            let temp2 = s0 &+ majority
            h = g; g = f; f = e; e = d &+ temp1
            d = c; c = b; b = a; a = temp1 &+ temp2
        }
        state[0] &+= a; state[1] &+= b; state[2] &+= c; state[3] &+= d
        state[4] &+= e; state[5] &+= f; state[6] &+= g; state[7] &+= h
    }
}

private extension UInt32 {
    func rotateRight(_ amount: UInt32) -> UInt32 {
        (self >> amount) | (self << (32 - amount))
    }
}

internal func sha256File(_ url: URL) throws -> String {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    var digest = SHA256()
    while let chunk = try handle.read(upToCount: 1024 * 1024), !chunk.isEmpty {
        digest.update(chunk)
    }
    return digest.finalize()
}

internal func sha256Data(_ data: Data) -> String {
    var digest = SHA256()
    digest.update(data)
    return digest.finalize()
}
