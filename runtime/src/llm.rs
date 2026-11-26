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
    /// The API key is attached only to an endpoint the user chose (or the provider's own
    /// built-in endpoint), never to an address that came from an app package.
    #[serde(skip)]
    pub key_allowed: bool,
}

#[allow(dead_code)]
/// True for http(s) URLs on this machine: localhost, 127.0.0.0/8 and ::1.
pub fn is_loopback(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else { return false };
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    let Some(h) = u.host_str() else { return false };
    let h = h.trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    h.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

fn norm(e: &str) -> String {
    e.trim().trim_end_matches('/').to_string()
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
///
/// An app package may choose the provider and the model, but it may not send your requests
/// somewhere you did not choose: an `endpoint` in app.json is honoured only when it is on this
/// machine (localhost) or is one you configured yourself. Your API key goes only to endpoints
/// you configured or to the provider's own built-in endpoint.
pub fn resolve(m: Option<&Manifest>, override_model: Option<&str>) -> Resolved {
    let cfg = store::load_config();
    let app = m.map(|m| &m.llm);
    let user_provider = cfg.provider.clone().unwrap_or_else(|| "ollama".into());
    // An app may pick a provider only among those the user set up (their default, or one they
    // configured with `deskhatch config --provider`): otherwise a web page could spend the
    // user's paid key on a provider they never chose for it.
    let provider = app
        .and_then(|l| l.provider.clone())
        .filter(|p| p == &user_provider || cfg.providers.contains_key(p))
        .unwrap_or(user_provider);
    let pc = cfg.providers.get(&provider);
    let user_endpoints: Vec<String> = [
        pc.and_then(|p| p.endpoint.clone()),
        cfg.endpoint.clone(),
        Some(default_endpoint(&provider).to_string()).filter(|e| !e.is_empty()),
    ]
    .into_iter()
    .flatten()
    .map(|e| norm(&e))
    .collect();
    // An endpoint from app.json is used only when it is exactly one the user configured or the
    // provider's built-in one. Anything else, local addresses included, could reach other
    // services on this machine.
    let app_endpoint = app.and_then(|l| l.endpoint.clone()).map(|e| norm(&e)).filter(|e| {
        let ok = user_endpoints.iter().any(|u| u == e);
        if !ok {
            eprintln!(
                "deskhatch: ignoring the model endpoint {e} from app.json; only endpoints you configured are used"
            );
        }
        ok
    });
    let endpoint = app_endpoint
        .or_else(|| pc.and_then(|p| p.endpoint.clone()))
        .or_else(|| cfg.endpoint.clone())
        .unwrap_or_else(|| default_endpoint(&provider).to_string());
    let endpoint = norm(&endpoint);
    let key_allowed = user_endpoints.iter().any(|u| u == &endpoint);
    let model = override_model
        .map(|s| s.to_string())
        .or_else(|| app.and_then(|l| l.model.clone()))
        .or_else(|| pc.and_then(|p| p.model.clone()))
        .or_else(|| cfg.model.clone())
        .unwrap_or_else(|| default_model(&provider).to_string());
    Resolved {
        has_key: store::get_api_key(&provider).is_some(),
        provider,
        endpoint,
        model,
        key_allowed,
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
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client
        .post(format!("{}/chat/completions", r.endpoint))
        .header("content-type", "application/json")
        .header("user-agent", "deskhatch/0.1");
    if r.key_allowed {
        if let Some(k) = store::get_api_key(&r.provider) {
            req = req.bearer_auth(k);
        }
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
        // Only a short, printable excerpt reaches the page.
        let excerpt: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
        eprintln!("deskhatch: model endpoint answered {status}: {excerpt}");
        return Err(format!("the model endpoint answered {status}{hint}"));
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
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(format!("{}/models", r.endpoint));
    if r.key_allowed {
        if let Some(k) = store::get_api_key(&r.provider) {
            req = req.bearer_auth(k);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_detection() {
        assert!(is_loopback("http://localhost:11434/v1"));
        assert!(is_loopback("http://127.0.0.1:8080/v1"));
        assert!(is_loopback("http://[::1]:1234/v1"));
        assert!(!is_loopback("https://attacker.example/v1"));
        assert!(!is_loopback("http://localhost.attacker.example/v1"));
        assert!(!is_loopback("http://10.0.0.5/v1"));
        assert!(!is_loopback("file:///etc/passwd"));
    }

    #[test]
    fn app_json_cannot_redirect_the_model_or_the_key() {
        let tmp = std::env::temp_dir().join(format!("deskhatch-llm-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("DESKHATCH_HOME", &tmp);
        let m: Manifest = serde_json::from_str(
            r#"{"id":"t","name":"t","llm":{"provider":"openai","endpoint":"https://attacker.example/v1"}}"#,
        )
        .unwrap();
        let r = resolve(Some(&m), None);
        assert_ne!(r.endpoint, "https://attacker.example/v1", "a foreign endpoint from app.json must be ignored");
        assert!(r.key_allowed, "the key may go to the user's own endpoint");

        let local: Manifest = serde_json::from_str(
            r#"{"id":"t","name":"t","llm":{"provider":"openai","endpoint":"http://localhost:9999/v1"}}"#,
        )
        .unwrap();
        let r = resolve(Some(&local), None);
        assert_ne!(r.endpoint, "http://localhost:9999/v1", "a local endpoint the user did not configure is ignored too");
        let ssrf: Manifest = serde_json::from_str(
            r#"{"id":"t","name":"t","llm":{"endpoint":"http://127.0.0.1:8765/admin/secret"}}"#,
        )
        .unwrap();
        assert_eq!(resolve(Some(&ssrf), None).endpoint, "http://localhost:11434/v1");
        let paid: Manifest = serde_json::from_str(r#"{"id":"t","name":"t","llm":{"provider":"openai"}}"#).unwrap();
        assert_eq!(resolve(Some(&paid), None).provider, "ollama", "an app cannot switch to a provider the user did not set up");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
