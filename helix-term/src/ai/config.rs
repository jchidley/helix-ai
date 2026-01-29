use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Stream,
    Tail,
}

impl TransportMode {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "stream" => Some(Self::Stream),
            "tail" => Some(Self::Tail),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AiConfig {
    pub general: GeneralConfig,
    pub drivers: HashMap<String, DriverConfig>,
}

#[derive(Debug, Clone)]
pub struct GeneralConfig {
    pub default_driver: String,
    pub output_buffer: String,
    pub input_buffer: String,
}

#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub mode: TransportMode,
    pub cmd: Vec<String>,
    pub session_dir: PathBuf,
    pub session_glob: String,
}

#[derive(Debug, Default, Deserialize)]
struct AiConfigFile {
    general: Option<GeneralConfigFile>,
    drivers: Option<HashMap<String, DriverConfigFile>>,
}

#[derive(Debug, Default, Deserialize)]
struct GeneralConfigFile {
    default_driver: Option<String>,
    output_buffer: Option<String>,
    input_buffer: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DriverConfigFile {
    mode: Option<String>,
    cmd: Option<Vec<String>>,
    session_dir: Option<String>,
    session_glob: Option<String>,
}

impl AiConfig {
    pub fn load() -> Result<Self> {
        let config_path = helix_loader::config_dir().join("ai.toml");
        let workspace_path = helix_loader::find_workspace()
            .0
            .join(".helix")
            .join("ai.toml");
        let parsed = read_config(&config_path)?;
        let parsed_workspace = read_config(&workspace_path)?;

        let mut general = GeneralConfig {
            default_driver: "pi".to_string(),
            output_buffer: "ai/output".to_string(),
            input_buffer: "ai/input".to_string(),
        };

        apply_general_overrides(&mut general, parsed.general);
        apply_general_overrides(&mut general, parsed_workspace.general);

        let mut drivers = default_drivers();
        apply_driver_overrides(&mut drivers, parsed.drivers);
        apply_driver_overrides(&mut drivers, parsed_workspace.drivers);

        Ok(Self { general, drivers })
    }
}

fn default_drivers() -> HashMap<String, DriverConfig> {
    let mut drivers = HashMap::new();

    drivers.insert(
        "pi".to_string(),
        DriverConfig {
            mode: TransportMode::Stream,
            cmd: vec!["pi".to_string(), "--mode".to_string(), "rpc".to_string()],
            session_dir: expand_path("~/.pi/agent/sessions"),
            session_glob: "--*--/*.jsonl".to_string(),
        },
    );

    drivers.insert(
        "codex".to_string(),
        DriverConfig {
            mode: TransportMode::Tail,
            cmd: vec!["codex".to_string(), "resume".to_string()],
            session_dir: expand_path("~/.codex/sessions"),
            session_glob: "**/*.jsonl".to_string(),
        },
    );

    drivers.insert(
        "claude".to_string(),
        DriverConfig {
            mode: TransportMode::Tail,
            cmd: vec!["claude".to_string(), "--resume".to_string()],
            session_dir: expand_path("~/.claude/projects"),
            session_glob: "-*/agent-*.jsonl".to_string(),
        },
    );

    drivers
}

fn read_config(path: &Path) -> Result<AiConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(contents) => toml::from_str::<AiConfigFile>(&contents)
            .with_context(|| format!("failed to parse {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(AiConfigFile::default()),
        Err(err) => Err(err).context(format!("failed to read {}", path.display())),
    }
}

fn apply_general_overrides(general: &mut GeneralConfig, overrides: Option<GeneralConfigFile>) {
    let Some(gen) = overrides else {
        return;
    };
    if let Some(driver) = gen.default_driver {
        general.default_driver = driver;
    }
    if let Some(buffer) = gen.output_buffer {
        general.output_buffer = buffer;
    }
    if let Some(buffer) = gen.input_buffer {
        general.input_buffer = buffer;
    }
}

fn apply_driver_overrides(
    drivers: &mut HashMap<String, DriverConfig>,
    overrides: Option<HashMap<String, DriverConfigFile>>,
) {
    let Some(overrides) = overrides else {
        return;
    };
    for (name, entry) in overrides {
        if let Some(driver) = drivers.get_mut(&name) {
            apply_driver_override(driver, entry);
        } else {
            let mut driver = DriverConfig {
                mode: TransportMode::Tail,
                cmd: Vec::new(),
                session_dir: PathBuf::new(),
                session_glob: "**/*.jsonl".to_string(),
            };
            apply_driver_override(&mut driver, entry);
            drivers.insert(name, driver);
        }
    }
}

fn apply_driver_override(driver: &mut DriverConfig, entry: DriverConfigFile) {
    if let Some(mode) = entry.mode.and_then(|m| TransportMode::parse(&m)) {
        driver.mode = mode;
    }
    if let Some(cmd) = entry.cmd {
        driver.cmd = cmd;
    }
    if let Some(dir) = entry.session_dir {
        driver.session_dir = expand_path(&dir);
    }
    if let Some(glob) = entry.session_glob {
        driver.session_glob = glob;
    }
}

fn expand_path(input: impl AsRef<Path>) -> PathBuf {
    helix_stdx::path::expand_tilde(input.as_ref()).to_path_buf()
}

pub fn require_driver<'a>(config: &'a AiConfig, name: &str) -> Result<&'a DriverConfig> {
    config
        .drivers
        .get(name)
        .ok_or_else(|| anyhow!("unknown ai driver '{name}'"))
}
