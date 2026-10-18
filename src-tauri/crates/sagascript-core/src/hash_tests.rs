use super::*;

const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const TWO_BLOCKS: &str = "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1";
const TWO_BLOCKS_INPUT: &str = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";

#[test]
fn the_standard_test_vectors() {
    assert_eq!(sha256_hex(""), EMPTY);
    assert_eq!(sha256_hex("abc"), ABC);
    assert_eq!(sha256_hex(TWO_BLOCKS_INPUT), TWO_BLOCKS);
}

#[test]
fn any_byte_container_hashes_alike() {
    assert_eq!(sha256_hex(b"abc"), ABC);
    assert_eq!(sha256_hex(b"abc".as_slice()), ABC);
    let owned: Vec<u8> = vec![b'a', b'b', b'c'];
    assert_eq!(sha256_hex(owned), ABC);
    assert_eq!(sha256_hex(String::from("abc")), ABC);
}

/// A digest that starts with a zero byte is still 64 digits: the leading zeros
/// are not dropped, the way a number printed in hexadecimal would.
#[test]
fn leading_zeros_are_kept() {
    let found = (0u32..10_000)
        .map(|n| sha256_hex(n.to_le_bytes()))
        .find(|digest| digest.starts_with("00"))
        .expect("about one input in 256 starts with a zero byte");
    assert_eq!(found.len(), 64);
}

#[test]
fn the_digest_is_lowercase_hex() {
    for n in 0u32..200 {
        let digest = sha256_hex(n.to_le_bytes());
        assert_eq!(digest.len(), 64);
        assert!(digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    }
}

/// The spelling the helpers of the workspace used before (`{byte:02x}` per byte)
/// is the one of `{:x}`.
#[test]
fn it_spells_the_digest_as_the_per_byte_form_did() {
    for n in 0u32..500 {
        let bytes = n.to_le_bytes();
        let per_byte: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(sha256_hex(bytes), per_byte);
    }
}

#[test]
fn streaming_equals_whole_for_any_pieces() {
    let data: Vec<u8> = (0..1_000u32).map(|n| (n % 251) as u8).collect();
    let whole = sha256_hex(&data);
    for piece in [1, 2, 7, 63, 64, 65, 1_000, 5_000] {
        let mut stream = StreamingSha256::new();
        for chunk in data.chunks(piece) {
            stream.update(chunk);
        }
        assert_eq!(stream.finish_hex(), whole, "pieces of {piece}");
    }
}

#[test]
fn an_empty_stream_is_the_digest_of_nothing() {
    assert_eq!(StreamingSha256::new().finish_hex(), EMPTY);
    let mut stream = StreamingSha256::new();
    stream.update([]);
    assert_eq!(stream.finish_hex(), EMPTY);
}

#[test]
fn a_stream_takes_arrays_and_strings_like_the_hash_it_replaces() {
    let mut stream = StreamingSha256::new();
    stream.update(b"sagascript-pcm-f32le-16000-v1\0");
    stream.update(1.5f32.to_le_bytes());
    stream.update(String::from("tail"));
    let mut expected = b"sagascript-pcm-f32le-16000-v1\0".to_vec();
    expected.extend(1.5f32.to_le_bytes());
    expected.extend(b"tail");
    assert_eq!(stream.finish_hex(), sha256_hex(expected));
}

/// The one-line oracle of the tests in other crates
/// (`sagascript_testsupport::sha256_hex`, which cannot depend on core) agrees.
#[test]
fn the_test_oracle_agrees() {
    for input in ["", "abc", TWO_BLOCKS_INPUT] {
        assert_eq!(sha256_hex(input), sagascript_testsupport::sha256_hex(input));
    }
}
