//! SSH Agent forwarding broker.
//!
//! The broker terminates the Agent protocol, merges identities from external
//! providers and stored keys, and applies policy filtering before routing
//! signing requests back to the owning provider. A compatible single-provider
//! topology may still use raw relay at the caller to preserve unknown Agent
//! extensions.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use futures::future::join_all;
use russh::keys::ssh_encoding::Encode;
use russh::keys::ssh_key::Signature;
use russh::keys::{HashAlg, PrivateKey};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::ShellEnvironmentCache;
use crate::ssh_agent::{DynamicAgentStream, connect_agent_stream_with_environment_until};
use crate::{
    SshAgentEndpoint, SshAgentForwardingConfig, SshAgentForwardingPolicy,
    SshAgentStoredKeyProvider, SshAgentStoredKeySnapshot,
};

const MAX_AGENT_FRAME_LEN: usize = 256 * 1024;
const MAX_AGENT_COMMENT_LEN: usize = 4096;
const MAX_IDENTITIES: usize = 1024;
const MAX_AGENT_CHANNELS: usize = 16;
const MAX_SIGN_CONCURRENCY: usize = 32;
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
// The timeout includes one login-shell environment initialization (up to ten
// seconds) and the subsequent Agent socket connection.
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(15);
const SIGN_TIMEOUT: Duration = Duration::from_secs(60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
const SESSION_BIND_EXTENSION: &[u8] = b"session-bind@openssh.com";
const QUERY_EXTENSION: &[u8] = b"query";

static AGENT_CHANNELS: OnceLock<Arc<Semaphore>> = OnceLock::new();
static SIGN_OPERATIONS: OnceLock<Arc<Semaphore>> = OnceLock::new();
static STORED_IDENTITY_LOADS: OnceLock<Arc<Semaphore>> = OnceLock::new();
static FINGERPRINT_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshAgentIdentityPreview {
    pub fingerprint: String,
    pub comment: String,
    pub source: String,
    pub custom_endpoint_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshAgentEndpointPreviewError {
    pub custom_endpoint_index: usize,
    pub endpoint_type: String,
    pub code: SshAgentEndpointPreviewErrorCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAgentEndpointPreviewErrorCode {
    ConnectFailed,
    IdentityEnumerationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SshAgentIdentityPreviewResponse {
    pub identities: Vec<SshAgentIdentityPreview>,
    pub endpoint_errors: Vec<SshAgentEndpointPreviewError>,
    pub truncated: bool,
}

#[derive(Debug)]
enum IdentitySource {
    External {
        upstream_index: usize,
        endpoint_index: usize,
    },
    Stored {
        key: Arc<PrivateKey>,
        revision: u64,
    },
}

#[derive(Debug)]
struct IdentityRecord {
    blob: Vec<u8>,
    fingerprint: String,
    comment: String,
    source: IdentitySource,
}

type StoredIdentity = (Vec<u8>, String, Arc<PrivateKey>);

struct ExternalUpstream {
    endpoint: SshAgentEndpoint,
    endpoint_index: usize,
    stream: DynamicAgentStream,
    healthy: bool,
}

#[derive(Clone)]
struct ExternalEndpointSpec {
    endpoint: SshAgentEndpoint,
    endpoint_index: usize,
}

#[derive(Debug)]
struct BrokerState {
    identities: Vec<IdentityRecord>,
    endpoint_errors: Vec<SshAgentEndpointPreviewError>,
    truncated: bool,
    /// Encoded `SSH_AGENT_IDENTITIES_ANSWER` length. Preview and forwarding
    /// therefore expose the same deterministic prefix.
    encoded_identities_len: usize,
}

impl Default for BrokerState {
    fn default() -> Self {
        Self {
            identities: Vec::new(),
            endpoint_errors: Vec::new(),
            truncated: false,
            // Message type plus identity count.
            encoded_identities_len: 1 + std::mem::size_of::<u32>(),
        }
    }
}

impl BrokerState {
    fn push_identity(&mut self, seen: &mut HashSet<Vec<u8>>, identity: IdentityRecord) {
        if self.truncated || seen.contains(&identity.blob) {
            return;
        }
        let Some(encoded_len) = self
            .encoded_identities_len
            .checked_add(std::mem::size_of::<u32>())
            .and_then(|length| length.checked_add(identity.blob.len()))
            .and_then(|length| length.checked_add(std::mem::size_of::<u32>()))
            .and_then(|length| length.checked_add(identity.comment.len()))
        else {
            self.truncated = true;
            return;
        };
        if self.identities.len() >= MAX_IDENTITIES || encoded_len > MAX_AGENT_FRAME_LEN {
            self.truncated = true;
            return;
        }
        seen.insert(identity.blob.clone());
        self.encoded_identities_len = encoded_len;
        self.identities.push(identity);
    }
}

/// Enumerates public identities without applying the saved allowlist.
pub async fn preview_identities(
    config: &SshAgentForwardingConfig,
    provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
) -> SshAgentIdentityPreviewResponse {
    preview_identities_with_environment(config, provider, ShellEnvironmentCache::global()).await
}

/// Enumerates identities using the supplied shell environment cache.
pub async fn preview_identities_with_environment(
    config: &SshAgentForwardingConfig,
    provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> SshAgentIdentityPreviewResponse {
    if !config.enabled {
        return SshAgentIdentityPreviewResponse::default();
    }
    let (mut external, mut endpoint_errors) =
        connect_external_upstreams(config, shell_environment).await;
    let mut state = collect_identities(config, provider, &mut external, false).await;
    endpoint_errors.extend(state.endpoint_errors);
    state.endpoint_errors = endpoint_errors;
    SshAgentIdentityPreviewResponse {
        identities: state
            .identities
            .iter()
            .map(IdentityRecord::preview)
            .collect(),
        endpoint_errors: state.endpoint_errors,
        truncated: state.truncated,
    }
}

/// Runs identity preview on a dedicated Tokio runtime for non-Tokio callers.
pub fn preview_identities_blocking(
    config: &SshAgentForwardingConfig,
    provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
) -> SshAgentIdentityPreviewResponse {
    preview_identities_blocking_with_environment(config, provider, ShellEnvironmentCache::global())
}

/// Runs identity preview with a shared shell environment cache.
pub fn preview_identities_blocking_with_environment(
    config: &SshAgentForwardingConfig,
    provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> SshAgentIdentityPreviewResponse {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "failed to create SSH Agent preview runtime");
            return SshAgentIdentityPreviewResponse::default();
        }
    };
    runtime.block_on(preview_identities_with_environment(
        config,
        provider,
        shell_environment,
    ))
}

pub(crate) fn try_acquire_agent_channel_permit() -> Option<OwnedSemaphorePermit> {
    AGENT_CHANNELS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_AGENT_CHANNELS)))
        .clone()
        .try_acquire_owned()
        .ok()
}

/// Relays a compatible single-provider Agent channel while retaining the
/// broker's frame bounds and inactivity deadlines.
pub(crate) async fn serve_raw_channel<C, A>(channel: C, agent: A, permit: OwnedSemaphorePermit)
where
    C: AsyncRead + AsyncWrite + Unpin,
    A: AsyncRead + AsyncWrite + Unpin,
{
    if let Err(error) = relay_raw_channel_inner(
        channel,
        agent,
        permit,
        FIRST_FRAME_TIMEOUT,
        IDLE_TIMEOUT,
        SIGN_TIMEOUT,
    )
    .await
    {
        tracing::debug!(%error, "SSH Agent raw forwarding channel closed");
    }
}

async fn relay_raw_channel_inner<C, A>(
    mut channel: C,
    mut agent: A,
    _permit: OwnedSemaphorePermit,
    first_frame_timeout: Duration,
    idle_timeout: Duration,
    request_timeout: Duration,
) -> anyhow::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
    A: AsyncRead + AsyncWrite + Unpin,
{
    let mut first = true;
    loop {
        let timeout = if first {
            first = false;
            first_frame_timeout
        } else {
            idle_timeout
        };
        let Some(request) = tokio::time::timeout(timeout, read_frame(&mut channel)).await?? else {
            return Ok(());
        };
        tokio::time::timeout(WRITE_TIMEOUT, write_frame(&mut agent, &request)).await??;
        let Some(response) =
            tokio::time::timeout(request_timeout, read_frame(&mut agent)).await??
        else {
            return Ok(());
        };
        tokio::time::timeout(WRITE_TIMEOUT, write_frame(&mut channel, &response)).await??;
    }
}

/// Serves an accepted `auth-agent@openssh.com` channel.
pub async fn serve_channel<S>(
    stream: S,
    config: SshAgentForwardingConfig,
    provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    shell_environment: Arc<ShellEnvironmentCache>,
    permit: OwnedSemaphorePermit,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Err(error) =
        serve_channel_inner(stream, config, provider, shell_environment, permit).await
    {
        tracing::debug!(%error, "SSH Agent forwarding broker closed");
    }
}

async fn serve_channel_inner<S>(
    mut stream: S,
    config: SshAgentForwardingConfig,
    provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    shell_environment: Arc<ShellEnvironmentCache>,
    _permit: OwnedSemaphorePermit,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut first = true;
    let (mut external, _) = connect_external_upstreams(&config, shell_environment).await;
    let initial_revision = provider
        .as_ref()
        .filter(|_| config.sources.stored_keys)
        .and_then(|provider| provider.revision().ok());
    let mut state = BrokerState::default();
    let mut identities_loaded = false;
    loop {
        let frame = if first {
            first = false;
            tokio::time::timeout(FIRST_FRAME_TIMEOUT, read_frame(&mut stream)).await??
        } else {
            tokio::time::timeout(IDLE_TIMEOUT, read_frame(&mut stream)).await??
        };
        let Some(frame) = frame else {
            return Ok(());
        };
        if let (Some(provider), Some(initial_revision)) = (&provider, initial_revision)
            && provider.revision().ok() != Some(initial_revision)
        {
            anyhow::bail!("stored SSH Agent keys changed");
        }
        if frame.is_empty() {
            anyhow::bail!("empty SSH Agent request");
        }
        let response = match frame[0] {
            11 => {
                if frame.len() != 1 {
                    encode_failure()
                } else {
                    if !identities_loaded {
                        state = collect_identities(&config, provider.clone(), &mut external, true)
                            .await;
                        identities_loaded = true;
                    }
                    let (response, response_truncated) = encode_identities_response(&state);
                    if response_truncated {
                        state.truncated = true;
                    }
                    response
                }
            }
            13 => {
                // OpenSSH clients normally enumerate first, but the Tauri
                // broker also supports a direct SIGN_REQUEST.  Load the same
                // bounded identity snapshot lazily for that compatibility path.
                if !identities_loaded {
                    state =
                        collect_identities(&config, provider.clone(), &mut external, true).await;
                    identities_loaded = true;
                }
                match sign_request(
                    &frame[1..],
                    &state,
                    &config,
                    provider.as_ref(),
                    &mut external,
                )
                .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::debug!(%error, "SSH Agent sign request rejected");
                        encode_failure()
                    }
                }
            }
            27 => match extension_request(&frame[1..], &config, &state, &mut external).await {
                Ok(response) => response,
                Err(error) => {
                    tracing::debug!(%error, "SSH Agent extension request rejected");
                    encode_failure()
                }
            },
            _ => encode_failure(),
        };
        tokio::time::timeout(WRITE_TIMEOUT, write_frame(&mut stream, &response)).await??;
    }
}

async fn collect_identities(
    config: &SshAgentForwardingConfig,
    provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    external: &mut [ExternalUpstream],
    apply_policy: bool,
) -> BrokerState {
    let mut state = BrokerState::default();
    let mut blobs = HashSet::new();
    let responses = join_all(external.iter_mut().enumerate().filter_map(
        |(upstream_index, upstream)| {
            if !upstream.healthy {
                return None;
            }
            Some(async move {
                let result = request_external_identities(&mut upstream.stream).await;
                (
                    upstream_index,
                    upstream.endpoint.clone(),
                    upstream.endpoint_index,
                    result,
                )
            })
        },
    ))
    .await;

    for (upstream_index, endpoint, endpoint_index, result) in responses {
        let identities = match result {
            Ok(identities) => identities,
            Err(error) => {
                if let Some(upstream) = external.get_mut(upstream_index) {
                    upstream.healthy = false;
                }
                state
                    .endpoint_errors
                    .push(endpoint_error(endpoint_index, &endpoint, true));
                tracing::debug!(endpoint_index, %error, "SSH Agent identity enumeration failed");
                continue;
            }
        };
        for (blob, comment) in identities {
            let identity = IdentityRecord {
                fingerprint: fingerprint(&blob),
                comment: bounded_agent_comment(&comment),
                source: IdentitySource::External {
                    upstream_index,
                    endpoint_index,
                },
                blob,
            };
            if apply_policy && !policy_allows(&config.policy, &identity.fingerprint) {
                continue;
            }
            state.push_identity(&mut blobs, identity);
            if state.truncated {
                break;
            }
        }
        if state.truncated {
            break;
        }
    }

    if config.sources.stored_keys
        && !state.truncated
        && let Some(provider) = provider
    {
        if let Ok((revision, identities)) = load_stored_identities_bounded(provider).await {
            for (blob, comment, key) in identities {
                let identity = IdentityRecord {
                    fingerprint: fingerprint(&blob),
                    comment,
                    source: IdentitySource::Stored { key, revision },
                    blob,
                };
                if apply_policy && !policy_allows(&config.policy, &identity.fingerprint) {
                    continue;
                }
                state.push_identity(&mut blobs, identity);
                if state.truncated {
                    break;
                }
            }
        } else {
            tracing::debug!("Stored SSH Agent identities are unavailable");
        }
    }
    if apply_policy {
        state
            .identities
            .retain(|identity| policy_allows(&config.policy, &identity.fingerprint));
    }
    state
}

async fn load_stored_identities_bounded(
    provider: Arc<dyn SshAgentStoredKeyProvider>,
) -> Result<(u64, Vec<StoredIdentity>), String> {
    let semaphore = STORED_IDENTITY_LOADS
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone();
    tokio::time::timeout(IDENTITY_TIMEOUT, async move {
        let permit = semaphore
            .acquire_owned()
            .await
            .map_err(|_| "stored SSH Agent identity loader closed".to_string())?;
        tokio::task::spawn_blocking(move || {
            // The permit intentionally lives inside the blocking task. If the
            // async timeout expires, abandoned work still counts toward the
            // global bound until it really finishes.
            let _permit = permit;
            load_stored_identities(provider)
        })
        .await
        .map_err(|error| format!("stored SSH Agent identity loader failed: {error}"))?
    })
    .await
    .map_err(|_| "stored SSH Agent identity request timed out".to_string())?
}

async fn connect_external_upstreams(
    config: &SshAgentForwardingConfig,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> (Vec<ExternalUpstream>, Vec<SshAgentEndpointPreviewError>) {
    let attempts = join_all(
        configured_external_endpoints(config)
            .into_iter()
            .map(|spec| async {
                let result = tokio::time::timeout(
                    IDENTITY_TIMEOUT,
                    connect_agent_stream_with_environment_until(
                        &spec.endpoint,
                        Some(shell_environment.clone()),
                        Instant::now() + IDENTITY_TIMEOUT,
                    ),
                )
                .await;
                (spec, result)
            }),
    )
    .await;
    let mut upstreams = Vec::new();
    let mut errors = Vec::new();
    for (spec, result) in attempts {
        match result {
            Ok(Ok(stream)) => upstreams.push(ExternalUpstream {
                endpoint: spec.endpoint,
                endpoint_index: spec.endpoint_index,
                stream,
                healthy: true,
            }),
            Ok(Err(_error)) => {
                tracing::debug!(
                    endpoint_index = spec.endpoint_index,
                    "SSH Agent endpoint connection failed"
                );
                errors.push(endpoint_error(spec.endpoint_index, &spec.endpoint, false));
            }
            Err(_) => errors.push(endpoint_error(spec.endpoint_index, &spec.endpoint, false)),
        }
    }
    (upstreams, errors)
}

fn configured_external_endpoints(config: &SshAgentForwardingConfig) -> Vec<ExternalEndpointSpec> {
    if !config.sources.external_agent {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    config
        .sources
        .external_agent_endpoints
        .iter()
        .cloned()
        .enumerate()
        .map(|(endpoint_index, endpoint)| ExternalEndpointSpec {
            endpoint,
            endpoint_index,
        })
        .filter(|spec| seen.insert(broker_endpoint_key(&spec.endpoint)))
        .collect()
}

fn load_stored_identities(
    provider: Arc<dyn SshAgentStoredKeyProvider>,
) -> Result<(u64, Vec<StoredIdentity>), String> {
    let SshAgentStoredKeySnapshot { revision, keys } = provider.load_snapshot()?;
    let mut identities = Vec::new();
    for key in keys {
        let Ok(private_key) =
            russh::keys::decode_secret_key(&key.key_data, key.passphrase.as_deref())
        else {
            tracing::debug!("Stored key passphrase is unavailable for Agent forwarding");
            continue;
        };
        let private_key = Arc::new(private_key);
        let (blob, comment) = match key.cert_data.as_deref() {
            Some(cert) => match russh::keys::Certificate::from_openssh(cert) {
                Ok(certificate)
                    if certificate.public_key() == private_key.public_key().key_data() =>
                {
                    let Ok(blob) = certificate.to_bytes() else {
                        continue;
                    };
                    let comment = if certificate.comment().is_empty() {
                        key.comment.clone()
                    } else {
                        certificate.comment().to_string()
                    };
                    (blob, comment)
                }
                Ok(_) => {
                    tracing::debug!("Stored certificate does not match its private key");
                    let Ok(blob) = private_key.public_key().key_data().encode_vec() else {
                        continue;
                    };
                    (blob, key.comment.clone())
                }
                Err(_) => {
                    tracing::debug!("Stored certificate could not be parsed");
                    let Ok(blob) = private_key.public_key().key_data().encode_vec() else {
                        continue;
                    };
                    (blob, key.comment.clone())
                }
            },
            None => {
                let Ok(blob) = private_key.public_key().key_data().encode_vec() else {
                    continue;
                };
                (blob, key.comment.clone())
            }
        };
        identities.push((blob, bounded_agent_comment(&comment), private_key));
    }
    if provider.revision()? != revision {
        return Err("stored SSH keys changed while parsing".to_string());
    }
    Ok((revision, identities))
}

/// Proxies the two extensions retained by the Tauri broker. Mixed providers
/// and multi-endpoint topologies reject extensions because stateful extension
/// semantics cannot be routed safely across providers.
async fn extension_request(
    payload: &[u8],
    config: &SshAgentForwardingConfig,
    _state: &BrokerState,
    external: &mut [ExternalUpstream],
) -> anyhow::Result<Vec<u8>> {
    let request_payload = payload;
    let mut payload = payload;
    let name = read_string(&mut payload)?;
    match name.as_slice() {
        QUERY_EXTENSION => anyhow::ensure!(payload.is_empty(), "malformed Agent query extension"),
        SESSION_BIND_EXTENSION => {
            // session-bind@openssh.com: host key, session id, signature, critical flag.
            read_string(&mut payload)?;
            read_string(&mut payload)?;
            read_string(&mut payload)?;
            let critical = read_u8(&mut payload)?;
            anyhow::ensure!(critical <= 1, "malformed Agent session-bind extension");
            anyhow::ensure!(payload.is_empty(), "malformed Agent session-bind extension");
        }
        _ => return Ok(encode_extension_failure()),
    }

    if single_external_extension_endpoint(config).is_none()
        || external.len() != 1
        || !external[0].healthy
    {
        return Ok(encode_extension_failure());
    }
    let mut request = Vec::with_capacity(request_payload.len() + 1);
    request.push(27);
    request.extend_from_slice(request_payload);
    match proxy_upstream_frame(&mut external[0].stream, &request).await {
        Ok(response) => Ok(response),
        Err(error) => {
            external[0].healthy = false;
            Err(error)
        }
    }
}

fn single_external_extension_endpoint(
    config: &SshAgentForwardingConfig,
) -> Option<SshAgentEndpoint> {
    if !config.sources.external_agent || config.sources.stored_keys {
        return None;
    }
    let mut endpoints = config.sources.external_agent_endpoints.iter().cloned();
    let endpoint = endpoints.next()?;
    let key = broker_endpoint_key(&endpoint);
    if endpoints.any(|candidate| broker_endpoint_key(&candidate) != key) {
        return None;
    }
    Some(endpoint)
}

async fn request_external_identities(
    stream: &mut DynamicAgentStream,
) -> anyhow::Result<Vec<(Vec<u8>, String)>> {
    tokio::time::timeout(IDENTITY_TIMEOUT, async {
        write_frame(stream, &[11]).await?;
        let response = read_frame(stream)
            .await?
            .ok_or_else(|| anyhow::anyhow!("external Agent closed"))?;
        let mut payload = response.as_slice();
        anyhow::ensure!(
            read_u8(&mut payload)? == 12,
            "external Agent rejected identities"
        );
        let count = read_u32(&mut payload)? as usize;
        anyhow::ensure!(count <= MAX_IDENTITIES, "too many Agent identities");
        let mut identities = Vec::with_capacity(count);
        for _ in 0..count {
            let blob = read_string(&mut payload)?;
            let comment = String::from_utf8(read_string(&mut payload)?)
                .map_err(|_| anyhow::anyhow!("invalid Agent identity comment"))?;
            anyhow::ensure!(
                comment.len() <= MAX_AGENT_COMMENT_LEN,
                "Agent comment is too long"
            );
            identities.push((blob, comment));
        }
        anyhow::ensure!(
            payload.is_empty(),
            "Agent identity response has trailing data"
        );
        Ok(identities)
    })
    .await
    .map_err(|_| anyhow::anyhow!("external Agent identity request timed out"))?
}

async fn proxy_upstream_frame(
    stream: &mut DynamicAgentStream,
    request: &[u8],
) -> anyhow::Result<Vec<u8>> {
    proxy_upstream_frame_until(stream, request, tokio::time::Instant::now() + SIGN_TIMEOUT).await
}

async fn proxy_upstream_frame_until(
    stream: &mut DynamicAgentStream,
    request: &[u8],
    deadline: tokio::time::Instant,
) -> anyhow::Result<Vec<u8>> {
    tokio::time::timeout_at(deadline, async {
        write_frame(stream, request).await?;
        read_frame(stream)
            .await?
            .ok_or_else(|| anyhow::anyhow!("external Agent closed during request"))
    })
    .await
    .map_err(|_| anyhow::anyhow!("external Agent request timed out"))?
}

async fn sign_request(
    payload: &[u8],
    state: &BrokerState,
    config: &SshAgentForwardingConfig,
    provider: Option<&Arc<dyn SshAgentStoredKeyProvider>>,
    external: &mut [ExternalUpstream],
) -> anyhow::Result<Vec<u8>> {
    let mut payload = payload;
    let blob = read_string(&mut payload)?;
    let data = read_string(&mut payload)?;
    let flags = read_u32(&mut payload)?;
    if !payload.is_empty() {
        return Ok(encode_failure());
    }
    let Some(identity) = state
        .identities
        .iter()
        .find(|identity| identity.blob == blob)
    else {
        return Ok(encode_failure());
    };
    if !policy_allows(&config.policy, &identity.fingerprint) {
        return Ok(encode_failure());
    }
    let deadline = tokio::time::Instant::now() + SIGN_TIMEOUT;
    let semaphore = SIGN_OPERATIONS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_SIGN_CONCURRENCY)))
        .clone();
    let lock = fingerprint_lock(&identity.fingerprint).await;
    let lock_guard = tokio::time::timeout_at(deadline, lock.lock_owned())
        .await
        .map_err(|_| anyhow::anyhow!("SSH Agent fingerprint lock timed out"))?;
    let Ok(permit) = semaphore.try_acquire_owned() else {
        return Ok(encode_failure());
    };
    let signature = match &identity.source {
        IdentitySource::External { upstream_index, .. } => {
            // Preserve the external Agent request flags verbatim.
            let mut request = vec![13];
            put_string(&mut request, &blob)?;
            put_string(&mut request, &data)?;
            request.extend_from_slice(&flags.to_be_bytes());
            let Some(upstream) = external.get_mut(*upstream_index) else {
                return Ok(encode_failure());
            };
            if !upstream.healthy {
                return Ok(encode_failure());
            }
            return match proxy_upstream_frame_until(&mut upstream.stream, &request, deadline).await
            {
                Ok(response) => Ok(response),
                Err(_) => {
                    upstream.healthy = false;
                    Ok(encode_failure())
                }
            };
        }
        IdentitySource::Stored { key, revision } => {
            let Some(provider) = provider else {
                return Ok(encode_failure());
            };
            if provider.revision().ok() != Some(*revision) {
                return Ok(encode_failure());
            }
            if !matches!(flags, 0 | 2 | 4) {
                return Ok(encode_failure());
            }
            let key = key.clone();
            let signature = tokio::time::timeout_at(
                deadline,
                tokio::task::spawn_blocking(move || {
                    // Keep the guards in the blocking task so a caller timeout
                    // cannot create unbounded work for this fingerprint.
                    let _lock_guard = lock_guard;
                    let _permit = permit;
                    sign_private_key(&key, flags, &data)
                }),
            )
            .await;
            let Ok(Ok(Ok(signature))) = signature else {
                return Ok(encode_failure());
            };
            if provider.revision().ok() != Some(*revision) {
                return Ok(encode_failure());
            }
            let mut encoded = Vec::new();
            if signature.encode(&mut encoded).is_err() {
                return Ok(encode_failure());
            }
            encoded
        }
    };
    let mut response = vec![14];
    put_string(&mut response, &signature)?;
    Ok(response)
}

fn sign_private_key(key: &PrivateKey, flags: u32, data: &[u8]) -> anyhow::Result<Signature> {
    use russh::keys::signature::Signer;
    if !matches!(key.algorithm(), russh::keys::Algorithm::Rsa { .. }) && flags != 0 {
        anyhow::bail!("non-RSA Agent signatures do not accept hash flags");
    }
    let hash_alg = hash_algorithm(flags);
    if let russh::keys::ssh_key::private::KeypairData::Rsa(rsa) = key.key_data() {
        return Signer::try_sign(&(rsa, hash_alg), data)
            .map_err(|error| anyhow::anyhow!("stored RSA Agent signing failed: {error}"));
    }
    Signer::try_sign(key, data)
        .map_err(|error| anyhow::anyhow!("stored Agent signing failed: {error}"))
}

fn hash_algorithm(flags: u32) -> Option<HashAlg> {
    match flags {
        0 => None,
        2 => Some(HashAlg::Sha256),
        4 => Some(HashAlg::Sha512),
        _ => None,
    }
}

fn policy_allows(policy: &SshAgentForwardingPolicy, fingerprint: &str) -> bool {
    match policy {
        SshAgentForwardingPolicy::All => true,
        SshAgentForwardingPolicy::Allowlist { fingerprints } => {
            fingerprints.iter().any(|value| value == fingerprint)
        }
    }
}

impl IdentityRecord {
    fn preview(&self) -> SshAgentIdentityPreview {
        let (source, custom_endpoint_index) = match &self.source {
            IdentitySource::External { endpoint_index, .. } => {
                ("external_agent".to_string(), Some(*endpoint_index))
            }
            IdentitySource::Stored { .. } => ("stored_key".to_string(), None),
        };
        SshAgentIdentityPreview {
            fingerprint: self.fingerprint.clone(),
            comment: self.comment.clone(),
            source,
            custom_endpoint_index,
        }
    }
}

fn endpoint_error(
    custom_endpoint_index: usize,
    endpoint: &SshAgentEndpoint,
    identity_enumeration: bool,
) -> SshAgentEndpointPreviewError {
    SshAgentEndpointPreviewError {
        custom_endpoint_index,
        endpoint_type: endpoint_type(endpoint).to_string(),
        code: if identity_enumeration {
            SshAgentEndpointPreviewErrorCode::IdentityEnumerationFailed
        } else {
            SshAgentEndpointPreviewErrorCode::ConnectFailed
        },
    }
}

fn endpoint_type(endpoint: &SshAgentEndpoint) -> &'static str {
    match endpoint {
        SshAgentEndpoint::Auto => "auto",
        SshAgentEndpoint::Environment { .. } => "environment",
        SshAgentEndpoint::UnixSocket { .. } => "unix_socket",
        SshAgentEndpoint::Pageant => "pageant",
        SshAgentEndpoint::WindowsOpenSsh => "windows_open_ssh",
    }
}

fn broker_endpoint_key(endpoint: &SshAgentEndpoint) -> String {
    match endpoint {
        SshAgentEndpoint::Auto if cfg!(unix) => "environment:SSH_AUTH_SOCK".to_string(),
        SshAgentEndpoint::Auto => "auto".to_string(),
        SshAgentEndpoint::Environment { variable } => {
            format!(
                "environment:{}",
                variable.trim().trim_start_matches('$').trim()
            )
        }
        SshAgentEndpoint::UnixSocket { path } => format!("unix_socket:{path}"),
        SshAgentEndpoint::Pageant => "pageant".to_string(),
        SshAgentEndpoint::WindowsOpenSsh => "windows_open_ssh".to_string(),
    }
}

fn fingerprint(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

async fn fingerprint_lock(fingerprint: &str) -> Arc<Mutex<()>> {
    let locks = FINGERPRINT_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().await;
    if let Some(lock) = locks.get(fingerprint) {
        return lock.clone();
    }
    if locks.len() >= MAX_IDENTITIES {
        return locks
            .entry("__overflow__".to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(fingerprint.to_string(), lock.clone());
    lock
}

async fn read_frame<S>(stream: &mut S) -> anyhow::Result<Option<Vec<u8>>>
where
    S: AsyncRead + Unpin,
{
    let mut length = [0u8; 4];
    match stream.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(length) as usize;
    anyhow::ensure!(
        length <= MAX_AGENT_FRAME_LEN,
        "SSH Agent frame is too large"
    );
    let mut frame = vec![0u8; length];
    stream.read_exact(&mut frame).await?;
    Ok(Some(frame))
}

async fn write_frame<S>(stream: &mut S, frame: &[u8]) -> anyhow::Result<()>
where
    S: AsyncWrite + Unpin,
{
    anyhow::ensure!(
        frame.len() <= MAX_AGENT_FRAME_LEN,
        "SSH Agent response is too large"
    );
    stream
        .write_all(&(frame.len() as u32).to_be_bytes())
        .await?;
    stream.write_all(frame).await?;
    stream.flush().await?;
    Ok(())
}

fn encode_identities_response(state: &BrokerState) -> (Vec<u8>, bool) {
    let mut response = vec![12];
    let mut count = 0u32;
    let count_offset = response.len();
    response.extend_from_slice(&[0; 4]);
    let mut truncated = false;
    for identity in &state.identities {
        let mut candidate = Vec::new();
        if put_string(&mut candidate, &identity.blob).is_err()
            || put_string(&mut candidate, identity.comment.as_bytes()).is_err()
            || response.len() + candidate.len() > MAX_AGENT_FRAME_LEN
        {
            truncated = true;
            break;
        }
        response.extend_from_slice(&candidate);
        count += 1;
    }
    response[count_offset..count_offset + 4].copy_from_slice(&count.to_be_bytes());
    (response, truncated)
}

fn encode_failure() -> Vec<u8> {
    vec![5]
}

fn encode_extension_failure() -> Vec<u8> {
    vec![28]
}

fn bounded_agent_comment(comment: &str) -> String {
    if comment.len() <= MAX_AGENT_COMMENT_LEN {
        return comment.to_string();
    }
    let mut end = MAX_AGENT_COMMENT_LEN;
    while !comment.is_char_boundary(end) {
        end -= 1;
    }
    comment[..end].to_string()
}

fn read_u32(payload: &mut &[u8]) -> anyhow::Result<u32> {
    anyhow::ensure!(payload.len() >= 4, "malformed SSH Agent integer");
    let (head, tail) = payload.split_at(4);
    *payload = tail;
    Ok(u32::from_be_bytes(head.try_into().expect("length checked")))
}

fn read_u8(payload: &mut &[u8]) -> anyhow::Result<u8> {
    anyhow::ensure!(!payload.is_empty(), "malformed SSH Agent byte");
    let (head, tail) = payload.split_at(1);
    *payload = tail;
    Ok(head[0])
}

fn read_string(payload: &mut &[u8]) -> anyhow::Result<Vec<u8>> {
    let length = read_u32(payload)? as usize;
    anyhow::ensure!(
        length <= MAX_AGENT_FRAME_LEN && length <= payload.len(),
        "malformed SSH Agent string"
    );
    let (value, tail) = payload.split_at(length);
    *payload = tail;
    Ok(value.to_vec())
}

fn put_string(output: &mut Vec<u8>, value: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        value.len() <= u32::MAX as usize,
        "SSH Agent string is too large"
    );
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BrokerState, ExternalUpstream, IdentityRecord, IdentitySource, bounded_agent_comment,
        collect_identities, encode_extension_failure, extension_request, fingerprint,
        hash_algorithm, policy_allows, put_string, read_frame, relay_raw_channel_inner,
        serve_channel_inner, sign_request, write_frame,
    };
    use crate::{
        ShellEnvironmentCache, SshAgentEndpoint, SshAgentForwardingConfig,
        SshAgentForwardingPolicy, SshAgentForwardingSources, SshAgentStoredKey,
        SshAgentStoredKeyProvider, SshAgentStoredKeySnapshot,
    };
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Semaphore;

    struct CountingStoredKeyProvider {
        loads: Arc<AtomicUsize>,
    }

    impl SshAgentStoredKeyProvider for CountingStoredKeyProvider {
        fn revision(&self) -> Result<u64, String> {
            Ok(1)
        }

        fn load_snapshot(&self) -> Result<SshAgentStoredKeySnapshot, String> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(SshAgentStoredKeySnapshot {
                revision: 1,
                keys: vec![SshAgentStoredKey {
                    key_data: "not a private key".to_string(),
                    cert_data: None,
                    passphrase: None,
                    comment: "invalid test key".to_string(),
                }],
            })
        }
    }

    #[test]
    fn fingerprint_uses_openssh_sha256_shape_without_padding() {
        let value = fingerprint(b"agent identity blob");
        assert!(value.starts_with("SHA256:"));
        assert!(!value.contains('='));
    }

    #[test]
    fn unknown_rsa_signature_flags_are_not_mapped_to_legacy_sha1() {
        assert_eq!(hash_algorithm(0), None);
        assert!(hash_algorithm(2).is_some());
        assert!(hash_algorithm(4).is_some());
        assert_eq!(hash_algorithm(8), None);
    }

    #[test]
    fn allowlist_policy_matches_only_exact_fingerprints() {
        let policy = SshAgentForwardingPolicy::Allowlist {
            fingerprints: vec!["SHA256:allowed".to_string()],
        };
        assert!(policy_allows(&policy, "SHA256:allowed"));
        assert!(!policy_allows(&policy, "SHA256:other"));
        assert!(policy_allows(&SshAgentForwardingPolicy::All, "SHA256:any"));
    }

    #[test]
    fn comments_are_bounded_without_splitting_utf8() {
        let comment = "密钥".repeat(3000);
        let bounded = bounded_agent_comment(&comment);
        assert!(bounded.len() <= super::MAX_AGENT_COMMENT_LEN);
        assert!(bounded.is_char_boundary(bounded.len()));
    }

    #[test]
    fn extension_requests_return_the_agent_extension_failure_type() {
        assert_eq!(encode_extension_failure(), vec![28]);
    }

    #[test]
    fn identity_snapshot_stops_at_the_shared_protocol_frame_limit() {
        let mut state = BrokerState::default();
        let mut seen = HashSet::new();
        for index in 0..super::MAX_IDENTITIES {
            let mut blob = vec![0; 512];
            blob[..8].copy_from_slice(&(index as u64).to_be_bytes());
            state.push_identity(
                &mut seen,
                IdentityRecord {
                    blob,
                    fingerprint: format!("SHA256:{index}"),
                    comment: "comment".to_string(),
                    source: IdentitySource::External {
                        upstream_index: 0,
                        endpoint_index: 0,
                    },
                },
            );
            if state.truncated {
                break;
            }
        }
        assert!(state.truncated);
        assert!(state.encoded_identities_len <= super::MAX_AGENT_FRAME_LEN);
    }

    #[tokio::test]
    async fn session_bind_and_sign_share_the_enumerated_upstream_connection() {
        let blob = b"public-key-blob".to_vec();
        let expected_blob = blob.clone();
        let (client, mut server) = tokio::io::duplex(4096);
        let agent = tokio::spawn(async move {
            assert_eq!(read_frame(&mut server).await.unwrap().unwrap(), vec![11]);
            let mut identities = vec![12];
            identities.extend_from_slice(&1u32.to_be_bytes());
            put_string(&mut identities, &expected_blob).unwrap();
            put_string(&mut identities, b"test key").unwrap();
            write_frame(&mut server, &identities).await.unwrap();

            let extension = read_frame(&mut server).await.unwrap().unwrap();
            assert_eq!(extension.first(), Some(&27));
            write_frame(&mut server, &[6]).await.unwrap();

            let sign = read_frame(&mut server).await.unwrap().unwrap();
            assert_eq!(sign.first(), Some(&13));
            let mut response = vec![14];
            put_string(&mut response, b"signature").unwrap();
            write_frame(&mut server, &response).await.unwrap();
        });

        let fingerprint = fingerprint(&blob);
        let config = SshAgentForwardingConfig {
            enabled: true,
            sources: SshAgentForwardingSources {
                external_agent: true,
                external_agent_endpoints: vec![SshAgentEndpoint::Auto],
                stored_keys: false,
            },
            policy: SshAgentForwardingPolicy::Allowlist {
                fingerprints: vec![fingerprint],
            },
        };
        let mut external = vec![ExternalUpstream {
            endpoint: SshAgentEndpoint::Auto,
            endpoint_index: 0,
            stream: Box::new(client),
            healthy: true,
        }];
        let state = collect_identities(&config, None, &mut external, true).await;
        assert_eq!(state.identities.len(), 1);

        let mut extension = Vec::new();
        put_string(&mut extension, super::SESSION_BIND_EXTENSION).unwrap();
        put_string(&mut extension, b"host-key").unwrap();
        put_string(&mut extension, b"session-id").unwrap();
        put_string(&mut extension, b"binding-signature").unwrap();
        extension.push(0);
        assert_eq!(
            extension_request(&extension, &config, &state, &mut external)
                .await
                .unwrap(),
            vec![6]
        );

        let mut sign = Vec::new();
        put_string(&mut sign, &blob).unwrap();
        put_string(&mut sign, b"data").unwrap();
        sign.extend_from_slice(&0u32.to_be_bytes());
        let response = sign_request(&sign, &state, &config, None, &mut external)
            .await
            .unwrap();
        assert_eq!(response.first(), Some(&14));
        agent.await.unwrap();
    }

    #[tokio::test]
    async fn raw_relay_releases_its_permit_after_first_frame_timeout() {
        let (_channel_peer, channel) = tokio::io::duplex(64);
        let (_agent_peer, agent) = tokio::io::duplex(64);
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();

        let result = relay_raw_channel_inner(
            channel,
            agent,
            permit,
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn repeated_identity_requests_reuse_one_stored_key_snapshot() {
        let loads = Arc::new(AtomicUsize::new(0));
        let provider: Arc<dyn SshAgentStoredKeyProvider> = Arc::new(CountingStoredKeyProvider {
            loads: loads.clone(),
        });
        let config = SshAgentForwardingConfig {
            enabled: true,
            sources: SshAgentForwardingSources {
                external_agent: false,
                external_agent_endpoints: Vec::new(),
                stored_keys: true,
            },
            policy: SshAgentForwardingPolicy::All,
        };
        let (mut client, server) = tokio::io::duplex(4096);
        let permit = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
        let broker = tokio::spawn(serve_channel_inner(
            server,
            config,
            Some(provider),
            ShellEnvironmentCache::global(),
            permit,
        ));

        write_frame(&mut client, &[11]).await.unwrap();
        assert_eq!(
            read_frame(&mut client).await.unwrap().unwrap(),
            vec![12, 0, 0, 0, 0]
        );
        write_frame(&mut client, &[11]).await.unwrap();
        assert_eq!(
            read_frame(&mut client).await.unwrap().unwrap(),
            vec![12, 0, 0, 0, 0]
        );
        drop(client);

        broker.await.unwrap().unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }
}
