use crate::{
    config::{Agent, Config, GlobalConfig},
    utils::*,
};

use anyhow::{anyhow, bail, Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[cfg(windows)]
const PATH_SEP: &str = ";";
#[cfg(not(windows))]
const PATH_SEP: &str = ":";

const MAX_TOOL_OUTPUT_CHARS: usize = 12_000;

fn truncate_tool_output(output: String) -> String {
    match output.char_indices().nth(MAX_TOOL_OUTPUT_CHARS) {
        Some((byte_idx, _)) => {
            let omitted = output.chars().count() - MAX_TOOL_OUTPUT_CHARS;
            format!(
                "{}\n\n[TRUNCATED: {} characters omitted (limit: {}). Narrow your query or use line ranges.]",
                &output[..byte_idx],
                omitted,
                MAX_TOOL_OUTPUT_CHARS
            )
        }
        None => output,
    }
}

pub fn eval_tool_calls(config: &GlobalConfig, mut calls: Vec<ToolCall>) -> Result<Vec<ToolResult>> {
    let mut output = vec![];
    if calls.is_empty() {
        return Ok(output);
    }
    calls = ToolCall::dedup(calls);
    if calls.is_empty() {
        bail!("The request was aborted because an infinite loop of function calls was detected.")
    }
    let mut is_all_null = true;
    for call in calls {
        if call.name == "load_toolkit" {
            let tk_name = call
                .arguments
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut cfg = config.write();
            let result = match cfg.toolkits.get(tk_name) {
                Some(tk) => {
                    let tools = tk.tools.clone();
                    cfg.active_toolkits.insert(tk_name.to_string());
                    json!({"status": "loaded", "toolkit": tk_name, "tools": tools})
                }
                None => json!({"error": format!("Unknown toolkit '{tk_name}'")}),
            };
            is_all_null = false;
            output.push(ToolResult::new(call, result));
            continue;
        }
        let mut result = match call.eval(config) {
            Ok(result) => result,
            Err(err) => json!({"error": err.to_string()}),
        };
        if result.is_null() {
            result = json!("DONE");
        } else {
            is_all_null = false;
        }
        output.push(ToolResult::new(call, result));
    }
    if is_all_null {
        output = vec![];
    }
    Ok(output)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolResult {
    pub call: ToolCall,
    pub output: Value,
}

impl ToolResult {
    pub fn new(call: ToolCall, output: Value) -> Self {
        Self { call, output }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Functions {
    declarations: Vec<FunctionDeclaration>,
}

impl Functions {
    pub fn init(declarations_path: &Path) -> Result<Self> {
        let declarations: Vec<FunctionDeclaration> = if declarations_path.exists() {
            let ctx = || {
                format!(
                    "Failed to load functions at {}",
                    declarations_path.display()
                )
            };
            let content = fs::read_to_string(declarations_path).with_context(ctx)?;
            serde_json::from_str(&content).with_context(ctx)?
        } else {
            vec![]
        };

        Ok(Self { declarations })
    }

    pub fn find(&self, name: &str) -> Option<&FunctionDeclaration> {
        self.declarations.iter().find(|v| v.name == name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.declarations.iter().any(|v| v.name == name)
    }

    pub fn declarations(&self) -> &[FunctionDeclaration] {
        &self.declarations
    }

    pub fn is_empty(&self) -> bool {
        self.declarations.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: JsonSchema,
    #[serde(skip_serializing, default)]
    pub agent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchema {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<IndexMap<String, JsonSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<JsonSchema>>,
    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<JsonSchema>>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_value: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
}

impl JsonSchema {
    pub fn is_empty_properties(&self) -> bool {
        match &self.properties {
            Some(v) => v.is_empty(),
            None => true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Toolkit {
    pub description: String,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Toolkits {
    map: IndexMap<String, Toolkit>,
}

impl Toolkits {
    pub fn init(path: &Path) -> Result<Self> {
        let map: IndexMap<String, Toolkit> = if path.exists() {
            let ctx = || format!("Failed to load toolkits at {}", path.display());
            let content = fs::read_to_string(path).with_context(ctx)?;
            serde_json::from_str(&content).with_context(ctx)?
        } else {
            IndexMap::new()
        };
        Ok(Self { map })
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Toolkit> {
        self.map.get(name)
    }

    pub fn contains_tool(&self, tool_name: &str) -> bool {
        self.map
            .values()
            .any(|tk| tk.tools.iter().any(|t| t == tool_name))
    }

    pub fn is_tool_active(&self, tool_name: &str, active: &HashSet<String>) -> bool {
        active.iter().any(|tk_name| {
            self.map
                .get(tk_name)
                .is_some_and(|tk| tk.tools.iter().any(|t| t == tool_name))
        })
    }

    pub fn loader_declaration(&self, active: &HashSet<String>) -> Option<FunctionDeclaration> {
        let available: Vec<(&String, &Toolkit)> = self
            .map
            .iter()
            .filter(|(name, _)| !active.contains(*name))
            .collect();
        if available.is_empty() {
            return None;
        }
        let summary = available
            .iter()
            .map(|(k, v)| format!("- {k}: {}", v.description))
            .collect::<Vec<_>>()
            .join("\n");
        let enum_values = available.iter().map(|(k, _)| (*k).clone()).collect();

        let mut props = IndexMap::new();
        props.insert(
            "name".to_string(),
            JsonSchema {
                type_value: Some("string".to_string()),
                description: Some("Toolkit name to load".to_string()),
                properties: None,
                items: None,
                any_of: None,
                enum_value: Some(enum_values),
                default: None,
                required: None,
            },
        );

        Some(FunctionDeclaration {
            name: "load_toolkit".to_string(),
            description: format!("Load a toolkit to unlock its tools for the current session.\nAvailable toolkits:\n{summary}"),
            parameters: JsonSchema {
                type_value: Some("object".to_string()),
                description: None,
                properties: Some(props),
                items: None,
                any_of: None,
                enum_value: None,
                default: None,
                required: Some(vec!["name".to_string()]),
            },
            agent: false,
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
    pub id: Option<String>,
}

type CallConfig = (String, String, Vec<String>, HashMap<String, String>);

impl ToolCall {
    pub fn dedup(calls: Vec<Self>) -> Vec<Self> {
        let mut new_calls = vec![];
        let mut seen_ids = HashSet::new();

        for call in calls.into_iter().rev() {
            if let Some(id) = &call.id {
                if !seen_ids.contains(id) {
                    seen_ids.insert(id.clone());
                    new_calls.push(call);
                }
            } else {
                new_calls.push(call);
            }
        }

        new_calls.reverse();
        new_calls
    }

    pub fn new(name: String, arguments: Value, id: Option<String>) -> Self {
        Self {
            name,
            arguments,
            id,
        }
    }

    pub fn eval(&self, config: &GlobalConfig) -> Result<Value> {
        let (call_name, cmd_name, mut cmd_args, envs) = match &config.read().agent {
            Some(agent) => self.extract_call_config_from_agent(config, agent)?,
            None => self.extract_call_config_from_config(config)?,
        };

        let json_data = if self.arguments.is_object() {
            self.arguments.clone()
        } else if let Some(arguments) = self.arguments.as_str() {
            let arguments: Value = serde_json::from_str(arguments).map_err(|_| {
                anyhow!("The call '{call_name}' has invalid arguments: {arguments}")
            })?;
            arguments
        } else {
            bail!(
                "The call '{call_name}' has invalid arguments: {}",
                self.arguments
            );
        };

        cmd_args.push(json_data.to_string());

        let output = match run_llm_function(cmd_name, cmd_args, envs)? {
            (0, Some(contents)) => {
                let contents = truncate_tool_output(contents);
                serde_json::from_str(&contents)
                    .ok()
                    .unwrap_or_else(|| json!({"output": contents}))
            }
            (0, None) => Value::Null,
            (exit_code, contents) => {
                let contents = contents.map(truncate_tool_output);
                json!({"error": format!("Tool call exit with {exit_code}"), "output": contents})
            }
        };

        Ok(output)
    }

    fn extract_call_config_from_agent(
        &self,
        config: &GlobalConfig,
        agent: &Agent,
    ) -> Result<CallConfig> {
        let function_name = self.name.clone();
        match agent.functions().find(&function_name) {
            Some(function) => {
                let agent_name = agent.name().to_string();
                if function.agent {
                    Ok((
                        format!("{agent_name}-{function_name}"),
                        agent_name,
                        vec![function_name],
                        agent.variable_envs(),
                    ))
                } else {
                    Ok((
                        function_name.clone(),
                        function_name,
                        vec![],
                        Default::default(),
                    ))
                }
            }
            None => self.extract_call_config_from_config(config),
        }
    }

    fn extract_call_config_from_config(&self, config: &GlobalConfig) -> Result<CallConfig> {
        let function_name = self.name.clone();
        match config.read().functions.contains(&function_name) {
            true => Ok((
                function_name.clone(),
                function_name,
                vec![],
                Default::default(),
            )),
            false => bail!("Unexpected call: {function_name} {}", self.arguments),
        }
    }
}

pub fn run_llm_function(
    cmd_name: String,
    cmd_args: Vec<String>,
    mut envs: HashMap<String, String>,
) -> Result<(i32, Option<String>)> {
    let prompt = format!("Call {cmd_name} {}", cmd_args.join(" "));

    let mut bin_dirs: Vec<PathBuf> = vec![];
    if cmd_args.len() > 1 {
        let dir = Config::agent_functions_dir(&cmd_name).join("bin");
        if dir.exists() {
            bin_dirs.push(dir);
        }
    }
    bin_dirs.push(Config::functions_bin_dir());
    let current_path = std::env::var("PATH").context("No PATH environment variable")?;
    let prepend_path = bin_dirs
        .iter()
        .map(|v| format!("{}{PATH_SEP}", v.display()))
        .collect::<Vec<_>>()
        .join("");
    envs.insert("PATH".into(), format!("{prepend_path}{current_path}"));

    let temp_file = temp_file("-eval-", "");
    envs.insert("LLM_OUTPUT".into(), temp_file.display().to_string());

    #[cfg(windows)]
    let cmd_name = polyfill_cmd_name(&cmd_name, &bin_dirs);
    if *IS_STDOUT_TERMINAL {
        println!("{}", dimmed_text(&prompt));
    }
    let exit_code = run_command(&cmd_name, &cmd_args, Some(envs))
        .map_err(|err| anyhow!("Unable to run {cmd_name}, {err}"))?;
    let mut output = None;
    if temp_file.exists() {
        let contents =
            fs::read_to_string(temp_file).context("Failed to retrieve tool call output")?;
        if !contents.is_empty() {
            output = Some(contents);
        }
    };
    Ok((exit_code, output))
}

#[cfg(windows)]
fn polyfill_cmd_name<T: AsRef<Path>>(cmd_name: &str, bin_dir: &[T]) -> String {
    let cmd_name = cmd_name.to_string();
    if let Ok(exts) = std::env::var("PATHEXT") {
        for name in exts.split(';').map(|ext| format!("{cmd_name}{ext}")) {
            for dir in bin_dir {
                let path = dir.as_ref().join(&name);
                if path.exists() {
                    return name.to_string();
                }
            }
        }
    }
    cmd_name
}
