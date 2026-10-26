//! Owner-selected programming capabilities. Never import ambient MCP configuration.
use super::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, future::Future, pin::Pin};

pub const CAPABILITY_VERSION: &str = "hagency-codex-programming-v1";
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Programming {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServer>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpServer {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Http {
        url: String,
    },
}
/// The host checks current owner, device lease and the original running execution.
pub trait ExecutionGate: Send + Sync {
    fn authorize<'a>(
        &'a self,
        dispatch: &'a str,
        execution: &'a str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
}
impl Programming {
    pub fn is_default(&self) -> bool {
        !self.enabled && self.mcp_servers.is_empty()
    }
    pub fn validate(&self) -> crate::Result<()> {
        if self.mcp_servers.len() > 16 || (!self.enabled && !self.mcp_servers.is_empty()) {
            return Err(crate::Error::Invalid("programming MCP configuration"));
        }
        for (id, server) in &self.mcp_servers {
            if id.is_empty()
                || id.len() > 64
                || !id
                    .bytes()
                    .all(|v| v.is_ascii_alphanumeric() || v == b'_' || v == b'-')
            {
                return Err(crate::Error::Invalid("MCP server name"));
            }
            match server {
                McpServer::Stdio { command, args } => {
                    let path = PathBuf::from(command);
                    if !path.is_absolute()
                        || !path.is_file()
                        || path.canonicalize().ok().as_ref() != Some(&path)
                        || args.len() > 64
                        || args.iter().any(|v| v.len() > 4096 || v.contains('\0'))
                    {
                        return Err(crate::Error::Invalid("MCP executable or arguments"));
                    }
                }
                McpServer::Http { url } => {
                    if url.len() > 4096
                        || url.contains(['\0', '\n', '\r', '@', '#'])
                        || !(url.starts_with("https://")
                            || url.starts_with("http://127.0.0.1:")
                            || url.starts_with("http://[::1]:"))
                    {
                        return Err(crate::Error::Invalid("MCP URL"));
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn servers(&self) -> Value {
        Value::Object(
            self.mcp_servers
                .iter()
                .map(|(id, server)| {
                    let mut value = match server {
                        McpServer::Stdio { command, args } => {
                            value!({"command":command,"args":args})
                        }
                        McpServer::Http { url } => value!({"url":url}),
                    };
                    value["enabled"] = true.into();
                    value["environment_id"] = "local".into();
                    value["default_tools_approval_mode"] = "prompt".into();
                    (id.clone(), value)
                })
                .collect(),
        )
    }
    pub(super) fn overrides(&self) -> Result<Vec<String>> {
        self.validate()?;
        if !self.enabled {
            return Ok(vec![]);
        }
        Ok(vec![
            format!("mcp_servers={}", toml(&self.servers())?),
            "features.shell_tool=true".into(),
            "features.unified_exec=true".into(),
        ])
    }
    pub(super) fn verify(&self, config: &Value) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if config["features"]["shell_tool"] != true || config["features"]["unified_exec"] != true {
            return Err(Error::Profile("programming shell features unavailable"));
        }
        let expected = self.servers();
        let actual = config["mcp_servers"]
            .as_object()
            .ok_or(Error::Profile("programming MCP configuration unavailable"))?;
        for (id, want) in expected.as_object().unwrap() {
            let got = actual
                .get(id)
                .ok_or(Error::Profile("configured MCP missing"))?;
            if want
                .as_object()
                .unwrap()
                .iter()
                .any(|(k, v)| got.get(k) != Some(v))
                || got.as_object().is_none_or(|fields| {
                    fields.iter().any(|(key, value)| {
                        !want.as_object().unwrap().contains_key(key) && !neutral(value)
                    })
                })
            {
                return Err(Error::Profile("configured MCP policy differs"));
            }
        }
        if actual
            .iter()
            .any(|(id, s)| !self.mcp_servers.contains_key(id) && s["enabled"] != false)
        {
            return Err(Error::Profile("ambient MCP is enabled"));
        }
        Ok(())
    }
}
fn neutral(value: &Value) -> bool {
    value.is_null()
        || value == false
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
}
fn toml(v: &Value) -> Result<String> {
    Ok(match v {
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| Ok(format!("{}={}", serde_json::to_string(k)?, toml(v)?)))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(toml).collect::<Result<Vec<_>>>()?.join(",")
        ),
        Value::String(_) | Value::Bool(_) | Value::Number(_) => serde_json::to_string(v)?,
        _ => return Err(Error::Profile("unsupported MCP config value")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_mcp_cannot_inherit_environment_or_tool_approval_bypasses() {
        let mut programming = Programming {
            enabled: true,
            ..Default::default()
        };
        programming.mcp_servers.insert(
            "docs".into(),
            McpServer::Http {
                url: "https://example.test/mcp".into(),
            },
        );
        let config = value!({"features":{"shell_tool":true,"unified_exec":true},"mcp_servers":programming.servers()});
        programming.verify(&config).unwrap();
        for (field, value) in [
            ("env", value!({"TOKEN":"private"})),
            ("http_headers", value!({"Authorization":"private"})),
            ("bearer_token_env_var", value!("TOKEN")),
            ("environment_id", value!("remote")),
            ("env_vars", value!(["TOKEN"])),
            ("tools", value!({"write":{"approval_mode":"approve"}})),
            ("default_tools_approval_mode", value!("approve")),
        ] {
            let mut unsafe_config = config.clone();
            unsafe_config["mcp_servers"]["docs"][field] = value;
            assert!(programming.verify(&unsafe_config).is_err(), "{field}");
        }
        let mut ambient = config.clone();
        ambient["mcp_servers"]["personal"] = value!({"enabled":true});
        assert!(programming.verify(&ambient).is_err());
        ambient["mcp_servers"]["personal"]["enabled"] = false.into();
        programming.verify(&ambient).unwrap();
    }
}
