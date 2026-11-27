use super::openai::*;
use super::*;

use crate::utils::strip_think_tag;

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use reqwest::RequestBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

const API_BASE: &str = "https://api.mistral.ai/v1";

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MistralConfig {
    pub name: Option<String>,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelData>,
    pub patch: Option<RequestPatch>,
    pub extra: Option<ExtraConfig>,
}

impl MistralClient {
    config_get_fn!(api_key, get_api_key);
    config_get_fn!(api_base, get_api_base);

    pub const PROMPTS: [PromptAction<'static>; 1] = [("api_key", "API Key", None)];
}

impl_client_trait!(
    MistralClient,
    (
        prepare_chat_completions,
        mistral_chat_completions,
        mistral_chat_completions_streaming
    ),
    (prepare_embeddings, openai_embeddings),
    (noop_prepare_rerank, noop_rerank),
);

fn prepare_chat_completions(
    self_: &MistralClient,
    data: ChatCompletionsData,
) -> Result<RequestData> {
    let api_key = self_.get_api_key()?;
    let api_base = self_
        .get_api_base()
        .unwrap_or_else(|_| API_BASE.to_string());

    let url = format!("{}/chat/completions", api_base.trim_end_matches('/'));
    let body = mistral_build_chat_completions_body(data, &self_.model);

    let mut request_data = RequestData::new(url, body);
    request_data.bearer_auth(api_key);

    Ok(request_data)
}

fn prepare_embeddings(self_: &MistralClient, data: &EmbeddingsData) -> Result<RequestData> {
    let api_key = self_.get_api_key()?;
    let api_base = self_
        .get_api_base()
        .unwrap_or_else(|_| API_BASE.to_string());

    let url = format!("{}/embeddings", api_base.trim_end_matches('/'));
    let body = openai_build_embeddings_body(data, &self_.model);

    let mut request_data = RequestData::new(url, body);
    request_data.bearer_auth(api_key);

    Ok(request_data)
}

pub async fn mistral_chat_completions(
    builder: RequestBuilder,
    _model: &Model,
) -> Result<ChatCompletionsOutput> {
    let res = builder.send().await?;
    let status = res.status();
    let data: Value = res.json().await?;
    if !status.is_success() {
        catch_error(&data, status.as_u16())?;
    }

    debug!("non-stream-data: {data}");
    mistral_extract_chat_completion(&data)
}

pub async fn mistral_chat_completions_streaming(
    builder: RequestBuilder,
    handler: &mut SseHandler,
    _model: &Model,
) -> Result<()> {
    let mut state = StreamState::default();
    let handle = |message: SseMmessage| -> Result<bool> {
        if message.data == "[DONE]" {
            return Ok(true);
        }
        let data: Value = serde_json::from_str(&message.data)?;
        debug!("stream-data: {data}");
        state.process(&data, handler)
    };

    sse_stream(builder, handle).await?;
    state.finish(handler)
}

pub fn mistral_build_chat_completions_body(data: ChatCompletionsData, model: &Model) -> Value {
    let ChatCompletionsData {
        messages,
        temperature,
        top_p,
        functions,
        stream,
    } = data;

    let mut messages_json: Vec<Value> = vec![];
    let mut call_index = 0;
    let mut tool_call_id = |id: &Option<String>| match id {
        Some(id) => id.clone(),
        None => {
            call_index += 1;
            format!("call{call_index:05}")
        }
    };

    for Message { role, content } in messages {
        match content {
            MessageContent::ToolCalls(MessageContentToolCalls {
                tool_results,
                text,
                sequence,
            }) => {
                let text = strip_think_tag(&text);
                let text = text.trim();
                let pairs: Vec<(Value, Value)> = tool_results
                    .iter()
                    .map(|tool_result| {
                        let id = tool_call_id(&tool_result.call.id);
                        (
                            json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": tool_result.call.name,
                                    "arguments": tool_result.call.arguments.to_string(),
                                },
                            }),
                            json!({
                                "role": "tool",
                                "tool_call_id": id,
                                "name": tool_result.call.name,
                                "content": tool_result.output.to_string(),
                            }),
                        )
                    })
                    .collect();

                if sequence {
                    for (i, (call, result)) in pairs.into_iter().enumerate() {
                        let mut assistant_msg = json!({
                            "role": "assistant",
                            "tool_calls": [call],
                        });
                        if i == 0 && !text.is_empty() {
                            assistant_msg["content"] = text.into();
                        }
                        messages_json.push(assistant_msg);
                        messages_json.push(result);
                    }
                } else {
                    let (calls, results): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                    let mut assistant_msg = json!({
                        "role": "assistant",
                        "tool_calls": calls,
                    });
                    if !text.is_empty() {
                        assistant_msg["content"] = text.into();
                    }
                    messages_json.push(assistant_msg);
                    messages_json.extend(results);
                }
            }
            content if role.is_system() => {
                let text = content.to_text();
                if !text.is_empty() {
                    messages_json.push(json!({
                        "role": "system",
                        "content": text,
                    }));
                }
            }
            content if role.is_assistant() => {
                let raw = content.to_text();
                let text = strip_think_tag(&raw);
                messages_json.push(json!({
                    "role": "assistant",
                    "content": text,
                }));
            }
            MessageContent::Text(text) => {
                messages_json.push(json!({
                    "role": "user",
                    "content": text,
                }));
            }
            MessageContent::Array(parts) => {
                let chunks: Vec<Value> = parts
                    .into_iter()
                    .map(|part| match part {
                        MessageContentPart::Text { text } => json!({
                            "type": "text",
                            "text": text,
                        }),
                        MessageContentPart::ImageUrl { image_url } => json!({
                            "type": "image_url",
                            "image_url": {
                                "url": image_url.url,
                            },
                        }),
                    })
                    .collect();
                messages_json.push(json!({
                    "role": "user",
                    "content": chunks,
                }));
            }
        }
    }

    let mut body = json!({
        "model": model.real_name(),
        "messages": messages_json,
    });

    if let Some(v) = model.max_tokens_param() {
        body["max_tokens"] = v.into();
    }
    if let Some(v) = temperature {
        body["temperature"] = v.into();
    }
    if let Some(v) = top_p {
        body["top_p"] = v.into();
    }
    if let Some(functions) = functions {
        if !functions.is_empty() {
            let tools: Vec<Value> = functions
                .iter()
                .map(|v| {
                    json!({
                        "type": "function",
                        "function": v,
                    })
                })
                .collect();
            body["tools"] = tools.into();
        }
    }
    if stream {
        body["stream"] = true.into();
        body["stream_options"] = json!({
            "include_usage": true,
        });
    }

    body
}

pub fn mistral_extract_chat_completion(data: &Value) -> Result<ChatCompletionsOutput> {
    let message = &data["choices"][0]["message"];
    if message.is_null() {
        bail!("Invalid response data: {data}");
    }

    let mut text = String::new();
    let mut tool_calls = vec![];
    {
        let mut renderer = OutputRenderer::default();
        let mut out = |v: &str| -> Result<()> {
            text.push_str(v);
            Ok(())
        };
        renderer.content(&message["content"], &mut out)?;
        renderer.finish(&mut out)?;
    }

    if let Some(calls) = message["tool_calls"].as_array() {
        for call in calls {
            let function = &call["function"];
            let name = function["name"].as_str().unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let arguments = parse_arguments(name, &function["arguments"])?;
            let id = call["id"].as_str().map(|v| v.to_string());
            tool_calls.push(ToolCall::new(name.to_string(), arguments, id));
        }
    }

    if text.is_empty() && tool_calls.is_empty() {
        bail!("Invalid response data: {data}");
    }

    Ok(ChatCompletionsOutput {
        text,
        tool_calls,
        id: data["id"].as_str().map(|v| v.to_string()),
        input_tokens: data["usage"]["prompt_tokens"].as_u64(),
        output_tokens: data["usage"]["completion_tokens"].as_u64(),
        cached_tokens: data["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64(),
    })
}

fn parse_arguments(name: &str, arguments: &Value) -> Result<Value> {
    match arguments {
        Value::String(v) if v.trim().is_empty() => Ok(json!({})),
        Value::String(v) => v
            .parse()
            .with_context(|| format!("Tool call '{name}' have non-JSON arguments '{v}'")),
        Value::Null => Ok(json!({})),
        v => Ok(v.clone()),
    }
}

#[derive(Debug, Default)]
struct OutputRenderer {
    thinking: bool,
    references: Vec<(String, String)>,
}

impl OutputRenderer {
    fn content(&mut self, content: &Value, out: &mut dyn FnMut(&str) -> Result<()>) -> Result<()> {
        match content {
            Value::String(text) => self.text(text, out),
            Value::Array(chunks) => {
                for chunk in chunks {
                    self.chunk(chunk, out)?;
                }
                Ok(())
            }
            Value::Object(_) => self.chunk(content, out),
            _ => Ok(()),
        }
    }

    fn chunk(&mut self, chunk: &Value, out: &mut dyn FnMut(&str) -> Result<()>) -> Result<()> {
        match chunk["type"].as_str() {
            Some("text") => self.text(chunk["text"].as_str().unwrap_or_default(), out),
            Some("thinking") => {
                let mut parts = vec![];
                match &chunk["thinking"] {
                    Value::String(v) => parts.push(v.as_str()),
                    Value::Array(list) => {
                        parts.extend(list.iter().filter_map(|v| v["text"].as_str()))
                    }
                    _ => {}
                }
                for part in parts.into_iter().filter(|v| !v.is_empty()) {
                    if !self.thinking {
                        out("<think>\n")?;
                        self.thinking = true;
                    }
                    out(part)?;
                }
                Ok(())
            }
            Some("tool_reference") => {
                let index = self.add_reference(chunk);
                self.text(&format!("[{index}]"), out)
            }
            Some("tool_file") => {
                let file_id = chunk["file_id"].as_str().unwrap_or_default();
                let file_name = chunk["file_name"].as_str().unwrap_or(file_id);
                self.text(&format!("[file: {file_name} ({file_id})]"), out)
            }
            _ => Ok(()),
        }
    }

    fn text(&mut self, text: &str, out: &mut dyn FnMut(&str) -> Result<()>) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.close_thinking(out)?;
        out(text)
    }

    fn close_thinking(&mut self, out: &mut dyn FnMut(&str) -> Result<()>) -> Result<()> {
        if self.thinking {
            out("\n</think>\n\n")?;
            self.thinking = false;
        }
        Ok(())
    }

    fn add_reference(&mut self, chunk: &Value) -> usize {
        let title = chunk["title"].as_str().unwrap_or_default();
        let url = chunk["url"].as_str().unwrap_or_default();
        let key = if url.is_empty() { title } else { url };
        if let Some(index) = self.references.iter().position(|(k, _)| k == key) {
            return index + 1;
        }
        let display = match (title.is_empty(), url.is_empty()) {
            (false, false) => format!("[{title}]({url})"),
            (true, false) => url.to_string(),
            _ => title.to_string(),
        };
        self.references.push((key.to_string(), display));
        self.references.len()
    }

    fn finish(&mut self, out: &mut dyn FnMut(&str) -> Result<()>) -> Result<()> {
        self.close_thinking(out)?;
        if !self.references.is_empty() {
            let lines: Vec<String> = self
                .references
                .iter()
                .enumerate()
                .map(|(i, (_, display))| format!("- [{}] {display}", i + 1))
                .collect();
            out(&format!("\n\nSources:\n{}\n", lines.join("\n")))?;
            self.references.clear();
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct StreamState {
    renderer: OutputRenderer,
    function_calls: IndexMap<usize, (Option<String>, String, String)>,
    usage: Option<(u64, u64, u64)>,
}

impl StreamState {
    fn process(&mut self, data: &Value, handler: &mut SseHandler) -> Result<bool> {
        if let Some(err_msg) = data["error"]["message"].as_str() {
            bail!("{err_msg}");
        }

        if !data["usage"].is_null() {
            let input = data["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
            let cached = data["usage"]["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0);
            let output = data["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            self.usage = Some((input, cached, output));
        }

        let delta = &data["choices"][0]["delta"];
        if !delta["content"].is_null() {
            self.renderer
                .content(&delta["content"], &mut |v| handler.text(v))?;
        }

        if let Some(calls) = delta["tool_calls"].as_array() {
            for call in calls {
                let index = call["index"].as_u64().unwrap_or(0) as usize;
                let entry = self.function_calls.entry(index).or_default();
                if let Some(id) = call["id"].as_str().filter(|v| !v.is_empty()) {
                    entry.0 = Some(id.to_string());
                }
                let function = &call["function"];
                if let Some(name) = function["name"].as_str().filter(|v| !v.is_empty()) {
                    entry.1.push_str(name);
                }
                match &function["arguments"] {
                    Value::String(v) => entry.2.push_str(v),
                    Value::Null => {}
                    v => entry.2.push_str(&v.to_string()),
                }
            }
        }

        Ok(false)
    }

    fn finish(&mut self, handler: &mut SseHandler) -> Result<()> {
        self.renderer.finish(&mut |v| handler.text(v))?;
        for (_, (id, name, arguments)) in std::mem::take(&mut self.function_calls) {
            if name.is_empty() {
                continue;
            }
            let arguments = parse_arguments(&name, &Value::String(arguments))?;
            handler.tool_call(ToolCall::new(name, arguments, id))?;
        }
        if let Some((input, cached, output)) = self.usage.take() {
            handler.usage(input, cached, output);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function::{FunctionDeclaration, ToolResult};
    use crate::utils::create_abort_signal;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    fn model() -> Model {
        Model::new("mistral", "mistral-large-latest")
    }

    fn data(messages: Vec<Message>, stream: bool) -> ChatCompletionsData {
        ChatCompletionsData {
            messages,
            temperature: Some(0.3),
            top_p: None,
            functions: None,
            stream,
        }
    }

    #[test]
    fn test_build_body() {
        let call = ToolCall::new(
            "get_weather".into(),
            json!({"city": "Paris"}),
            Some("abc123XYZ".into()),
        );
        let messages = vec![
            Message::new(MessageRole::System, MessageContent::Text("Be brief".into())),
            Message::new(MessageRole::User, MessageContent::Text("Hi".into())),
            Message::new(
                MessageRole::Assistant,
                MessageContent::Text("<think>\nhmm\n</think>\n\nHello".into()),
            ),
            Message::new(
                MessageRole::User,
                MessageContent::Array(vec![
                    MessageContentPart::Text {
                        text: "What is this?".into(),
                    },
                    MessageContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/png;base64,AAA".into(),
                        },
                    },
                ]),
            ),
            Message::new(
                MessageRole::Assistant,
                MessageContent::ToolCalls(MessageContentToolCalls::new(
                    vec![ToolResult::new(call, json!({"temp": 20}))],
                    String::new(),
                )),
            ),
        ];
        let mut data = data(messages, true);
        data.functions = Some(vec![serde_json::from_value::<FunctionDeclaration>(json!({
            "name": "get_weather",
            "description": "Get weather",
            "parameters": {"type": "object", "properties": {}}
        }))
        .unwrap()]);
        let body = mistral_build_chat_completions_body(data, &model());
        assert_eq!(body["model"], "mistral-large-latest");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert_eq!(body["temperature"], 0.3);
        assert!(body.get("store").is_none());
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(
            body["messages"],
            json!([
                {"role": "system", "content": "Be brief"},
                {"role": "user", "content": "Hi"},
                {"role": "assistant", "content": "Hello"},
                {"role": "user", "content": [
                    {"type": "text", "text": "What is this?"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}
                ]},
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "abc123XYZ",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Paris\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "abc123XYZ",
                    "name": "get_weather",
                    "content": "{\"temp\":20}"
                }
            ])
        );
    }

    #[test]
    fn test_extract_chat_completion() {
        let data = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": [{"type": "text", "text": "Let me think."}]},
                        {"type": "text", "text": "Done."}
                    ],
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "notify", "arguments": "{\"x\":1}"}
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 25,
                "prompt_tokens_details": {"cached_tokens": 80}
            }
        });
        let output = mistral_extract_chat_completion(&data).unwrap();
        assert_eq!(output.text, "<think>\nLet me think.\n</think>\n\nDone.");
        assert_eq!(output.id.as_deref(), Some("chatcmpl_1"));
        assert_eq!(output.input_tokens, Some(100));
        assert_eq!(output.output_tokens, Some(25));
        assert_eq!(output.cached_tokens, Some(80));
        assert_eq!(output.tool_calls.len(), 1);
    }

    #[test]
    fn test_stream_events_with_usage() {
        let (tx, mut rx) = unbounded_channel();
        let mut handler = SseHandler::new(tx, create_abort_signal());
        let mut state = StreamState::default();
        let events = [
            json!({"choices": [{"delta": {"content": [{"type": "thinking", "thinking": [{"type": "text", "text": "ponder"}]}]}}]}),
            json!({"choices": [{"delta": {"content": "Hel"}}]}),
            json!({
                "choices": [{"delta": {"content": "lo"}, "finish_reason": "stop"}],
                "usage": {
                    "prompt_tokens": 120,
                    "completion_tokens": 15,
                    "prompt_tokens_details": {"cached_tokens": 90}
                }
            }),
        ];
        for event in events {
            assert!(!state.process(&event, &mut handler).unwrap());
        }
        state.finish(&mut handler).unwrap();
        rx.close();
        let (text, calls, usage) = handler.take_with_usage();
        assert_eq!(text, "<think>\nponder\n</think>\n\nHello");
        assert!(calls.is_empty());
        assert_eq!(usage, Some((120, 90, 15)));
    }
}
