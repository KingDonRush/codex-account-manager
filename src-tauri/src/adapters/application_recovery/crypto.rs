use crate::error::AppError;
use ring::{aead, hkdf};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u8,
    salt: String,
    ciphertext: String,
}
struct KeySize;
impl hkdf::KeyType for KeySize {
    fn len(&self) -> usize {
        32
    }
}

fn key(master: &[u8; 32], salt: &[u8; 32], id: &str) -> Result<aead::LessSafeKey, AppError> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(master);
    let context = [
        b"codex-manager/application-recovery/v1".as_slice(),
        id.as_bytes(),
    ];
    let mut derived = Zeroizing::new([0_u8; 32]);
    prk.expand(&context, KeySize)
        .and_then(|okm| okm.fill(derived.as_mut()))
        .map_err(|_| invalid())?;
    let key =
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, derived.as_ref()).map_err(|_| invalid())?;
    Ok(aead::LessSafeKey::new(key))
}
pub(super) fn seal(master: &[u8; 32], id: &str, document: &Value) -> Result<Vec<u8>, AppError> {
    let mut salt = [0_u8; 32];
    getrandom::fill(&mut salt).map_err(|_| invalid())?;
    let mut plaintext = Zeroizing::new(serde_json::to_vec(document)?);
    if plaintext.len() > 128 * 1024 {
        return Err(AppError::Validation(
            "Credencial excede o limite de recuperação.".to_owned(),
        ));
    }
    // A fresh 256-bit HKDF salt produces a one-envelope key; nonce zero is used
    // exactly once for that key. Rewrites never reuse a key/nonce pair.
    key(master, &salt, id)?
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key([0; 12]),
            aead::Aad::from(id.as_bytes()),
            &mut *plaintext,
        )
        .map_err(|_| invalid())?;
    Ok(serde_json::to_vec(&Envelope {
        version: 1,
        salt: hex::encode(salt),
        ciphertext: hex::encode(plaintext.as_slice()),
    })?)
}
pub(super) fn open(master: &[u8; 32], id: &str, bytes: &[u8]) -> Result<Value, AppError> {
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if envelope.version != 1 {
        return Err(invalid());
    }
    let salt: [u8; 32] = hex::decode(&envelope.salt)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(invalid)?;
    let mut encrypted = Zeroizing::new(hex::decode(envelope.ciphertext).map_err(|_| invalid())?);
    let plaintext = key(master, &salt, id)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key([0; 12]),
            aead::Aad::from(id.as_bytes()),
            &mut encrypted,
        )
        .map_err(|_| invalid())?;
    serde_json::from_slice(plaintext).map_err(|_| invalid())
}
fn invalid() -> AppError {
    AppError::CredentialUnavailable(
        "Não foi possível validar a recuperação protegida desta sessão.".to_owned(),
    )
}
