use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufReader, Read},
    net::{Ipv4Addr, Ipv6Addr},
    path::Path,
};

use prost::Message;

const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;
const MAX_CATEGORIES: usize = 16 * 1024;
const MAX_GEOIP_CIDRS: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedRuleDataKind {
    Geosite,
    Geoip,
    CountryMmdb,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GeoDataError {
    #[error("failed to open rule data {path}: {source}")]
    Open {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to read rule data: {0}")]
    Read(#[from] std::io::Error),
    #[error("invalid rule data: {0}")]
    Invalid(String),
    #[error("failed to decode rule data entry: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("GeoIP category {0} is not present in the data file")]
    MissingGeoipCategory(String),
}

#[derive(Clone, PartialEq, Message)]
struct DomainAttribute {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(oneof = "domain_attribute::TypedValue", tags = "2, 3")]
    typed_value: Option<domain_attribute::TypedValue>,
}

mod domain_attribute {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub(super) enum TypedValue {
        #[prost(bool, tag = "2")]
        BoolValue(bool),
        #[prost(int64, tag = "3")]
        IntValue(i64),
    }
}

#[derive(Clone, PartialEq, Message)]
struct Domain {
    #[prost(enumeration = "DomainType", tag = "1")]
    domain_type: i32,
    #[prost(string, tag = "2")]
    value: String,
    #[prost(message, repeated, tag = "3")]
    attributes: Vec<DomainAttribute>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
enum DomainType {
    Plain = 0,
    Regex = 1,
    Domain = 2,
    Full = 3,
}

#[derive(Clone, PartialEq, Message)]
struct GeoSite {
    #[prost(string, tag = "1")]
    country_code: String,
    #[prost(message, repeated, tag = "2")]
    domains: Vec<Domain>,
}

#[derive(Clone, PartialEq, Message)]
struct Cidr {
    #[prost(bytes = "vec", tag = "1")]
    ip: Vec<u8>,
    #[prost(uint32, tag = "2")]
    prefix: u32,
}

#[derive(Clone, PartialEq, Message)]
struct GeoIp {
    #[prost(string, tag = "1")]
    country_code: String,
    #[prost(message, repeated, tag = "2")]
    cidrs: Vec<Cidr>,
    #[prost(bool, tag = "3")]
    reverse_match: bool,
}

pub fn validate_managed_rule_data(
    kind: ManagedRuleDataKind,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match kind {
        ManagedRuleDataKind::Geosite => validate_geosite(path)?,
        ManagedRuleDataKind::Geoip => {
            let categories = load_geoip_categories(path, &BTreeSet::new())?;
            if !categories.contains_key("CN") {
                return Err(GeoDataError::Invalid(
                    "GeoIP data does not contain a CN category".to_owned(),
                )
                .into());
            }
        }
        ManagedRuleDataKind::CountryMmdb => validate_country_mmdb(path)?,
    }
    Ok(())
}

fn validate_country_mmdb(path: &Path) -> Result<(), GeoDataError> {
    let reader = maxminddb::Reader::open_readfile(path)
        .map_err(|error| GeoDataError::Invalid(format!("failed to open Country MMDB: {error}")))?;
    if reader.metadata.node_count == 0 {
        return Err(GeoDataError::Invalid(
            "Country MMDB contains no network nodes".to_owned(),
        ));
    }
    if !reader
        .metadata
        .database_type
        .to_ascii_lowercase()
        .contains("country")
    {
        return Err(GeoDataError::Invalid(format!(
            "MMDB database type {:?} is not a Country database",
            reader.metadata.database_type
        )));
    }
    Ok(())
}

fn validate_geosite(path: &Path) -> Result<(), GeoDataError> {
    let mut reader = open(path)?;
    let mut entries = 0usize;
    let mut has_cn = false;
    let mut categories = BTreeSet::new();
    while let Some(encoded) = read_framed_entry(&mut reader)? {
        let group = GeoSite::decode(encoded.as_slice())?;
        let category = group.country_code.trim().to_ascii_uppercase();
        if category.is_empty() {
            return Err(GeoDataError::Invalid(
                "Geosite entry has an empty category".to_owned(),
            ));
        }
        if !categories.insert(category.clone()) {
            return Err(GeoDataError::Invalid(format!(
                "Geosite category {category} appears more than once"
            )));
        }
        if categories.len() > MAX_CATEGORIES {
            return Err(GeoDataError::Invalid(format!(
                "Geosite data exceeds the {MAX_CATEGORIES} category limit"
            )));
        }
        for domain in &group.domains {
            if DomainType::try_from(domain.domain_type).is_err() {
                return Err(GeoDataError::Invalid(format!(
                    "Geosite category {} contains unsupported domain type {}",
                    group.country_code, domain.domain_type
                )));
            }
            if domain.value.is_empty() {
                return Err(GeoDataError::Invalid(format!(
                    "Geosite category {} contains an empty domain",
                    group.country_code
                )));
            }
        }
        has_cn |= category == "CN" && !group.domains.is_empty();
        entries += 1;
    }
    if entries == 0 {
        return Err(GeoDataError::Invalid(
            "Geosite data contains no categories".to_owned(),
        ));
    }
    if !has_cn {
        return Err(GeoDataError::Invalid(
            "Geosite data does not contain a non-empty CN category".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn load_geoip_categories(
    path: &Path,
    requested: &BTreeSet<String>,
) -> Result<BTreeMap<String, Vec<String>>, GeoDataError> {
    let mut reader = open(path)?;
    let mut categories = BTreeMap::new();
    let mut seen_categories = BTreeSet::new();
    let mut total_cidrs = 0usize;
    while let Some(encoded) = read_framed_entry(&mut reader)? {
        let entry = GeoIp::decode(encoded.as_slice())?;
        let code = entry.country_code.trim().to_ascii_uppercase();
        if code.is_empty() {
            return Err(GeoDataError::Invalid(
                "GeoIP entry has an empty category".to_owned(),
            ));
        }
        if !seen_categories.insert(code.clone()) {
            return Err(GeoDataError::Invalid(format!(
                "GeoIP category {code} appears more than once"
            )));
        }
        if seen_categories.len() > MAX_CATEGORIES {
            return Err(GeoDataError::Invalid(format!(
                "GeoIP data exceeds the {MAX_CATEGORIES} category limit"
            )));
        }
        if entry.reverse_match {
            return Err(GeoDataError::Invalid(format!(
                "GeoIP category {code} uses unsupported reverse matching"
            )));
        }
        if requested.is_empty() || requested.contains(&code) {
            total_cidrs = total_cidrs.saturating_add(entry.cidrs.len());
            if total_cidrs > MAX_GEOIP_CIDRS {
                return Err(GeoDataError::Invalid(format!(
                    "selected GeoIP categories exceed the {MAX_GEOIP_CIDRS} CIDR limit"
                )));
            }
            let mut cidrs = Vec::with_capacity(entry.cidrs.len());
            for cidr in entry.cidrs {
                cidrs.push(format_cidr(&cidr)?);
            }
            if cidrs.is_empty() {
                return Err(GeoDataError::Invalid(format!(
                    "GeoIP category {code} contains no CIDRs"
                )));
            }
            categories.insert(code, cidrs);
        }
    }
    for code in requested {
        if !categories.contains_key(code) {
            return Err(GeoDataError::MissingGeoipCategory(code.clone()));
        }
    }
    Ok(categories)
}

pub(crate) fn lan_cidrs() -> Vec<String> {
    [
        "0.0.0.0/32",
        "10.0.0.0/8",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "224.0.0.0/4",
        "255.255.255.255/32",
        "::/128",
        "::1/128",
        "fc00::/7",
        "fe80::/10",
        "ff00::/8",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn format_cidr(cidr: &Cidr) -> Result<String, GeoDataError> {
    match cidr.ip.as_slice() {
        bytes if bytes.len() == 4 && cidr.prefix <= 32 => {
            let address = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
            Ok(format!("{address}/{}", cidr.prefix))
        }
        bytes if bytes.len() == 16 && cidr.prefix <= 128 => {
            let address = Ipv6Addr::from(<[u8; 16]>::try_from(bytes).expect("length checked"));
            Ok(format!("{address}/{}", cidr.prefix))
        }
        bytes => Err(GeoDataError::Invalid(format!(
            "GeoIP CIDR has {} address bytes and prefix {}",
            bytes.len(),
            cidr.prefix
        ))),
    }
}

fn open(path: &Path) -> Result<BufReader<File>, GeoDataError> {
    File::open(path)
        .map(BufReader::new)
        .map_err(|source| GeoDataError::Open {
            path: path.display().to_string(),
            source,
        })
}

fn read_framed_entry(reader: &mut impl Read) -> Result<Option<Vec<u8>>, GeoDataError> {
    let Some(tag) = read_optional_byte(reader)? else {
        return Ok(None);
    };
    if tag != 0x0a {
        return Err(GeoDataError::Invalid(
            "top-level entry is not field 1 length-delimited".to_owned(),
        ));
    }
    let length = read_varint(reader)?;
    if length > MAX_ENTRY_BYTES as u64 {
        return Err(GeoDataError::Invalid(
            "single geodata entry exceeds 64 MiB".to_owned(),
        ));
    }
    let mut encoded = vec![0u8; length as usize];
    reader.read_exact(&mut encoded)?;
    Ok(Some(encoded))
}

fn read_optional_byte(reader: &mut impl Read) -> Result<Option<u8>, std::io::Error> {
    let mut byte = [0u8; 1];
    match reader.read(&mut byte)? {
        0 => Ok(None),
        _ => Ok(Some(byte[0])),
    }
}

fn read_varint(reader: &mut impl Read) -> Result<u64, GeoDataError> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = read_optional_byte(reader)?
            .ok_or_else(|| GeoDataError::Invalid("truncated entry length varint".to_owned()))?;
        if shift == 63 && byte > 1 {
            return Err(GeoDataError::Invalid(
                "entry length varint overflows u64".to_owned(),
            ));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(GeoDataError::Invalid(
        "entry length varint is too long".to_owned(),
    ))
}

#[cfg(test)]
pub(crate) fn write_test_geoip(path: &Path, code: &str, cidrs: Vec<(Vec<u8>, u32)>) {
    use std::io::Write;

    let encoded = GeoIp {
        country_code: code.to_owned(),
        cidrs: cidrs
            .into_iter()
            .map(|(ip, prefix)| Cidr { ip, prefix })
            .collect(),
        reverse_match: false,
    }
    .encode_to_vec();
    let mut file = File::create(path).unwrap();
    file.write_all(&[0x0a]).unwrap();
    let mut length = encoded.len() as u64;
    while length >= 0x80 {
        file.write_all(&[(length as u8) | 0x80]).unwrap();
        length >>= 7;
    }
    file.write_all(&[length as u8]).unwrap();
    file.write_all(&encoded).unwrap();
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn append_entry(file: &mut File, message: impl Message) {
        let encoded = message.encode_to_vec();
        file.write_all(&[0x0a]).unwrap();
        let mut length = encoded.len() as u64;
        while length >= 0x80 {
            file.write_all(&[(length as u8) | 0x80]).unwrap();
            length >>= 7;
        }
        file.write_all(&[length as u8]).unwrap();
        file.write_all(&encoded).unwrap();
    }

    fn write_entry(path: &Path, message: impl Message) {
        append_entry(&mut File::create(path).unwrap(), message);
    }

    #[test]
    fn validates_geosite_and_rejects_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geosite.dat");
        write_entry(
            &path,
            GeoSite {
                country_code: "CN".to_owned(),
                domains: vec![Domain {
                    domain_type: DomainType::Domain as i32,
                    value: "example.cn".to_owned(),
                    attributes: Vec::new(),
                }],
            },
        );
        validate_geosite(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes.pop();
        std::fs::write(&path, bytes).unwrap();
        assert!(validate_geosite(&path).is_err());
    }

    #[test]
    fn rejects_unknown_geosite_domain_types_before_leaf_loads_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geosite.dat");
        write_entry(
            &path,
            GeoSite {
                country_code: "CN".to_owned(),
                domains: vec![Domain {
                    domain_type: 99,
                    value: "example.cn".to_owned(),
                    attributes: Vec::new(),
                }],
            },
        );

        assert!(matches!(
            validate_geosite(&path),
            Err(GeoDataError::Invalid(message))
                if message.contains("unsupported domain type 99")
        ));
    }

    #[test]
    fn loads_requested_geoip_category_as_cidrs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geoip.dat");
        write_test_geoip(
            &path,
            "GOOGLE",
            vec![
                (vec![8, 8, 8, 0], 24),
                (Ipv6Addr::LOCALHOST.octets().to_vec(), 128),
            ],
        );
        let categories =
            load_geoip_categories(&path, &BTreeSet::from(["GOOGLE".to_owned()])).unwrap();
        assert_eq!(categories["GOOGLE"], ["8.8.8.0/24", "::1/128"]);
    }

    #[test]
    fn rejects_duplicate_geoip_categories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geoip.dat");
        let mut file = File::create(&path).unwrap();
        for code in ["CN", "cn"] {
            append_entry(
                &mut file,
                GeoIp {
                    country_code: code.to_owned(),
                    cidrs: vec![Cidr {
                        ip: vec![1, 1, 1, 0],
                        prefix: 24,
                    }],
                    reverse_match: false,
                },
            );
        }
        drop(file);
        assert!(load_geoip_categories(&path, &BTreeSet::new()).is_err());
    }

    #[test]
    fn lan_category_is_self_contained() {
        let cidrs = lan_cidrs();
        assert!(cidrs.contains(&"10.0.0.0/8".to_owned()));
        assert!(cidrs.contains(&"fc00::/7".to_owned()));
    }

    #[test]
    fn rejects_non_mmdb_country_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("country.mmdb");
        std::fs::write(&path, b"not an mmdb").unwrap();
        assert!(validate_country_mmdb(&path).is_err());
    }
}
