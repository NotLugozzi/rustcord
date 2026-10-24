use crate::error::ClientError;
use openmls::prelude::tls_codec::*;
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::types::SignatureScheme;

pub const DAVE_PROTOCOL_VERSION: u32 = 1;

pub const OP_PREPARE_TRANSITION: u8 = 21;
pub const OP_EXECUTE_TRANSITION: u8 = 22;
pub const OP_TRANSITION_READY: u8 = 23;
pub const OP_PREPARE_EPOCH: u8 = 24;
pub const OP_MLS_EXTERNAL_SENDER: u8 = 25;
pub const OP_MLS_KEY_PACKAGE: u8 = 26;
pub const OP_MLS_PROPOSALS: u8 = 27;
pub const OP_MLS_COMMIT_WELCOME: u8 = 28;
pub const OP_MLS_ANNOUNCE_COMMIT_TRANSITION: u8 = 29;
pub const OP_MLS_WELCOME: u8 = 30;
pub const OP_MLS_INVALID_COMMIT_WELCOME: u8 = 31;

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

#[derive(Debug, Clone)]
pub struct ServerBinaryFrame {
    pub seq: u16,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

pub fn decode_server_frame(bytes: &[u8]) -> Result<ServerBinaryFrame, ClientError> {
    if bytes.len() < 3 {
        return Err(ClientError::GatewayClosed("dave frame too short".into()));
    }
    let seq = u16::from_be_bytes([bytes[0], bytes[1]]);
    let opcode = bytes[2];
    let payload = bytes[3..].to_vec();
    Ok(ServerBinaryFrame { seq, opcode, payload })
}

pub fn encode_client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + payload.len());
    out.push(opcode);
    out.extend_from_slice(payload);
    out
}

pub struct DaveSession {
    provider: OpenMlsRustCrypto,
    signature_keys: SignatureKeyPair,
    credential_with_key: CredentialWithKey,
    group: Option<MlsGroup>,
}

impl DaveSession {
    pub fn new(user_id: &str) -> Result<Self, ClientError> {
        let provider = OpenMlsRustCrypto::default();
        let signature_keys = SignatureKeyPair::new(SignatureScheme::ED25519)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to create signature keys: {e:?}")))?;
        signature_keys
            .store(provider.storage())
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to store signature keys: {e:?}")))?;

        let credential = BasicCredential::new(user_id.as_bytes().to_vec());
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signature_keys.to_public_vec().into(),
        };

        Ok(Self { provider, signature_keys, credential_with_key, group: None })
    }

    pub fn generate_key_package(&self) -> Result<Vec<u8>, ClientError> {
        let bundle = KeyPackage::builder()
            .build(CIPHERSUITE, &self.provider, &self.signature_keys, self.credential_with_key.clone())
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to build key package: {e:?}")))?;
        bundle
            .key_package()
            .tls_serialize_detached()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to serialize key package: {e:?}")))
    }

    pub fn create_new_group(&mut self) -> Result<(), ClientError> {
        let config = MlsGroupCreateConfig::builder().ciphersuite(CIPHERSUITE).use_ratchet_tree_extension(true).build();
        let group = MlsGroup::new(&self.provider, &self.signature_keys, &config, self.credential_with_key.clone())
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to create group: {e:?}")))?;
        self.group = Some(group);
        Ok(())
    }

    pub fn process_welcome(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        let message = MlsMessageIn::tls_deserialize_exact(bytes)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse welcome: {e:?}")))?;
        let MlsMessageBodyIn::Welcome(welcome) = message.extract() else {
            return Err(ClientError::GatewayClosed("dave: expected welcome message".into()));
        };
        let join_config = MlsGroupJoinConfig::builder().use_ratchet_tree_extension(true).build();
        let staged = StagedWelcome::new_from_welcome(&self.provider, &join_config, welcome, None)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to stage welcome: {e:?}")))?;
        let group = staged
            .into_group(&self.provider)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to join group from welcome: {e:?}")))?;
        self.group = Some(group);
        Ok(())
    }

    pub fn process_commit(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        let Some(group) = self.group.as_mut() else {
            return Err(ClientError::GatewayClosed("dave: no active group to process commit against".into()));
        };
        let message = MlsMessageIn::tls_deserialize_exact(bytes)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse commit: {e:?}")))?;
        let protocol_message = message
            .try_into_protocol_message()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: commit is not a protocol message: {e:?}")))?;
        let processed = group
            .process_message(&self.provider, protocol_message)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to process commit: {e:?}")))?;
        if let ProcessedMessageContent::StagedCommitMessage(staged_commit) = processed.into_content() {
            group
                .merge_staged_commit(&self.provider, *staged_commit)
                .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to merge commit: {e:?}")))?;
        }
        Ok(())
    }

    pub fn add_member(&mut self, key_package_bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>), ClientError> {
        let Some(group) = self.group.as_mut() else {
            return Err(ClientError::GatewayClosed("dave: no active group to add a member to".into()));
        };
        let message = MlsMessageIn::tls_deserialize_exact(key_package_bytes)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse key package: {e:?}")))?;
        let MlsMessageBodyIn::KeyPackage(key_package_in) = message.extract() else {
            return Err(ClientError::GatewayClosed("dave: expected key package message".into()));
        };
        let key_package = key_package_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: invalid key package: {e:?}")))?;
        let (commit, welcome, _group_info) = group
            .add_members(&self.provider, &self.signature_keys, &[key_package])
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to add member: {e:?}")))?;
        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to merge pending commit: {e:?}")))?;
        let commit_bytes = commit
            .tls_serialize_detached()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to serialize commit: {e:?}")))?;
        let welcome_bytes = welcome
            .tls_serialize_detached()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to serialize welcome: {e:?}")))?;
        Ok((commit_bytes, welcome_bytes))
    }
}
