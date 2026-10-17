//! The Model Context Protocol's messages as zcode answers them, without
//! any UI: which protocol revision a request speaks, the handshake of the
//! earlier revisions, `server/discover` of the current one, and the shape
//! of results and errors.
//!
//! zcode is a *dual-era* server (the spec's term). Clients of the
//! handshake-based revisions (2025-11-25, 2025-06-18, 2025-03-26) send
//! `initialize` first and are answered in the revision they negotiated;
//! clients of the current revision (2026-07-28) put the revision in every
//! request's `_meta` and may ask `server/discover` first.

use serde_json::{Map, Value, json};

/// The current revision: stateless, the version in every request's `_meta`.
pub const MODERN: &str = "2026-07-28";
/// The handshake-based revisions zcode answers `initialize` in, newest first.
pub const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

pub const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
pub const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// JSON-RPC's and MCP's error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// 2026-07-28's `UnsupportedProtocolVersionError`.
pub const UNSUPPORTED_VERSION: i64 = -32022;

/// How long a client may keep `tools/list` (2026-07-28's `ttlMs`): the
/// tools don't change while zcode runs.
const LIST_TTL_MS: u64 = 3_600_000;

pub const INSTRUCTIONS: &str = "zcode is an editor for Xcode projects and Swift packages. These tools act on the project open in zcode: \
build, run and test it with its schemes and run destinations (results come back structured), read its issues and build log, open files, \
read the editor's selection, evaluate in the debugger while it's paused, and take screenshots of the window or a simulator. \
Builds and tests can take minutes; progress is reported while they run, and cancelling the request stops them.";

/// Every revision zcode speaks, newest first.
pub fn supported_versions() -> Vec<&'static str> {
    std::iter::once(MODERN).chain(LEGACY.iter().copied()).collect()
}

/// The revision to answer an `initialize` in: the client's if zcode speaks
/// it, else the newest handshake-based one (the client then decides).
pub fn negotiate(requested: Option<&str>) -> &'static str {
    LEGACY.iter().find(|v| Some(**v) == requested).copied().unwrap_or(LEGACY[0])
}

/// The revision a request declares in its `_meta` (2026-07-28 and later).
pub fn request_version(params: &Value) -> Option<&str> {
    params.get("_meta")?.get(VERSION_KEY)?.as_str()
}

/// The progress token a request asked progress notifications for.
pub fn progress_token(params: &Value) -> Option<Value> {
    params.get("_meta")?.get("progressToken").filter(|t| t.is_string() || t.is_number()).cloned()
}

pub fn server_info() -> Value {
    json!({ "name": "zcode", "title": "zcode", "version": env!("CARGO_PKG_VERSION") })
}

pub fn capabilities() -> Value {
    json!({ "tools": { "listChanged": false } })
}

/// The answer to a handshake-based client's `initialize`.
pub fn initialize_result(params: &Value) -> Value {
    let version = negotiate(params.get("protocolVersion").and_then(Value::as_str));
    json!({
        "protocolVersion": version,
        "capabilities": capabilities(),
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS,
    })
}

/// The answer to `server/discover` (2026-07-28).
pub fn discover_result() -> Value {
    json!({
        "supportedVersions": supported_versions(),
        "capabilities": capabilities(),
        "instructions": INSTRUCTIONS,
        "ttlMs": LIST_TTL_MS,
        "cacheScope": "private",
    })
}

/// What a request's revision needs added to its result: 2026-07-28's
/// `resultType` and the server's identity in `_meta`.
pub fn finish_result(mut result: Value, modern: bool) -> Value {
    if modern && let Some(object) = result.as_object_mut() {
        object.insert("resultType".into(), "complete".into());
        let meta = object.entry("_meta").or_insert_with(|| Value::Object(Map::new()));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(SERVER_INFO_KEY.into(), server_info());
        }
    }
    result
}

/// `tools/list`'s result for these tools (cacheable in 2026-07-28).
pub fn tools_list_result(tools: Vec<Value>, modern: bool) -> Value {
    let mut result = json!({ "tools": tools });
    if modern {
        result["ttlMs"] = LIST_TTL_MS.into();
        result["cacheScope"] = "private".into();
    }
    result
}

/// A request asked for a revision zcode doesn't speak.
pub fn unsupported_version(requested: &str) -> (i64, String, Option<Value>) {
    (
        UNSUPPORTED_VERSION,
        "Unsupported protocol version".into(),
        Some(json!({ "supported": supported_versions(), "requested": requested })),
    )
}

pub fn response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error_response(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

pub fn progress_notification(token: &Value, progress: u64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": { "progressToken": token, "progress": progress, "message": message },
    })
}

/// What a tool returns: its structured result, the same as JSON text for
/// clients that read only text, and any images (PNG bytes).
#[derive(Debug, Default)]
pub struct ToolOutput {
    pub structured: Value,
    pub images: Vec<Vec<u8>>,
}

impl ToolOutput {
    pub fn json(structured: Value) -> Self {
        Self { structured, images: vec![] }
    }
}

/// `tools/call`'s result. `is_error`: the tool ran and failed (a message
/// for the model), which isn't a protocol error.
pub fn tool_result(output: &ToolOutput) -> Value {
    let mut content = vec![json!({ "type": "text", "text": output.structured.to_string() })];
    for png in &output.images {
        content.push(json!({ "type": "image", "mimeType": "image/png", "data": base64(png) }));
    }
    json!({ "content": content, "structuredContent": output.structured, "isError": false })
}

pub fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// Standard base64 with padding, for image content.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_revisions_are_negotiated() {
        assert_eq!(negotiate(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiate(Some("2025-03-26")), "2025-03-26");
        // One zcode doesn't know: its newest, for the client to accept or not.
        assert_eq!(negotiate(Some("2099-01-01")), "2025-11-25");
        assert_eq!(negotiate(None), "2025-11-25");
        let result = initialize_result(&json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }));
        assert_eq!(result["protocolVersion"], "2025-06-18");
        assert_eq!(result["serverInfo"]["name"], "zcode");
        assert_eq!(result["capabilities"], json!({ "tools": { "listChanged": false } }));
    }

    #[test]
    fn modern_requests_carry_their_revision() {
        let params = json!({ "_meta": { VERSION_KEY: "2026-07-28", "progressToken": 7 }, "name": "build" });
        assert_eq!(request_version(&params), Some("2026-07-28"));
        assert_eq!(progress_token(&params), Some(json!(7)));
        assert_eq!(request_version(&json!({ "name": "build" })), None);
        assert_eq!(progress_token(&json!({ "_meta": { "progressToken": null } })), None);
        let (code, _, data) = unsupported_version("1900-01-01");
        assert_eq!(code, -32022);
        assert_eq!(data.unwrap(), json!({ "supported": ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"], "requested": "1900-01-01" }));
        let discovered = finish_result(discover_result(), true);
        assert_eq!(discovered["resultType"], "complete");
        assert_eq!(discovered["_meta"][SERVER_INFO_KEY]["name"], "zcode");
        assert_eq!(discovered["supportedVersions"][0], "2026-07-28");
        // Handshake-era results stay as those revisions define them.
        assert_eq!(finish_result(json!({ "tools": [] }), false), json!({ "tools": [] }));
        assert_eq!(tools_list_result(vec![], true)["cacheScope"], "private");
    }

    #[test]
    fn tool_results_have_text_structure_and_images() {
        let output = ToolOutput { structured: json!({ "success": true }), images: vec![b"PNG!".to_vec()] };
        let result = tool_result(&output);
        assert_eq!(result["content"][0], json!({ "type": "text", "text": "{\"success\":true}" }));
        assert_eq!(result["content"][1], json!({ "type": "image", "mimeType": "image/png", "data": "UE5HIQ==" }));
        assert_eq!(result["structuredContent"], json!({ "success": true }));
        assert_eq!(tool_error("no")["isError"], true);
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }
}
