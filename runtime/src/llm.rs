//! The model bridge: `window.hatch.llm.chat()` ends up here. Talks to any OpenAI-compatible
//! chat-completions endpoint (Ollama, LM Studio, llama.cpp server, OpenAI, OpenRouter, ...).

use crate::manifest::Manifest;
use crate::store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub has_key: bool,
}

fn default_endpoint(provider: &str) -> &'static str {
    match provider {
        "ollama" => "http://localhost:11434/v1",
        "openai" => "https://api.openai.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "lmstudio" => "http://localhost:1234/v1",
        "llamacpp" | "llama.cpp" => "http://localhost:8080/v1",
        "groq" => "https://api.groq.com/openai/v1",
        "mistral" => "https://api.mistral.ai/v1",
        "deepseek" => "https://api.deepseek.com/v1",
        _ => "",
    }
}

fn default_model(provider: &str) -> &'static str {
    match provider {
        "ollama" => "qwen2.5-coder:7b",
        "openai" => "gpt-4o-mini",
        "openrouter" => "openai/gpt-4o-mini",
        "groq" => "llama-3.3-70b-versatile",
        "mistral" => "mistral-small-latest",
        "deepseek" => "deepseek-chat",
        _ => "",
    }
}

/// Precedence: app manifest > runtime config (per provider) > runtime config (global) > built-in.
pub fn resolve(m: Option<&Manifest>, override_model: Option<&str>) -> Resolved {
    let cfg = store::load_config();
    let app = m.map(|m| &m.llm);
    let provider = app
        .and_then(|l| l.provider.clone())
        .or_else(|| cfg.provider.clone())
        .unwrap_or_else(|| "ollama".into());
    let pc = cfg.providers.get(&provider);
    let endpoint = app
        .and_then(|l| l.endpoint.clone())
        .or_else(|| pc.and_then(|p| p.endpoint.clone()))
        .or_else(|| cfg.endpoint.clone())
        .unwrap_or_else(|| default_endpoint(&provider).to_string());
    let model = override_model
        .map(|s| s.to_string())
        .or_else(|| app.and_then(|l| l.model.clone()))
        .or_else(|| pc.and_then(|p| p.model.clone()))
        .or_else(|| cfg.model.clone())
        .unwrap_or_else(|| default_model(&provider).to_string());
    Resolved {
        has_key: store::get_api_key(&provider).is_some(),
        provider,
        endpoint: endpoint.trim_end_matches('/').to_string(),
        model,
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatOptions {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system: Option<String>,
    /// Ask for a JSON object response (where the provider supports response_format).
    #[serde(default)]
    pub json: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatReply {
    pub text: String,
    pub model: String,
    pub provider: String,
    pub usage: Option<Value>,
}

pub async fn chat(
    m: Option<&Manifest>,
    messages: Vec<Value>,
    opts: ChatOptions,
) -> Result<ChatReply, String> {
    let r = resolve(m, opts.model.as_deref());
    if r.endpoint.is_empty() {
        return Err(format!(
            "no endpoint configured for provider {:?}; run: deskhatch config --provider {} --endpoint URL",
            r.provider, r.provider
        ));
    }
    if r.model.is_empty() {
        return Err(format!(
            "no model configured for provider {:?}; run: deskhatch config --provider {} --model NAME",
            r.provider, r.provider
        ));
    }

    let mut msgs: Vec<Value> = vec![];
    let system = opts
        .system
        .clone()
        .or_else(|| m.and_then(|m| m.llm.system.clone()));
    if let Some(s) = system {
        msgs.push(json!({"role": "system", "content": s}));
    }
    msgs.extend(messages);

    let mut body = json!({
        "model": r.model,
        "messages": msgs,
        "stream": false,
    });
    if let Some(t) = opts.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(mt) = opts.max_tokens {
        body["max_tokens"] = json!(mt);
    }
    if opts.json {
        body["response_format"] = json!({"type": "json_object"});
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client
        .post(format!("{}/chat/completions", r.endpoint))
        .header("content-type", "application/json")
        .header("user-agent", "deskhatch/0.1");
    if let Some(k) = store::get_api_key(&r.provider) {
        req = req.bearer_auth(k);
    }
    let resp = req.json(&body).send().await.map_err(|e| {
        if r.provider == "ollama" {
            format!("cannot reach Ollama at {} ({e}). Is `ollama serve` running?", r.endpoint)
        } else {
            format!("request to {} failed: {e}", r.endpoint)
        }
    })?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let hint = if status.as_u16() == 401 || status.as_u16() == 403 {
            format!(" (no or wrong API key for {}; run: deskhatch key {})", r.provider, r.provider)
        } else {
            String::new()
        };
        return Err(format!("{status}{hint}: {}", text.chars().take(500).collect::<String>()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON from model: {e}"))?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| v["message"]["content"].as_str().map(|s| s.to_string()))
        .ok_or_else(|| format!("unexpected response shape: {}", text.chars().take(300).collect::<String>()))?;
    Ok(ChatReply {
        text: content,
        model: v["model"].as_str().unwrap_or(&r.model).to_string(),
        provider: r.provider,
        usage: v.get("usage").cloned(),
    })
}

/// List models from the endpoint (GET /models), best effort.
pub async fn models(m: Option<&Manifest>) -> Result<Vec<String>, String> {
    let r = resolve(m, None);
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{}/models", r.endpoint));
    if let Some(k) = store::get_api_key(&r.provider) {
        req = req.bearer_auth(k);
    }
    let v: Value = req
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["id"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default())
}
