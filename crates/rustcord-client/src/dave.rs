use crate::error::ClientError;
use openmls::prelude::tls_codec::*;
use openmls::prelude::*;
use openmls::ciphersuite::hash_ref::ProposalRef;
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

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;

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
    external_sender: Option<ExternalSender>,
}

impl DaveSession {
    pub fn new(user_id: &str) -> Result<Self, ClientError> {
        let provider = OpenMlsRustCrypto::default();
        let signature_keys = SignatureKeyPair::new(SignatureScheme::ECDSA_SECP256R1_SHA256)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to create signature keys: {e:?}")))?;
        signature_keys
            .store(provider.storage())
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to store signature keys: {e:?}")))?;

        let user_id = user_id.parse::<u64>().map_err(|e| {
            ClientError::GatewayClosed(format!("dave: invalid Discord user id: {e}"))
        })?;
        let credential = BasicCredential::new(user_id.to_be_bytes().to_vec());
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signature_keys.to_public_vec().into(),
        };

        Ok(Self { provider, signature_keys, credential_with_key, group: None, external_sender: None })
    }

    pub fn generate_key_package(&self) -> Result<Vec<u8>, ClientError> {
        let capabilities = dave_capabilities();
        let bundle = KeyPackage::builder()
            .key_package_lifetime(Lifetime::init(0, u64::MAX))
            .leaf_node_capabilities(capabilities)
            .build(CIPHERSUITE, &self.provider, &self.signature_keys, self.credential_with_key.clone())
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to build key package: {e:?}")))?;
        bundle
            .key_package()
            .tls_serialize_detached()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to serialize key package: {e:?}")))
    }

    pub fn set_external_sender(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        let sender = ExternalSender::tls_deserialize_exact(bytes)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse external sender: {e:?}")))?;
        self.external_sender = Some(sender);
        Ok(())
    }

    pub fn has_external_sender(&self) -> bool {
        self.external_sender.is_some()
    }

    pub fn create_new_group(&mut self, group_id: u64) -> Result<Vec<u8>, ClientError> {
        let external_sender = self.external_sender.clone()
            .ok_or_else(|| ClientError::GatewayClosed("dave: external sender is not available".into()))?;
        let extensions = Extensions::<GroupContext>::single(Extension::ExternalSenders(vec![external_sender]))
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to build external sender extension: {e:?}")))?;
        let capabilities = dave_capabilities();
        let key_package_bundle = KeyPackage::builder()
            .key_package_lifetime(Lifetime::init(0, u64::MAX))
            .leaf_node_capabilities(capabilities.clone())
            .build(CIPHERSUITE, &self.provider, &self.signature_keys, self.credential_with_key.clone())
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to build initial key package: {e:?}")))?;
        let key_package = key_package_bundle.key_package().tls_serialize_detached()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to serialize initial key package: {e:?}")))?;
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .capabilities(capabilities)
            .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(true)
            .with_group_context_extensions(extensions)
            .build();
        let group = MlsGroup::new_with_group_id_and_key_package(
            &self.provider,
            &self.signature_keys,
            &config,
            GroupId::from_slice(&group_id.to_be_bytes()),
            self.credential_with_key.clone(),
            key_package_bundle,
        )
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to create group: {e:?}")))?;
        self.group = Some(group);
        Ok(key_package)
    }

    pub fn process_welcome(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        let welcome = Welcome::tls_deserialize_exact(bytes)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse welcome: {e:?}")))?;
        if let Some(mut pending_group) = self.group.take() {
            pending_group.delete(self.provider.storage())
                .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to discard pending group before welcome: {e:?}")))?;
        }
        let join_config = MlsGroupJoinConfig::builder()
            .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(true)
            .build();
        let staged = StagedWelcome::new_from_welcome(&self.provider, &join_config, welcome, None)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to stage welcome: {e:?}")))?;
        let group = staged
            .into_group(&self.provider)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to join group from welcome: {e:?}")))?;
        self.group = Some(group);
        Ok(())
    }

    pub fn process_welcome_with_transition(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        self.process_welcome(bytes)
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
        match processed.into_content() {
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                group.merge_staged_commit(&self.provider, *staged_commit)
                    .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to merge commit: {e:?}")))?;
            }
            ProcessedMessageContent::OwnPendingCommit => {
                group.merge_pending_commit(&self.provider)
                    .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to merge our announced commit: {e:?}")))?;
            }
            other => return Err(ClientError::GatewayClosed(format!("dave: unexpected commit content: {other:?}"))),
        }
        Ok(())
    }

    pub fn process_proposals(&mut self, payload: &[u8], recognized_users: &[String]) -> Result<Option<Vec<u8>>, ClientError> {
        let Some(group) = self.group.as_mut() else { return Ok(None) };
        let (&operation, encoded) = payload.split_first()
            .ok_or_else(|| ClientError::GatewayClosed("dave: empty proposals payload".into()))?;
        if operation == 1 {
            let (refs, rest) = VLBytes::tls_deserialize_bytes(encoded)
                .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse proposal revocations: {e:?}")))?;
            if !rest.is_empty() { return Err(ClientError::GatewayClosed("dave: trailing proposal revocation bytes".into())); }
            let mut bytes = refs.as_slice();
            while !bytes.is_empty() {
                let (proposal_ref, rest) = ProposalRef::tls_deserialize_bytes(bytes)
                    .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse proposal reference: {e:?}")))?;
                group.remove_pending_proposal(self.provider.storage(), &proposal_ref)
                    .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to revoke proposal: {e:?}")))?;
                bytes = rest;
            }
        } else if operation == 0 {
            let (messages, rest) = VLBytes::tls_deserialize_bytes(encoded)
                .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse proposal list: {e:?}")))?;
            if !rest.is_empty() { return Err(ClientError::GatewayClosed("dave: trailing proposal list bytes".into())); }
            let mut bytes = messages.as_slice();
            while !bytes.is_empty() {
                let (message, rest) = MlsMessageIn::tls_deserialize_bytes(bytes)
                    .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to parse proposal message: {e:?}")))?;
                let protocol_message = message.try_into_protocol_message()
                    .map_err(|e| ClientError::GatewayClosed(format!("dave: proposal is not a protocol message: {e:?}")))?;
                let processed = group.process_message(&self.provider, protocol_message)
                    .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to process proposal: {e:?}")))?;
                let ProcessedMessageContent::ProposalMessage(proposal) = processed.into_content() else {
                    return Err(ClientError::GatewayClosed("dave: expected a standalone proposal".into()));
                };
                if !matches!(proposal.sender(), Sender::External(_)) {
                    return Err(ClientError::GatewayClosed("dave: proposal was not sent by the voice gateway".into()));
                }
                match proposal.proposal() {
                    Proposal::Add(add) => {
                        let identity = BasicCredential::try_from(add.key_package().leaf_node().credential().clone())
                            .map_err(|e| ClientError::GatewayClosed(format!("dave: add proposal has invalid credential: {e:?}")))?;
                        let id = identity.identity();
                        if id.len() != 8 || !recognized_users.iter().any(|candidate| candidate.parse::<u64>().ok().map(|v| v.to_be_bytes().as_slice() == id).unwrap_or(false)) {
                            return Err(ClientError::GatewayClosed("dave: add proposal is for an unrecognized user".into()));
                        }
                    }
                    Proposal::Remove(_) => {}
                    _ => return Err(ClientError::GatewayClosed("dave: unsupported MLS proposal type".into())),
                }
                group.store_pending_proposal(self.provider.storage(), *proposal)
                    .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to store proposal: {e:?}")))?;
                bytes = rest;
            }
        } else {
            return Err(ClientError::GatewayClosed("dave: unknown proposal operation".into()));
        }
        if !group.has_pending_proposals() { return Ok(None); }
        let (commit, welcome, _) = group.commit_to_pending_proposals(&self.provider, &self.signature_keys)
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to commit proposals: {e:?}")))?;
        let mut output = commit.tls_serialize_detached()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to serialize commit: {e:?}")))?;
        if let Some(welcome_message) = welcome {
            let MlsMessageBodyOut::Welcome(welcome) = welcome_message.body() else {
                return Err(ClientError::GatewayClosed("dave: commit returned a non-welcome message".into()));
            };
            output.extend_from_slice(&welcome.tls_serialize_detached()
                .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to serialize welcome body: {e:?}")))?);
        }
        Ok(Some(output))
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
        let MlsMessageBodyOut::Welcome(welcome_body) = welcome.body() else {
            return Err(ClientError::GatewayClosed("dave: add-member operation returned a non-welcome message".into()));
        };
        let welcome_bytes = welcome_body
            .tls_serialize_detached()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: failed to serialize welcome body: {e:?}")))?;
        Ok((commit_bytes, welcome_bytes))
    }

    /// MLS epoch and the last 4 bytes of the epoch authenticator. The authenticator is not a key:
    /// it is meant for out-of-band comparison and is identical for every member of the group.
    pub fn epoch_info(&self) -> Option<(u64, [u8; 4])> {
        let group = self.group.as_ref()?;
        let auth = group.epoch_authenticator().as_slice();
        let mut code = [0u8; 4];
        code.copy_from_slice(auth.get(auth.len().checked_sub(4)?..)?);
        Some((group.epoch().as_u64(), code))
    }

    pub fn sender_base_secret(&self, user_id: &str) -> Result<[u8; 16], ClientError> {
        let Some(group) = self.group.as_ref() else {
            return Err(ClientError::GatewayClosed("dave: no active MLS group for media key export".into()));
        };
        let user_id = user_id.parse::<u64>()
            .map_err(|e| ClientError::GatewayClosed(format!("dave: invalid media sender user id: {e}")))?;
        let exported = group.export_secret(
            self.provider.crypto(),
            "Discord Secure Frames v0",
            &user_id.to_le_bytes(),
            16,
        ).map_err(|e| ClientError::GatewayClosed(format!("dave: failed to export media sender secret: {e:?}")))?;
        let mut secret = [0u8; 16];
        secret.copy_from_slice(&exported);
        Ok(secret)
    }
}

fn dave_capabilities() -> Capabilities {
    Capabilities::new(
        Some(&[ProtocolVersion::Mls10]),
        Some(&[CIPHERSUITE]),
        None,
        None,
        Some(&[CredentialType::Basic]),
    )
}
