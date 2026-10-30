use std::hash::Hasher;
use std::path::Path;

pub struct Fnv1a(fnv::FnvHasher);

impl Default for Fnv1a {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv1a {
    pub fn new() -> Self {
        Self(fnv::FnvHasher::default())
    }

    pub fn eat(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    pub fn hex(&self) -> String {
        format!("{:016x}", self.0.finish())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub version: String,
    pub host: String,
    pub profile: String,
    pub source: String,
}

impl Stamp {
    pub fn render(&self) -> String {
        format!(
            "version = {}\nhost = {}\nprofile = {}\nsource = {}\n",
            self.version, self.host, self.profile, self.source
        )
    }

    pub fn parse(text: &str) -> Self {
        let get = |key: &str| {
            text.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().to_string())
                .unwrap_or_default()
        };
        Self {
            version: get("version"),
            host: get("host"),
            profile: get("profile"),
            source: get("source"),
        }
    }
}

pub fn key_for(root: &Path) -> String {
    let mut hash = Fnv1a::new();
    hash.eat(root.as_os_str().as_encoded_bytes());
    let slug = root.file_name().map_or_else(
        || "workspace".to_string(),
        |name| {
            name.as_encoded_bytes()
                .iter()
                .take(32)
                .map(|b| {
                    let c = char::from(*b);
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                        c
                    } else {
                        '-'
                    }
                })
                .collect()
        },
    );
    format!("{slug}-{}", hash.hex())
}
