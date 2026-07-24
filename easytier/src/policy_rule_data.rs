use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, ensure};
use easytier_policy::{
    ManagedRuleDataKind, RuleSet, RuleSetKind, list_managed_rule_data_categories,
    validate_managed_rule_data,
};
use reqwest::{
    Url,
    blocking::Client,
    redirect::{Action, Attempt, Policy},
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MAX_RULE_DATA_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RULE_DATA_REDIRECTS: usize = 5;
const RULE_DATA_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const BUILTIN_SNAPSHOT: &str = "metacubex-4178770b";
const BUILTIN_GEOSITE_SHA256: &str =
    "0f464192b311ee9b8a2cdc309118928c532b6b5982b486c6a42060db671e3038";
const BUILTIN_GEOIP_SHA256: &str =
    "cba612b84b6c023ad2ec110b57c04c88c6ac888935963279b00884731af53301";
const BUILTIN_GEOSITE: &[u8] = include_bytes!("../resources/policy-rule-data/geosite.dat");
const BUILTIN_GEOIP: &[u8] = include_bytes!("../resources/policy-rule-data/geoip-lite.dat");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PolicyRuleDataResource {
    Geosite,
    Geoip,
    CountryMmdb,
}

impl PolicyRuleDataResource {
    fn name(self) -> &'static str {
        match self {
            Self::Geosite => "geosite",
            Self::Geoip => "geoip",
            Self::CountryMmdb => "mmdb",
        }
    }

    fn default_source_url(self) -> &'static str {
        match self {
            Self::Geosite => {
                "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geosite.dat"
            }
            Self::Geoip => {
                "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geoip-lite.dat"
            }
            Self::CountryMmdb => {
                "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/country-lite.mmdb"
            }
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Geosite => "geosite.dat",
            Self::Geoip => "geoip-lite.dat",
            Self::CountryMmdb => "country-lite.mmdb",
        }
    }

    fn validate(self, path: &Path) -> anyhow::Result<()> {
        match self {
            Self::Geosite => validate_managed_rule_data(ManagedRuleDataKind::Geosite, path)
                .map_err(|error| anyhow::anyhow!(error.to_string())),
            Self::Geoip => validate_managed_rule_data(ManagedRuleDataKind::Geoip, path)
                .map_err(|error| anyhow::anyhow!(error.to_string())),
            Self::CountryMmdb => validate_managed_rule_data(ManagedRuleDataKind::CountryMmdb, path)
                .map_err(|error| anyhow::anyhow!(error.to_string())),
        }
    }

    fn builtin(self) -> Option<(&'static [u8], &'static str)> {
        match self {
            Self::Geosite => Some((BUILTIN_GEOSITE, BUILTIN_GEOSITE_SHA256)),
            Self::Geoip => Some((BUILTIN_GEOIP, BUILTIN_GEOIP_SHA256)),
            Self::CountryMmdb => None,
        }
    }

    fn rule_set_kind(self) -> Option<RuleSetKind> {
        match self {
            Self::Geosite => Some(RuleSetKind::Geosite),
            Self::Geoip => Some(RuleSetKind::Geoip),
            Self::CountryMmdb => None,
        }
    }
}

impl FromStr for PolicyRuleDataResource {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "geosite" => Ok(Self::Geosite),
            "geoip" => Ok(Self::Geoip),
            "mmdb" | "country-mmdb" => Ok(Self::CountryMmdb),
            _ => anyhow::bail!("unsupported policy rule data resource: {value}"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct PolicyRuleDataUpdate {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub source_url: String,
    pub categories: Vec<String>,
    pub updated: bool,
}

#[derive(Debug)]
pub(crate) struct PolicyRuleDataCategories {
    pub resource: String,
    pub sha256: String,
    pub size: u64,
    pub categories: Vec<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct PolicyRuleDataCategoryIndex {
    version: u32,
    size: u64,
    sha256: String,
    categories: Vec<String>,
}

const POLICY_RULE_DATA_CATEGORY_INDEX_VERSION: u32 = 1;

pub(crate) fn builtin_rule_set_default(
    base_dir: &Path,
    kind: RuleSetKind,
) -> anyhow::Result<Option<(String, RuleSet)>> {
    let resource = match kind {
        RuleSetKind::Geosite => PolicyRuleDataResource::Geosite,
        RuleSetKind::Geoip => PolicyRuleDataResource::Geoip,
        RuleSetKind::Mmdb => return Ok(None),
    };
    materialize_builtin_rule_set(base_dir, resource).map(Some)
}

fn materialize_builtin_rule_set(
    base_dir: &Path,
    resource: PolicyRuleDataResource,
) -> anyhow::Result<(String, RuleSet)> {
    let preferred_dir = base_dir
        .join(".easytier-policy-rule-data")
        .join(BUILTIN_SNAPSHOT);
    match materialize_builtin_rule_set_at(&preferred_dir, resource) {
        Ok(default) => Ok(default),
        Err(preferred_error) => {
            let fallback_dir = std::env::temp_dir()
                .join("easytier-policy-rule-data")
                .join(BUILTIN_SNAPSHOT);
            if fallback_dir == preferred_dir {
                return Err(preferred_error);
            }
            materialize_builtin_rule_set_at(&fallback_dir, resource).with_context(|| {
                format!(
                    "materializing builtin rule data under {} failed first: {preferred_error:#}",
                    preferred_dir.display()
                )
            })
        }
    }
}

fn materialize_builtin_rule_set_at(
    cache_dir: &Path,
    resource: PolicyRuleDataResource,
) -> anyhow::Result<(String, RuleSet)> {
    let path = materialize_builtin_rule_data(cache_dir, resource)?;
    let (_, sha256) = resource
        .builtin()
        .expect("only builtin resources are materialized");
    Ok((
        format!("easytier-builtin-{}", resource.name()),
        RuleSet {
            kind: resource
                .rule_set_kind()
                .expect("only builtin resources are materialized"),
            path,
            update: "manual".to_owned(),
            sha256: Some(sha256.to_owned()),
            source_url: Some(resource.default_source_url().to_owned()),
        },
    ))
}

fn materialize_builtin_rule_data(
    cache_dir: &Path,
    resource: PolicyRuleDataResource,
) -> anyhow::Result<PathBuf> {
    let (bytes, expected_sha256) = resource
        .builtin()
        .ok_or_else(|| anyhow::anyhow!("{} has no builtin snapshot", resource.name()))?;
    fs::create_dir_all(cache_dir).with_context(|| {
        format!(
            "creating builtin policy rule data directory {}",
            cache_dir.display()
        )
    })?;
    let target_path = cache_dir.join(resource.file_name());
    if target_path
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == bytes.len() as u64)
        && sha256_file(&target_path).is_ok_and(|digest| digest == expected_sha256)
    {
        return Ok(target_path);
    }

    let temporary_path = cache_dir.join(format!(
        ".{}.{}.builtin",
        resource.file_name(),
        Uuid::new_v4()
    ));
    let mut pending = PendingFile::new(temporary_path.clone());
    let mut file = create_private_file(&temporary_path)?;
    file.write_all(bytes)
        .context("writing builtin policy rule data")?;
    file.flush().context("flushing builtin policy rule data")?;
    file.sync_all()
        .context("syncing builtin policy rule data")?;
    drop(file);
    resource.validate(&temporary_path)?;
    ensure!(
        sha256_file(&temporary_path)? == expected_sha256,
        "builtin policy rule data digest mismatch"
    );
    replace_file(&temporary_path, &target_path)?;
    pending.commit();
    sync_parent_dir(cache_dir);
    Ok(target_path)
}

pub(crate) async fn update_policy_rule_data(
    config_dir: PathBuf,
    instance_id: Uuid,
    resource: PolicyRuleDataResource,
    source_url: Option<String>,
) -> anyhow::Result<PolicyRuleDataUpdate> {
    tokio::task::spawn_blocking(move || {
        update_policy_rule_data_sync(&config_dir, instance_id, resource, source_url.as_deref())
    })
    .await
    .context("policy rule data update task failed")?
}

pub(crate) async fn list_policy_rule_data_categories(
    config_dir: Option<PathBuf>,
    instance_id: Uuid,
    resource: PolicyRuleDataResource,
    expected_sha256: Option<String>,
    configured_path: Option<String>,
) -> anyhow::Result<PolicyRuleDataCategories> {
    tokio::task::spawn_blocking(move || {
        list_policy_rule_data_categories_sync(
            config_dir.as_deref(),
            instance_id,
            resource,
            expected_sha256.as_deref(),
            configured_path.as_deref(),
        )
    })
    .await
    .context("policy rule data category task failed")?
}

fn list_policy_rule_data_categories_sync(
    config_dir: Option<&Path>,
    instance_id: Uuid,
    resource: PolicyRuleDataResource,
    expected_sha256: Option<&str>,
    configured_path: Option<&str>,
) -> anyhow::Result<PolicyRuleDataCategories> {
    ensure!(
        resource != PolicyRuleDataResource::CountryMmdb,
        "Country MMDB does not provide GeoSite or GeoIP categories"
    );
    let configured_path = configured_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                Ok(path)
            } else {
                config_dir
                    .map(|config_dir| config_dir.join(path))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "relative rule data path requires a managed config directory"
                        )
                    })
            }
        })
        .transpose()?;
    let managed_path = config_dir.map(|config_dir| {
        config_dir
            .join("policy-rule-data")
            .join(instance_id.to_string())
            .join(resource.file_name())
    });
    let (path, builtin_sha256) = if let Some(path) = configured_path {
        ensure!(
            path.is_file(),
            "configured rule data file does not exist: {}",
            path.display()
        );
        (path, None)
    } else if let Some(managed_path) = managed_path.filter(|path| path.is_file()) {
        (managed_path, None)
    } else {
        let cache_dir = builtin_category_cache_dir(config_dir);
        let path = materialize_builtin_rule_data(&cache_dir, resource)?;
        let sha256 = resource.builtin().map(|(_, sha256)| sha256);
        (path, sha256)
    };
    let expected_sha256 = expected_sha256
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(builtin_sha256);
    let index = load_or_rebuild_category_index(&path, resource, expected_sha256)?;
    Ok(PolicyRuleDataCategories {
        resource: resource.name().to_owned(),
        sha256: index.sha256,
        size: index.size,
        categories: index.categories,
    })
}

fn builtin_category_cache_dir(config_dir: Option<&Path>) -> PathBuf {
    // Mihomo component/geodata/utils.go::LoadGeoSiteMatcher caches successful
    // decoded lists and forgets failures. GUI category discovery likewise
    // reuses a validated sidecar index, but normal GUI mode has no managed
    // config directory, so it must share the runtime's existing temp fallback.
    config_dir.map_or_else(
        || {
            std::env::temp_dir()
                .join("easytier-policy-rule-data")
                .join(BUILTIN_SNAPSHOT)
        },
        |config_dir| {
            config_dir
                .join(".easytier-policy-rule-data")
                .join(BUILTIN_SNAPSHOT)
        },
    )
}

fn update_policy_rule_data_sync(
    config_dir: &Path,
    instance_id: Uuid,
    resource: PolicyRuleDataResource,
    source_url: Option<&str>,
) -> anyhow::Result<PolicyRuleDataUpdate> {
    let source_url = validated_source_url(resource, source_url)?;
    let target_dir = config_dir
        .join("policy-rule-data")
        .join(instance_id.to_string());
    fs::create_dir_all(&target_dir).with_context(|| {
        format!(
            "creating policy rule data directory {}",
            target_dir.display()
        )
    })?;

    let target_path = target_dir.join(resource.file_name());
    let temporary_path = target_dir.join(format!(
        ".{}.{}.download",
        resource.file_name(),
        Uuid::new_v4()
    ));
    let mut response = policy_rule_data_client()?
        .get(source_url.as_str())
        .send()
        .with_context(|| format!("downloading policy rule data from {source_url}"))?;
    ensure!(
        response.status().is_success(),
        "policy rule data download returned HTTP {}",
        response.status()
    );
    let declared_size = response.content_length();
    if let Some(content_length) = declared_size {
        ensure!(
            content_length <= MAX_RULE_DATA_BYTES,
            "policy rule data declares {content_length} bytes, exceeding the 256 MiB limit"
        );
        if target_path
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == content_length)
            && let Some(index) = load_cached_category_index(&target_path, content_length)
        {
            return Ok(PolicyRuleDataUpdate {
                path: target_path,
                sha256: index.sha256,
                size: content_length,
                source_url,
                categories: index.categories,
                updated: false,
            });
        }
    }

    let mut pending = PendingFile::new(temporary_path.clone());
    let file = create_private_file(&temporary_path)?;
    let mut writer = BoundedWriter::new(file, MAX_RULE_DATA_BYTES);
    io::copy(&mut response, &mut writer)
        .with_context(|| format!("reading policy rule data from {source_url}"))?;

    let (mut file, size) = writer.finish();
    ensure!(size > 0, "policy rule data download is empty");
    ensure!(
        declared_size.is_none_or(|declared_size| declared_size == size),
        "policy rule data size mismatch: expected {} bytes, received {size}",
        declared_size.unwrap_or_default()
    );
    file.flush().context("flushing policy rule data download")?;
    file.sync_all()
        .context("syncing policy rule data download")?;
    drop(file);

    resource.validate(&temporary_path)?;
    let categories = resource.categories(&temporary_path)?;
    let sha256 = sha256_file(&temporary_path)?;
    replace_file(&temporary_path, &target_path)?;
    pending.commit();
    sync_parent_dir(&target_dir);

    let index = PolicyRuleDataCategoryIndex {
        version: POLICY_RULE_DATA_CATEGORY_INDEX_VERSION,
        size,
        sha256: sha256.clone(),
        categories: categories.clone(),
    };
    let _ = write_category_index(&target_path, &index);

    Ok(PolicyRuleDataUpdate {
        path: target_path,
        sha256,
        size,
        source_url,
        categories,
        updated: true,
    })
}

impl PolicyRuleDataResource {
    fn categories(self, path: &Path) -> anyhow::Result<Vec<String>> {
        let kind = match self {
            Self::Geosite => ManagedRuleDataKind::Geosite,
            Self::Geoip => ManagedRuleDataKind::Geoip,
            Self::CountryMmdb => return Ok(Vec::new()),
        };
        list_managed_rule_data_categories(kind, path)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

fn load_or_rebuild_category_index(
    data_path: &Path,
    resource: PolicyRuleDataResource,
    expected_sha256: Option<&str>,
) -> anyhow::Result<PolicyRuleDataCategoryIndex> {
    let size = data_path
        .metadata()
        .with_context(|| format!("reading rule data metadata {}", data_path.display()))?
        .len();
    let index_path = category_index_path(data_path);
    if let Ok(file) = File::open(&index_path)
        && let Ok(index) = serde_json::from_reader::<_, PolicyRuleDataCategoryIndex>(file)
        && index.version == POLICY_RULE_DATA_CATEGORY_INDEX_VERSION
        && index.size == size
    {
        if expected_sha256.is_some_and(|expected| expected == index.sha256) {
            return Ok(index);
        }
        if expected_sha256.is_none()
            && sha256_file(data_path).is_ok_and(|sha256| sha256 == index.sha256)
        {
            return Ok(index);
        }
    }

    let sha256 = sha256_file(data_path)?;
    if let Some(expected_sha256) = expected_sha256 {
        ensure!(
            sha256 == expected_sha256,
            "rule data digest mismatch for {}",
            data_path.display()
        );
    }
    let index = PolicyRuleDataCategoryIndex {
        version: POLICY_RULE_DATA_CATEGORY_INDEX_VERSION,
        size,
        sha256,
        categories: resource.categories(data_path)?,
    };
    let _ = write_category_index(data_path, &index);
    Ok(index)
}

fn load_cached_category_index(
    data_path: &Path,
    expected_size: u64,
) -> Option<PolicyRuleDataCategoryIndex> {
    let file = File::open(category_index_path(data_path)).ok()?;
    let index = serde_json::from_reader::<_, PolicyRuleDataCategoryIndex>(file).ok()?;
    (index.version == POLICY_RULE_DATA_CATEGORY_INDEX_VERSION
        && index.size == expected_size
        && index.sha256.len() == 64
        && index.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then_some(index)
}

fn category_index_path(data_path: &Path) -> PathBuf {
    let mut path = data_path.as_os_str().to_owned();
    path.push(".categories.json");
    PathBuf::from(path)
}

fn write_category_index(
    data_path: &Path,
    index: &PolicyRuleDataCategoryIndex,
) -> anyhow::Result<()> {
    let index_path = category_index_path(data_path);
    let parent = index_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = index_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("categories.json");
    let temporary_path = parent.join(format!(".{file_name}.{}.pending", Uuid::new_v4()));
    let mut pending = PendingFile::new(temporary_path.clone());
    let mut file = create_private_file(&temporary_path)?;
    serde_json::to_writer(&mut file, index).context("writing policy category index")?;
    file.flush().context("flushing policy category index")?;
    file.sync_all().context("syncing policy category index")?;
    drop(file);
    replace_file(&temporary_path, &index_path)?;
    pending.commit();
    sync_parent_dir(parent);
    Ok(())
}

// Reference semantics:
// - Mihomo component/resource/vehicle.go::HTTPVehicle.Read downloads through
//   component/http/http.go::HttpRequest.
// - github.com/metacubex/http@v0.1.6/client.go::Client.do resolves Location before
//   policy evaluation, shares one context deadline across the chain, and strips
//   sensitive headers across hosts; defaultCheckRedirect stops after ten requests.
// EasyTier intentionally keeps its existing five-redirect and HTTPS-only contract
// because managed rule data controls policy behavior. A rejected hop, timeout, bad
// status, oversized body, or validation error leaves the installed file untouched.
fn policy_rule_data_client() -> anyhow::Result<Client> {
    Client::builder()
        .user_agent(concat!("easytier/", env!("CARGO_PKG_VERSION")))
        .timeout(RULE_DATA_DOWNLOAD_TIMEOUT)
        .https_only(true)
        .referer(false)
        .redirect(Policy::custom(policy_rule_data_redirect))
        .build()
        .context("building policy rule data HTTP client")
}

fn policy_rule_data_redirect(attempt: Attempt<'_>) -> Action {
    if attempt.previous().len() > MAX_RULE_DATA_REDIRECTS {
        return attempt.error(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("policy rule data redirect limit of {MAX_RULE_DATA_REDIRECTS} exceeded"),
        ));
    }
    if let Err(error) = validate_policy_rule_data_url(attempt.url()) {
        return attempt.error(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe policy rule data redirect: {error}"),
        ));
    }
    attempt.follow()
}

fn validated_source_url(
    resource: PolicyRuleDataResource,
    source_url: Option<&str>,
) -> anyhow::Result<String> {
    let source_url = source_url
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .unwrap_or_else(|| resource.default_source_url());
    let parsed = Url::parse(source_url).context("parsing policy rule data source URL")?;
    validate_policy_rule_data_url(&parsed)?;
    Ok(parsed.to_string())
}

fn validate_policy_rule_data_url(parsed: &Url) -> anyhow::Result<()> {
    ensure!(
        parsed.scheme() == "https",
        "policy rule data source must use HTTPS"
    );
    ensure!(
        parsed.host_str().is_some(),
        "policy rule data source must include a host"
    );
    ensure!(
        parsed.username().is_empty() && parsed.password().is_none(),
        "policy rule data source must not contain embedded credentials"
    );
    ensure!(
        parsed.fragment().is_none(),
        "policy rule data source must not contain a URL fragment"
    );
    Ok(())
}

fn create_private_file(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).with_context(|| {
        format!(
            "creating temporary policy rule data file {}",
            path.display()
        )
    })
}

struct BoundedWriter<W> {
    inner: W,
    written: u64,
    limit: u64,
}

impl<W> BoundedWriter<W> {
    fn new(inner: W, limit: u64) -> Self {
        Self {
            inner,
            written: 0,
            limit,
        }
    }

    fn finish(self) -> (W, u64) {
        (self.inner, self.written)
    }
}

impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() as u64 > self.limit.saturating_sub(self.written) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "policy rule data exceeds the 256 MiB limit",
            ));
        }
        let written = self.inner.write(buffer)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct PendingFile {
    path: PathBuf,
    committed: bool,
}

impl PendingFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::rename(source, destination).with_context(|| {
        format!(
            "atomically replacing policy rule data {} with {}",
            destination.display(),
            source.display()
        )
    })
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .context("atomically replacing policy rule data on Windows")
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_: &Path) {}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("opening policy rule data {} for hashing", path.display()))?;
    let mut digest = Sha256::new();
    io::copy(&mut file, &mut digest).context("hashing policy rule data")?;
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_have_fixed_names_and_sources() {
        let resources = [
            ("geosite", "geosite.dat"),
            ("geoip", "geoip-lite.dat"),
            ("mmdb", "country-lite.mmdb"),
        ];
        for (name, file_name) in resources {
            let resource = name.parse::<PolicyRuleDataResource>().unwrap();
            assert_eq!(resource.file_name(), file_name);
            assert!(resource.default_source_url().ends_with(file_name));
        }
        assert!("asn".parse::<PolicyRuleDataResource>().is_err());
    }

    #[test]
    fn builtin_snapshots_materialize_with_pinned_digests() {
        let directory = tempfile::tempdir().unwrap();
        for resource in [
            PolicyRuleDataResource::Geosite,
            PolicyRuleDataResource::Geoip,
        ] {
            let (name, rule_set) =
                materialize_builtin_rule_set_at(directory.path(), resource).unwrap();
            let (_, expected_sha256) = resource.builtin().unwrap();
            assert_eq!(name, format!("easytier-builtin-{}", resource.name()));
            assert_eq!(rule_set.sha256.as_deref(), Some(expected_sha256));
            assert_eq!(sha256_file(&rule_set.path).unwrap(), expected_sha256);
            resource.validate(&rule_set.path).unwrap();
        }
    }

    #[test]
    fn category_cache_without_managed_config_matches_runtime_fallback() {
        assert_eq!(
            builtin_category_cache_dir(None),
            std::env::temp_dir()
                .join("easytier-policy-rule-data")
                .join(BUILTIN_SNAPSHOT)
        );

        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            builtin_category_cache_dir(Some(directory.path())),
            directory
                .path()
                .join(".easytier-policy-rule-data")
                .join(BUILTIN_SNAPSHOT)
        );
    }

    #[test]
    fn normal_gui_mode_lists_and_caches_builtin_geosite_categories() {
        let listing = list_policy_rule_data_categories_sync(
            None,
            Uuid::nil(),
            PolicyRuleDataResource::Geosite,
            None,
            None,
        )
        .unwrap();

        assert!(listing.categories.iter().any(|category| category == "CN"));
        let data_path =
            builtin_category_cache_dir(None).join(PolicyRuleDataResource::Geosite.file_name());
        assert!(category_index_path(&data_path).is_file());
    }

    #[test]
    fn custom_sources_are_https_urls_without_credentials_or_fragments() {
        let resource = PolicyRuleDataResource::Geoip;
        assert_eq!(
            validated_source_url(resource, None).unwrap(),
            resource.default_source_url()
        );
        assert_eq!(
            validated_source_url(resource, Some(" https://rules.example/geoip.dat ")).unwrap(),
            "https://rules.example/geoip.dat"
        );
        assert!(validated_source_url(resource, Some("http://rules.example/geoip.dat")).is_err());
        assert!(
            validated_source_url(resource, Some("https://user@rules.example/geoip.dat")).is_err()
        );
        assert!(
            validated_source_url(resource, Some("https://rules.example/geoip.dat#fragment"))
                .is_err()
        );
    }

    #[test]
    fn redirect_targets_preserve_url_safety_and_five_hop_limit() {
        for target in [
            "http://rules.example/geoip.dat",
            "https://user@rules.example/geoip.dat",
            "https://rules.example/geoip.dat#fragment",
        ] {
            assert!(validate_policy_rule_data_url(&Url::parse(target).unwrap()).is_err());
        }
        assert!(
            validate_policy_rule_data_url(
                &Url::parse("https://cdn.example/releases/geoip.dat").unwrap()
            )
            .is_ok()
        );
        const { assert!(MAX_RULE_DATA_REDIRECTS == 5) };
    }

    #[test]
    fn bounded_writer_rejects_oversized_input_without_partial_write() {
        let mut writer = BoundedWriter::new(Vec::new(), 4);
        writer.write_all(b"1234").unwrap();
        assert!(writer.write_all(b"5").is_err());
        let (bytes, written) = writer.finish();
        assert_eq!(bytes, b"1234");
        assert_eq!(written, 4);
    }

    #[test]
    fn same_size_check_reuses_saved_index_without_hashing_rule_data() {
        let directory = tempfile::tempdir().unwrap();
        let data_path = directory.path().join("country-lite.mmdb");
        fs::write(&data_path, b"not parsed or hashed on the size-only path").unwrap();
        let saved_digest = "a".repeat(64);
        let index = PolicyRuleDataCategoryIndex {
            version: POLICY_RULE_DATA_CATEGORY_INDEX_VERSION,
            size: 42,
            sha256: saved_digest.clone(),
            categories: Vec::new(),
        };
        write_category_index(&data_path, &index).unwrap();

        let cached = load_cached_category_index(&data_path, 42).unwrap();
        assert_eq!(cached.sha256, saved_digest);
        assert!(cached.categories.is_empty());
        assert!(load_cached_category_index(&data_path, 43).is_none());
    }

    #[test]
    fn same_size_check_rejects_missing_corrupt_or_unverified_index() {
        let directory = tempfile::tempdir().unwrap();
        let data_path = directory.path().join("country-lite.mmdb");
        fs::write(&data_path, b"1234").unwrap();

        assert!(load_cached_category_index(&data_path, 4).is_none());

        fs::write(category_index_path(&data_path), b"not json").unwrap();
        assert!(load_cached_category_index(&data_path, 4).is_none());

        let unverified = PolicyRuleDataCategoryIndex {
            version: POLICY_RULE_DATA_CATEGORY_INDEX_VERSION,
            size: 4,
            sha256: String::new(),
            categories: Vec::new(),
        };
        write_category_index(&data_path, &unverified).unwrap();
        assert!(load_cached_category_index(&data_path, 4).is_none());
    }
}
