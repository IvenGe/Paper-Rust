//! Player identity: offline UUIDs, the login encryption handshake and Mojang session
//! verification for online mode.

use anyhow::{anyhow, bail, Context, Result};
use md5::{Digest as _, Md5};
use paper_protocol::types::{GameProfile, Property};
use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::Sha1;
use std::time::Duration;
use uuid::Uuid;

/// The UUID vanilla assigns in offline mode: Java's `UUID.nameUUIDFromBytes` (a version 3
/// UUID over the raw bytes, without a namespace) of `OfflinePlayer:<name>`.
pub fn offline_uuid(name: &str) -> Uuid {
    let mut hash: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    hash[6] = (hash[6] & 0x0F) | 0x30;
    hash[8] = (hash[8] & 0x3F) | 0x80;
    Uuid::from_bytes(hash)
}

/// Vanilla's player name rule: 1-16 characters, none of them a control character, space
/// or outside ASCII.
pub fn is_valid_player_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 16 && name.bytes().all(|b| b > 32 && b < 127)
}

/// The server's RSA key pair used to exchange the AES secret during login.
pub struct ServerKeys {
    private: RsaPrivateKey,
    /// X.509 `SubjectPublicKeyInfo` encoding sent to clients.
    pub public_der: Vec<u8>,
}

impl ServerKeys {
    /// Generates the 1024-bit key pair the protocol mandates.
    pub fn generate() -> Result<Self> {
        let mut rng = rsa::rand_core::OsRng;
        let private = RsaPrivateKey::new(&mut rng, 1024).context("generating the server RSA key")?;
        let public_der = RsaPublicKey::from(&private).to_public_key_der().context("encoding the server public key")?.as_bytes().to_vec();
        Ok(Self { private, public_der })
    }

    /// Decrypts a PKCS#1 v1.5 block sent by a client.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        self.private.decrypt(Pkcs1v15Encrypt, data).map_err(|e| anyhow!("RSA decryption failed: {e}"))
    }
}

/// The "server hash" both sides send to the session server: SHA-1 over the server id, the
/// shared secret and the public key, printed like Java's `new BigInteger(digest).toString(16)`
/// (two's complement, so it may be negative).
pub fn server_hash(server_id: &str, shared_secret: &[u8], public_key_der: &[u8]) -> String {
    let mut sha = Sha1::new();
    sha.update(server_id.as_bytes());
    sha.update(shared_secret);
    sha.update(public_key_der);
    java_hex_digest(&sha.finalize())
}

fn java_hex_digest(digest: &[u8]) -> String {
    num_bigint::BigInt::from_signed_bytes_be(digest).to_str_radix(16)
}

/// Asks Mojang's session server whether `username` has joined a server with this hash.
/// Returns the authenticated profile, or `None` if the session server says no.
///
/// This is a blocking HTTP request; call it from a blocking task.
pub fn has_joined(username: &str, hash: &str, client_ip: Option<&str>) -> Result<Option<GameProfile>> {
    let mut url = format!("https://sessionserver.mojang.com/session/minecraft/hasJoined?username={}&serverId={}", url_encode(username), url_encode(hash));
    if let Some(ip) = client_ip {
        url.push_str(&format!("&ip={}", url_encode(ip)));
    }
    let response = match ureq::get(&url).timeout(Duration::from_secs(15)).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(code, _)) => bail!("session server answered HTTP {code}"),
        Err(e) => bail!("session server unreachable: {e}"),
    };
    if response.status() == 204 {
        return Ok(None);
    }
    let body: serde_json::Value = response.into_json().context("reading the session server response")?;
    parse_profile(&body).map(Some)
}

fn parse_profile(body: &serde_json::Value) -> Result<GameProfile> {
    let id = body["id"].as_str().ok_or_else(|| anyhow!("session response has no id"))?;
    let name = body["name"].as_str().ok_or_else(|| anyhow!("session response has no name"))?;
    let uuid = Uuid::parse_str(id).context("session response id is not a UUID")?;
    let properties = body["properties"]
        .as_array()
        .map(|props| {
            props
                .iter()
                .filter_map(|p| {
                    Some(Property { name: p["name"].as_str()?.to_owned(), value: p["value"].as_str()?.to_owned(), signature: p["signature"].as_str().map(str::to_owned) })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(GameProfile { uuid, name: name.to_owned(), properties })
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuids_match_vanilla() {
        // Well-known values produced by UUID.nameUUIDFromBytes("OfflinePlayer:<name>").
        assert_eq!(offline_uuid("Notch").to_string(), "b50ad385-829d-3141-a216-7e7d7539ba7f");
        assert_eq!(offline_uuid("jeb_").to_string(), "a762f560-4fce-3236-812a-b80efff0b62b");
        assert_eq!(offline_uuid("Notch").get_version_num(), 3);
    }

    #[test]
    fn hex_digest_matches_documented_vectors() {
        // Test vectors from the protocol encryption documentation.
        let digest = |s: &str| java_hex_digest(&Sha1::digest(s.as_bytes()));
        assert_eq!(digest("Notch"), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
        assert_eq!(digest("jeb_"), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
        assert_eq!(digest("simon"), "88e16a1019277b15d58faf0541e11910eb756f6");
    }

    #[test]
    fn name_validation() {
        assert!(is_valid_player_name("Steve"));
        assert!(is_valid_player_name("a_16_char_name__"));
        assert!(!is_valid_player_name(""));
        assert!(!is_valid_player_name("seventeen_chars__"));
        assert!(!is_valid_player_name("has space"));
        assert!(!is_valid_player_name("ünïcode"));
    }

    #[test]
    fn key_exchange_round_trip() {
        let keys = ServerKeys::generate().unwrap();
        // What a client does: parse the DER public key and encrypt the secret with PKCS#1 v1.5.
        use rsa::pkcs8::DecodePublicKey;
        let public = RsaPublicKey::from_public_key_der(&keys.public_der).unwrap();
        let secret = [7u8; 16];
        let encrypted = public.encrypt(&mut rsa::rand_core::OsRng, Pkcs1v15Encrypt, &secret).unwrap();
        assert_eq!(encrypted.len(), 128);
        assert_eq!(keys.decrypt(&encrypted).unwrap(), secret);
        assert!(keys.decrypt(&[0u8; 128]).is_err());
    }

    #[test]
    fn parses_session_profile() {
        let body = serde_json::json!({
            "id": "069a79f444e94726a5befca90e38aaf5",
            "name": "Notch",
            "properties": [{ "name": "textures", "value": "abc", "signature": "sig" }]
        });
        let profile = parse_profile(&body).unwrap();
        assert_eq!(profile.uuid.to_string(), "069a79f4-44e9-4726-a5be-fca90e38aaf5");
        assert_eq!(profile.name, "Notch");
        assert_eq!(profile.properties[0].signature.as_deref(), Some("sig"));
        assert!(parse_profile(&serde_json::json!({ "name": "x" })).is_err());
    }

    #[test]
    fn url_encoding() {
        assert_eq!(url_encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(url_encode("-7c9d"), "-7c9d");
    }
}
