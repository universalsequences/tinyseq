use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Value};

use super::actions::{AgentAppAction, AgentSessionContext};
use super::protocol::{AgentToolRuntime, ToolCall, ToolCallOutcome, ToolSpec};
use super::providers::{AgentMessage, AgentMessageRole, AgentProviderKind, AgentTurnRequest};

const OPENAI_CHAT_COMPLETIONS_URL: &str = "https://api.openai.com/v1/chat/completions";
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const MAX_TOOL_ROUNDS: usize = 8;
const MAX_REPEAT_TOOL_FAILURES: usize = 2;
const MAX_REPEAT_TOOL_CALL_ROUNDS: usize = 2;

#[derive(Debug, Clone)]
pub struct AgentTurnResult {
    pub text: String,
    pub tool_outcomes: Vec<ToolCallOutcome>,
    pub pending_actions: Vec<AgentAppAction>,
}

pub struct AgentNetworkClient {
    http: Client,
    tools: AgentToolRuntime,
}

impl AgentNetworkClient {
    pub fn load_default() -> Result<Self, String> {
        let http = Client::builder()
            .build()
            .map_err(|error| format!("Failed to build HTTP client: {error}"))?;
        Ok(Self {
            http,
            tools: AgentToolRuntime::load_default()?,
        })
    }

    pub fn execute_turn(
        &self,
        provider: AgentProviderKind,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
        session_context: AgentSessionContext,
    ) -> Result<AgentTurnResult, String> {
        let request = AgentTurnRequest {
            model: model.to_string(),
            system_prompt: system_prompt.to_string(),
            messages: messages.to_vec(),
            tools: self.tools.specs(),
            session_context,
        };

        match provider {
            AgentProviderKind::OpenAi => self.execute_openai_turn(&request),
            AgentProviderKind::Gemini => self.execute_gemini_turn(&request),
        }
    }

    fn execute_openai_turn(&self, request: &AgentTurnRequest) -> Result<AgentTurnResult, String> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| "Missing required OPENAI_API_KEY.".to_string())?;
        let mut messages = openai_messages(request);
        let tools = openai_tools(&request.tools);
        let mut tool_outcomes = Vec::new();
        let mut pending_actions = Vec::new();
        let mut last_failure_signature = None::<String>;
        let mut repeated_failure_rounds = 0usize;
        let mut last_tool_signature = None::<String>;
        let mut repeated_tool_call_rounds = 0usize;

        for _ in 0..MAX_TOOL_ROUNDS {
            let payload = json!({
                "model": request.model,
                "messages": messages,
                "tools": tools,
                "tool_choice": "auto"
            });
            let response: OpenAiChatCompletionResponse = self
                .http
                .post(OPENAI_CHAT_COMPLETIONS_URL)
                .header(AUTHORIZATION, format!("Bearer {api_key}"))
                .header(CONTENT_TYPE, "application/json")
                .json(&payload)
                .send()
                .map_err(|error| format!("OpenAI request failed: {error}"))?
                .error_for_status()
                .map_err(|error| format!("OpenAI request failed: {error}"))?
                .json()
                .map_err(|error| format!("Failed to decode OpenAI response: {error}"))?;

            let message = response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| "OpenAI returned no choices.".to_string())?
                .message;

            let tool_calls = message.tool_calls.unwrap_or_default();
            let assistant_content = message.content.unwrap_or_default();
            if !tool_calls.is_empty() {
                let tool_signature = openai_tool_call_signature(&tool_calls);
                if last_tool_signature.as_deref() == Some(tool_signature.as_str()) {
                    repeated_tool_call_rounds += 1;
                } else {
                    repeated_tool_call_rounds = 1;
                    last_tool_signature = Some(tool_signature.clone());
                }
                if repeated_tool_call_rounds >= MAX_REPEAT_TOOL_CALL_ROUNDS {
                    return Err(format!(
                        "Agent repeated the same tool-call plan: {tool_signature}"
                    ));
                }

                messages.push(json!({
                    "role": "assistant",
                    "content": assistant_content,
                    "tool_calls": tool_calls,
                }));

                let mut round_outcomes = Vec::new();
                for tool_call in tool_calls {
                    let call = ToolCall {
                        name: tool_call.function.name.clone(),
                        arguments: if tool_call.function.arguments.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&tool_call.function.arguments).map_err(
                                |error| {
                                    format!(
                                        "OpenAI tool arguments for '{}' were invalid JSON: {error}",
                                        tool_call.function.name
                                    )
                                },
                            )?
                        },
                    };
                    let outcome = self.tools.execute(call, &request.session_context);
                    pending_actions.extend(outcome.pending_actions.clone());
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_call.id,
                        "content": outcome.content.clone(),
                    }));
                    round_outcomes.push(outcome.clone());
                    tool_outcomes.push(outcome);
                }

                if round_outcomes
                    .iter()
                    .any(|outcome| !outcome.pending_actions.is_empty())
                {
                    return Ok(AgentTurnResult {
                        text: assistant_content,
                        tool_outcomes,
                        pending_actions,
                    });
                }

                if let Some(signature) = repeated_failure_signature(&round_outcomes) {
                    if last_failure_signature.as_deref() == Some(signature.as_str()) {
                        repeated_failure_rounds += 1;
                    } else {
                        repeated_failure_rounds = 1;
                        last_failure_signature = Some(signature.clone());
                    }
                    if repeated_failure_rounds >= MAX_REPEAT_TOOL_FAILURES {
                        return Err(format!(
                            "Agent repeated the same failing tool call: {signature}"
                        ));
                    }
                } else {
                    repeated_failure_rounds = 0;
                    last_failure_signature = None;
                }
                continue;
            }

            return Ok(AgentTurnResult {
                text: assistant_content,
                tool_outcomes,
                pending_actions,
            });
        }

        Err("OpenAI tool loop exceeded maximum rounds.".to_string())
    }

    fn execute_gemini_turn(&self, request: &AgentTurnRequest) -> Result<AgentTurnResult, String> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .map_err(|_| "Missing required GEMINI_API_KEY.".to_string())?;
        let endpoint = format!("{GEMINI_API_BASE}/{}:generateContent", request.model);
        let mut contents = gemini_contents(request);
        let tools = gemini_tools(&request.tools);
        let mut tool_outcomes = Vec::new();
        let mut pending_actions = Vec::new();
        let mut last_failure_signature = None::<String>;
        let mut repeated_failure_rounds = 0usize;
        let mut last_tool_signature = None::<String>;
        let mut repeated_tool_call_rounds = 0usize;

        for _ in 0..MAX_TOOL_ROUNDS {
            let payload = json!({
                "systemInstruction": {
                    "parts": [{ "text": request.system_prompt }]
                },
                "contents": contents,
                "tools": [{
                    "functionDeclarations": tools
                }]
            });

            let response = self
                .http
                .post(&endpoint)
                .header("x-goog-api-key", api_key.clone())
                .header(CONTENT_TYPE, "application/json")
                .json(&payload)
                .send()
                .map_err(|error| format!("Gemini request failed: {error}"))?;
            let status = response.status();
            if !status.is_success() {
                let body = response
                    .text()
                    .unwrap_or_else(|_| "<failed to read response body>".to_string());
                return Err(format!("Gemini request failed: HTTP {status} body: {body}"));
            }
            let response: GeminiGenerateContentResponse = response
                .json()
                .map_err(|error| format!("Failed to decode Gemini response: {error}"))?;

            let candidate = response
                .candidates
                .into_iter()
                .next()
                .ok_or_else(|| "Gemini returned no candidates.".to_string())?;
            let content = candidate
                .content
                .ok_or_else(|| "Gemini returned no content.".to_string())?;
            let function_calls = extract_gemini_function_calls(&content.parts);
            let assistant_text = extract_gemini_text(&content.parts);

            contents.push(json!({
                "role": content.role.unwrap_or_else(|| "model".to_string()),
                "parts": content.parts,
            }));

            if function_calls.is_empty() {
                return Ok(AgentTurnResult {
                    text: assistant_text,
                    tool_outcomes,
                    pending_actions,
                });
            }

            let tool_signature = gemini_tool_call_signature(&function_calls);
            if last_tool_signature.as_deref() == Some(tool_signature.as_str()) {
                repeated_tool_call_rounds += 1;
            } else {
                repeated_tool_call_rounds = 1;
                last_tool_signature = Some(tool_signature.clone());
            }
            if repeated_tool_call_rounds >= MAX_REPEAT_TOOL_CALL_ROUNDS {
                return Err(format!(
                    "Agent repeated the same tool-call plan: {tool_signature}"
                ));
            }

            let mut response_parts = Vec::new();
            let mut round_outcomes = Vec::new();
            for function_call in function_calls {
                let outcome = self.tools.execute(
                    ToolCall {
                        name: function_call.name.clone(),
                        arguments: function_call.args.clone().unwrap_or_else(|| json!({})),
                    },
                    &request.session_context,
                );
                pending_actions.extend(outcome.pending_actions.clone());
                response_parts.push(json!({
                    "functionResponse": {
                        "name": function_call.name,
                        "response": {
                            "summary": outcome.summary,
                            "content": outcome.content,
                            "ok": outcome.ok
                        }
                    }
                }));
                round_outcomes.push(outcome.clone());
                tool_outcomes.push(outcome);
            }

            if round_outcomes
                .iter()
                .any(|outcome| !outcome.pending_actions.is_empty())
            {
                return Ok(AgentTurnResult {
                    text: assistant_text,
                    tool_outcomes,
                    pending_actions,
                });
            }

            if let Some(signature) = repeated_failure_signature(&round_outcomes) {
                if last_failure_signature.as_deref() == Some(signature.as_str()) {
                    repeated_failure_rounds += 1;
                } else {
                    repeated_failure_rounds = 1;
                    last_failure_signature = Some(signature.clone());
                }
                if repeated_failure_rounds >= MAX_REPEAT_TOOL_FAILURES {
                    return Err(format!(
                        "Agent repeated the same failing tool call: {signature}"
                    ));
                }
            } else {
                repeated_failure_rounds = 0;
                last_failure_signature = None;
            }

            contents.push(json!({
                "role": "user",
                "parts": response_parts,
            }));
        }

        Err("Gemini tool loop exceeded maximum rounds.".to_string())
    }
}

fn openai_messages(request: &AgentTurnRequest) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system_prompt,
    })];
    for message in &request.messages {
        let role = match message.role {
            AgentMessageRole::System => "system",
            AgentMessageRole::User => "user",
            AgentMessageRole::Assistant => "assistant",
            AgentMessageRole::Tool => "tool",
        };
        let mut object = json!({
            "role": role,
            "content": message.content,
        });
        if matches!(message.role, AgentMessageRole::Tool) {
            object["name"] = match &message.tool_name {
                Some(name) => json!(name),
                None => Value::Null,
            };
        }
        messages.push(object);
    }
    messages
}

fn openai_tools(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            json!({
                "type": "function",
                "function": {
                    "name": spec.name,
                    "description": spec.description,
                    "parameters": spec.input_schema,
                    "strict": true
                }
            })
        })
        .collect()
}

fn gemini_contents(request: &AgentTurnRequest) -> Vec<Value> {
    request
        .messages
        .iter()
        .map(|message| {
            let role = match message.role {
                AgentMessageRole::Assistant => "model",
                _ => "user",
            };
            json!({
                "role": role,
                "parts": [{ "text": message.content }]
            })
        })
        .collect()
}

fn gemini_tools(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "parameters": sanitize_gemini_schema(&spec.input_schema),
            })
        })
        .collect()
}

fn sanitize_gemini_schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if key == "properties" {
                    if let Value::Object(properties) = child {
                        let mut sanitized_properties = serde_json::Map::new();
                        for (prop_name, prop_schema) in properties {
                            sanitized_properties
                                .insert(prop_name.clone(), sanitize_gemini_schema(prop_schema));
                        }
                        out.insert(key.clone(), Value::Object(sanitized_properties));
                    }
                    continue;
                }

                if matches!(
                    key.as_str(),
                    "type" | "description" | "required" | "enum" | "items"
                ) {
                    out.insert(key.clone(), sanitize_gemini_schema(child));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_gemini_schema).collect()),
        _ => value.clone(),
    }
}

fn extract_gemini_text(parts: &[GeminiPart]) -> String {
    parts
        .iter()
        .filter_map(|part| part.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_gemini_function_calls(parts: &[GeminiPart]) -> Vec<GeminiFunctionCall> {
    parts
        .iter()
        .filter_map(|part| part.function_call.clone())
        .collect()
}

fn repeated_failure_signature(outcomes: &[ToolCallOutcome]) -> Option<String> {
    if outcomes.is_empty() || outcomes.iter().any(|outcome| outcome.ok) {
        return None;
    }
    Some(
        outcomes
            .iter()
            .map(|outcome| format!("{}: {}", outcome.name, outcome.summary))
            .collect::<Vec<_>>()
            .join(" | "),
    )
}

fn openai_tool_call_signature(tool_calls: &[OpenAiToolCall]) -> String {
    tool_calls
        .iter()
        .map(|call| format!("{}({})", call.function.name, call.function.arguments))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn gemini_tool_call_signature(tool_calls: &[GeminiFunctionCall]) -> String {
    tool_calls
        .iter()
        .map(|call| {
            format!(
                "{}({})",
                call.name,
                call.args.clone().unwrap_or_else(|| json!({}))
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[derive(Debug, Deserialize)]
struct OpenAiChatCompletionResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiToolFunction,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
struct OpenAiToolFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct GeminiGenerateContentResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    role: Option<String>,
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
struct GeminiPart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
struct GeminiFunctionCall {
    name: String,
    args: Option<Value>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        extract_gemini_function_calls, extract_gemini_text, gemini_tool_call_signature,
        repeated_failure_signature, sanitize_gemini_schema, GeminiFunctionCall, GeminiPart,
    };
    use crate::agent::protocol::ToolCallOutcome;

    #[test]
    fn extract_gemini_parts() {
        let parts = vec![
            GeminiPart {
                text: Some("hi".to_string()),
                function_call: None,
                thought_signature: None,
            },
            GeminiPart {
                text: None,
                function_call: Some(GeminiFunctionCall {
                    name: "lookup_dgen_docs".to_string(),
                    args: Some(json!({"query": "biquad"})),
                }),
                thought_signature: Some("sig123".to_string()),
            },
        ];
        assert_eq!(extract_gemini_text(&parts), "hi");
        assert_eq!(extract_gemini_function_calls(&parts).len(), 1);
    }

    #[test]
    fn sanitize_gemini_schema_strips_extra_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "count",
                    "minimum": 1,
                    "default": 5
                }
            },
            "required": ["limit"],
            "default": {}
        });
        let sanitized = sanitize_gemini_schema(&schema);
        assert_eq!(sanitized["type"], json!("object"));
        assert!(sanitized["default"].is_null());
        assert!(sanitized["properties"]["limit"]["minimum"].is_null());
        assert!(sanitized["properties"]["limit"]["default"].is_null());
    }

    #[test]
    fn gemini_thought_signature_round_trips() {
        let json_value = json!({
            "text": null,
            "functionCall": {
                "name": "list_examples",
                "args": { "kind": "instrument" }
            },
            "thoughtSignature": "abc123"
        });
        let part: GeminiPart = serde_json::from_value(json_value).expect("deserialize part");
        let round_trip = serde_json::to_value(&part).expect("serialize part");
        assert_eq!(round_trip["thoughtSignature"], json!("abc123"));
    }

    #[test]
    fn repeated_failure_signature_only_triggers_for_all_failed_rounds() {
        let outcomes = vec![
            ToolCallOutcome {
                name: "update_current_instrument".to_string(),
                ok: false,
                summary: "compile failed".to_string(),
                content: "compile failed".to_string(),
                pending_actions: Vec::new(),
            },
            ToolCallOutcome {
                name: "read_current_instrument_source".to_string(),
                ok: false,
                summary: "no custom track".to_string(),
                content: "no custom track".to_string(),
                pending_actions: Vec::new(),
            },
        ];
        assert!(repeated_failure_signature(&outcomes).is_some());
    }

    #[test]
    fn gemini_tool_call_signature_is_stable() {
        let tool_calls = vec![
            GeminiFunctionCall {
                name: "lookup_dgen_docs".to_string(),
                args: Some(json!({"query": "horn"})),
            },
            GeminiFunctionCall {
                name: "list_examples".to_string(),
                args: Some(json!({"kind": "instrument"})),
            },
        ];
        let signature = gemini_tool_call_signature(&tool_calls);
        assert!(signature.contains("lookup_dgen_docs"));
        assert!(signature.contains("list_examples"));
    }
}
