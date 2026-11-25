use std::collections::BTreeSet;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::plugins::{
    API_VERSION, ErrorCode, MANIFEST_SCHEMA, MAX_ACTIONS, MAX_MANIFEST_BYTES, MAX_PARAMETERS,
    PluginError, PluginResult, validate_id, validate_resource_path,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub authors: Vec<String>,
    #[serde(default = "local_publisher")]
    pub publisher: String,
    pub description: String,
    pub host_version: String,
    pub api_version: String,
    pub component: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub contributions: crate::plugins::extension::Contributions,
    pub actions: Vec<ActionManifest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub result: ResultKind,
    pub template: Option<String>,
    #[serde(default)]
    pub quoting: Quoting,
    #[serde(default)]
    pub parameters: Vec<ParameterManifest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    Command,
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quoting {
    #[default]
    Literal,
    Posix,
    Powershell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    String,
    Integer,
    Boolean,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterManifest {
    pub id: String,
    pub name: String,
    pub kind: ParameterKind,
    #[serde(default)]
    pub required: bool,
    pub default: Option<String>,
}

fn local_publisher() -> String {
    "local".into()
}

impl PluginManifest {
    pub fn parse(raw: &str, host_version: &str) -> PluginResult<Self> {
        let invalid = || {
            PluginError::new(
                ErrorCode::InvalidManifest,
                "Invalid plugin.toml; check the V2 manifest specification",
            )
        };
        if raw.len() > MAX_MANIFEST_BYTES {
            return Err(invalid());
        }
        let value: toml::Value = toml::from_str(raw).map_err(|_| invalid())?;
        if value
            .get("schema_version")
            .and_then(toml::Value::as_integer)
            != Some(i64::from(MANIFEST_SCHEMA))
        {
            return Err(PluginError::new(
                ErrorCode::Incompatible,
                "Unsupported plugin manifest schema",
            ));
        }
        let manifest: Self = value.try_into().map_err(|_| invalid())?;
        manifest.validate(host_version)?;
        Ok(manifest)
    }

    pub fn validate(&self, host_version: &str) -> PluginResult<()> {
        let invalid = || {
            PluginError::new(
                ErrorCode::InvalidManifest,
                "Invalid plugin manifest metadata or action declaration",
            )
        };
        validate_id(&self.id)?;
        validate_id(&self.publisher)?;
        if self.id.starts_with("nyaterm-native-") {
            return Err(PluginError::new(
                ErrorCode::InvalidManifest,
                "Official native component IDs are reserved",
            ));
        }
        if self.schema_version != MANIFEST_SCHEMA || self.api_version != API_VERSION {
            return Err(PluginError::new(
                ErrorCode::Incompatible,
                "Unsupported plugin schema or API version",
            ));
        }
        Version::parse(&self.version).map_err(|_| invalid())?;
        let host = Version::parse(host_version).map_err(|_| invalid())?;
        let requirement = VersionReq::parse(&self.host_version).map_err(|_| invalid())?;
        if !requirement.matches(&host) {
            return Err(PluginError::new(
                ErrorCode::Incompatible,
                "Plugin requires a different NyaTerm version",
            ));
        }
        let mut capabilities = BTreeSet::new();
        for capability in &self.capabilities {
            crate::plugins::extension::Capability::parse(capability).map_err(|_| invalid())?;
            if !capabilities.insert(capability) {
                return Err(invalid());
            }
        }
        if self.name.trim().is_empty()
            || self.name.len() > 160
            || self.description.len() > 4096
            || self.authors.len() > 16
            || self.authors.iter().any(|a| a.len() > 160)
            || self.actions.is_empty()
            || self.actions.len() > MAX_ACTIONS
        {
            return Err(invalid());
        }
        if let Some(path) = &self.component {
            validate_resource_path(path)?;
        }
        let mut ids = BTreeSet::new();
        for action in &self.actions {
            validate_id(&action.id)?;
            if !ids.insert(&action.id) {
                return Err(PluginError::new(
                    ErrorCode::Conflict,
                    "Duplicate plugin contribution ID",
                ));
            }
            if action.name.trim().is_empty()
                || action.name.len() > 160
                || action.description.len() > 4096
                || action.parameters.len() > MAX_PARAMETERS
            {
                return Err(invalid());
            }
            match &action.template {
                Some(path) => {
                    validate_resource_path(path)?;
                    if action.result != ResultKind::Command {
                        return Err(invalid());
                    }
                }
                None if self.component.is_none() => return Err(invalid()),
                None => {}
            }
            let mut parameters = BTreeSet::new();
            for parameter in &action.parameters {
                validate_id(&parameter.id)?;
                if !parameters.insert(&parameter.id) || parameter.name.len() > 160 {
                    return Err(invalid());
                }
                if let Some(value) = &parameter.default {
                    crate::plugins::invocation::parse_parameter(parameter.kind, value)?;
                }
            }
        }
        for contribution in self
            .contributions
            .terminal_menu
            .iter()
            .chain(&self.contributions.file_menu)
            .chain(&self.contributions.session_connected)
            .chain(&self.contributions.session_disconnected)
        {
            if !ids.contains(contribution) {
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub fn contribution_id(&self, action: &ActionManifest) -> String {
        format!("{}:{}", self.id, action.id)
    }
}
