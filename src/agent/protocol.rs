use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::actions::{normalize_patch_name, AgentAppAction, AgentSessionContext};
use super::tools::{AgentToolRegistry, ExampleKind, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallOutcome {
    pub name: String,
    pub ok: bool,
    pub summary: String,
    pub content: String,
    #[serde(default)]
    pub pending_actions: Vec<AgentAppAction>,
}

pub struct AgentToolRuntime {
    registry: AgentToolRegistry,
}

impl AgentToolRuntime {
    pub fn load_default() -> Result<Self, String> {
        Ok(Self {
            registry: AgentToolRegistry::load_default()?,
        })
    }

    pub fn new(registry: AgentToolRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &AgentToolRegistry {
        &self.registry
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "lookup_dgen_docs".to_string(),
                description: "Look up DGenLisp operators, attributes, and related examples."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Single operator, attribute, topic, or example search term." },
                        "queries": {
                            "type": "array",
                            "description": "List of operators, attributes, topics, or example search terms to look up in one call.",
                            "items": { "type": "string" }
                        },
                        "limit": { "type": "integer", "minimum": 1, "default": 5 }
                    },
                    "anyOf": [
                        { "required": ["query"] },
                        { "required": ["queries"] }
                    ]
                }),
            },
            ToolSpec {
                name: "list_examples".to_string(),
                description:
                    "List available local DGenLisp instrument or effect examples from this repo."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["any", "instrument", "effect"],
                            "default": "any"
                        },
                        "limit": { "type": "integer", "minimum": 1, "default": 20 }
                    }
                }),
            },
            ToolSpec {
                name: "read_example".to_string(),
                description:
                    "Read the full source of a known indexed instrument or effect example."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Indexed example name, e.g. prophet-5." }
                    },
                    "required": ["name"]
                }),
            },
            ToolSpec {
                name: "read_patch_source".to_string(),
                description:
                    "Read a patch source file directly by kind and base name from instruments/ or effects/."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["instrument", "effect"]
                        },
                        "name": { "type": "string", "description": "Patch base name without .lisp suffix." }
                    },
                    "required": ["kind", "name"]
                }),
            },
            ToolSpec {
                name: "create_instrument_track".to_string(),
                description:
                    "Create a new instrument track from generated DGenLisp instrument source."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Short patch name used for saving and the new track label." },
                        "source": { "type": "string", "description": "Complete DGenLisp instrument source code." }
                    },
                    "required": ["name", "source"]
                }),
            },
            ToolSpec {
                name: "read_current_instrument_source".to_string(),
                description:
                    "Read the current track's custom instrument source so you can iterate on it instead of rewriting from scratch."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolSpec {
                name: "update_current_instrument".to_string(),
                description:
                    "Replace the current custom instrument track's source, save it, and hot-reload it in place."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Instrument name to save under and show on the current track." },
                        "source": { "type": "string", "description": "Complete replacement DGenLisp instrument source code." }
                    },
                    "required": ["name", "source"]
                }),
            },
            ToolSpec {
                name: "read_current_effect_source".to_string(),
                description:
                    "Read the currently selected custom effect source so you can iterate on it instead of adding another effect."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolSpec {
                name: "apply_effect_to_current_track".to_string(),
                description:
                    "Apply generated DGenLisp effect source to the current track using the next free custom effect slot."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Short patch name used for saving the effect." },
                        "source": { "type": "string", "description": "Complete DGenLisp effect source code." }
                    },
                    "required": ["name", "source"]
                }),
            },
            ToolSpec {
                name: "update_current_effect".to_string(),
                description:
                    "Replace the currently selected custom effect slot's source, save it, and reload it in place."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Effect name to save under and show for the current slot." },
                        "source": { "type": "string", "description": "Complete replacement DGenLisp effect source code." }
                    },
                    "required": ["name", "source"]
                }),
            },
        ]
    }

    pub fn execute(&self, call: ToolCall, session: &AgentSessionContext) -> ToolCallOutcome {
        let result = match call.name.as_str() {
            "lookup_dgen_docs" => self.execute_lookup_docs(&call.arguments),
            "list_examples" => self.execute_list_examples(&call.arguments),
            "read_example" => self.execute_read_example(&call.arguments),
            "read_patch_source" => self.execute_read_patch_source(&call.arguments),
            "create_instrument_track" => self.execute_create_instrument_track(&call.arguments),
            "read_current_instrument_source" => {
                self.execute_read_current_instrument_source(session)
            }
            "update_current_instrument" => {
                self.execute_update_current_instrument(&call.arguments, session)
            }
            "read_current_effect_source" => self.execute_read_current_effect_source(session),
            "apply_effect_to_current_track" => {
                self.execute_apply_effect_to_current_track(&call.arguments, session)
            }
            "update_current_effect" => self.execute_update_current_effect(&call.arguments, session),
            _ => Err(format!("Unknown tool '{}'.", call.name)),
        };

        match result {
            Ok(result) => ToolCallOutcome {
                name: call.name,
                ok: true,
                summary: result.summary,
                content: result.content,
                pending_actions: result.pending_actions,
            },
            Err(error) => ToolCallOutcome {
                name: call.name,
                ok: false,
                summary: error.clone(),
                content: error,
                pending_actions: Vec::new(),
            },
        }
    }

    fn execute_lookup_docs(&self, arguments: &Value) -> Result<ToolResult, String> {
        let queries = lookup_queries(arguments)?;
        let limit = optional_usize(arguments, "limit").unwrap_or(5);
        Ok(self.registry.lookup_dgen_docs(&queries, limit))
    }

    fn execute_list_examples(&self, arguments: &Value) -> Result<ToolResult, String> {
        let kind = optional_kind(arguments, "kind")?.unwrap_or(ExampleKind::Any);
        let limit = optional_usize(arguments, "limit").unwrap_or(20);
        Ok(self.registry.list_examples(kind, limit))
    }

    fn execute_read_example(&self, arguments: &Value) -> Result<ToolResult, String> {
        let name = required_string(arguments, "name")?;
        self.registry.read_example(name)
    }

    fn execute_read_patch_source(&self, arguments: &Value) -> Result<ToolResult, String> {
        let kind = optional_kind(arguments, "kind")?
            .ok_or_else(|| "Missing required string field 'kind'.".to_string())?;
        if kind == ExampleKind::Any {
            return Err("Field 'kind' must be 'instrument' or 'effect'.".to_string());
        }
        let name = required_string(arguments, "name")?;
        self.registry.read_patch_source(kind, name)
    }

    fn execute_create_instrument_track(&self, arguments: &Value) -> Result<ToolResult, String> {
        let name =
            normalize_patch_name(required_string(arguments, "name")?, "generated-instrument");
        let source = required_string(arguments, "source")?;
        Ok(ToolResult {
            summary: format!("Queued creation of instrument track '{}'.", name),
            content: format!(
                "Create a new instrument track from generated source '{}'.",
                name
            ),
            pending_actions: vec![AgentAppAction::CreateInstrumentTrack {
                name,
                source: source.to_string(),
            }],
        })
    }

    fn execute_read_current_instrument_source(
        &self,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        let name = session.current_instrument_name.as_deref().ok_or_else(|| {
            "No current custom instrument track is selected. Create or select a custom instrument track first."
                .to_string()
        })?;
        let source = session
            .current_instrument_source
            .as_deref()
            .ok_or_else(|| {
                format!(
                    "Current instrument '{}' does not have readable source.",
                    name
                )
            })?;
        Ok(ToolResult {
            summary: format!("Loaded current instrument source for '{}'.", name),
            content: source.to_string(),
            pending_actions: Vec::new(),
        })
    }

    fn execute_update_current_instrument(
        &self,
        arguments: &Value,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        if !session.can_update_current_instrument {
            return Err(
                "The current track is not a custom instrument track. Create or select a custom instrument track first."
                    .to_string(),
            );
        }
        let name =
            normalize_patch_name(required_string(arguments, "name")?, "generated-instrument");
        let source = required_string(arguments, "source")?;
        let track = session
            .current_track_name
            .as_deref()
            .unwrap_or("current track");
        Ok(ToolResult {
            summary: format!("Queued instrument update '{}' for {}.", name, track),
            content: format!(
                "Update the current instrument track '{}' using '{}'.",
                track, name
            ),
            pending_actions: vec![AgentAppAction::UpdateCurrentInstrument {
                name,
                source: source.to_string(),
            }],
        })
    }

    fn execute_apply_effect_to_current_track(
        &self,
        arguments: &Value,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        if !session.has_tracks {
            return Err(
                "No current track is available. Ask the user to create a track first, then apply the effect."
                    .to_string(),
            );
        }
        if !session.can_apply_effect_to_current_track {
            let track = session
                .current_track_name
                .as_deref()
                .unwrap_or("current track");
            return Err(format!(
                "Track '{}' has no free custom effect slot. Ask the user to free a slot or choose another track.",
                track
            ));
        }
        let name = normalize_patch_name(required_string(arguments, "name")?, "generated-effect");
        let source = required_string(arguments, "source")?;
        let track = session
            .current_track_name
            .as_deref()
            .unwrap_or("current track");
        Ok(ToolResult {
            summary: format!("Queued effect '{}' for {}.", name, track),
            content: format!("Apply generated effect '{}' to {}.", name, track),
            pending_actions: vec![AgentAppAction::ApplyEffectToCurrentTrack {
                name,
                source: source.to_string(),
            }],
        })
    }

    fn execute_read_current_effect_source(
        &self,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        let name = session.current_effect_name.as_deref().ok_or_else(|| {
            "No current custom effect slot is selected. Select a custom effect slot first."
                .to_string()
        })?;
        let source = session
            .current_effect_source
            .as_deref()
            .ok_or_else(|| format!("Current effect '{}' does not have readable source.", name))?;
        Ok(ToolResult {
            summary: format!("Loaded current effect source for '{}'.", name),
            content: source.to_string(),
            pending_actions: Vec::new(),
        })
    }

    fn execute_update_current_effect(
        &self,
        arguments: &Value,
        session: &AgentSessionContext,
    ) -> Result<ToolResult, String> {
        if !session.can_update_current_effect {
            return Err(
                "No current custom effect slot is selected. Select a custom effect slot first."
                    .to_string(),
            );
        }
        let name = normalize_patch_name(required_string(arguments, "name")?, "generated-effect");
        let source = required_string(arguments, "source")?;
        let target = session
            .current_effect_name
            .as_deref()
            .unwrap_or("current effect");
        Ok(ToolResult {
            summary: format!("Queued effect update '{}' for {}.", name, target),
            content: format!("Replace current effect '{}' using '{}'.", target, name),
            pending_actions: vec![AgentAppAction::UpdateCurrentEffect {
                name,
                source: source.to_string(),
            }],
        })
    }
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Missing required string field '{key}'."))
}

fn optional_usize(value: &Value, key: &str) -> Option<usize> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

fn optional_kind(value: &Value, key: &str) -> Result<Option<ExampleKind>, String> {
    match value.get(key).and_then(Value::as_str) {
        Some(raw) => ExampleKind::from_wire_value(raw).map(Some),
        None => Ok(None),
    }
}

fn lookup_queries(value: &Value) -> Result<Vec<String>, String> {
    if let Some(query) = value
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        return Ok(vec![query.to_string()]);
    }

    if let Some(items) = value.get("queries").and_then(Value::as_array) {
        let queries = items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if !queries.is_empty() {
            return Ok(queries);
        }
    }

    Err("Missing required string field 'query' or non-empty string array field 'queries'.".to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AgentSessionContext, AgentToolRuntime, ToolCall};

    #[test]
    fn specs_include_lookup_docs() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let names: Vec<String> = runtime.specs().into_iter().map(|spec| spec.name).collect();
        assert!(names.contains(&"lookup_dgen_docs".to_string()));
        assert!(names.contains(&"read_example".to_string()));
    }

    #[test]
    fn execute_lookup_docs_returns_success() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "lookup_dgen_docs".to_string(),
                arguments: json!({ "query": "compressor", "limit": 2 }),
            },
            &AgentSessionContext {
                has_tracks: false,
                current_track_name: None,
                current_track_index: None,
                can_apply_effect_to_current_track: false,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
            },
        );
        assert!(outcome.ok);
        assert!(outcome.content.contains("compressor"));
    }

    #[test]
    fn execute_lookup_docs_accepts_multiple_queries() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "lookup_dgen_docs".to_string(),
                arguments: json!({ "queries": ["biquad", "compressor"], "limit": 2 }),
            },
            &AgentSessionContext {
                has_tracks: false,
                current_track_name: None,
                current_track_index: None,
                can_apply_effect_to_current_track: false,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
            },
        );
        assert!(outcome.ok);
        assert!(outcome.content.contains("query: biquad"));
        assert!(outcome.content.contains("query: compressor"));
    }

    #[test]
    fn execute_unknown_tool_returns_error() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "not_real".to_string(),
                arguments: json!({}),
            },
            &AgentSessionContext {
                has_tracks: false,
                current_track_name: None,
                current_track_index: None,
                can_apply_effect_to_current_track: false,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
            },
        );
        assert!(!outcome.ok);
        assert!(outcome.summary.contains("Unknown tool"));
    }

    #[test]
    fn apply_effect_requires_existing_track() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "apply_effect_to_current_track".to_string(),
                arguments: json!({
                    "name": "wash",
                    "source": "(effect ...)"
                }),
            },
            &AgentSessionContext {
                has_tracks: false,
                current_track_name: None,
                current_track_index: None,
                can_apply_effect_to_current_track: false,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
            },
        );
        assert!(!outcome.ok);
        assert!(outcome.summary.contains("create a track first"));
    }

    #[test]
    fn read_current_instrument_source_requires_custom_track() {
        let runtime = AgentToolRuntime::load_default().expect("load runtime");
        let outcome = runtime.execute(
            ToolCall {
                name: "read_current_instrument_source".to_string(),
                arguments: json!({}),
            },
            &AgentSessionContext {
                has_tracks: true,
                current_track_name: Some("kick".to_string()),
                current_track_index: Some(0),
                can_apply_effect_to_current_track: true,
                current_effect_name: None,
                current_effect_source: None,
                current_effect_slot: None,
                can_update_current_effect: false,
                current_instrument_name: None,
                current_instrument_source: None,
                can_update_current_instrument: false,
            },
        );
        assert!(!outcome.ok);
        assert!(outcome.summary.contains("custom instrument track"));
    }
}
