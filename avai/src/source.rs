use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

#[cfg(unix)]
use base::net::{
    transport::MessageTransport,
    uds::{ManagedUnixStream, UnixTransportConfig},
};
use base::{
    base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
    bytes::{Bytes, BytesMut},
    futures::StreamExt,
    sha2::{Digest, Sha256},
    utils::rt::GlobalRuntime,
};
use gmv_protocol::{
    avai::v1::{ImageMetadata, SourceSpec, source_spec},
    common::v1::NodeIdentity,
    session::v1::{ReadGrantedImageRequest, ReadGrantedImageResponse},
};
use prost::Message;
use reqwest::{StatusCode, header};
use url::{Host, Url};

const DEFAULT_MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;

#[derive(Debug, Clone)]
pub struct SourcePolicy {
    pub max_image_bytes: usize,
    pub request_timeout: Duration,
    pub allow_private_image_urls: bool,
    pub allowed_internal_hosts: HashSet<String>,
    pub object_root: PathBuf,
    pub uds_socket_root: PathBuf,
}

impl Default for SourcePolicy {
    fn default() -> Self {
        Self {
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
            request_timeout: Duration::from_secs(10),
            allow_private_image_urls: false,
            allowed_internal_hosts: HashSet::new(),
            object_root: PathBuf::from("./data/objects"),
            uds_socket_root: PathBuf::from("./run"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedImage {
    pub bytes: Bytes,
    pub content_type: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub source_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub code: &'static str,
    pub message: String,
}

impl SourceError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone)]
pub struct SourceResolver {
    identity: NodeIdentity,
    policy: SourcePolicy,
    runtime: GlobalRuntime,
}

impl SourceResolver {
    pub fn new(
        identity: NodeIdentity,
        policy: SourcePolicy,
        runtime: &GlobalRuntime,
    ) -> Result<Self, SourceError> {
        if policy.max_image_bytes == 0 {
            return Err(SourceError::new(
                "invalid_source_policy",
                "max_image_bytes must be greater than zero",
            ));
        }
        std::fs::create_dir_all(&policy.object_root).map_err(|_| {
            SourceError::new(
                "invalid_source_policy",
                "Avai object root cannot be created",
            )
        })?;
        let mut policy = policy;
        policy.object_root = policy.object_root.canonicalize().map_err(|_| {
            SourceError::new(
                "invalid_source_policy",
                "Avai object root cannot be resolved",
            )
        })?;
        std::fs::create_dir_all(&policy.uds_socket_root).map_err(|_| {
            SourceError::new(
                "invalid_source_policy",
                "Avai UDS socket root cannot be created",
            )
        })?;
        policy.uds_socket_root = policy.uds_socket_root.canonicalize().map_err(|_| {
            SourceError::new(
                "invalid_source_policy",
                "Avai UDS socket root cannot be resolved",
            )
        })?;
        Ok(Self {
            identity,
            policy,
            runtime: runtime.clone(),
        })
    }

    pub async fn resolve(
        &self,
        source: &SourceSpec,
        capability: &str,
        now_epoch_ms: i64,
    ) -> Result<ResolvedImage, SourceError> {
        match source.source.as_ref() {
            Some(source_spec::Source::ImageUrl(source)) => {
                let requested_max = usize::try_from(source.max_bytes)
                    .unwrap_or(usize::MAX)
                    .min(self.policy.max_image_bytes);
                let max_bytes = if requested_max == 0 {
                    self.policy.max_image_bytes
                } else {
                    requested_max
                };
                let fetched = fetch_url(
                    &source.url,
                    max_bytes,
                    self.policy.request_timeout,
                    self.policy.allow_private_image_urls,
                    None,
                )
                .await?;
                validate_image(
                    fetched,
                    source.expected.as_ref(),
                    format!("url:{}", stable_url_identity(&source.url)?),
                )
            }
            Some(source_spec::Source::OwnedImage(source)) => {
                let owner = source.owner.as_ref().ok_or_else(|| {
                    SourceError::new("invalid_source", "owned image has no owner identity")
                })?;
                let resource = source.resource.as_ref().ok_or_else(|| {
                    SourceError::new("invalid_source", "owned image has no resource identity")
                })?;
                let grant = source.access.as_ref().ok_or_else(|| {
                    SourceError::new("source_fetch_denied", "owned image has no access grant")
                })?;
                validate_grant(grant, &self.identity, capability, now_epoch_ms)?;

                if owner.node_id == self.identity.node_id
                    && owner.instance_id == self.identity.instance_id
                    && grant.endpoints.iter().any(|endpoint| {
                        endpoint.uri == format!("gmv-object://{}", resource.resource_id)
                    })
                {
                    let fetched = read_local_object(
                        &self.policy.object_root,
                        &resource.resource_id,
                        self.policy.max_image_bytes,
                    )
                    .await?;
                    return validate_image(
                        fetched,
                        source.metadata.as_ref(),
                        format!(
                            "owned:{}/{}/{}",
                            owner.node_id, resource.resource_type, resource.resource_id
                        ),
                    );
                }

                #[cfg(unix)]
                if let Some(endpoint) = grant
                    .endpoints
                    .iter()
                    .find(|endpoint| endpoint.uri.starts_with("unix://"))
                {
                    let fetched = read_granted_uds(
                        endpoint,
                        grant,
                        &resource.resource_id,
                        &self.policy,
                        &self.runtime,
                    )
                    .await?;
                    return validate_image(
                        fetched,
                        source.metadata.as_ref(),
                        format!(
                            "owned:{}/{}/{}",
                            owner.node_id, resource.resource_type, resource.resource_id
                        ),
                    );
                }

                #[cfg(not(unix))]
                if grant
                    .endpoints
                    .iter()
                    .any(|endpoint| endpoint.uri.starts_with("unix://"))
                {
                    return Err(SourceError::new(
                        "source_transport_unsupported",
                        "owned image UDS endpoint is unsupported on this platform",
                    ));
                }

                let endpoint = grant.endpoints.iter().find(|endpoint| {
                    endpoint.uri.starts_with("https://") || endpoint.uri.starts_with("http://")
                });
                let endpoint = endpoint.ok_or_else(|| {
                    SourceError::new(
                        "source_transport_unsupported",
                        "owned image grant has no supported HTTP endpoint",
                    )
                })?;
                let parsed = Url::parse(&endpoint.uri).map_err(|_| {
                    SourceError::new("invalid_source", "owned image endpoint is not a valid URL")
                })?;
                let host = parsed.host_str().ok_or_else(|| {
                    SourceError::new("invalid_source", "owned image endpoint has no host")
                })?;
                if !self.policy.allowed_internal_hosts.contains(host) {
                    return Err(SourceError::new(
                        "source_fetch_denied",
                        "owned image endpoint host is not allowlisted",
                    ));
                }
                let proof = URL_SAFE_NO_PAD.encode(&grant.proof);
                let fetched = fetch_url(
                    parsed.as_str(),
                    self.policy.max_image_bytes,
                    self.policy.request_timeout,
                    true,
                    Some(("x-gmv-access-proof", proof.as_str())),
                )
                .await?;
                validate_image(
                    fetched,
                    source.metadata.as_ref(),
                    format!(
                        "owned:{}/{}/{}",
                        owner.node_id, resource.resource_type, resource.resource_id
                    ),
                )
            }
            Some(source_spec::Source::StreamFrame(_)) => Err(SourceError::new(
                "source_transport_unsupported",
                "stream frame source is not enabled in this release",
            )),
            None => Err(SourceError::new(
                "invalid_source",
                "task source is required",
            )),
        }
    }
}

#[cfg(unix)]
async fn read_granted_uds(
    endpoint: &gmv_protocol::common::v1::DataEndpoint,
    grant: &gmv_protocol::common::v1::AccessGrant,
    image_id: &str,
    policy: &SourcePolicy,
    runtime: &GlobalRuntime,
) -> Result<FetchedBody, SourceError> {
    let path = endpoint
        .uri
        .strip_prefix("unix://")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| SourceError::new("invalid_source", "owned image UDS endpoint is invalid"))?;
    let advertised_max = endpoint
        .capabilities
        .as_ref()
        .and_then(|capabilities| usize::try_from(capabilities.max_message_size).ok())
        .filter(|size| *size > 0)
        .unwrap_or(policy.max_image_bytes.saturating_add(64 * 1024));
    let mut config = UnixTransportConfig::new(&policy.uds_socket_root, path);
    config.max_message_size = advertised_max.min(policy.max_image_bytes.saturating_add(64 * 1024));
    let connection = ManagedUnixStream::connect(
        config,
        runtime,
        format!("avai-image-source-uds-{}", grant.grant_id),
    )
    .await
    .map_err(|error| {
        SourceError::new(
            "source_transport_unavailable",
            format!("owned image UDS connection failed: {error}"),
        )
    })?;
    let request = ReadGrantedImageRequest {
        grant_id: grant.grant_id.clone(),
        proof: grant.proof.clone(),
        image_id: image_id.to_string(),
    };
    let exchanged = base::tokio::time::timeout(policy.request_timeout, async {
        connection
            .send(Bytes::from(request.encode_to_vec()))
            .await
            .map_err(|error| {
                SourceError::new(
                    "source_transport_unavailable",
                    format!("owned image UDS request failed: {error}"),
                )
            })?;
        let message = connection.receive().await.map_err(|error| {
            SourceError::new(
                "source_transport_unavailable",
                format!("owned image UDS response failed: {error}"),
            )
        })?;
        ReadGrantedImageResponse::decode(message.payload).map_err(|_| {
            SourceError::new(
                "source_transport_invalid",
                "owned image UDS response is invalid",
            )
        })
    })
    .await
    .map_err(|_| {
        SourceError::new(
            "source_transport_timeout",
            "owned image UDS request timed out",
        )
    });
    let close_result = connection.close_and_wait().await;
    if let Err(error) = close_result {
        base::log::debug!("close owned image UDS connection failed: {error}");
    }
    let response = exchanged??;
    if let Some(error) = response.error {
        return Err(SourceError::new(
            "source_fetch_denied",
            format!("owned image source rejected: {}", error.code),
        ));
    }
    if response.image.len() > policy.max_image_bytes {
        return Err(SourceError::new(
            "source_too_large",
            "owned image exceeds the configured maximum",
        ));
    }
    Ok(FetchedBody {
        bytes: Bytes::from(response.image),
        content_type: (!response.content_type.is_empty()).then_some(response.content_type),
    })
}

async fn read_local_object(
    object_root: &std::path::Path,
    resource_id: &str,
    max_bytes: usize,
) -> Result<FetchedBody, SourceError> {
    if resource_id.is_empty()
        || resource_id.contains('/')
        || resource_id.contains('\\')
        || resource_id == "."
        || resource_id == ".."
    {
        return Err(SourceError::new(
            "invalid_source",
            "object resource identity is invalid",
        ));
    }
    let path = object_root.join(resource_id);
    let canonical = path
        .canonicalize()
        .map_err(|_| SourceError::new("source_fetch_denied", "object resource does not exist"))?;
    if !canonical.starts_with(object_root) {
        return Err(SourceError::new(
            "source_fetch_denied",
            "object resource is outside the configured root",
        ));
    }
    let metadata = std::fs::metadata(&canonical)
        .map_err(|_| SourceError::new("source_fetch_denied", "object metadata is unavailable"))?;
    if !metadata.is_file() || metadata.len() > max_bytes as u64 {
        return Err(SourceError::new(
            "source_too_large",
            "object is not a bounded regular image file",
        ));
    }
    let bytes = base::tokio::fs::read(canonical)
        .await
        .map_err(|_| SourceError::new("source_fetch_denied", "object read failed"))?;
    if bytes.len() > max_bytes {
        return Err(SourceError::new(
            "source_too_large",
            "object exceeds the configured maximum",
        ));
    }
    Ok(FetchedBody {
        bytes: Bytes::from(bytes),
        content_type: None,
    })
}

struct FetchedBody {
    bytes: Bytes,
    content_type: Option<String>,
}

async fn fetch_url(
    input: &str,
    max_bytes: usize,
    timeout: Duration,
    allow_private: bool,
    extra_header: Option<(&str, &str)>,
) -> Result<FetchedBody, SourceError> {
    let mut current = Url::parse(input)
        .map_err(|_| SourceError::new("invalid_source", "image URL is invalid"))?;
    for redirect_count in 0..=MAX_REDIRECTS {
        let (host, resolved) = validate_network_target(&current, allow_private).await?;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout)
            .timeout(timeout);
        for address in resolved {
            builder = builder.resolve(&host, address);
        }
        let client = builder.build().map_err(|_| {
            SourceError::new(
                "source_fetch_denied",
                "image fetch client initialization failed",
            )
        })?;
        let mut request = client.get(current.clone());
        if let Some((name, value)) = extra_header {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|_| SourceError::new("source_fetch_denied", "image fetch request failed"))?;
        if response.status().is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err(SourceError::new(
                    "source_fetch_denied",
                    "image URL exceeded redirect limit",
                ));
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    SourceError::new(
                        "source_fetch_denied",
                        "image redirect has no valid location",
                    )
                })?;
            let next = current.join(location).map_err(|_| {
                SourceError::new("source_fetch_denied", "image redirect URL is invalid")
            })?;
            if extra_header.is_some() && !same_origin(&current, &next) {
                return Err(SourceError::new(
                    "source_fetch_denied",
                    "credentialed image fetch cannot redirect to another origin",
                ));
            }
            current = next;
            continue;
        }
        if response.status() != StatusCode::OK {
            return Err(SourceError::new(
                "source_fetch_denied",
                format!("image owner returned HTTP status {}", response.status()),
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(SourceError::new(
                "source_too_large",
                "image content length exceeds the configured maximum",
            ));
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
        let mut body = BytesMut::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| {
                SourceError::new("source_fetch_denied", "image body transfer failed")
            })?;
            if body.len().saturating_add(chunk.len()) > max_bytes {
                return Err(SourceError::new(
                    "source_too_large",
                    "image body exceeds the configured maximum",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        return Ok(FetchedBody {
            bytes: body.freeze(),
            content_type,
        });
    }
    unreachable!("redirect loop has an explicit upper bound")
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host() == right.host()
        && left.port_or_known_default() == right.port_or_known_default()
}

async fn validate_network_target(
    url: &Url,
    allow_private: bool,
) -> Result<(String, Vec<SocketAddr>), SourceError> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(SourceError::new(
            "source_fetch_denied",
            "only credential-free HTTP(S) image URLs are allowed",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| SourceError::new("source_fetch_denied", "image URL has no network host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| SourceError::new("source_fetch_denied", "image URL has no valid port"))?;
    let addresses = match url.host() {
        Some(Host::Ipv4(address)) => vec![SocketAddr::new(IpAddr::V4(address), port)],
        Some(Host::Ipv6(address)) => vec![SocketAddr::new(IpAddr::V6(address), port)],
        Some(Host::Domain(domain)) => base::tokio::net::lookup_host((domain, port))
            .await
            .map_err(|_| {
                SourceError::new("source_fetch_denied", "image URL host cannot be resolved")
            })?
            .collect(),
        None => Vec::new(),
    };
    if addresses.is_empty()
        || (!allow_private
            && addresses
                .iter()
                .any(|address| !is_public_address(address.ip())))
    {
        return Err(SourceError::new(
            "source_fetch_denied",
            "image URL resolves to a non-public network",
        ));
    }
    Ok((host.to_string(), addresses))
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !address.is_private()
        && !address.is_loopback()
        && !address.is_link_local()
        && !address.is_multicast()
        && !address.is_unspecified()
        && address != Ipv4Addr::BROADCAST
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 192 && b == 0 && c == 2)
        && !(a == 198 && (b == 18 || b == 19))
        && !(a == 198 && b == 51 && c == 100)
        && !(a == 203 && b == 0 && c == 113)
        && a != 0
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(address) = address.to_ipv4_mapped() {
        return is_public_ipv4(address);
    }
    let segments = address.segments();
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

fn validate_grant(
    grant: &gmv_protocol::common::v1::AccessGrant,
    identity: &NodeIdentity,
    capability: &str,
    now_epoch_ms: i64,
) -> Result<(), SourceError> {
    let expected = grant.expected_consumer.as_ref().ok_or_else(|| {
        SourceError::new(
            "source_fetch_denied",
            "access grant has no expected consumer",
        )
    })?;
    if expected.node_id != identity.node_id || expected.instance_id != identity.instance_id {
        return Err(SourceError::new(
            "source_fetch_denied",
            "access grant is bound to another Avai instance",
        ));
    }
    if grant.expires_at_epoch_ms <= now_epoch_ms {
        return Err(SourceError::new(
            "source_expired",
            "access grant has expired",
        ));
    }
    if grant.purpose != capability {
        return Err(SourceError::new(
            "source_fetch_denied",
            "access grant purpose does not match task capability",
        ));
    }
    if grant.proof.is_empty() {
        return Err(SourceError::new(
            "source_fetch_denied",
            "access grant proof is empty",
        ));
    }
    Ok(())
}

fn validate_image(
    fetched: FetchedBody,
    expected: Option<&ImageMetadata>,
    source_identity: String,
) -> Result<ResolvedImage, SourceError> {
    let format = image::guess_format(&fetched.bytes).map_err(|_| {
        SourceError::new(
            "source_decode_failed",
            "source body is not a supported image",
        )
    })?;
    let content_type = match format {
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::WebP => "image/webp",
        _ => {
            return Err(SourceError::new(
                "source_decode_failed",
                "only JPEG, PNG and WebP images are supported",
            ));
        }
    }
    .to_string();
    if fetched
        .content_type
        .as_deref()
        .is_some_and(|declared| declared != content_type)
    {
        return Err(SourceError::new(
            "source_decode_failed",
            "declared content type does not match image bytes",
        ));
    }
    let decoded = image::load_from_memory_with_format(&fetched.bytes, format)
        .map_err(|_| SourceError::new("source_decode_failed", "image decoding failed"))?;
    let sha256 = format!("{:x}", Sha256::digest(&fetched.bytes));
    if let Some(expected) = expected {
        if expected.size_bytes != 0 && expected.size_bytes != fetched.bytes.len() as u64 {
            return Err(SourceError::new(
                "source_size_mismatch",
                "image size does not match source metadata",
            ));
        }
        if !expected.content_type.is_empty() && expected.content_type != content_type {
            return Err(SourceError::new(
                "source_type_mismatch",
                "image content type does not match source metadata",
            ));
        }
        if !expected.sha256.is_empty() && !expected.sha256.eq_ignore_ascii_case(&sha256) {
            return Err(SourceError::new(
                "source_hash_mismatch",
                "image hash does not match source metadata",
            ));
        }
        if expected.width != 0 && expected.width != decoded.width()
            || expected.height != 0 && expected.height != decoded.height()
        {
            return Err(SourceError::new(
                "source_dimensions_mismatch",
                "image dimensions do not match source metadata",
            ));
        }
    }
    Ok(ResolvedImage {
        bytes: fetched.bytes,
        content_type,
        sha256,
        width: decoded.width(),
        height: decoded.height(),
        source_identity,
    })
}

fn stable_url_identity(input: &str) -> Result<String, SourceError> {
    let mut url = Url::parse(input)
        .map_err(|_| SourceError::new("invalid_source", "image URL is invalid"))?;
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gmv_protocol::common::v1::{AccessGrant, NodeKind};

    #[test]
    fn public_address_filter_rejects_internal_and_documentation_networks() {
        assert!(!is_public_address("127.0.0.1".parse().unwrap()));
        assert!(!is_public_address("10.0.0.1".parse().unwrap()));
        assert!(!is_public_address("169.254.1.1".parse().unwrap()));
        assert!(!is_public_address("192.0.2.1".parse().unwrap()));
        assert!(!is_public_address("2001:db8::1".parse().unwrap()));
        assert!(is_public_address("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn url_identity_does_not_persist_sensitive_query_or_fragment() {
        assert_eq!(
            stable_url_identity("https://example.com/image.jpg?token=secret#x").unwrap(),
            "https://example.com/image.jpg"
        );
    }

    #[test]
    fn credentialed_redirect_origin_comparison_includes_scheme_host_and_port() {
        let source = Url::parse("https://session.internal:8443/image/1").unwrap();
        assert!(same_origin(
            &source,
            &Url::parse("https://session.internal:8443/image/2").unwrap()
        ));
        assert!(!same_origin(
            &source,
            &Url::parse("https://other.internal:8443/image/2").unwrap()
        ));
        assert!(!same_origin(
            &source,
            &Url::parse("http://session.internal:8443/image/2").unwrap()
        ));
        assert!(!same_origin(
            &source,
            &Url::parse("https://session.internal:9443/image/2").unwrap()
        ));
    }

    #[test]
    fn grant_rejects_expired_wrong_audience_and_wrong_purpose() {
        let identity = NodeIdentity {
            node_id: "avai-1".to_string(),
            instance_id: "instance-1".to_string(),
            kind: NodeKind::Avai as i32,
        };
        let grant = |expected: NodeIdentity, purpose: &str, expires_at_epoch_ms| AccessGrant {
            grant_id: "grant-1".to_string(),
            expected_consumer: Some(expected),
            purpose: purpose.to_string(),
            expires_at_epoch_ms,
            endpoints: Vec::new(),
            proof: vec![1],
        };
        assert_eq!(
            validate_grant(
                &grant(identity.clone(), "capability", 99),
                &identity,
                "capability",
                100
            )
            .unwrap_err()
            .code,
            "source_expired"
        );
        let mut other = identity.clone();
        other.instance_id = "instance-2".to_string();
        assert_eq!(
            validate_grant(
                &grant(other, "capability", 101),
                &identity,
                "capability",
                100
            )
            .unwrap_err()
            .code,
            "source_fetch_denied"
        );
        assert_eq!(
            validate_grant(
                &grant(identity.clone(), "other", 101),
                &identity,
                "capability",
                100
            )
            .unwrap_err()
            .code,
            "source_fetch_denied"
        );
    }

    #[test]
    fn image_validation_rejects_metadata_mismatch() {
        use base::base64::Engine;
        let bytes = base::base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap();
        let fetched = || FetchedBody {
            bytes: Bytes::from(bytes.clone()),
            content_type: Some("image/png".to_string()),
        };
        let wrong_hash = ImageMetadata {
            sha256: "wrong".to_string(),
            ..ImageMetadata::default()
        };
        assert_eq!(
            validate_image(fetched(), Some(&wrong_hash), "test".to_string())
                .unwrap_err()
                .code,
            "source_hash_mismatch"
        );
        let wrong_size = ImageMetadata {
            size_bytes: bytes.len() as u64 + 1,
            ..ImageMetadata::default()
        };
        assert_eq!(
            validate_image(fetched(), Some(&wrong_size), "test".to_string())
                .unwrap_err()
                .code,
            "source_size_mismatch"
        );
    }
}
