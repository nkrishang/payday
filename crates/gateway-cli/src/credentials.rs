use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CliError;

#[derive(Default, Deserialize, Serialize)]
struct Credentials(BTreeMap<String, Profile>);

#[derive(Deserialize, Serialize)]
struct Profile {
    api_key: String,
    #[serde(default)]
    api_url: String,
}

pub fn path() -> Result<PathBuf, CliError> {
    let app_data = cfg!(windows).then(|| std::env::var_os("APPDATA")).flatten();
    let home = std::env::var_os("HOME").or_else(|| {
        cfg!(windows)
            .then(|| std::env::var_os("USERPROFILE"))
            .flatten()
    });
    resolve_path(
        std::env::var_os("PAYDAY_CONFIG_DIR").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        app_data.map(PathBuf::from),
        home.map(PathBuf::from),
    )
    .ok_or_else(|| CliError::Config("cannot find a config directory; set PAYDAY_CONFIG_DIR".into()))
}

fn resolve_path(
    override_dir: Option<PathBuf>,
    xdg: Option<PathBuf>,
    app_data: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    override_dir
        .map(|dir| dir.join("credentials"))
        .or_else(|| xdg.map(|dir| dir.join("payday/credentials")))
        .or_else(|| app_data.map(|dir| dir.join("payday/credentials")))
        .or_else(|| home.map(|dir| dir.join(".config/payday/credentials")))
}

pub fn normalized_api_url(api_url: &str) -> Result<String, CliError> {
    let url = reqwest::Url::parse(api_url)
        .map_err(|error| CliError::InvalidInput(format!("invalid API URL: {error}")))?;
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn profile(api_url: &str) -> &'static str {
    reqwest::Url::parse(api_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .filter(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
        .map_or("default", |_| "local")
}

pub fn load(file: &Path, name: &str, api_url: &str) -> Result<Option<String>, CliError> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::Config(format!(
                "cannot read {}: {error}",
                file.display()
            )));
        }
    };
    let credentials: Credentials = toml::from_str(&text)
        .map_err(|error| CliError::Config(format!("invalid {}: {error}", file.display())))?;
    let Some(profile) = credentials.0.get(name) else {
        return Ok(None);
    };
    let requested = normalized_api_url(api_url)?;
    if profile.api_url != requested {
        let owner = if profile.api_url.is_empty() {
            "an older Payday version"
        } else {
            profile.api_url.as_str()
        };
        return Err(CliError::Config(format!(
            "saved {name} credentials belong to {owner}, not {requested}; run `payday login --api-url {requested}`"
        )));
    }
    Ok(Some(profile.api_key.clone()))
}

pub fn save(file: &Path, name: &str, api_url: &str, api_key: &str) -> Result<(), CliError> {
    let mut credentials = if file.exists() {
        toml::from_str(&fs::read_to_string(file).map_err(config_io)?)
            .map_err(|e| CliError::Config(format!("invalid {}: {e}", file.display())))?
    } else {
        Credentials::default()
    };
    credentials.0.insert(
        name.into(),
        Profile {
            api_key: api_key.into(),
            api_url: normalized_api_url(api_url)?,
        },
    );
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(config_io)?;
    }
    let text = toml::to_string(&credentials).map_err(|e| CliError::Config(e.to_string()))?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(file).map_err(config_io)?;
    output.write_all(text.as_bytes()).map_err(config_io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).map_err(config_io)?;
    }
    Ok(())
}

pub fn prepare(file: &Path) -> Result<(), CliError> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(config_io)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(file).map_err(config_io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).map_err(config_io)?;
    }
    Ok(())
}

pub fn remove(file: &Path, name: &str) -> Result<bool, CliError> {
    if !file.exists() {
        return Ok(false);
    }
    let mut credentials: Credentials =
        toml::from_str(&fs::read_to_string(file).map_err(config_io)?)
            .map_err(|e| CliError::Config(format!("invalid {}: {e}", file.display())))?;
    let removed = credentials.0.remove(name).is_some();
    if removed {
        if credentials.0.is_empty() {
            fs::remove_file(file).map_err(config_io)?;
        } else {
            save_all(file, &credentials)?;
        }
    }
    Ok(removed)
}

fn save_all(file: &Path, value: &Credentials) -> Result<(), CliError> {
    let text = toml::to_string(value).map_err(|e| CliError::Config(e.to_string()))?;
    fs::write(file, text).map_err(config_io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).map_err(config_io)?;
    }
    Ok(())
}
fn config_io(error: std::io::Error) -> CliError {
    CliError::Config(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_are_independent_and_file_is_private() {
        let dir = std::env::temp_dir().join(format!("payday-{}", uuid::Uuid::now_v7()));
        let file = dir.join("credentials");
        prepare(&file).unwrap();
        save(&file, "default", "https://api.payday.sh", "production").unwrap();
        save(&file, "local", "http://localhost:3000", "development").unwrap();
        let contents = fs::read_to_string(&file).unwrap();
        assert!(contents.contains("[default]"));
        assert!(contents.contains("[local]"));
        assert_eq!(
            load(&file, "default", "https://api.payday.sh/")
                .unwrap()
                .as_deref(),
            Some("production")
        );
        assert_eq!(
            load(&file, "local", "http://localhost:3000")
                .unwrap()
                .as_deref(),
            Some("development")
        );
        remove(&file, "local").unwrap();
        assert!(
            load(&file, "local", "http://localhost:3000")
                .unwrap()
                .is_none()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        remove(&file, "default").unwrap();
        assert!(!file.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn profile_is_bound_to_the_api_url() {
        let dir = std::env::temp_dir().join(format!("payday-{}", uuid::Uuid::now_v7()));
        let file = dir.join("credentials");
        save(&file, "default", "https://api.payday.sh", "production").unwrap();

        let error = load(&file, "default", "https://attacker.example").unwrap_err();
        assert!(error.to_string().contains("not https://attacker.example"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn path_resolution_supports_windows_app_data() {
        assert_eq!(
            resolve_path(
                None,
                None,
                Some(PathBuf::from("C:/Users/A/AppData/Roaming")),
                None
            ),
            Some(PathBuf::from(
                "C:/Users/A/AppData/Roaming/payday/credentials"
            ))
        );
    }
}
