//! The Claude API as a prediction provider: the Messages API, streamed.
//!
//! A request carries the file around the cursor (the cursor marked), the
//! edits made lately and the file's path and language, and asks for the
//! text to insert between `<insert>` tags (`</insert>` is a stop sequence).
//! The answer streams in as server-sent events and each piece of text
//! updates the prediction on screen. The model, key, endpoint and limits
//! come from `edit_predictions.claude` in the user's settings; the key from
//! there or from `ANTHROPIC_API_KEY` (zcode's environment, else the login
//! shell's). The key goes only into the request's `x-api-key` header:
//! never into a log, an error or a `{:?}`.
//!
//! HTTP is blocking (`ureq`, TLS through macOS) on a thread per request, so
//! nothing waits on the network but that thread. Dropping the stream
//! cancels it: the thread stops at the next line it reads and the
//! connection closes.

use std::io::BufRead as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::channel::mpsc;
use serde_json::{Value, json};

use super::{PredictionRequest, RecentEdit, Update, sse};
use crate::settings::{ClaudeSettings, Secret};

const SYSTEM: &str = "You are the inline code completion engine of a code editor. You see a file with the cursor marked <|cursor|>, and the user's latest edits. Predict the text the user is about to type at the cursor: finish the current line or statement, or write the next few lines when it's clear what comes next. Match the file's language, indentation and style, and don't repeat text that is already after the cursor. Answer with only the text to insert, between <insert> and </insert>: no explanation, no Markdown. Start right after <insert> with the first character to insert (a line break if the text starts on a new line). If there's nothing useful to insert, answer <insert></insert>.";
const CURSOR: &str = "<|cursor|>";
const OPEN: &str = "<insert>";
const CLOSE: &str = "</insert>";
const API_VERSION: &str = "2023-06-01";

/// A prediction streaming in; dropping it cancels the request.
pub struct Stream {
    pub updates: mpsc::UnboundedReceiver<Update>,
    _cancel: Cancel,
}

struct Cancel(Arc<AtomicBool>);

impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Asks for a prediction for `request`, on a thread of its own.
pub fn predict(request: &PredictionRequest, settings: &ClaudeSettings) -> Stream {
    let (tx, updates) = mpsc::unbounded();
    let cancelled = Arc::new(AtomicBool::new(false));
    let body = body(request, settings);
    let (settings, flag) = (settings.clone(), cancelled.clone());
    let spawned = std::thread::Builder::new().name("zcode-prediction".into()).spawn(move || {
        let update = match run(&body, &settings, &tx, &flag) {
            Ok(()) => Update::Done,
            Err(message) => {
                log::warn!("edit prediction: {message}");
                Update::Failed(message)
            }
        };
        if !flag.load(Ordering::Relaxed) {
            tx.unbounded_send(update).ok();
        }
    });
    if let Err(err) = spawned {
        log::warn!("edit prediction: no thread: {err}");
    }
    Stream { updates, _cancel: Cancel(cancelled) }
}

/// The Messages API request.
pub fn body(request: &PredictionRequest, settings: &ClaudeSettings) -> Value {
    let mut body = json!({
        "model": settings.model,
        "max_tokens": settings.max_tokens,
        "stream": true,
        "system": SYSTEM,
        "stop_sequences": [CLOSE],
        "messages": [{ "role": "user", "content": prompt(request) }],
    });
    if let Some(effort) = &settings.effort {
        body["output_config"] = json!({ "effort": effort });
    }
    body
}

/// The file around the cursor, after the edits made lately.
pub fn prompt(request: &PredictionRequest) -> String {
    let mut prompt = String::new();
    if !request.recent_edits.is_empty() {
        prompt.push_str("The user's latest edits, oldest first:\n\n");
        for edit in &request.recent_edits {
            push_edit(&mut prompt, edit);
        }
        prompt.push('\n');
    }
    let (before, after, row) = super::excerpt(&request.text, request.cursor);
    prompt.push_str(&format!(
        "<file path=\"{}\" language=\"{}\" first_line=\"{}\">\n{before}{CURSOR}{after}\n</file>",
        request.display_path,
        request.language,
        row + 1
    ));
    prompt
}

fn push_edit(prompt: &mut String, edit: &RecentEdit) {
    prompt.push_str(&format!("--- {}, line {}\n", edit.path, edit.row + 1));
    for line in edit.old.lines() {
        prompt.push_str(&format!("-{line}\n"));
    }
    for line in edit.new.lines() {
        prompt.push_str(&format!("+{line}\n"));
    }
}

/// The text between the tags as it streams in.
#[derive(Default)]
pub struct Extractor {
    raw: String,
}

impl Extractor {
    /// Adds a piece of the answer; the prediction so far once the opening
    /// tag has come (holding back what may be the start of the closing one).
    pub fn push(&mut self, delta: &str) -> Option<String> {
        self.raw.push_str(delta);
        let start = self.raw.find(OPEN)? + OPEN.len();
        let inside = &self.raw[start..];
        let end = match inside.find(CLOSE) {
            Some(end) => end,
            None => (1..CLOSE.len()).rev().find(|n| inside.ends_with(&CLOSE[..*n])).map_or(inside.len(), |n| inside.len() - n),
        };
        Some(inside[..end].to_string())
    }

    /// The whole prediction: between the tags, or (when the model left
    /// them out) the answer without Markdown fences.
    pub fn finish(&self) -> String {
        if let Some(start) = self.raw.find(OPEN) {
            let inside = &self.raw[start + OPEN.len()..];
            return inside[..inside.find(CLOSE).unwrap_or(inside.len())].to_string();
        }
        let trimmed = self.raw.trim();
        match trimmed.strip_prefix("```") {
            Some(fenced) => {
                let body = fenced.split_once('\n').map_or("", |(_, body)| body);
                body.strip_suffix("```").unwrap_or(body).trim_end_matches('\n').to_string()
            }
            None => trimmed.to_string(),
        }
    }
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .user_agent(concat!("zcode/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    })
}

/// The key: the settings', else `ANTHROPIC_API_KEY`'s.
fn api_key(settings: &ClaudeSettings) -> Result<Secret, String> {
    if let Some(key) = &settings.api_key {
        return Ok(key.clone());
    }
    if let Some(key) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty()) {
        return Ok(Secret::new(key));
    }
    static FROM_SHELL: OnceLock<Option<Secret>> = OnceLock::new();
    FROM_SHELL
        .get_or_init(|| {
            if std::env::var_os("ZCODE_AGENT_NO_LOGIN_SHELL").is_some() {
                return None;
            }
            crate::acp::agents::login_shell_var("ANTHROPIC_API_KEY", Duration::from_secs(5))
                .filter(|k| !k.trim().is_empty())
                .map(Secret::new)
        })
        .clone()
        .ok_or_else(|| "no API key: set ANTHROPIC_API_KEY, or edit_predictions.claude.api_key in your settings".to_string())
}

fn endpoint(api_url: &str) -> String {
    let url = api_url.trim_end_matches('/');
    if url.ends_with("/v1/messages") { url.to_string() } else { format!("{url}/v1/messages") }
}

fn run(body: &Value, settings: &ClaudeSettings, tx: &mpsc::UnboundedSender<Update>, cancelled: &AtomicBool) -> Result<(), String> {
    let key = api_key(settings)?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let url = endpoint(&settings.api_url);
    let response = agent()
        .post(&url)
        .header("x-api-key", key.expose())
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .send(body.to_string().as_bytes())
        .map_err(|err| format!("{url}: {err}"))?;
    let status = response.status().as_u16();
    let mut body = response.into_body();
    if status != 200 {
        let text = body.with_config().limit(64 * 1024).read_to_string().unwrap_or_default();
        return Err(api_error(status, &text));
    }
    let mut reader = std::io::BufReader::new(body.into_reader());
    let mut parser = sse::Parser::default();
    let mut extractor = Extractor::default();
    let mut shown: Option<String> = None;
    let mut line = String::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        line.clear();
        let read = reader.read_line(&mut line).map_err(|err| format!("reading the answer: {err}"))?;
        let event = if read == 0 { parser.finish() } else { parser.line(line.trim_end_matches('\n')) };
        if let Some(event) = event {
            let data: Value = serde_json::from_str(&event.data).unwrap_or(Value::Null);
            match event.event.as_str() {
                "content_block_delta" if data.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta") => {
                    let text = data.pointer("/delta/text").and_then(Value::as_str).unwrap_or_default();
                    if let Some(so_far) = extractor.push(text)
                        && shown.as_ref() != Some(&so_far)
                    {
                        tx.unbounded_send(Update::Text(so_far.clone())).ok();
                        shown = Some(so_far);
                    }
                }
                "error" => return Err(api_error(200, &event.data)),
                "message_stop" => break,
                _ => {}
            }
        }
        if read == 0 {
            break;
        }
    }
    let whole = extractor.finish();
    if shown.as_ref() != Some(&whole) {
        tx.unbounded_send(Update::Text(whole)).ok();
    }
    Ok(())
}

/// What went wrong, from the API's error object (never the request).
fn api_error(status: u16, body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let kind = value.pointer("/error/type").and_then(Value::as_str);
    let message = value.pointer("/error/message").and_then(Value::as_str);
    match (status, kind, message) {
        (401, _, _) => "the API key was refused (401)".into(),
        (_, Some(kind), Some(message)) => format!("{kind}: {message}{}", if status != 200 { format!(" ({status})") } else { String::new() }),
        _ => format!("HTTP {status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zcode_editor::Rope;

    fn request() -> PredictionRequest {
        let text = "func greet(name: String) {\n    print(\n}\n";
        PredictionRequest {
            path: "/p/Sources/A.swift".into(),
            display_path: "Sources/A.swift".into(),
            language: "swift".into(),
            text: Rope::from(text),
            cursor: text.find("print(").unwrap() + 6,
            recent_edits: vec![RecentEdit { path: "Sources/A.swift".into(), row: 0, rows: 1, old: "func greet() {".into(), new: "func greet(name: String) {".into() }],
            invoked: false,
        }
    }

    #[test]
    fn the_request_marks_the_cursor_and_says_what_changed() {
        let settings = crate::settings::EditPredictionSettings::default().claude;
        let body = body(&request(), &settings);
        assert_eq!(body["model"], settings.model.as_str());
        assert_eq!(body["stream"], true);
        assert_eq!(body["stop_sequences"], json!(["</insert>"]));
        assert_eq!(body["output_config"]["effort"], "low");
        let prompt = body["messages"][0]["content"].as_str().unwrap();
        assert!(prompt.contains("<file path=\"Sources/A.swift\" language=\"swift\" first_line=\"1\">\nfunc greet(name: String) {\n    print(<|cursor|>\n}\n"), "{prompt}");
        assert!(prompt.contains("--- Sources/A.swift, line 1\n-func greet() {\n+func greet(name: String) {\n"), "{prompt}");
        let no_effort = ClaudeSettings { effort: None, ..settings };
        assert!(super::body(&request(), &no_effort).get("output_config").is_none());
    }

    #[test]
    fn the_prediction_is_between_the_tags_as_it_streams() {
        let mut extractor = Extractor::default();
        assert_eq!(extractor.push("<ins"), None);
        assert_eq!(extractor.push("ert>name"), Some("name".into()));
        // What may be the closing tag waits for the next piece.
        assert_eq!(extractor.push(")</ins"), Some("name)".into()));
        assert_eq!(extractor.push("ert>"), Some("name)".into()));
        assert_eq!(extractor.finish(), "name)");
        let mut fenced = Extractor::default();
        fenced.push("```swift\nlet x = 1\n```");
        assert_eq!(fenced.finish(), "let x = 1");
        let mut empty = Extractor::default();
        empty.push("<insert></insert>");
        assert_eq!(empty.finish(), "");
    }

    #[test]
    fn errors_name_the_problem_not_the_key() {
        assert_eq!(api_error(401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#), "the API key was refused (401)");
        assert_eq!(api_error(529, r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#), "overloaded_error: Overloaded (529)");
        assert_eq!(api_error(500, "<html>"), "HTTP 500");
        assert_eq!(endpoint("https://api.anthropic.com/"), "https://api.anthropic.com/v1/messages");
        assert_eq!(endpoint("http://127.0.0.1:9/v1/messages"), "http://127.0.0.1:9/v1/messages");
    }

    #[test]
    fn a_key_in_the_settings_comes_first() {
        let mut settings = crate::settings::EditPredictionSettings::default().claude;
        settings.api_key = Some(Secret::new("sk-ant-from-settings"));
        assert_eq!(api_key(&settings).unwrap().expose(), "sk-ant-from-settings");
        assert_eq!(format!("{:?}", api_key(&settings)), "Ok(Secret(…))");
    }
}
