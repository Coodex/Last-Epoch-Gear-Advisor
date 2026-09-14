//! LLM access for the two AI features, provider-agnostic:
//!
//! * [`analyze_item`]: judge a hovered item from the tooltip crops (images)
//!   plus the deterministic verdict, the build's priorities and the character.
//! * [`analyze_guide`]: turn a pasted build guide into a [`GuideProfile`]
//!   (stat priorities as data) that the deterministic scorer then uses.
//!
//! Two wire formats cover every supported provider: the Anthropic Messages
//! API, and the OpenAI-compatible chat-completions API used by OpenAI, Kimi
//! (Moonshot) and DeepSeek. Raw HTTPS via `ureq` (rustls); no SDKs.
//! Every call costs tokens; guides are hashed and reused by
//! [`crate::build_library`] so a guide is analysed once.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::character_state::CharacterState;
use crate::compare::Verdict;
use crate::guide_profile::GuideProfile;

pub const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// One selectable model. `provider` is "anthropic", "openai", "kimi",
/// "deepseek" or "openai-compatible" (any chat-completions server).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelConfig {
    pub provider: String,
    pub model: String,
    /// Empty: read from the provider's environment variable.
    pub api_key: String,
    /// Empty: the provider's default endpoint.
    pub base_url: String,
    /// Send tooltip screenshots (models without vision get the OCR text only).
    pub vision: bool,
    /// Reasoning effort where supported: "low" | "medium" | "high"
    pub effort: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        ModelConfig { provider: "anthropic".into(), model: "claude-opus-5".into(), api_key: String::new(), base_url: String::new(), vision: true, effort: "medium".into() }
    }
}

impl ModelConfig {
    /// The default list the settings start with; keys come from the environment
    /// or the Builds & AI window.
    pub fn defaults() -> Vec<ModelConfig> {
        vec![
            ModelConfig { provider: "anthropic".into(), model: "claude-opus-5".into(), ..Default::default() },
            ModelConfig { provider: "anthropic".into(), model: "claude-sonnet-5".into(), ..Default::default() },
            ModelConfig { provider: "openai".into(), model: "gpt-5".into(), ..Default::default() },
            ModelConfig { provider: "kimi".into(), model: "kimi-k2.5".into(), ..Default::default() },
            ModelConfig { provider: "deepseek".into(), model: "deepseek-chat".into(), vision: false, ..Default::default() },
        ]
    }

    pub fn label(&self) -> String {
        format!("{} · {}", self.provider, self.model)
    }

    pub fn env_var(&self) -> &'static str {
        match self.provider.as_str() {
            "anthropic" => "ANTHROPIC_API_KEY",
            "openai" => "OPENAI_API_KEY",
            "kimi" | "moonshot" => "MOONSHOT_API_KEY",
            "deepseek" => "DEEPSEEK_API_KEY",
            _ => "LLM_API_KEY",
        }
    }

    pub fn default_base_url(&self) -> &'static str {
        match self.provider.as_str() {
            "anthropic" => "https://api.anthropic.com",
            "openai" => "https://api.openai.com/v1",
            "kimi" | "moonshot" => "https://api.moonshot.ai/v1",
            "deepseek" => "https://api.deepseek.com/v1",
            _ => "",
        }
    }

    pub fn is_anthropic(&self) -> bool {
        self.provider == "anthropic"
    }

    /// The configured key, else the provider's environment variable.
    pub fn resolve_key(&self) -> Option<String> {
        let configured = self.api_key.trim();
        if !configured.is_empty() {
            return Some(configured.to_string());
        }
        std::env::var(self.env_var()).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
    }

    pub fn has_key(&self) -> bool {
        self.resolve_key().is_some()
    }

    fn chat_url(&self) -> String {
        let base = if self.base_url.trim().is_empty() { self.default_base_url().to_string() } else { self.base_url.trim().trim_end_matches('/').to_string() };
        if self.is_anthropic() {
            if base.ends_with("/v1/messages") { base } else { format!("{base}/v1/messages") }
        } else if base.ends_with("/chat/completions") {
            base
        } else {
            format!("{base}/chat/completions")
        }
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    pub config: ModelConfig,
    pub api_key: String,
    pub timeout: Duration,
}

/// A piece of a user turn.
#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    /// PNG bytes with a caption
    Image { caption: String, png: Vec<u8> },
}

#[derive(Debug, Clone)]
pub struct Turn {
    /// "user" | "assistant"
    pub role: &'static str,
    pub parts: Vec<Part>,
}

pub struct Completion {
    pub text: String,
    pub usage: String,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "API {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

impl Client {
    pub fn new(config: ModelConfig) -> Result<Client> {
        if config.model.trim().is_empty() {
            bail!("the {} entry has no model name (Builds & AI window > Model)", config.provider);
        }
        if config.provider == "openai-compatible" && config.base_url.trim().is_empty() {
            bail!("an OpenAI-compatible model needs a base URL");
        }
        let api_key = config.resolve_key().ok_or_else(|| anyhow!("no API key for {} (set it in Builds & AI or the {} environment variable)", config.label(), config.env_var()))?;
        Ok(Client { config, api_key, timeout: Duration::from_secs(600) })
    }

    fn post(&self, url: &str, headers: &[(&str, &str)], body: &Value) -> Result<Value> {
        let agent = ureq::AgentBuilder::new().timeout(self.timeout).build();
        let mut req = agent.post(url).set("content-type", "application/json");
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.send_string(&body.to_string()) {
            Ok(resp) => {
                let text = resp.into_string().context("reading API response")?;
                serde_json::from_str(&text).context("API response is not JSON")
            }
            Err(ureq::Error::Status(code, resp)) => {
                let text = resp.into_string().unwrap_or_default();
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                    .unwrap_or(text);
                Err(anyhow!(ApiError { status: code, message }))
            }
            Err(err) => Err(anyhow!("API request failed: {err}")),
        }
    }

    /// One completion. `schema` (JSON schema for the answer) is used natively
    /// where the provider supports it and as a "JSON object" hint elsewhere.
    pub fn complete(&self, system: &str, turns: &[Turn], max_tokens: u32, schema: Option<&Value>) -> Result<Completion> {
        if self.config.is_anthropic() {
            self.complete_anthropic(system, turns, max_tokens, schema)
        } else {
            self.complete_openai(system, turns, max_tokens, schema)
        }
    }

    fn anthropic_parts(&self, parts: &[Part]) -> Vec<Value> {
        let mut out = Vec::new();
        for part in parts {
            match part {
                Part::Text(t) => out.push(json!({ "type": "text", "text": t })),
                Part::Image { caption, png } => {
                    out.push(json!({ "type": "text", "text": caption }));
                    if self.config.vision {
                        out.push(json!({
                            "type": "image",
                            "source": { "type": "base64", "media_type": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png) }
                        }));
                    } else {
                        out.push(json!({ "type": "text", "text": "(screenshot omitted: this model is configured without vision)" }));
                    }
                }
            }
        }
        out
    }

    fn complete_anthropic(&self, system: &str, turns: &[Turn], max_tokens: u32, schema: Option<&Value>) -> Result<Completion> {
        let messages: Vec<Value> = turns.iter().map(|t| json!({ "role": t.role, "content": self.anthropic_parts(&t.parts) })).collect();
        let base = json!({ "model": self.config.model, "max_tokens": max_tokens, "system": system, "messages": messages });
        let headers = [("x-api-key", self.api_key.as_str()), ("anthropic-version", ANTHROPIC_VERSION)];
        let url = self.config.chat_url();

        let mut body = base.clone();
        let mut output_config = json!({ "effort": effort_word(&self.config.effort) });
        if let Some(schema) = schema {
            output_config["format"] = json!({ "type": "json_schema", "schema": schema });
        }
        body["output_config"] = output_config;
        body["thinking"] = json!({ "type": "adaptive" });
        let response = match self.post(&url, &headers, &body) {
            Ok(v) => v,
            Err(err) => {
                // older models reject adaptive thinking / output_config: degrade to the plain request
                let rejected = err.downcast_ref::<ApiError>().map_or(false, |e| {
                    e.status == 400 && ["output_config", "thinking", "format", "effort"].iter().any(|w| e.message.contains(w))
                });
                if rejected { self.post(&url, &headers, &base)? } else { return Err(err) }
            }
        };
        let text = response["content"]
            .as_array()
            .map(|blocks| blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""))
            .unwrap_or_default();
        let usage = format!(
            "{}: {} in / {} out",
            response["model"].as_str().unwrap_or(&self.config.model),
            response["usage"]["input_tokens"].as_u64().unwrap_or(0),
            response["usage"]["output_tokens"].as_u64().unwrap_or(0)
        );
        Ok(Completion { text, usage })
    }

    fn openai_parts(&self, parts: &[Part]) -> Value {
        let mut out = Vec::new();
        for part in parts {
            match part {
                Part::Text(t) => out.push(json!({ "type": "text", "text": t })),
                Part::Image { caption, png } => {
                    out.push(json!({ "type": "text", "text": caption }));
                    if self.config.vision {
                        let data = base64::engine::general_purpose::STANDARD.encode(png);
                        out.push(json!({ "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{data}") } }));
                    } else {
                        out.push(json!({ "type": "text", "text": "(screenshot omitted: this model is configured without vision)" }));
                    }
                }
            }
        }
        // text-only turns as a plain string: every compatible server accepts that
        if out.iter().all(|p| p["type"] == "text") {
            return Value::String(out.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"));
        }
        Value::Array(out)
    }

    fn complete_openai(&self, system: &str, turns: &[Turn], max_tokens: u32, schema: Option<&Value>) -> Result<Completion> {
        let mut messages = vec![json!({ "role": "system", "content": system })];
        messages.extend(turns.iter().map(|t| json!({ "role": t.role, "content": self.openai_parts(&t.parts) })));
        let mut body = json!({ "model": self.config.model, "messages": messages });
        if self.config.provider == "openai" {
            body["max_completion_tokens"] = json!(max_tokens);
            body["reasoning_effort"] = json!(effort_word(&self.config.effort));
        } else {
            body["max_tokens"] = json!(max_tokens);
        }
        if schema.is_some() {
            body["response_format"] = json!({ "type": "json_object" });
        }
        let auth = format!("Bearer {}", self.api_key);
        let headers = [("authorization", auth.as_str())];
        let url = self.config.chat_url();
        let response = match self.post(&url, &headers, &body) {
            Ok(v) => v,
            Err(err) => {
                let (rejected, no_images) = err.downcast_ref::<ApiError>().map_or((false, false), |e| {
                    let m = e.message.to_lowercase();
                    (
                        e.status == 400 && ["reasoning_effort", "response_format", "max_completion_tokens", "max_tokens", "image", "vision", "content"].iter().any(|w| m.contains(w)),
                        m.contains("image") || m.contains("vision") || m.contains("content"),
                    )
                });
                if !rejected {
                    return Err(err);
                }
                if no_images && self.config.vision {
                    // the model does not take images: resend with the OCR text only
                    let mut text_only = self.clone();
                    text_only.config.vision = false;
                    return text_only.complete_openai(system, turns, max_tokens, schema);
                }
                // strip the optional parameters and try once more
                let mut plain = json!({ "model": self.config.model, "messages": body["messages"].clone() });
                if self.config.provider == "openai" {
                    plain["max_completion_tokens"] = json!(max_tokens);
                } else {
                    plain["max_tokens"] = json!(max_tokens);
                }
                self.post(&url, &headers, &plain)?
            }
        };
        let message = &response["choices"][0]["message"];
        let text = match &message["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""),
            _ => String::new(),
        };
        if text.is_empty() {
            let finish = response["choices"][0]["finish_reason"].as_str().unwrap_or("?");
            bail!("empty answer from {} (finish_reason {finish})", self.config.label());
        }
        let usage = format!(
            "{}: {} in / {} out",
            response["model"].as_str().unwrap_or(&self.config.model),
            response["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            response["usage"]["completion_tokens"].as_u64().unwrap_or(0)
        );
        Ok(Completion { text, usage })
    }
}

// ---------------------------------------------------------------------------
// Model listing and prices
// ---------------------------------------------------------------------------

const PRICES_JSON: &str = include_str!("../data/model_prices.json");

/// Rough token budget of one item verdict (two tooltip crops + text) and one
/// guide analysis, used for the per-call estimates shown in the UI.
pub const ITEM_TOKENS: (f64, f64) = (6_000.0, 600.0);
pub const GUIDE_TOKENS: (f64, f64) = (30_000.0, 8_000.0);

/// USD per 1M tokens, matched by the longest model-id prefix.
#[derive(Debug, Clone, Default)]
pub struct PriceTable {
    prices: Vec<(String, f64, f64)>,
}

impl PriceTable {
    /// Embedded table plus, when present, `profile/model_prices.json` overrides.
    pub fn load(profile_dir: Option<&std::path::Path>) -> PriceTable {
        let mut table = PriceTable::default();
        table.merge_json(PRICES_JSON);
        if let Some(dir) = profile_dir {
            if let Ok(text) = std::fs::read_to_string(dir.join("model_prices.json")) {
                table.merge_json(&text);
            }
        }
        table
    }

    fn merge_json(&mut self, text: &str) {
        let Ok(value) = serde_json::from_str::<Value>(text) else { return };
        let Some(prices) = value["prices"].as_object() else { return };
        for (id, pair) in prices {
            let (Some(i), Some(o)) = (pair[0].as_f64(), pair[1].as_f64()) else { continue };
            self.prices.retain(|(k, _, _)| k != id);
            self.prices.push((id.clone(), i, o));
        }
    }

    /// (input, output) USD per 1M tokens for a model id.
    pub fn price_for(&self, model_id: &str) -> Option<(f64, f64)> {
        let id = model_id.trim().to_lowercase();
        // a "mini"/"nano" variant never inherits its big sibling's price: better unknown than wrong
        let tier = |s: &str| ["mini", "nano", "lite", "flash"].iter().find(|t| s.contains(*t)).copied();
        self.prices
            .iter()
            .filter(|(k, _, _)| {
                let k = k.to_lowercase();
                // the key must end at a version boundary: "gpt-5" covers "gpt-5-mini-2025..." (tier aside) but not "gpt-5.6"
                let boundary = id.len() == k.len() || id.as_bytes().get(k.len()) == Some(&b'-');
                id.starts_with(&k) && boundary && tier(&k) == tier(&id)
            })
            .max_by_key(|(k, _, _)| k.len())
            .map(|(_, i, o)| (*i, *o))
    }

    pub fn estimate(price: (f64, f64), tokens: (f64, f64)) -> f64 {
        (tokens.0 * price.0 + tokens.1 * price.1) / 1_000_000.0
    }
}

/// One model the provider lists for the given key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    /// USD per 1M input tokens, when known
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    /// estimated USD per item verdict / per guide analysis, when the price is known
    pub item_cost: Option<f64>,
    pub guide_cost: Option<f64>,
}

impl Default for ModelInfo {
    fn default() -> Self {
        ModelInfo { id: String::new(), display_name: String::new(), input_price: None, output_price: None, item_cost: None, guide_cost: None }
    }
}

/// Whether a provider/model pair can take screenshots. DeepSeek's chat API is
/// text-only; everything else is assumed to accept images (a rejection is
/// retried without them).
pub fn supports_vision(provider: &str, model: &str) -> bool {
    let model = model.to_lowercase();
    provider != "deepseek" && !model.contains("deepseek") && !model.contains("embedding")
}

/// ids that are clearly not chat models (OpenAI lists everything under /models)
fn is_chat_model(id: &str) -> bool {
    const SKIP: [&str; 20] = [
        "tts", "whisper", "dall-e", "embedding", "moderation", "babbage", "davinci", "realtime", "audio",
        "transcribe", "image", "sora", "search", "instruct", "computer-use", "text-", "ada", "curie", "vision-preview", "-preview-",
    ];
    let id = id.to_lowercase();
    !SKIP.iter().any(|s| id.contains(s)) || id.starts_with("kimi") || id.starts_with("moonshot") || id.starts_with("deepseek")
}

/// GET the provider's model list with this key and pair it with prices.
pub fn list_models(config: &ModelConfig, prices: &PriceTable) -> Result<Vec<ModelInfo>> {
    let api_key = config.resolve_key().ok_or_else(|| anyhow!("no API key for {} (enter one or set {})", config.provider, config.env_var()))?;
    let base = if config.base_url.trim().is_empty() { config.default_base_url().to_string() } else { config.base_url.trim().trim_end_matches('/').to_string() };
    if base.is_empty() {
        bail!("an OpenAI-compatible model needs a base URL");
    }
    let base = base.trim_end_matches("/chat/completions").trim_end_matches("/v1/messages").to_string();
    let url = if config.is_anthropic() { format!("{base}/v1/models?limit=1000") } else { format!("{base}/models") };
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
    let mut req = agent.get(&url);
    if config.is_anthropic() {
        req = req.set("x-api-key", &api_key).set("anthropic-version", ANTHROPIC_VERSION);
    } else {
        req = req.set("authorization", &format!("Bearer {api_key}"));
    }
    let value: Value = match req.call() {
        Ok(resp) => serde_json::from_str(&resp.into_string().context("reading model list")?).context("model list is not JSON")?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_string)).unwrap_or(text);
            return Err(anyhow!(ApiError { status: code, message }));
        }
        Err(err) => return Err(anyhow!("request failed: {err}")),
    };
    let entries = value["data"].as_array().or_else(|| value["models"].as_array()).cloned().unwrap_or_default();
    let mut models: Vec<ModelInfo> = entries
        .iter()
        .filter_map(|m| m["id"].as_str().map(|id| (id.to_string(), m["display_name"].as_str().unwrap_or("").to_string())))
        .filter(|(id, _)| is_chat_model(id))
        .map(|(id, display_name)| {
            let price = prices.price_for(&id);
            ModelInfo {
                item_cost: price.map(|p| PriceTable::estimate(p, ITEM_TOKENS)),
                guide_cost: price.map(|p| PriceTable::estimate(p, GUIDE_TOKENS)),
                input_price: price.map(|p| p.0),
                output_price: price.map(|p| p.1),
                id,
                display_name,
            }
        })
        .collect();
    // priced models first, then alphabetical
    models.sort_by(|a, b| b.input_price.is_some().cmp(&a.input_price.is_some()).then(a.id.cmp(&b.id)));
    models.dedup_by(|a, b| a.id == b.id);
    if models.is_empty() {
        bail!("the provider returned no chat models for this key");
    }
    Ok(models)
}

fn effort_word(effort: &str) -> &str {
    match effort.trim().to_lowercase().as_str() {
        "low" => "low",
        "high" => "high",
        "max" => "max",
        _ => "medium",
    }
}

/// First `{` to last `}`: models sometimes wrap JSON in prose or fences.
pub fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

// ---------------------------------------------------------------------------
// Item analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemVerdict {
    /// UPGRADE | SIDEGRADE | WORSE | REVIEW
    pub verdict: String,
    /// one sentence for the card
    pub summary: String,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// low | medium | high
    #[serde(default)]
    pub confidence: String,
}

pub struct ItemRequest<'a> {
    pub candidate_png: Vec<u8>,
    pub equipped_png: Option<Vec<u8>>,
    pub candidate_text: String,
    pub equipped_text: Option<String>,
    /// The game's green/red compare block under the hovered item (see `Panel::compare_lines`).
    pub game_diff: Vec<String>,
    pub deterministic: &'a Verdict,
    pub profile: &'a GuideProfile,
    pub state: &'a CharacterState,
}

fn item_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string", "enum": ["UPGRADE", "SIDEGRADE", "WORSE", "REVIEW"] },
            "summary": { "type": "string" },
            "reasons": { "type": "array", "items": { "type": "string" } },
            "warnings": { "type": "array", "items": { "type": "string" } },
            "confidence": { "type": "string", "enum": ["low", "medium", "high"] }
        },
        "required": ["verdict", "summary", "reasons", "warnings", "confidence"],
        "additionalProperties": false
    })
}

/// Game mechanics the model must apply instead of guessing (Last Epoch 1.x,
/// checked 2026-09: the Vitality figure was confirmed against an in-game
/// health change; the others follow the Maxroll defenses guide and the wiki).
pub const GAME_FACTS: &str = "LAST EPOCH MECHANICS (apply these; do not rely on memory):
- Health: flat Health from all sources is summed, then multiplied by (1 + total % increased Health). Each point of Vitality = +6 flat Health and +1% Poison and +1% Necrotic Resistance. So '+35 Health' vs '-6 Vitality' is roughly a wash in flat terms (+35 vs -36) before the % multiplier.
- Strength: +4% increased Armour per point. Dexterity: +4 Dodge Rating per point. Intelligence: +4% Ward Retention per point. Attunement: +2 Mana per point (and minion/attunement scaling for some skills).
- Resistances cap at 75%; points above the cap only buffer resistance shred. Enemies penetrate 1% per area level (up to 75%), so being under cap hurts more the higher the zone. Resistance applies to hits and damage over time.
- Armour reduces hit damage only (not DoTs): 85% cap vs physical, 59.5% vs other types, with strong diminishing returns, so small armour changes matter little. Dodge avoids hits (not DoTs), diminishing returns, no practical cap.
- Endurance: 20% base, up to 60% less damage taken while below the Endurance Threshold (a flat HP amount). Block: chance stacks additively; block effectiveness is formula based. Ward is a decaying shield in front of Health.
- Damage-reduction layers multiply. 'Increased' modifiers of one kind add together; 'more/less' multiply.
- The green/red compare block the game draws under the hovered item is the game's own computation of the net change versus the equipped item: green = better, red = worse. Trust it over your own arithmetic for the stats it lists.
- Level requirement above the character's level means the item cannot be equipped yet.";

const ITEM_SYSTEM: &str = "You are a Last Epoch gear advisor for one specific build. The player hovers a bag item; \
you decide whether it beats the item currently equipped in the same slot for THIS build while leveling. \
Screenshots, when present, are the source of truth (the OCR text may contain errors). Weigh the build's stat priorities, \
resistance caps (75%), the level requirement versus the character's level, implicit stats, weapon base damage and \
attack rate, unique effects, and whether the equipped item really occupies the same slot. \
A deterministic scorer's verdict is provided as a hint: agree when it is right, override it when it missed \
something visible in the tooltips (implicits, base type, unique effect, a stat it did not recognise). \
Be concise: summary is one sentence; reasons are at most 4 short lines starting with + (gain) or - (loss); \
warnings only for real problems (cannot equip yet, resistance drops below cap, unique needs review). \
Answer with a JSON object with the keys verdict (UPGRADE|SIDEGRADE|WORSE|REVIEW), summary, reasons (array of strings), \
warnings (array of strings) and confidence (low|medium|high). Output the JSON only.";

fn describe_priorities(profile: &GuideProfile, state: &CharacterState) -> String {
    let mut out = format!("Build profile: {}\n", profile.name);
    let mut stats: Vec<_> = profile.stats.iter().collect();
    stats.sort_by(|a, b| b.effective_weight(state).partial_cmp(&a.effective_weight(state)).unwrap().then(a.rank.cmp(&b.rank)));
    for s in stats {
        let status = match s.why_inactive(state) {
            None => format!("weight {:.2}", s.weight),
            Some(why) => format!("INACTIVE now (weight {:.2}, {why})", s.inactive_weight),
        };
        out.push_str(&format!("- [{}] {} — {}", format!("{:?}", s.group).to_lowercase(), s.name, status));
        if !s.note.is_empty() {
            out.push_str(&format!(" ({})", s.note));
        }
        out.push('\n');
    }
    out
}

fn describe_state(state: &CharacterState, profile: &GuideProfile) -> String {
    let mut out = format!("Character: level {}, phase {}, endurance {:.0}%", state.level, state.phase(), state.endurance);
    if state.health > 0.0 {
        out.push_str(&format!(", maximum Health {:.0}", state.health));
    }
    if state.mana > 0.0 {
        out.push_str(&format!(", maximum Mana {:.0}", state.mana));
    }
    out.push('\n');
    out.push_str("Resistances (cap 75%): ");
    out.push_str(&state.resistance_list().iter().map(|(e, v)| format!("{e} {v:.0}%")).collect::<Vec<_>>().join(", "));
    out.push('\n');
    let facts = profile.describe_facts(state);
    if !facts.is_empty() {
        out.push_str(&format!("Build facts: {}\n", facts.join("; ")));
    }
    out
}

fn describe_verdict(v: &Verdict) -> String {
    let mut out = format!(
        "Deterministic scorer: {} (delta {:+.2}; hovered {:.2} vs equipped {:.2})\n",
        v.label.as_str(), v.delta, v.candidate_score, v.equipped_score
    );
    if let Some(slot) = &v.slot {
        out.push_str(&format!("slot: {}\n", slot.label()));
    }
    for r in &v.reasons {
        out.push_str(&format!("  reason: {r}\n"));
    }
    for w in &v.warnings {
        out.push_str(&format!("  warning: {w}\n"));
    }
    out.push_str("hovered item affixes as scored:\n");
    for a in &v.candidate.affixes {
        out.push_str(&format!("  {} T{} -> {:+.2} {}\n", a.name, a.tier, a.contribution, a.note));
    }
    for u in &v.candidate.unrecognised {
        out.push_str(&format!("  unrecognised line: {u}\n"));
    }
    if let Some(eq) = &v.equipped {
        out.push_str("equipped item affixes as scored:\n");
        for a in &eq.affixes {
            out.push_str(&format!("  {} T{} -> {:+.2} {}\n", a.name, a.tier, a.contribution, a.note));
        }
    }
    out
}

pub fn analyze_item(client: &Client, req: ItemRequest<'_>) -> Result<ItemVerdict> {
    let mut parts = vec![Part::Image { caption: "Screenshot 1: the HOVERED bag item's tooltip.".into(), png: req.candidate_png }];
    if let Some(png) = req.equipped_png {
        parts.push(Part::Image { caption: "Screenshot 2: the game's EQUIPPED compare tooltip (the item currently worn in that slot).".into(), png });
    }
    let mut text = String::new();
    text.push_str(GAME_FACTS);
    text.push_str("\n\n");
    text.push_str(&describe_priorities(req.profile, req.state));
    text.push('\n');
    text.push_str(&describe_state(req.state, req.profile));
    text.push_str("\nOCR of the hovered tooltip:\n");
    text.push_str(&req.candidate_text);
    if !req.game_diff.is_empty() {
        text.push_str("\n\nThe game's own compare block under the hovered item (net change if equipped; [better]/[worse] is the game's colouring):\n");
        text.push_str(&req.game_diff.join("\n"));
    }
    match &req.equipped_text {
        Some(t) => {
            text.push_str("\n\nOCR of the EQUIPPED tooltip:\n");
            text.push_str(t);
        }
        None => text.push_str("\n\nNo EQUIPPED tooltip was visible; the equipped item below comes from the advisor's memory of earlier compares (may be stale or empty)."),
    }
    text.push_str("\n\n");
    text.push_str(&describe_verdict(req.deterministic));
    text.push_str("\nJudge the hovered item for this build. Respond with the JSON object only.");
    parts.push(Part::Text(text));

    let schema = item_schema();
    let completion = client.complete(ITEM_SYSTEM, &[Turn { role: "user", parts }], 2048, Some(&schema))?;
    let raw = completion.text;
    let json_text = extract_json(&raw).ok_or_else(|| anyhow!("AI answer contained no JSON: {raw}"))?;
    let mut verdict: ItemVerdict = serde_json::from_str(json_text).with_context(|| format!("AI answer was not the expected JSON: {raw}"))?;
    verdict.verdict = verdict.verdict.trim().to_uppercase();
    if !matches!(verdict.verdict.as_str(), "UPGRADE" | "SIDEGRADE" | "WORSE" | "REVIEW") {
        verdict.verdict = "REVIEW".into();
    }
    Ok(verdict)
}

// ---------------------------------------------------------------------------
// Guide analysis
// ---------------------------------------------------------------------------

pub struct GuideRequest<'a> {
    pub guide_text: &'a str,
    pub character_name: &'a str,
    pub build_name: &'a str,
    /// The embedded Paladin profile, as the worked example of the schema.
    pub template_json: &'a str,
    /// Distinct affix display names from the game data (what the patterns match).
    pub affix_names: &'a [String],
}

pub struct GuideAnalysis {
    pub profile: GuideProfile,
    pub profile_json: String,
    pub usage: String,
}

const GUIDE_SYSTEM: &str = "You convert a Last Epoch build guide into a stat-priority profile for a deterministic gear scorer. \
Output one JSON object and nothing else: no markdown fences, no commentary.";

fn guide_instructions(req: &GuideRequest<'_>) -> String {
    format!(r#"SCHEMA (all keys shown; keep exactly these names):
{{
  "name": string (guide/build title, mention the class),
  "planner_id": string (Maxroll planner id if the guide mentions one, else ""),
  "source": string (URL or "pasted guide"),
  "resistance_cap": 75,
  "resistance_reference_roll": 30,
  "tier_target": 5,
  "phase_levels": {{ "intermediate_from": int, "final_from": int }}  (level brackets the guide uses for its setups; omit only if the guide has no leveling phases),
  "facts": [ {{ "key": snake_case, "label": short question shown to the player, "kind": "flag" | "counter", "default_on": bool, "default_count": int, "note": string }} ],
  "stats": [ {{
      "key": snake_case unique,
      "name": short display name,
      "group": "offense" | "defense",
      "rank": int (1 = the guide's top priority within the group),
      "weight": number 0..2 (1.0 = a top priority stat; 0.5 = useful; 0.1-0.3 = filler; use steep spacing so priorities differ),
      "inactive_weight": number (weight when a condition fails, usually 0 or 0.05),
      "patterns": [ regex strings ],
      "conditions": [ {{ "phase": ["early"|"intermediate"|"final", ...], "level": {{"min": int, "max": int}}, "flags": {{ fact_key: bool }}, "counters": {{ fact_key: {{"min": int, "max": int}} }} }} ],
      "only_item_types": [ ] (leave empty),
      "tier_target": int (optional, e.g. 5 for a stat the guide says to push),
      "saturation": {{ "kind": "resistance", "element": "fire"|"cold"|"lightning"|"physical"|"necrotic"|"void"|"poison"|"all" }} (only on resistance stats),
      "note": string (why / when, quoting the guide's reasoning briefly)
  }} ]
}}

RULES
- patterns are Rust-style regexes matched case-insensitively against the affix DISPLAY NAMES listed below (and the raw names). Use the exact names from the list; anchor with ^ and $ when the name is exact; several patterns per stat are fine. Never invent affix names that are not in the list.
- Resistances: one stat per element the guide cares about (or a "res_all" stat with element "all"), each with a saturation block, plus a physical-resistance stat if mentioned; the scorer only counts the part of a roll that fits under the 75% cap.
- Skill levels appear as "Level of <Skill>" in the list; ailment chances as "Chance To Ignite" etc.
- conditions: every field set in one condition must hold; use phase/level for "until setup X", and facts (flags/counters) for things the scorer cannot see, e.g. "once <skill> has 5 points" -> a counter fact "<skill>_points" with a counters condition {{"min": 5}}. Every flag/counter used in a condition MUST be declared in "facts". Declare only facts you actually use, named for THIS guide's skills and items (the reference profile's facts are Paladin-specific examples, not a fixed set).
- Cover both offense and defense; while leveling, prioritise the stats the guide lists under its stat priorities, then health/resistances/endurance style defenses. 8-25 stats is typical.
- Weights must be within 0..=5; keys unique; do not output comments.

REFERENCE: this is the profile for the Maxroll Paladin leveling guide (same schema, for style and weight scale):
{template}

AFFIX DISPLAY NAMES (one per line):
{names}

CHARACTER: {character}
BUILD: {build}

GUIDE TEXT:
{guide}
"#,
        template = req.template_json,
        names = req.affix_names.join("\n"),
        character = req.character_name,
        build = req.build_name,
        guide = req.guide_text.trim(),
    )
}

pub fn analyze_guide(client: &Client, req: GuideRequest<'_>) -> Result<GuideAnalysis> {
    if req.guide_text.trim().len() < 200 {
        bail!("the guide text is too short to analyse (paste the whole guide page)");
    }
    let mut turns = vec![Turn { role: "user", parts: vec![Part::Text(guide_instructions(&req))] }];
    let mut usage = Vec::new();
    let mut last_error = String::new();
    // guides deserve real thinking: never run them at low effort
    let mut client = client.clone();
    if effort_word(&client.config.effort) == "low" {
        client.config.effort = "medium".into();
    }
    let schema = json!({ "type": "object" });
    for attempt in 0..2 {
        let completion = client.complete(GUIDE_SYSTEM, &turns, 16000, Some(&schema))?;
        usage.push(completion.usage);
        let raw = completion.text;
        match extract_json(&raw).ok_or_else(|| anyhow!("no JSON object in the answer")).and_then(validate_profile) {
            Ok((profile, pretty)) => return Ok(GuideAnalysis { profile, profile_json: pretty, usage: usage.join("; ") }),
            Err(err) => {
                last_error = err.to_string();
                if attempt == 0 {
                    turns.push(Turn { role: "assistant", parts: vec![Part::Text(raw)] });
                    turns.push(Turn {
                        role: "user",
                        parts: vec![Part::Text(format!("That profile failed validation: {last_error}\nReturn the complete corrected JSON object only."))],
                    });
                }
            }
        }
    }
    bail!("the AI profile did not validate after two attempts: {last_error}")
}

fn validate_profile(json_text: &str) -> Result<(GuideProfile, String)> {
    let value: Value = serde_json::from_str(json_text).context("invalid JSON")?;
    let pretty = serde_json::to_string_pretty(&value)?;
    let profile = GuideProfile::from_json(&pretty)?;
    Ok((profile, pretty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_prose_and_fences() {
        assert_eq!(extract_json("here:\n```json\n{\"a\": 1}\n```"), Some("{\"a\": 1}"));
        assert_eq!(extract_json("no braces"), None);
    }

    #[test]
    fn urls_per_provider() {
        let a = ModelConfig::default();
        assert_eq!(a.chat_url(), "https://api.anthropic.com/v1/messages");
        let d = ModelConfig { provider: "deepseek".into(), ..Default::default() };
        assert_eq!(d.chat_url(), "https://api.deepseek.com/v1/chat/completions");
        let custom = ModelConfig { provider: "openai-compatible".into(), base_url: "http://localhost:11434/v1/".into(), ..Default::default() };
        assert_eq!(custom.chat_url(), "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn prices_match_longest_prefix_and_overrides() {
        let table = PriceTable::load(None);
        assert_eq!(table.price_for("gpt-5-mini-2025-08-07"), Some((0.25, 2.0)));
        assert_eq!(table.price_for("gpt-5"), Some((1.25, 10.0)));
        assert_eq!(table.price_for("claude-sonnet-4-5-20250929"), Some((3.0, 15.0)));
        assert_eq!(table.price_for("made-up-model"), None);
        assert_eq!(table.price_for("gpt-5.4-mini"), None, "a mini variant must not inherit gpt-5's price");
        assert_eq!(table.price_for("gpt-5.1-2025-11-13"), Some((1.25, 10.0)));
        assert_eq!(table.price_for("gpt-5.6-terra"), None, "a newer version must not inherit gpt-5's price");
        let mut table = table;
        table.merge_json(r#"{"prices": {"made-up-model": [1, 2], "gpt-5": [9, 9]}}"#);
        assert_eq!(table.price_for("made-up-model"), Some((1.0, 2.0)));
        assert_eq!(table.price_for("gpt-5"), Some((9.0, 9.0)));
        assert!((PriceTable::estimate((1.0, 10.0), (1_000_000.0, 100_000.0)) - 2.0).abs() < 1e-9);
        assert!(is_chat_model("gpt-5-mini"));
        assert!(!is_chat_model("text-embedding-3-small"));
        assert!(is_chat_model("deepseek-chat"));
    }

    #[test]
    fn item_schema_is_strict() {
        let s = item_schema();
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["required"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn openai_text_only_turn_is_a_plain_string() {
        let client = Client { config: ModelConfig { provider: "deepseek".into(), ..Default::default() }, api_key: "k".into(), timeout: Duration::from_secs(1) };
        let v = client.openai_parts(&[Part::Text("a".into()), Part::Text("b".into())]);
        assert_eq!(v, Value::String("a\nb".into()));
        let with_image = client.openai_parts(&[Part::Image { caption: "c".into(), png: vec![1, 2] }]);
        assert!(with_image.is_array());
    }
}
