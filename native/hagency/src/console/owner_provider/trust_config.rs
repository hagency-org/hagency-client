//! Codex persists project trust during thread startup. That metadata does not
//! change the keyring locator or authorize provider, tool, or approval settings.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustConfig {
    projects: BTreeMap<String, ProjectTrust>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectTrust {
    trust_level: TrustLevel,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum TrustLevel {
    Trusted,
    Untrusted,
}

pub(super) fn accepted(home: &Path) -> bool {
    let path = home.join("config.toml");
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
        Ok(_) => {}
    }
    let Ok(file) = hagency_store::private::open(&path, false) else {
        return false;
    };
    let mut raw = String::new();
    if file.take(16 * 1024 + 1).read_to_string(&mut raw).is_err() || raw.len() > 16 * 1024 {
        return false;
    }
    let Ok(config) = toml::from_str::<TrustConfig>(&raw) else {
        return false;
    };
    !config.projects.is_empty()
        && config.projects.len() <= 64
        && config.projects.iter().all(|(path, project)| {
            // Parse the exact known values; do not interpret them as host authority.
            let _ = &project.trust_level;
            !path.is_empty()
                && path.len() <= 4096
                && !path.chars().any(char::is_control)
                && Path::new(path).is_absolute()
                && !Path::new(path)
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        })
}

#[cfg(test)]
mod tests {
    use super::super::credential_reference;
    #[test]
    fn codex_generated_trust_preserves_keyring_identity_and_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap().join("provider");
        hagency_store::private::directory(&home).unwrap();
        let before = credential_reference(&home).unwrap();
        let bytes=b"[projects.\"/original/workspace\"]\ntrust_level = \"trusted\"\n[projects.\"/other/workspace\"]\ntrust_level = \"untrusted\"\n";
        hagency_store::private::write_new(&home.join("config.toml"), bytes).unwrap();
        assert_eq!(credential_reference(&home).unwrap(), before);
        assert_eq!(std::fs::read(home.join("config.toml")).unwrap(), bytes);
        hagency_store::private::write_new(&home.join("auth.json"), b"{}").unwrap();
        assert!(credential_reference(&home).is_err());
    }
    #[test]
    fn trust_metadata_rejects_execution_settings_unknown_fields_and_bad_paths() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap().join("provider");
        hagency_store::private::directory(&home).unwrap();
        for text in [
            "",
            "[projects]",
            "[projects.\"relative\"]\ntrust_level=\"trusted\"",
            "[projects.\"/a/../b\"]\ntrust_level=\"trusted\"",
            "[projects.\"/work\"]\ntrust_level=\"unknown\"",
            "approval_policy=\"never\"\n[projects.\"/work\"]\ntrust_level=\"trusted\"",
            "[projects.\"/work\"]\ntrust_level=\"trusted\"\nmodel=\"other\"",
            "[mcp_servers.other]\ncommand=\"other\"",
            "[projects.\"/work\"]\ntrust_level=\"trusted\"\ntrust_level=\"untrusted\"",
        ] {
            hagency_store::private::replace(&home.join("config.toml"), text.as_bytes()).unwrap();
            assert!(credential_reference(&home).is_err(), "{text}");
        }
        hagency_store::private::replace(&home.join("config.toml"), &vec![b' '; 16385]).unwrap();
        assert!(credential_reference(&home).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(home.join("config.toml")).unwrap();
            let outside = home.join("outside");
            hagency_store::private::write_new(
                &outside,
                b"[projects.\"/work\"]\ntrust_level=\"trusted\"",
            )
            .unwrap();
            std::os::unix::fs::symlink(&outside, home.join("config.toml")).unwrap();
            assert!(credential_reference(&home).is_err());
        }
    }
}
