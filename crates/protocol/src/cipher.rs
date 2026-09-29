//! AES-128/CFB8 stream cipher for online-mode transport (P19-04, ADR-0008).
//!
//! Vanilla encrypts everything past the login handshake with AES in CFB8
//! mode (`AES/CFB8/NoPadding`), key and IV both the 16-byte shared secret.
//! The cipher state lives outside [`crate::framing::FrameCodec`] on purpose:
//! the codec stays a pure function of bytes in and packets out, and the
//! connection decrypts socket bytes before `feed` and encrypts encoded
//! frames before the write — one choke point each way, no codec-signature
//! churn.
//!
//! Why the mode is hand-rolled on top of an audited block cipher instead of
//! calling the `cfb8` crate: that crate's streaming API consumes the cipher
//! per call (`AsyncStreamCipher::encrypt(mut self, …)`), which is one-shot
//! by construction — a connection that must keep one stream alive across
//! thousands of packets cannot use it. The AES primitive stays `aes`
//! (`RustCrypto`, audited); the mode itself is a 16-byte shift register plus XOR,
//! reviewed below and pinned three ways in tests: the crate's published
//! known-answer vector, agreement with a one-shot `cfb8` call from the same
//! state, and byte-at-a-time stream continuity.
//!
//! Hazmat note: CFB8 gives no integrity. A bit-flip decrypts to garbage
//! that the frame layer then refuses; there is no MAC to check, exactly
//! like Vanilla.

use aes::Aes128;
use aes::cipher::{Block, BlockEncrypt, KeyInit};

/// One direction-pair of the login stream cipher.
///
/// Both shift registers start at the secret (Vanilla uses it as key and
/// IV alike) and advance independently per direction from then on.
pub struct PacketCipher {
    cipher: Aes128,
    enc_shift: [u8; 16],
    dec_shift: [u8; 16],
}

impl PacketCipher {
    /// Build both directions from the shared secret (key and IV alike).
    #[must_use]
    pub fn new(secret: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(&Block::<Aes128>::clone_from_slice(secret)),
            enc_shift: *secret,
            dec_shift: *secret,
        }
    }

    /// Encrypt bytes in place before the write.
    pub fn encrypt_bytes(&mut self, bytes: &mut [u8]) {
        for byte in bytes.iter_mut() {
            let mut block: Block<Aes128> = Block::<Aes128>::clone_from_slice(&self.enc_shift);
            self.cipher.encrypt_block(&mut block);
            let keystream: [u8; 16] = block.into();
            let cipher_byte = keystream[0] ^ *byte;
            self.enc_shift.rotate_left(1);
            self.enc_shift[15] = cipher_byte;
            *byte = cipher_byte;
        }
    }

    /// Decrypt bytes in place after the read, before `feed`.
    pub fn decrypt_bytes(&mut self, bytes: &mut [u8]) {
        for byte in bytes.iter_mut() {
            let mut block: Block<Aes128> = Block::<Aes128>::clone_from_slice(&self.dec_shift);
            self.cipher.encrypt_block(&mut block);
            let keystream: [u8; 16] = block.into();
            let plain_byte = keystream[0] ^ *byte;
            self.dec_shift.rotate_left(1);
            // Feedback is the received cipher byte, not the output: a
            // corrupt byte poisons only its own position's successor state
            // the same way on both ends, which is what keeps a lossy
            // stream in sync rather than diverging it.
            self.dec_shift[15] = *byte;
            *byte = plain_byte;
        }
    }
}

impl std::fmt::Debug for PacketCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PacketCipher(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::PacketCipher;

    /// Decode an even-length lowercase hex string (test helper; panics on
    /// programmer error, never on input — vectors are literals below).
    fn unhex(hex: &str) -> Vec<u8> {
        assert!(hex.len().is_multiple_of(2));
        hex.as_bytes()
            .chunks(2)
            .map(|pair| {
                let hi = (pair[0] as char).to_digit(16).expect("hex") as u8;
                let lo = (pair[1] as char).to_digit(16).expect("hex") as u8;
                hi << 4 | lo
            })
            .collect()
    }

    /// The `cfb8` doctest vector through the `cfb8` crate itself, one-shot:
    /// key `0x42`*16, IV `0x24`*16, `"hello world! this is my plaintext."`
    /// must come out as the published `33b356ce…` bytes. This pins the
    /// oracle plumbing (hex transcription included — a slip fails by
    /// length first, then by bytes).
    #[test]
    fn oracle_vector_replays() {
        use aes::cipher::{AsyncStreamCipher, KeyIvInit};
        let key = [0x42u8; 16];
        let iv = [0x24u8; 16];
        let expected =
            unhex("33b356ce9184290c4c8facc1c0b1f918d5475aeb75b88c161ca65bdf05c7137ff4b0");
        let mut buf = *b"hello world! this is my plaintext.";
        assert_eq!(expected.len(), buf.len());
        cfb8::Encryptor::<aes::Aes128>::new(&key.into(), &iv.into()).encrypt(&mut buf);
        assert_eq!(buf.to_vec(), expected);
    }

    /// Our shift register agrees with the audited implementation from the
    /// same state (key K, IV K — what `PacketCipher::new` builds).
    #[test]
    fn wiring_matches_cfb8() {
        use aes::cipher::{AsyncStreamCipher, KeyIvInit};
        let key = [0x42u8; 16];
        let plaintext = *b"hello world! this is my plaintext.";
        let mut expected = plaintext;
        cfb8::Encryptor::<aes::Aes128>::new(&key.into(), &key.into()).encrypt(&mut expected);
        let mut cipher = PacketCipher::new(&key);
        let mut buf = plaintext;
        cipher.encrypt_bytes(&mut buf);
        assert_eq!(buf, expected);
    }

    /// Stream continuity across odd TCP segmentations: encrypt once, feed
    /// the ciphertext byte-by-byte through decrypt, and every byte matches.
    /// A state bug (IV reuse, feedback of the wrong half) fails within the
    /// first block.
    #[test]
    fn chunked_stream_survives_segmentation() {
        let mut enc = PacketCipher::new(&[7u8; 16]);
        let mut dec = PacketCipher::new(&[7u8; 16]);
        let plain: Vec<u8> = (0..300).map(|i| (i * 37 % 251) as u8).collect();
        let mut cipher_text = plain.clone();
        enc.encrypt_bytes(&mut cipher_text);
        assert_ne!(cipher_text, plain, "encryption must change the bytes");
        for (i, byte) in cipher_text.iter().enumerate() {
            let mut one = [*byte];
            dec.decrypt_bytes(&mut one);
            assert_eq!(one[0], plain[i], "byte {i} survives one-byte reads");
        }
        // ...and the tail of one encrypt call decrypts as the head of the
        // next: state carries across calls, not just within them.
        let mut more = *b"second message";
        enc.encrypt_bytes(&mut more);
        dec.decrypt_bytes(&mut more);
        assert_eq!(&more, b"second message");
    }
}
