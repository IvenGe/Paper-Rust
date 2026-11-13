//! AES-128 in CFB8 mode, the stream cipher Minecraft uses once login encryption is enabled.
//!
//! Both directions use the shared secret as key *and* as initialisation vector. CFB8 feeds
//! one byte of ciphertext back into the shift register per byte of data, so it needs one AES
//! block operation per byte and has no padding.

use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use aes::Aes128;

/// One direction of an encrypted connection.
pub struct Cfb8 {
    cipher: Aes128,
    register: [u8; 16],
}

impl Cfb8 {
    /// Creates a cipher with `secret` as both key and IV.
    pub fn new(secret: &[u8; 16]) -> Self {
        Self { cipher: Aes128::new(GenericArray::from_slice(secret)), register: *secret }
    }

    #[inline]
    fn keystream_byte(&self) -> u8 {
        let mut block = GenericArray::clone_from_slice(&self.register);
        self.cipher.encrypt_block(&mut block);
        block[0]
    }

    #[inline]
    fn feed(&mut self, ciphertext: u8) {
        self.register.copy_within(1.., 0);
        self.register[15] = ciphertext;
    }

    /// Encrypts `data` in place.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data {
            let c = *byte ^ self.keystream_byte();
            self.feed(c);
            *byte = c;
        }
    }

    /// Decrypts `data` in place.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data {
            let c = *byte;
            *byte = c ^ self.keystream_byte();
            self.feed(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// NIST SP 800-38A, F.3.7 "CFB8-AES128.Encrypt".
    #[test]
    fn nist_cfb8_aes128() {
        let key: [u8; 16] = hex("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap();
        let iv: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let plain = hex("6bc1bee22e409f96e93d7e117393172aae2d");
        let expected = hex("3b79424c9c0dd436bace9e0ed4586a4f32b9");

        let mut enc = Cfb8 { cipher: Aes128::new(GenericArray::from_slice(&key)), register: iv };
        let mut data = plain.clone();
        enc.encrypt(&mut data);
        assert_eq!(data, expected);

        let mut dec = Cfb8 { cipher: Aes128::new(GenericArray::from_slice(&key)), register: iv };
        dec.decrypt(&mut data);
        assert_eq!(data, plain);
    }

    #[test]
    fn streaming_matches_one_shot() {
        let secret = [7u8; 16];
        let message: Vec<u8> = (0..=255u8).cycle().take(1000).collect();

        let mut one_shot = message.clone();
        Cfb8::new(&secret).encrypt(&mut one_shot);

        let mut streamed = message.clone();
        let mut enc = Cfb8::new(&secret);
        for chunk in streamed.chunks_mut(7) {
            enc.encrypt(chunk);
        }
        assert_eq!(one_shot, streamed);

        let mut dec = Cfb8::new(&secret);
        for chunk in streamed.chunks_mut(13) {
            dec.decrypt(chunk);
        }
        assert_eq!(streamed, message);
    }
}
