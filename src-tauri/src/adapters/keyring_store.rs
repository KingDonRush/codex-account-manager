use super::native_keyring::Entry;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::adapters::auth_store::validate_auth_document;
use crate::error::AppError;
use crate::ports::{CredentialStore, StoredCredentialGeneration};

const SERVICE: &str = "com.codex.account-manager";
const MAX_CHUNK_UTF16_BYTES: usize = 2_400;
const MAX_CHUNKS: usize = 10_000;
const V2_PREFIX: &str = "codex-keyring-v2\n";
const V3_PREFIX: &str = "codex-keyring-v3\n";
const V4_PREFIX: &str = "codex-keyring-v4\n";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct V2Manifest {
    version: u8,
    generation: String,
    chunks: usize,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationManifest {
    generation_id: String,
    chunks: usize,
    sha256: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct V3Manifest {
    version: u8,
    active_generation: String,
    generations: Vec<GenerationManifest>,
}

enum StoredMain {
    Legacy(String),
    V2(V2Manifest),
    V3(V3Manifest),
}

pub struct KeyringStore;
mod sessions;
pub use sessions::ApplicationCredentialStore;

impl CredentialStore for KeyringStore {
    fn account_ids(&self) -> Result<Vec<String>, AppError> {
        let mut ids = super::native_keyring::account_user_names()?
            .iter()
            .filter_map(|user| main_account_id(user).map(str::to_owned))
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
    fn check_read_access(&self) -> Result<(), AppError> {
        super::native_keyring::check_unlocked()
    }

    fn save(&self, document: &Value) -> Result<String, AppError> {
        Ok(self.save_generation(document)?.account_id)
    }

    fn save_generation(&self, document: &Value) -> Result<StoredCredentialGeneration, AppError> {
        store_generation(document, GenerationSelection::Activate)
    }

    fn archive_generation(&self, document: &Value) -> Result<StoredCredentialGeneration, AppError> {
        store_generation(document, GenerationSelection::Preserve)
    }

    fn load(&self, account_id: &str) -> Result<Value, AppError> {
        self.load_optional(account_id)?.ok_or_else(|| {
            AppError::Keyring(format!("credencial ausente para a conta {account_id}"))
        })
    }

    fn load_optional(&self, account_id: &str) -> Result<Option<Value>, AppError> {
        let Some(stored) = read_stored_main(account_id)? else {
            return Ok(None);
        };
        let serialized = match stored {
            StoredMain::Legacy(serialized) => serialized,
            StoredMain::V2(manifest) => read_v2_document(account_id, &manifest)?,
            StoredMain::V3(manifest) => {
                let generation = manifest
                    .generations
                    .iter()
                    .find(|item| item.generation_id == manifest.active_generation)
                    .ok_or_else(|| AppError::Keyring("geração ativa ausente".to_owned()))?;
                read_generation_document(account_id, generation)?
            }
        };
        Ok(Some(parse_document(account_id, &serialized)?))
    }

    fn generation_refs(
        &self,
        account_id: &str,
    ) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        let Some(stored) = read_stored_main(account_id)? else {
            return Ok(Vec::new());
        };
        let generations = match stored {
            StoredMain::Legacy(serialized) => vec![StoredCredentialGeneration {
                account_id: account_id.to_owned(),
                generation_id: "legacy".to_owned(),
                fingerprint: digest(&serialized),
                created_at: Utc::now(),
                is_active: true,
            }],
            StoredMain::V2(manifest) => vec![StoredCredentialGeneration {
                account_id: account_id.to_owned(),
                generation_id: manifest.generation,
                fingerprint: manifest.sha256,
                created_at: Utc::now(),
                is_active: true,
            }],
            StoredMain::V3(manifest) => manifest
                .generations
                .iter()
                .map(|generation| {
                    generation_ref(account_id, generation, &manifest.active_generation)
                })
                .collect(),
        };
        Ok(generations)
    }

    fn load_generation(&self, account_id: &str, generation_id: &str) -> Result<Value, AppError> {
        let stored = read_stored_main(account_id)?.ok_or_else(|| {
            AppError::Keyring(format!("credencial ausente para a conta {account_id}"))
        })?;
        let serialized = match stored {
            StoredMain::Legacy(serialized) if generation_id == "legacy" => serialized,
            StoredMain::V2(manifest) if generation_id == manifest.generation => {
                read_v2_document(account_id, &manifest)?
            }
            StoredMain::V3(manifest) => {
                let generation = manifest
                    .generations
                    .iter()
                    .find(|item| item.generation_id == generation_id)
                    .ok_or_else(|| AppError::Keyring("geração de credencial ausente".to_owned()))?;
                read_generation_document(account_id, generation)?
            }
            _ => {
                return Err(AppError::Keyring(
                    "geração de credencial ausente".to_owned(),
                ))
            }
        };
        parse_document(account_id, &serialized)
    }

    fn activate_generation(&self, account_id: &str, generation_id: &str) -> Result<(), AppError> {
        let Some(stored) = read_stored_main(account_id)? else {
            return Err(AppError::Keyring("credencial ausente".to_owned()));
        };
        match stored {
            StoredMain::V3(mut manifest) => {
                if !manifest
                    .generations
                    .iter()
                    .any(|item| item.generation_id == generation_id)
                {
                    return Err(AppError::Keyring(
                        "geração de credencial ausente".to_owned(),
                    ));
                }
                manifest.active_generation = generation_id.to_owned();
                write_v3_manifest(account_id, &manifest)
            }
            StoredMain::V2(manifest) if manifest.generation == generation_id => Ok(()),
            StoredMain::Legacy(_) if generation_id == "legacy" => Ok(()),
            _ => Err(AppError::Keyring(
                "geração de credencial ausente".to_owned(),
            )),
        }
    }

    fn delete_generation(&self, account_id: &str, generation_id: &str) -> Result<(), AppError> {
        let Some(stored) = read_stored_main(account_id)? else {
            return Ok(());
        };
        match stored {
            StoredMain::Legacy(_) if generation_id == "legacy" => delete_main(account_id),
            StoredMain::V2(manifest) if manifest.generation == generation_id => {
                cleanup_generation(account_id, &manifest.generation, manifest.chunks)?;
                delete_main(account_id)
            }
            StoredMain::V3(mut manifest) => {
                let Some(index) = manifest
                    .generations
                    .iter()
                    .position(|item| item.generation_id == generation_id)
                else {
                    return Ok(());
                };
                let removed = manifest.generations.remove(index);
                cleanup_generation(account_id, &removed.generation_id, removed.chunks)?;
                if manifest.generations.is_empty() {
                    delete_main(account_id)
                } else {
                    if manifest.active_generation == generation_id {
                        manifest.active_generation = manifest
                            .generations
                            .last()
                            .expect("non-empty generations")
                            .generation_id
                            .clone();
                    }
                    write_v3_manifest(account_id, &manifest)
                }
            }
            _ => Ok(()),
        }
    }

    fn delete(&self, account_id: &str) -> Result<(), AppError> {
        let stored = read_stored_main(account_id)?;
        let Some(stored) = stored else {
            return Ok(());
        };
        match stored {
            StoredMain::Legacy(_) => delete_main(account_id),
            StoredMain::V2(manifest) => {
                cleanup_generation(account_id, &manifest.generation, manifest.chunks)?;
                delete_main(account_id)
            }
            StoredMain::V3(manifest) => {
                let mut first_error = None;
                for generation in manifest.generations {
                    if let Err(error) =
                        cleanup_generation(account_id, &generation.generation_id, generation.chunks)
                    {
                        first_error.get_or_insert(error);
                    }
                }
                if let Some(error) = first_error {
                    return Err(error);
                }
                delete_main(account_id)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GenerationSelection {
    Activate,
    Preserve,
}

// Only chunks created by this write are temporary. The existing main entry
// remains authoritative until the manifest commit, including legacy conversion.
struct PendingChunks<'a> {
    account: &'a str,
    generations: Vec<GenerationManifest>,
}

impl Drop for PendingChunks<'_> {
    fn drop(&mut self) {
        for generation in &self.generations {
            let _ = cleanup_generation(self.account, &generation.generation_id, generation.chunks);
        }
    }
}

fn store_generation(
    document: &Value,
    selection: GenerationSelection,
) -> Result<StoredCredentialGeneration, AppError> {
    let account = validate_auth_document(document, None)?;
    let serialized = serde_json::to_string(document)?;
    let stored = read_stored_main(&account)?;
    if selection == GenerationSelection::Preserve && stored.is_none() {
        return Err(AppError::Keyring(
            "não há seleção para preservar".to_owned(),
        ));
    }
    if selection == GenerationSelection::Preserve {
        if let Some(StoredMain::Legacy(value)) = stored.as_ref() {
            parse_document(&account, value)?;
        }
    }
    let normalized = !matches!(stored, Some(StoredMain::V3(_)));
    let legacy = matches!(stored, Some(StoredMain::Legacy(_)));
    let mut manifest = normalize_manifest(&account, stored)?;
    let mut pending = PendingChunks {
        account: &account,
        generations: if legacy {
            manifest.generations.clone()
        } else {
            Vec::new()
        },
    };
    let existing = matching_generation(&account, &manifest, &digest(&serialized))?;
    if let Some(generation) = existing
        .as_ref()
        .filter(|(_, readable)| *readable)
        .map(|(g, _)| g)
    {
        let changed = select_generation(&mut manifest, generation, selection);
        if normalized || changed {
            write_v3_manifest(&account, &manifest)?;
        }
        pending.generations.clear();
        return Ok(generation_ref(
            &account,
            generation,
            &manifest.active_generation,
        ));
    }
    let obsolete = existing.map(|(generation, _)| generation);
    if let Some(generation) = obsolete.as_ref() {
        if selection == GenerationSelection::Preserve
            && manifest.active_generation == generation.generation_id
        {
            return Err(AppError::Keyring(
                "a geração selecionada não está íntegra; seleção preservada".to_owned(),
            ));
        }
        manifest
            .generations
            .retain(|item| item.generation_id != generation.generation_id);
    }
    let generation = write_document_chunks(&account, &serialized)?;
    pending.generations.push(generation.clone());
    select_generation(&mut manifest, &generation, selection);
    manifest.generations.push(generation.clone());
    write_v3_manifest(&account, &manifest)?;
    pending.generations.clear();
    if let Some(previous) = obsolete {
        let _ = cleanup_generation(&account, &previous.generation_id, previous.chunks);
    }
    Ok(generation_ref(
        &account,
        &generation,
        &manifest.active_generation,
    ))
}

fn matching_generation(
    account: &str,
    manifest: &V3Manifest,
    fingerprint: &str,
) -> Result<Option<(GenerationManifest, bool)>, AppError> {
    let Some(generation) = manifest
        .generations
        .iter()
        .find(|g| g.sha256 == fingerprint)
    else {
        return Ok(None);
    };
    let readable = readable_generation(
        read_generation_document(account, generation)
            .and_then(|stored| parse_document(account, &stored)),
    )?;
    Ok(Some((generation.clone(), readable)))
}

fn select_generation(
    manifest: &mut V3Manifest,
    generation: &GenerationManifest,
    selection: GenerationSelection,
) -> bool {
    if selection == GenerationSelection::Preserve
        || manifest.active_generation == generation.generation_id
    {
        return false;
    }
    manifest.active_generation = generation.generation_id.clone();
    true
}

fn readable_generation(result: Result<Value, AppError>) -> Result<bool, AppError> {
    match result {
        Ok(_) => Ok(true),
        Err(error @ AppError::CredentialUnavailable(_)) => Err(error),
        Err(_) => Ok(false),
    }
}

fn normalize_manifest(
    account_id: &str,
    stored: Option<StoredMain>,
) -> Result<V3Manifest, AppError> {
    match stored {
        None => Ok(V3Manifest {
            version: 3,
            active_generation: String::new(),
            generations: Vec::new(),
        }),
        Some(StoredMain::V3(manifest)) => Ok(manifest),
        Some(StoredMain::V2(manifest)) => {
            let generation = GenerationManifest {
                generation_id: manifest.generation.clone(),
                chunks: manifest.chunks,
                sha256: manifest.sha256,
                created_at: Utc::now(),
            };
            Ok(V3Manifest {
                version: 3,
                active_generation: manifest.generation,
                generations: vec![generation],
            })
        }
        Some(StoredMain::Legacy(serialized)) => {
            let generation = write_document_chunks(account_id, &serialized)?;
            Ok(V3Manifest {
                version: 3,
                active_generation: generation.generation_id.clone(),
                generations: vec![generation],
            })
        }
    }
}

fn write_document_chunks(
    account_id: &str,
    serialized: &str,
) -> Result<GenerationManifest, AppError> {
    let chunks = split_for_keyring(serialized)?;
    let generation_id = Uuid::new_v4().simple().to_string();
    for (index, chunk) in chunks.iter().enumerate() {
        let entry = chunk_entry(account_id, &generation_id, index)?;
        if let Err(error) = entry.set_password(chunk) {
            let _ = cleanup_generation(account_id, &generation_id, index);
            return Err(native_error(error));
        }
    }
    Ok(GenerationManifest {
        generation_id,
        chunks: chunks.len(),
        sha256: digest(serialized),
        created_at: Utc::now(),
    })
}

fn generation_ref(
    account_id: &str,
    manifest: &GenerationManifest,
    active: &str,
) -> StoredCredentialGeneration {
    StoredCredentialGeneration {
        account_id: account_id.to_owned(),
        generation_id: manifest.generation_id.clone(),
        fingerprint: manifest.sha256.clone(),
        created_at: manifest.created_at,
        is_active: manifest.generation_id == active,
    }
}

fn main_entry(account_id: &str) -> Result<Entry, AppError> {
    Entry::new(SERVICE, &format!("account:{account_id}")).map_err(native_error)
}

fn chunk_entry(account_id: &str, generation: &str, index: usize) -> Result<Entry, AppError> {
    Entry::new(
        SERVICE,
        &format!("account:{account_id}:chunk:{generation}:{index}"),
    )
    .map_err(native_error)
}

fn read_stored_main(account_id: &str) -> Result<Option<StoredMain>, AppError> {
    let entry = main_entry(account_id)?;
    let value = match entry.get_password() {
        Ok(value) => value,
        Err(keyring_core::Error::NoEntry) => return Ok(None),
        Err(error) => return Err(native_error(error)),
    };
    if let Some(index) = index_pointer(&value)? {
        let encoded = read_v2_document(account_id, &index)?;
        let manifest: V3Manifest = serde_json::from_str(&encoded)
            .map_err(|_| AppError::Keyring("índice de credenciais inválido".to_owned()))?;
        validate_v3_manifest(&manifest)?;
        return Ok(Some(StoredMain::V3(manifest)));
    }
    if let Some(encoded) = value.strip_prefix(V3_PREFIX) {
        let manifest: V3Manifest = serde_json::from_str(encoded)
            .map_err(|_| AppError::Keyring("manifesto v3 inválido".to_owned()))?;
        validate_v3_manifest(&manifest)?;
        return Ok(Some(StoredMain::V3(manifest)));
    }
    if let Some(encoded) = value.strip_prefix(V2_PREFIX) {
        let manifest: V2Manifest = serde_json::from_str(encoded)
            .map_err(|_| AppError::Keyring("manifesto v2 inválido".to_owned()))?;
        validate_generation(&manifest.generation, manifest.chunks, &manifest.sha256)?;
        if manifest.version != 2 {
            return Err(AppError::Keyring("manifesto v2 inconsistente".to_owned()));
        }
        return Ok(Some(StoredMain::V2(manifest)));
    }
    if value.starts_with("codex-keyring-") {
        return Err(AppError::Keyring(
            "formato do cofre não reconhecido por esta versão; conteúdo preservado".to_owned(),
        ));
    }
    Ok(Some(StoredMain::Legacy(value)))
}

fn validate_v3_manifest(manifest: &V3Manifest) -> Result<(), AppError> {
    if manifest.version != 3
        || manifest.generations.is_empty()
        || !manifest
            .generations
            .iter()
            .any(|item| item.generation_id == manifest.active_generation)
    {
        return Err(AppError::Keyring("manifesto v3 inconsistente".to_owned()));
    }
    for generation in &manifest.generations {
        validate_generation(
            &generation.generation_id,
            generation.chunks,
            &generation.sha256,
        )?;
    }
    Ok(())
}

fn validate_generation(id: &str, chunks: usize, sha256: &str) -> Result<(), AppError> {
    if chunks == 0
        || chunks > MAX_CHUNKS
        || Uuid::parse_str(id).is_err()
        || sha256.len() != 64
        || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::Keyring(
            "manifesto de credencial inconsistente".to_owned(),
        ));
    }
    Ok(())
}

fn write_v3_manifest(account_id: &str, manifest: &V3Manifest) -> Result<(), AppError> {
    validate_v3_manifest(manifest)?;
    let entry = main_entry(account_id)?;
    let previous = match entry.get_password() {
        Ok(value) => index_pointer(&value)?,
        Err(keyring_core::Error::NoEntry) => None,
        Err(error) => return Err(native_error(error)),
    };
    let serialized = serde_json::to_string(manifest)?;
    let inline = format!("{V3_PREFIX}{serialized}");
    let index = if fits_native_entry(&inline) {
        None
    } else {
        Some(write_document_chunks(account_id, &serialized)?)
    };
    let encoded = match index.as_ref() {
        None => inline,
        Some(index) => encode_index_pointer(index)?,
    };
    if let Err(error) = entry.set_password(&encoded) {
        if let Some(index) = index {
            let _ = cleanup_generation(account_id, &index.generation_id, index.chunks);
        }
        return Err(native_error(error));
    }
    if let Some(previous) = previous {
        let _ = cleanup_generation(account_id, &previous.generation, previous.chunks);
    }
    Ok(())
}

fn fits_native_entry(value: &str) -> bool {
    value.encode_utf16().count() * 2 <= MAX_CHUNK_UTF16_BYTES
}

fn encode_index_pointer(index: &GenerationManifest) -> Result<String, AppError> {
    Ok(format!(
        "{V4_PREFIX}{}",
        serde_json::to_string(&V2Manifest {
            version: 4,
            generation: index.generation_id.clone(),
            chunks: index.chunks,
            sha256: index.sha256.clone(),
        })?
    ))
}

fn index_pointer(value: &str) -> Result<Option<V2Manifest>, AppError> {
    let Some(encoded) = value.strip_prefix(V4_PREFIX) else {
        return Ok(None);
    };
    let pointer: V2Manifest = serde_json::from_str(encoded)
        .map_err(|_| AppError::Keyring("ponteiro do índice inválido".to_owned()))?;
    validate_generation(&pointer.generation, pointer.chunks, &pointer.sha256)?;
    if pointer.version != 4 {
        return Err(AppError::Keyring("versão do índice inválida".to_owned()));
    }
    Ok(Some(pointer))
}

fn read_v2_document(account_id: &str, manifest: &V2Manifest) -> Result<String, AppError> {
    read_chunks(
        account_id,
        &manifest.generation,
        manifest.chunks,
        &manifest.sha256,
    )
}

fn read_generation_document(
    account_id: &str,
    manifest: &GenerationManifest,
) -> Result<String, AppError> {
    read_chunks(
        account_id,
        &manifest.generation_id,
        manifest.chunks,
        &manifest.sha256,
    )
}

fn read_chunks(
    account_id: &str,
    generation_id: &str,
    chunks: usize,
    expected_digest: &str,
) -> Result<String, AppError> {
    let mut serialized = String::new();
    for index in 0..chunks {
        let entry = chunk_entry(account_id, generation_id, index)?;
        let chunk = entry.get_password().map_err(|error| match error {
            keyring_core::Error::NoEntry => {
                AppError::Keyring("parte de credencial ausente".to_owned())
            }
            other => native_error(other),
        })?;
        serialized.push_str(&chunk);
    }
    if digest(&serialized) != expected_digest {
        return Err(AppError::Keyring(
            "checksum da credencial não confere".to_owned(),
        ));
    }
    Ok(serialized)
}

fn parse_document(account_id: &str, serialized: &str) -> Result<Value, AppError> {
    let document = serde_json::from_str(serialized)?;
    validate_auth_document(&document, Some(account_id))?;
    Ok(document)
}

fn delete_main(account_id: &str) -> Result<(), AppError> {
    let entry = main_entry(account_id)?;
    let index = match entry.get_password() {
        Ok(value) => index_pointer(&value)?,
        Err(keyring_core::Error::NoEntry) => return Ok(()),
        Err(error) => return Err(native_error(error)),
    };
    match entry.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(error) => Err(native_error(error)),
    }?;
    if let Some(index) = index {
        cleanup_generation(account_id, &index.generation, index.chunks)?;
    }
    Ok(())
}

fn split_for_keyring(serialized: &str) -> Result<Vec<String>, AppError> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_size = 0;
    for character in serialized.chars() {
        let character_size = character.len_utf16() * 2;
        if current_size + character_size > MAX_CHUNK_UTF16_BYTES && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_size = 0;
        }
        current.push(character);
        current_size += character_size;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() || chunks.len() > MAX_CHUNKS {
        return Err(AppError::Keyring(
            "a credencial excede o tamanho suportado pelo armazenamento nativo".to_owned(),
        ));
    }
    Ok(chunks)
}

fn cleanup_generation(account_id: &str, generation: &str, chunks: usize) -> Result<(), AppError> {
    let mut first_error = None;
    for index in 0..chunks {
        let entry = match chunk_entry(account_id, generation, index) {
            Ok(entry) => entry,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        if let Err(error) = entry.delete_credential() {
            if !matches!(error, keyring_core::Error::NoEntry) {
                first_error.get_or_insert_with(|| native_error(error));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn digest(serialized: &str) -> String {
    hex::encode(Sha256::digest(serialized.as_bytes()))
}

fn native_error(error: keyring_core::Error) -> AppError {
    match error {
        keyring_core::Error::PlatformFailure(_)
        | keyring_core::Error::NoStorageAccess(_)
        | keyring_core::Error::NoDefaultStore => AppError::CredentialUnavailable(
            "Não foi possível acessar o cofre do sistema. Tente novamente ou desbloqueie-o; suas contas continuam salvas.".to_owned(),
        ),
        other => AppError::Keyring(other.to_string()),
    }
}

fn main_account_id(user: &str) -> Option<&str> {
    let id = user.strip_prefix("account:")?;
    (!id.is_empty() && id.len() <= 256 && !id.contains(':') && !id.chars().any(char::is_control))
        .then_some(id)
}

#[cfg(test)]
mod tests;
