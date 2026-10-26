use super::*;

/// Separate keyring namespace; never overwrite the Desktop's refresh chain.
pub struct ApplicationCredentialStore;
impl crate::ports::ApplicationSessionStore for ApplicationCredentialStore {
    fn save(&self, id: &str, document: &Value) -> Result<(), AppError> {
        Self::save(id, document)
    }
    fn load(&self, id: &str, expected_account: &str) -> Result<Value, AppError> {
        Self::load(id, expected_account)
    }
    fn delete(&self, id: &str) -> Result<(), AppError> {
        KeyringStore.delete(&namespace(id)?)
    }
}

impl ApplicationCredentialStore {
    pub fn save(id: &str, document: &Value) -> Result<(), AppError> {
        validate_auth_document(document, None)?;
        let namespace = namespace(id)?;
        let serialized = serde_json::to_string(document)?;
        let previous = read_stored_main(&namespace)?;
        let generation = write_document_chunks(&namespace, &serialized)?;
        let manifest = V3Manifest {
            version: 3,
            active_generation: generation.generation_id.clone(),
            generations: vec![generation.clone()],
        };
        if let Err(error) = write_v3_manifest(&namespace, &manifest) {
            let _ = cleanup_generation(&namespace, &generation.generation_id, generation.chunks);
            return Err(error);
        }
        if let Some(StoredMain::V3(old)) = previous {
            for generation in old.generations {
                cleanup_generation(&namespace, &generation.generation_id, generation.chunks)?;
            }
        }
        Ok(())
    }

    pub fn load(id: &str, expected_account: &str) -> Result<Value, AppError> {
        super::super::native_keyring::check_unlocked()?;
        let namespace = namespace(id)?;
        let Some(StoredMain::V3(manifest)) = read_stored_main(&namespace)? else {
            return Err(AppError::AuthenticationRequired(
                "Conecte a sessão desta conta para aplicações.".to_owned(),
            ));
        };
        let generation = manifest
            .generations
            .iter()
            .find(|generation| generation.generation_id == manifest.active_generation)
            .ok_or_else(|| AppError::Keyring("Sessão de aplicação incompleta.".to_owned()))?;
        let document: Value =
            serde_json::from_str(&read_generation_document(&namespace, generation)?)?;
        validate_auth_document(&document, Some(expected_account))?;
        Ok(document)
    }
}

fn namespace(id: &str) -> Result<String, AppError> {
    uuid::Uuid::parse_str(id)
        .map_err(|_| AppError::Validation("Identificador de sessão inválido.".to_owned()))?;
    Ok(format!("application-session:{id}"))
}
