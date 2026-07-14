use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, ensure};
use easytier_policy::{ManagedRuleDataKind, validate_managed_rule_data};
use http_req::request::{RedirectPolicy, Request};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MAX_RULE_DATA_BYTES: u64 = 256 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PolicyRuleDataResource {
    Geosite,
    Geoip,
    CountryMmdb,
}

impl PolicyRuleDataResource {
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
    let mut pending = PendingFile::new(temporary_path.clone());
    let file = create_private_file(&temporary_path)?;
    let mut writer = BoundedWriter::new(file, MAX_RULE_DATA_BYTES);
    let uri = http_req::uri::Uri::try_from(source_url.as_str())
        .context("parsing policy rule data URL")?;
    let response = Request::new(&uri)
        .header(
            "User-Agent",
            concat!("easytier/", env!("CARGO_PKG_VERSION")),
        )
        .redirect_policy(RedirectPolicy::Limit(5))
        .timeout(Duration::from_secs(120))
        .send(&mut writer)
        .with_context(|| format!("downloading policy rule data from {}", source_url))?;
    ensure!(
        response.status_code().is_success(),
        "policy rule data download returned HTTP {}",
        response.status_code()
    );

    let (mut file, size) = writer.finish();
    ensure!(size > 0, "policy rule data download is empty");
    file.flush().context("flushing policy rule data download")?;
    file.sync_all()
        .context("syncing policy rule data download")?;
    drop(file);

    resource.validate(&temporary_path)?;
    let sha256 = sha256_file(&temporary_path)?;
    replace_file(&temporary_path, &target_path)?;
    pending.commit();
    sync_parent_dir(&target_dir);

    Ok(PolicyRuleDataUpdate {
        path: target_path,
        sha256,
        size,
        source_url,
    })
}

fn validated_source_url(
    resource: PolicyRuleDataResource,
    source_url: Option<&str>,
) -> anyhow::Result<String> {
    let source_url = source_url
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .unwrap_or_else(|| resource.default_source_url());
    let parsed = url::Url::parse(source_url).context("parsing policy rule data source URL")?;
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
    Ok(parsed.to_string())
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
    fn bounded_writer_rejects_oversized_input_without_partial_write() {
        let mut writer = BoundedWriter::new(Vec::new(), 4);
        writer.write_all(b"1234").unwrap();
        assert!(writer.write_all(b"5").is_err());
        let (bytes, written) = writer.finish();
        assert_eq!(bytes, b"1234");
        assert_eq!(written, 4);
    }
}
