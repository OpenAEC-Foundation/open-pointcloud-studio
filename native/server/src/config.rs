//! The settings of the service, read from the environment once at start.
//! A setting that is wrong stops the start with a line that names the
//! variable; a key is never repeated in it.

use std::path::PathBuf;
use std::time::Duration;

/// Who may use the protected routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// No key check: for local use only.
    Open,
    /// `Authorization: Bearer <key>`, whose SHA-256 is one of these.
    Keys(Vec<[u8; 32]>),
}

/// The environment variables that say the service runs in a managed
/// container runtime, where open access is refused.
pub const MANAGED_RUNTIME: [&str; 3] = [
    "ECS_CONTAINER_METADATA_URI_V4",
    "ECS_CONTAINER_METADATA_URI",
    "AWS_EXECUTION_ENV",
];

/// The working set of the largest call, in MiB: the largest section
/// drawing the core makes, as the comment on `MAX_DRAWING_POINTS` reckons.
pub const WORKING_SET_MIB: u64 = 1229;

/// The settings of the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub access: Access,
    /// Largest request body, in bytes.
    pub max_body: usize,
    /// Largest result file before base64, in bytes.
    pub max_output: u64,
    /// Calls computing at once.
    pub concurrent: usize,
    /// Large requests that may wait for a place to compute.
    pub waiting_room: usize,
    /// How long a call waits for a place to compute before 503.
    pub wait: Duration,
    /// How long a call may compute before 504.
    pub time_limit: Duration,
    /// How long a cancelled call may go on after its time limit before the
    /// health route reports the service as stuck.
    pub grace: Duration,
    /// Where the work folder of each call is made.
    pub work_dir: PathBuf,
    /// The `Origin` values allowed on the protected routes.
    pub origins: Vec<String>,
    /// The memory the service may use, in MiB, when it was given.
    pub memory_mib: Option<u64>,
}

/// A whole number from `range`, or its default when the variable is unset.
fn number(
    source: &dyn Fn(&str) -> Option<String>,
    name: &str,
    default: u64,
    range: std::ops::RangeInclusive<u64>,
) -> Result<u64, String> {
    let Some(value) = source(name).filter(|value| !value.trim().is_empty()) else {
        return Ok(default);
    };
    let shown: String = value.chars().take(32).collect();
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|number| range.contains(number))
        .ok_or_else(|| {
            format!(
                "{name} must be a whole number from {} to {}, not \"{shown}\".",
                range.start(),
                range.end()
            )
        })
}

fn hex_hash(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut hash = [0u8; 32];
    for (at, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * at..2 * at + 2], 16).ok()?;
    }
    Some(hash)
}

/// The origins of `OPENAEC_API_HERKOMSTEN`: `scheme://host[:port]` each,
/// separated by commas.
fn origins(value: &str) -> Result<Vec<String>, String> {
    let mut origins = Vec::new();
    for (at, origin) in value
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .enumerate()
    {
        let origin = origin.to_ascii_lowercase();
        let rest = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"));
        let valid = rest.is_some_and(|host| {
            !host.is_empty()
                && host.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "-.:[]".contains(character)
                })
        });
        if !valid {
            return Err(format!(
                "OPENAEC_API_HERKOMSTEN: value {} is not an origin such as https://example.org (a scheme, a host and an optional port, without a path).",
                at + 1
            ));
        }
        origins.push(origin);
    }
    Ok(origins)
}

impl Config {
    /// The settings in the environment of the process.
    pub fn from_env() -> Result<Self, String> {
        Self::from_source(&|name| std::env::var(name).ok())
    }

    /// The settings `source` gives: the environment, or a test. Without keys
    /// and without `OPENAEC_API_TOEGANG=open` the service does not start, so
    /// that it never stands open by mistake; open access is refused with
    /// keys and in a managed runtime.
    pub fn from_source(source: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let open = match source("OPENAEC_API_TOEGANG")
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
        {
            None => false,
            Some(value) if value.eq_ignore_ascii_case("open") => true,
            Some(_) => {
                return Err("OPENAEC_API_TOEGANG may only be open, or unset.".to_owned());
            }
        };
        let keys = source("OPENAEC_API_SLEUTELS").unwrap_or_default();
        if open && !keys.trim().is_empty() {
            return Err("OPENAEC_API_TOEGANG=open and OPENAEC_API_SLEUTELS are both set: choose one. With open the service would ignore the keys.".to_owned());
        }
        let access = if open {
            if let Some(name) = MANAGED_RUNTIME
                .iter()
                .find(|name| source(name).is_some_and(|value| !value.is_empty()))
            {
                return Err(format!(
                    "OPENAEC_API_TOEGANG=open is for local use only and is refused in a managed runtime ({name} is set): set OPENAEC_API_SLEUTELS."
                ));
            }
            Access::Open
        } else {
            let mut hashes = Vec::new();
            for (at, key) in keys
                .split(',')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .enumerate()
            {
                // The value itself never goes into the line: a raw key
                // pasted by mistake would end up in the logs.
                hashes.push(hex_hash(key).ok_or_else(|| {
                    format!(
                        "OPENAEC_API_SLEUTELS: value {} ({} characters) is not a SHA-256 of 64 hex digits.",
                        at + 1,
                        key.chars().count()
                    )
                })?);
            }
            if hashes.is_empty() {
                return Err("The service does not start without access control: set OPENAEC_API_SLEUTELS (the SHA-256 of each key, comma-separated) or, for local use only, OPENAEC_API_TOEGANG=open.".to_owned());
            }
            Access::Keys(hashes)
        };
        let mib = 1024 * 1024;
        let max_body = number(source, "OPENAEC_API_MAX_LICHAAM_MIB", 64, 1..=512)?;
        let max_output = number(source, "OPENAEC_API_MAX_UITKOMST_MIB", 256, 1..=1024)?;
        let concurrent = number(source, "OPENAEC_API_GELIJKTIJDIG", 2, 1..=64)?;
        let waiting_room = number(source, "OPENAEC_API_WACHTRIJ", 4, 0..=256)?;
        let wait = number(source, "OPENAEC_API_WACHTEN_S", 30, 0..=600)?;
        let time_limit = number(source, "OPENAEC_API_TIJDSLIMIET_S", 300, 1..=3600)?;
        let memory_mib =
            match source("OPENAEC_API_GEHEUGEN_MIB").filter(|value| !value.trim().is_empty()) {
                Some(_) => Some(number(
                    source,
                    "OPENAEC_API_GEHEUGEN_MIB",
                    0,
                    1..=16 * 1024 * 1024,
                )?),
                None => None,
            };
        let work_dir = source("OPENAEC_API_WERKMAP")
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let origins = origins(&source("OPENAEC_API_HERKOMSTEN").unwrap_or_default())?;
        let config = Self {
            access,
            max_body: (max_body * mib) as usize,
            max_output: max_output * mib,
            concurrent: concurrent as usize,
            waiting_room: waiting_room as usize,
            wait: Duration::from_secs(wait),
            time_limit: Duration::from_secs(time_limit),
            grace: Duration::from_secs(120),
            work_dir,
            origins,
            memory_mib,
        };
        if let Some(limit) = memory_mib {
            config.check_memory(limit, "OPENAEC_API_GEHEUGEN_MIB")?;
        }
        Ok(config)
    }

    /// The most memory the service can take, in MiB: every running call
    /// with its body, its result in base64 and the working set of the
    /// largest drawing, and every waiting call with its body.
    pub fn worst_case_mib(&self) -> u64 {
        let mib = 1024 * 1024;
        let body = (self.max_body as u64).div_ceil(mib);
        let output = (self.max_output * 134).div_ceil(100 * mib);
        self.concurrent as u64 * (body + output + WORKING_SET_MIB) + self.waiting_room as u64 * body
    }

    /// Refuse a worst case above `limit` MiB, which `source` names.
    pub fn check_memory(&self, limit: u64, source: &str) -> Result<u64, String> {
        let worst = self.worst_case_mib();
        if worst > limit {
            return Err(format!(
                "The worst case of {worst} MiB exceeds the {limit} MiB of {source}: lower OPENAEC_API_GELIJKTIJDIG, OPENAEC_API_WACHTRIJ, OPENAEC_API_MAX_LICHAAM_MIB or OPENAEC_API_MAX_UITKOMST_MIB, or give the service more memory."
            ));
        }
        Ok(worst)
    }
}

/// The memory limit of the control group the process runs in, in MiB, when
/// it is finite: `memory.max` of cgroup v2, else the limit of cgroup v1.
pub fn cgroup_memory_mib() -> Option<u64> {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let bytes = read("/sys/fs/cgroup/memory.max")
        .or_else(|| read("/sys/fs/cgroup/memory/memory.limit_in_bytes"))?
        .trim()
        .parse::<u64>()
        .ok()?;
    // cgroup v1 states no limit as a number near the largest one.
    (bytes < 1 << 60).then_some(bytes / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn with(pairs: &[(&str, &str)]) -> Result<Config, String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        Config::from_source(&|name| map.get(name).cloned())
    }

    fn open(pairs: &[(&str, &str)]) -> Result<Config, String> {
        let mut all = vec![("OPENAEC_API_TOEGANG", "open")];
        all.extend_from_slice(pairs);
        with(&all)
    }

    #[test]
    fn the_service_refuses_to_start_without_access_control() {
        let refusal = with(&[]).unwrap_err();
        assert!(refusal.contains("OPENAEC_API_SLEUTELS"), "{refusal}");
        assert!(with(&[("OPENAEC_API_SLEUTELS", " , ")]).is_err());
        assert!(with(&[("OPENAEC_API_TOEGANG", "closed")]).is_err());
    }

    #[test]
    fn open_access_and_keys_are_refused_together() {
        let hex = "a".repeat(64);
        let refusal = with(&[
            ("OPENAEC_API_TOEGANG", "open"),
            ("OPENAEC_API_SLEUTELS", &hex),
        ])
        .unwrap_err();
        assert!(refusal.contains("OPENAEC_API_TOEGANG"), "{refusal}");
        assert_eq!(open(&[]).unwrap().access, Access::Open);
        let keys = with(&[(
            "OPENAEC_API_SLEUTELS",
            &format!("{hex}, {}", "B".repeat(64)),
        )])
        .unwrap();
        assert_eq!(keys.access, Access::Keys(vec![[0xaa; 32], [0xbb; 32]]));
    }

    #[test]
    fn a_bad_key_is_named_by_place_and_length_only() {
        let secret = "this-is-a-raw-key-not-its-hash";
        let refusal = with(&[(
            "OPENAEC_API_SLEUTELS",
            &format!("{},{secret}", "c".repeat(64)),
        )])
        .unwrap_err();
        assert!(!refusal.contains(secret), "{refusal}");
        assert!(
            refusal.contains("value 2") && refusal.contains("30 characters"),
            "{refusal}"
        );
        assert!(!refusal.contains("  "), "{refusal}");
    }

    #[test]
    fn open_access_is_refused_in_a_managed_runtime() {
        for name in MANAGED_RUNTIME {
            let refusal = open(&[(name, "http://169.254.170.2/v4/x")]).unwrap_err();
            assert!(
                refusal.contains(name) && refusal.contains("OPENAEC_API_TOEGANG"),
                "{refusal}"
            );
        }
        // Keys are fine there.
        let hex = "d".repeat(64);
        assert!(with(&[
            ("OPENAEC_API_SLEUTELS", &hex),
            ("AWS_EXECUTION_ENV", "AWS_ECS_FARGATE")
        ])
        .is_ok());
    }

    #[test]
    fn every_limit_has_its_default_its_range_and_a_refusal() {
        let config = open(&[]).unwrap();
        let mib = 1024 * 1024;
        assert_eq!(config.max_body, 64 * mib);
        assert_eq!(config.max_output, 256 * mib as u64);
        assert_eq!((config.concurrent, config.waiting_room), (2, 4));
        assert_eq!(config.wait, Duration::from_secs(30));
        assert_eq!(config.time_limit, Duration::from_secs(300));
        assert_eq!(config.grace, Duration::from_secs(120));
        assert_eq!(config.work_dir, std::env::temp_dir());
        assert!(config.origins.is_empty());
        assert_eq!(config.memory_mib, None);

        let cases: [(&str, &str, &str, &str); 7] = [
            ("OPENAEC_API_MAX_LICHAAM_MIB", "1", "512", "513"),
            ("OPENAEC_API_MAX_UITKOMST_MIB", "1", "1024", "0"),
            ("OPENAEC_API_GELIJKTIJDIG", "1", "64", "65"),
            ("OPENAEC_API_WACHTRIJ", "0", "256", "257"),
            ("OPENAEC_API_WACHTEN_S", "0", "600", "-1"),
            ("OPENAEC_API_TIJDSLIMIET_S", "1", "3600", "0"),
            ("OPENAEC_API_GEHEUGEN_MIB", "100000", "16777216", "0"),
        ];
        for (name, low, high, wrong) in cases {
            assert!(open(&[(name, low)]).is_ok(), "{name}={low}");
            assert!(open(&[(name, high)]).is_ok(), "{name}={high}");
            for wrong in [wrong, "many", "1.5"] {
                let refusal = open(&[(name, wrong)]).unwrap_err();
                assert!(refusal.starts_with(name), "{refusal}");
            }
        }
        let config = open(&[
            ("OPENAEC_API_MAX_LICHAAM_MIB", "8"),
            ("OPENAEC_API_MAX_UITKOMST_MIB", "16"),
            ("OPENAEC_API_GELIJKTIJDIG", "3"),
            ("OPENAEC_API_WACHTRIJ", "0"),
            ("OPENAEC_API_WACHTEN_S", "5"),
            ("OPENAEC_API_TIJDSLIMIET_S", "60"),
            ("OPENAEC_API_WERKMAP", "/work"),
        ])
        .unwrap();
        assert_eq!(config.max_body, 8 * mib);
        assert_eq!(config.max_output, 16 * mib as u64);
        assert_eq!((config.concurrent, config.waiting_room), (3, 0));
        assert_eq!(config.wait, Duration::from_secs(5));
        assert_eq!(config.time_limit, Duration::from_secs(60));
        assert_eq!(config.work_dir, PathBuf::from("/work"));
    }

    #[test]
    fn origins_are_read_as_a_list() {
        let config = open(&[(
            "OPENAEC_API_HERKOMSTEN",
            " https://app.example.org, http://localhost:3000 ,,HTTPS://Other.Example",
        )])
        .unwrap();
        assert_eq!(
            config.origins,
            [
                "https://app.example.org",
                "http://localhost:3000",
                "https://other.example"
            ]
        );
        for wrong in [
            "example.org",
            "https://example.org/path",
            "ftp://example.org",
            "https://",
        ] {
            let refusal = open(&[("OPENAEC_API_HERKOMSTEN", wrong)]).unwrap_err();
            assert!(
                refusal.starts_with("OPENAEC_API_HERKOMSTEN"),
                "{wrong}: {refusal}"
            );
        }
    }

    #[test]
    fn the_memory_worst_case_is_held_to_the_limit() {
        let config = open(&[]).unwrap();
        // 2 × (64 + 343 + 1229) + 4 × 64 MiB.
        assert_eq!(
            config.worst_case_mib(),
            2 * (64 + 344 + WORKING_SET_MIB) + 4 * 64
        );
        assert!(open(&[("OPENAEC_API_GEHEUGEN_MIB", "8192")]).is_ok());
        let refusal = open(&[("OPENAEC_API_GEHEUGEN_MIB", "2048")]).unwrap_err();
        assert!(refusal.contains("OPENAEC_API_GEHEUGEN_MIB"), "{refusal}");
        assert!(config.check_memory(1000, "the cgroup limit").is_err());
        assert_eq!(
            config.check_memory(4096, "the cgroup limit"),
            Ok(config.worst_case_mib())
        );
    }
}
