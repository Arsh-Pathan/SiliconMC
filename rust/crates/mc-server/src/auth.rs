use anyhow::{Context, Result, bail};
use mc_proto::{GameProfile, ProfileProperty, minecraft_server_hash};
use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey};
use serde::Deserialize;
use uuid::Uuid;

pub struct ServerKeypair {
    private_key: RsaPrivateKey,
    public_key_der: Vec<u8>,
}

impl ServerKeypair {
    pub fn generate() -> Result<Self> {
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 1024).context("RSA-1024 keygen failed")?;
        let public_key_der = private_key
            .to_public_key()
            .to_public_key_der()
            .context("Failed to encode SPKI DER")?
            .as_bytes()
            .to_vec();
        Ok(Self {
            private_key,
            public_key_der,
        })
    }

    pub fn public_key_der(&self) -> &[u8] {
        &self.public_key_der
    }

    pub fn decrypt_pkcs1v15(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        self.private_key
            .decrypt(Pkcs1v15Encrypt, ciphertext)
            .context("RSA PKCS1v15 decryption failed")
    }
}

#[derive(Deserialize)]
struct MojangHasJoinedResponse {
    id: String,
    name: String,
    #[serde(default)]
    properties: Vec<ProfileProperty>,
}

/// Verify an online-mode player against Mojang's official session server.
pub async fn verify_mojang_session(
    username: &str,
    server_id: &str,
    shared_secret: &[u8; 16],
    public_key_der: &[u8],
) -> Result<GameProfile> {
    let hash = minecraft_server_hash(server_id, shared_secret, public_key_der);
    let url = format!(
        "https://sessionserver.mojang.com/session/minecraft/hasJoined?username={username}&serverId={hash}"
    );
    let resp = reqwest::get(&url)
        .await
        .context("Failed contacting Mojang session server")?;
    if !resp.status().is_success() {
        bail!(
            "Mojang session verification failed for {username}: HTTP {}",
            resp.status()
        );
    }
    let parsed: MojangHasJoinedResponse = resp
        .json()
        .await
        .context("Failed parsing Mojang session JSON")?;
    let uuid = Uuid::parse_str(&parsed.id).context("Invalid UUID from Mojang session server")?;
    Ok(GameProfile {
        uuid,
        name: parsed.name,
        properties: parsed.properties,
    })
}
