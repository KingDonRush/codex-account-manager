use crate::domain::integrations::{AuthorizationGrant, GrantScope};
use crate::error::AppError;
use chrono::Utc;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionRequest {
    pub application_id: Option<String>,
    pub name: String,
    pub slug: String,
    pub requested_scope: GrantScope,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationRequest {
    pub id: String,
    pub request: ConnectionRequest,
    pub peer_pid: u32,
    pub executable: String,
    pub confirmation_code: String,
}

pub struct Claim {
    pub grant_id: String,
    pub signing_key: SigningKey,
}

struct Pending {
    public: AuthorizationRequest,
    peer_identity: String,
    channel: String,
    created: Instant,
    decision: Decision,
}

enum Decision {
    Awaiting,
    Committing,
    Approved(Box<Claim>),
    Denied,
    Claimed,
}

#[derive(Default)]
pub struct AuthorizationBook {
    requests: Mutex<HashMap<String, Pending>>,
}

impl AuthorizationBook {
    pub fn request(
        &self,
        request: ConnectionRequest,
        peer: &super::local_transport::PeerIdentity,
        channel: &str,
    ) -> Result<AuthorizationRequest, AppError> {
        if request.name.trim().is_empty()
            || request.name.chars().count() > 80
            || request.name.chars().any(char::is_control)
            || request.slug.is_empty()
            || request.slug.len() > 64
            || !request
                .slug
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(AppError::Validation(
                "Nome/identificador da integração inválido.".to_owned(),
            ));
        }
        let mut requests = self.requests.lock();
        requests.retain(|_, pending| pending.created.elapsed() < Duration::from_secs(180));
        if requests.len() >= 8
            || requests.values().any(|pending| {
                pending.peer_identity == peer.identity
                    && pending.created.elapsed() < Duration::from_secs(10)
            })
        {
            return Err(AppError::Busy(
                "Há um pedido recente desta integração; aguarde.".to_owned(),
            ));
        }
        let public = AuthorizationRequest {
            id: uuid::Uuid::new_v4().to_string(),
            request,
            peer_pid: peer.pid,
            executable: peer.executable.clone(),
            confirmation_code: random_hex(4)?.to_uppercase(),
        };
        requests.insert(
            public.id.clone(),
            Pending {
                public: public.clone(),
                peer_identity: peer.identity.clone(),
                channel: channel.to_owned(),
                created: Instant::now(),
                decision: Decision::Awaiting,
            },
        );
        Ok(public)
    }

    pub fn pending(&self) -> Vec<AuthorizationRequest> {
        self.requests
            .lock()
            .values()
            .filter(|pending| {
                pending.created.elapsed() < Duration::from_secs(180)
                    && matches!(pending.decision, Decision::Awaiting)
            })
            .map(|pending| pending.public.clone())
            .collect()
    }

    pub fn approve(
        &self,
        id: &str,
        application_id: &str,
        scope: GrantScope,
    ) -> Result<(AuthorizationGrant, SigningKey), AppError> {
        let mut requests = self.requests.lock();
        let pending = requests.get_mut(id).ok_or_else(|| {
            AppError::ConsentRequired("Pedido não está mais disponível.".to_owned())
        })?;
        if pending.created.elapsed() >= Duration::from_secs(180)
            || !matches!(pending.decision, Decision::Awaiting)
        {
            return Err(AppError::ConsentRequired(
                "Pedido expirou ou já foi respondido.".to_owned(),
            ));
        }
        if scope
            .expires_at
            .is_some_and(|expires| expires <= Utc::now())
            || scope.permanent && scope.expires_at.is_some()
        {
            return Err(AppError::Validation(
                "Duração da autorização inválida.".to_owned(),
            ));
        }
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret)
            .map_err(|_| AppError::Internal("Gerador de segurança indisponível.".to_owned()))?;
        let key = SigningKey::from_bytes(&secret);
        secret.fill(0);
        let grant = AuthorizationGrant {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: application_id.to_owned(),
            scope,
            verifier: hex::encode(key.verifying_key().as_bytes()),
            peer_identity: pending.peer_identity.clone(),
            revoked: false,
            created_at: Utc::now(),
        };
        pending.decision = Decision::Committing;
        // The persistence boundary is in ManagerCore; the key isn't claimable yet.
        Ok((grant, key))
    }

    pub fn publish(&self, id: &str, grant_id: String, key: SigningKey) -> Result<(), AppError> {
        let mut requests = self.requests.lock();
        let pending = requests
            .get_mut(id)
            .ok_or_else(|| AppError::ConsentRequired("Canal encerrado.".to_owned()))?;
        if !matches!(pending.decision, Decision::Committing) {
            return Err(AppError::AccessDenied("Pedido já respondido.".to_owned()));
        }
        pending.decision = Decision::Approved(Box::new(Claim {
            grant_id,
            signing_key: key,
        }));
        Ok(())
    }

    pub fn claim(&self, id: &str, channel: &str, peer: &str) -> Result<Option<Claim>, AppError> {
        let mut requests = self.requests.lock();
        let pending = requests
            .get_mut(id)
            .ok_or_else(|| AppError::ConsentRequired("Pedido expirado.".to_owned()))?;
        if pending.channel != channel
            || pending.peer_identity != peer
            || pending.created.elapsed() >= Duration::from_secs(180)
        {
            return Err(AppError::AccessDenied(
                "O pedido pertence a outro canal ou expirou.".to_owned(),
            ));
        }
        match std::mem::replace(&mut pending.decision, Decision::Claimed) {
            Decision::Awaiting => {
                pending.decision = Decision::Awaiting;
                Ok(None)
            }
            Decision::Committing => {
                pending.decision = Decision::Committing;
                Ok(None)
            }
            Decision::Approved(claim) => Ok(Some(*claim)),
            Decision::Denied => Err(AppError::AccessDenied(
                "Conexão negada pelo usuário.".to_owned(),
            )),
            Decision::Claimed => Err(AppError::AccessDenied(
                "Autenticação já resgatada.".to_owned(),
            )),
        }
    }

    pub fn deny(&self, id: &str) {
        if let Some(pending) = self.requests.lock().get_mut(id) {
            pending.decision = Decision::Denied;
        }
    }
}

pub fn random_hex(bytes: usize) -> Result<String, AppError> {
    let mut value = vec![0_u8; bytes];
    getrandom::fill(&mut value)
        .map_err(|_| AppError::Internal("Gerador de segurança indisponível.".to_owned()))?;
    Ok(hex::encode(value))
}

pub fn verify_proof(
    grant: &AuthorizationGrant,
    challenge: &str,
    proof: &str,
    peer: &str,
) -> Result<(), AppError> {
    if grant.revoked
        || grant
            .scope
            .expires_at
            .is_some_and(|expires| Utc::now() >= expires)
        || grant.peer_identity != peer
    {
        return Err(AppError::AccessDenied(
            "Autorização revogada, expirada ou de outro consumidor.".to_owned(),
        ));
    }
    let public: [u8; 32] = hex::decode(&grant.verifier)
        .ok()
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| AppError::AccessDenied("Verificador inválido.".to_owned()))?;
    let signature = hex::decode(proof)
        .ok()
        .and_then(|value| Signature::from_slice(&value).ok())
        .ok_or_else(|| AppError::AccessDenied("Prova inválida.".to_owned()))?;
    VerifyingKey::from_bytes(&public)
        .map_err(|_| AppError::AccessDenied("Verificador inválido.".to_owned()))?
        .verify_strict(challenge.as_bytes(), &signature)
        .map_err(|_| AppError::AccessDenied("Prova não confere.".to_owned()))
}
