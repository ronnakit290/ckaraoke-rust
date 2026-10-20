//! Primitives for the `.ckr` pack format: PBKDF2-HMAC-SHA256 key derivation, HMAC-SHA256 tags, and the
//! ChaCha20 stream cipher (RFC 8439). The C++ build writes these by hand so it needs nothing beyond zlib;
//! the Rust port uses the RustCrypto crates, which implement the same standards.

use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use chacha20::ChaCha20;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::Zeroize;

use crate::{Error, Result};

pub type Digest = [u8; 32];

/// Derives the cipher key and the MAC key from a passphrase and the file's salt.
pub fn derive_keys(passphrase: &str, salt: &[u8; 16], iterations: u32) -> ([u8; 32], [u8; 32]) {
    let mut out = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, iterations, &mut out);
    let mut enc = [0u8; 32];
    let mut mac = [0u8; 32];
    enc.copy_from_slice(&out[..32]);
    mac.copy_from_slice(&out[32..]);
    out.zeroize();
    (enc, mac)
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn hmac(key: &[u8; 32], parts: &[&[u8]]) -> Digest {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    for part in parts {
        m.update(part);
    }
    m.finalize().into_bytes().into()
}

/// Checks an HMAC in constant time.
pub fn hmac_verify(key: &[u8; 32], parts: &[&[u8]], tag: &Digest) -> bool {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    for part in parts {
        m.update(part);
    }
    m.verify_slice(tag).is_ok()
}

/// Tag of one song record: HMAC over the record's file offset (8 bytes, little-endian) and its ciphertext.
pub fn record_tag(mac_key: &[u8; 32], offset: u64, ciphertext: &[u8]) -> Digest {
    hmac(mac_key, &[&offset.to_le_bytes(), ciphertext])
}

/// XORs `data` with the ChaCha20 keystream for `key`/`nonce`, starting at byte `pos` of that keystream.
/// Applying it twice with the same arguments undoes it. Fails if the block counter would pass 2^32.
pub fn chacha_xor(key: &[u8; 32], nonce: &[u8; 12], pos: u64, data: &mut [u8]) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let last = pos
        .checked_add(data.len() as u64 - 1)
        .ok_or_else(|| Error::new("ไฟล์ .ckr ใหญ่เกินไป"))?;
    if last / 64 > u64::from(u32::MAX) {
        return Err(Error::new("ไฟล์ .ckr ใหญ่เกินไป"));
    }
    let mut cipher = ChaCha20::new(key.into(), nonce.into());
    cipher.seek(pos);
    cipher.apply_keystream(data);
    Ok(())
}

/// Fills `out` with random bytes from the operating system (for salts and nonces).
pub fn random_bytes(out: &mut [u8]) -> Result<()> {
    getrandom::getrandom(out).map_err(|e| Error::new(format!("สร้างเลขสุ่มไม่ได้: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chacha_xor_is_its_own_inverse_and_seekable() {
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        let plain: Vec<u8> = (0..300u32).map(|i| i as u8).collect();

        let mut whole = plain.clone();
        chacha_xor(&key, &nonce, 0, &mut whole).unwrap();
        assert_ne!(whole, plain);

        // Starting in the middle of a block must match the same bytes of the whole stream.
        let mut tail = plain[100..].to_vec();
        chacha_xor(&key, &nonce, 100, &mut tail).unwrap();
        assert_eq!(tail, whole[100..]);

        chacha_xor(&key, &nonce, 0, &mut whole).unwrap();
        assert_eq!(whole, plain);
    }

    #[test]
    fn hmac_verify_detects_changes() {
        let key = [1u8; 32];
        let tag = hmac(&key, &[b"header", b"body"]);
        assert!(hmac_verify(&key, &[b"header", b"body"], &tag));
        assert!(!hmac_verify(&key, &[b"header", b"bodY"], &tag));
    }
}
