//! A Model Context Protocol server for save files.
//!
//! The server speaks JSON-RPC 2.0 over stdin/stdout, one message per line —
//! the "stdio" transport every MCP client can launch — and exposes the edit
//! layer's operations as tools. That is what lets an agent answer a request
//! like "add 100,000 nanites to my Pioneers save": find the save, resolve the
//! target, edit it, and report what changed.
//!
//! Nothing here bypasses the edit layer. Every write goes through
//! [`nms_edit::SaveBundle`], so a timestamped backup is written first, an
//! unedited save is never rewritten, and a live save folder is refused unless
//! the caller passes `confirm_live` — which the tool descriptions tell an
//! agent never to do without the user asking for it.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use nms_data::DataPack;
use nms_model::KeyMap;
use serde_json::{json, Value};

mod discover;
mod tools;

pub use discover::{default_save_roots, save_dirs, scan, FoundSave};

/// Name reported to the client.
pub const SERVER_NAME: &str = "nmsedit";

/// The protocol revision this server was written against.
///
/// The client's requested revision is echoed when it sends one: the tools here
/// are simple and stable, so a newer client is better served by a version it
/// asked for than by a refusal to talk.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// What the client is told about how this server behaves.
pub const INSTRUCTIONS: &str = "\
nmsedit edits No Man's Sky save files. Every write goes through the same edit \
layer as the command-line tool: a timestamped zip backup is written first, the \
planned bytes are verified before they touch the disk, an unedited save is \
never rewritten, and the metadata file is patched rather than regenerated. \
Writes go wherever the path points, the game's own save folder included; when \
a write result carries a \"note:\" line, that folder is the live, Steam \
Cloud-synced one — relay it, because the edit will reach every machine signed \
into the account. list_backups shows the state each backup holds, and \
pin_backup keeps a known-good one out of pruning. Balances (units, nanites, \
quicksilver) are currencies: use set_currency, not add_item. Item ids are the \
game's own ids such as ^CARBON; search_items resolves a name to an id. \
Inventories and learned lists need the community key mapping, and item names \
need a data pack; the tools say so when one is missing.";

/// A protocol-level failure, as JSON-RPC wants them.
///
/// Tool failures are not errors here: they are results with `isError` set, so
/// the model gets to read what went wrong.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// A request did not have a method.
    #[error("{0}")]
    InvalidParams(String),
    /// The method is not one this server implements.
    #[error("method not found: {0}")]
    MethodNotFound(String),
}

impl ProtocolError {
    /// The JSON-RPC error code.
    fn code(&self) -> i64 {
        match self {
            ProtocolError::InvalidParams(_) => -32602,
            ProtocolError::MethodNotFound(_) => -32601,
        }
    }
}

/// The server.
pub struct Server {
    /// Keys used to resolve readable field names.
    pub(crate) keys: KeyMap,
    /// Names for item ids, when a data pack has been generated.
    pub(crate) pack: Option<DataPack>,
    /// Where the community mapping was loaded from, if anywhere.
    pub(crate) mapping: Option<PathBuf>,
}

impl Server {
    /// Build with the user's configured mapping and data pack.
    pub fn from_config() -> Server {
        let mapping = configured_mapping();
        let keys = match &mapping {
            Some(path) => match KeyMap::load(path) {
                Ok(loaded) => KeyMap::builtin().extended_with(&loaded),
                Err(_) => KeyMap::builtin(),
            },
            None => KeyMap::builtin(),
        };
        let pack = configured_pack().and_then(|path| DataPack::load(&path).ok());
        Server { keys, pack, mapping }
    }

    /// Build with explicit inputs, for tests.
    pub fn new(keys: KeyMap, pack: Option<DataPack>, mapping: Option<PathBuf>) -> Server {
        Server { keys, pack, mapping }
    }

    /// Where the community key mapping was loaded from, if anywhere.
    pub fn mapping(&self) -> Option<&std::path::Path> {
        self.mapping.as_deref()
    }

    /// Whether a data pack is loaded, so item names and search work.
    pub fn has_pack(&self) -> bool {
        self.pack.is_some()
    }

    /// Handle one JSON-RPC message, returning the response to send, if any.
    ///
    /// Notifications (messages without an id) are handled and answered with
    /// `None`, as the specification requires.
    pub fn handle(&self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to something this server never sent; nothing to do.
            return None;
        };
        if method.starts_with("notifications/") {
            return None;
        }
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let outcome = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::definitions() })),
            "tools/call" => self.call_tool(&params),
            other => Err(ProtocolError::MethodNotFound(other.to_string())),
        };

        if id.is_none() {
            return None;
        }
        let id = id.unwrap_or(Value::Null);
        Some(match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": error.code(), "message": error.to_string() },
            }),
        })
    }

    /// Run the stdio loop until the input ends.
    ///
    /// Every message is answered before the next is read; a client that waits
    /// for a response before sending the next request (which most do) would
    /// otherwise deadlock.
    pub fn run(&self, input: impl BufRead, output: &mut impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(&line) {
                Ok(message) => message,
                Err(error) => {
                    // A parse failure has no id to answer to, so the response
                    // carries a null one.
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": { "code": -32700, "message": format!("parse error: {error}") },
                    });
                    writeln!(output, "{response}")?;
                    output.flush()?;
                    continue;
                }
            };
            if let Some(response) = self.handle(&message) {
                writeln!(output, "{response}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        json!({
            "protocolVersion": requested.unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        })
    }

    fn call_tool(&self, params: &Value) -> Result<Value, ProtocolError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ProtocolError::InvalidParams("tools/call without a name".to_string()))?;
        let empty = serde_json::Map::new();
        let arguments = params
            .get("arguments")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let Some(result) = tools::call(self, name, arguments) else {
            return Err(ProtocolError::InvalidParams(format!("unknown tool {name:?}")));
        };
        Ok(match result {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Err(text) => json!({
                "content": [{ "type": "text", "text": text }],
                "isError": true,
            }),
        })
    }
}

/// The community mapping the CLI and GUI also use.
fn configured_mapping() -> Option<PathBuf> {
    std::env::var("NMSEDIT_MAPPING")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let base = match std::env::var("XDG_CONFIG_HOME") {
                Ok(value) if !value.is_empty() => PathBuf::from(value),
                _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
            };
            Some(base.join("nmsedit").join("mapping.json")).filter(|path| path.is_file())
        })
}

/// The data pack the CLI's `data refresh` writes.
fn configured_pack() -> Option<PathBuf> {
    std::env::var("NMSEDIT_PACK")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| DataPack::default_path().filter(|path| path.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::new(KeyMap::builtin(), None, None)
    }

    #[test]
    fn initialize_answers_with_the_clients_version_and_the_tool_capability() {
        let response = server()
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "test" } },
            }))
            .expect("a response");
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert!(response["result"]["capabilities"]["tools"].is_object());
        assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
        let instructions = response["result"]["instructions"].as_str().unwrap_or_default();
        assert!(instructions.contains("backup"), "{instructions}");
        assert!(instructions.contains("note:"), "{instructions}");
    }

    #[test]
    fn notifications_get_no_response() {
        assert!(server().handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
        assert!(server().handle(&json!({ "jsonrpc": "2.0", "method": "notifications/cancelled" })).is_none());
    }

    #[test]
    fn tools_list_describes_every_tool_with_a_schema() {
        let response = server()
            .handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .expect("a response");
        let tools = response["result"]["tools"].as_array().expect("tools");
        assert!(tools.len() >= 15, "the tool list should cover the editor: {}", tools.len());
        for tool in tools {
            let name = tool["name"].as_str().expect("a name");
            assert!(!tool["description"].as_str().unwrap_or_default().is_empty(), "{name} has no description");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name} has no object schema");
            assert!(tool["inputSchema"]["properties"].is_object(), "{name} has no properties");
        }
        let names: Vec<&str> = tools.iter().filter_map(|tool| tool["name"].as_str()).collect();
        for expected in ["list_slots", "read_currencies", "set_currency", "add_item", "search_items", "get_field"] {
            assert!(names.contains(&expected), "{expected} is missing from {names:?}");
        }
    }

    #[test]
    fn an_unknown_method_is_a_protocol_error() {
        let response = server()
            .handle(&json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }))
            .expect("a response");
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn an_unknown_tool_is_a_protocol_error_but_a_failed_tool_is_a_result() {
        let response = server()
            .handle(&json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": { "name": "no_such_tool", "arguments": {} },
            }))
            .expect("a response");
        assert_eq!(response["error"]["code"], -32602);

        // A tool that runs but refuses is a successful call with isError set,
        // so the model reads the reason instead of losing it.
        let response = server()
            .handle(&json!({
                "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": { "name": "save_info", "arguments": { "save": "/nonexistent/save.hg" } },
            }))
            .expect("a response");
        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["content"][0]["text"].as_str().unwrap_or_default().contains("save.hg"));
    }

    #[test]
    fn the_stdio_loop_answers_each_line_and_survives_junk() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n",
            "not json\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let mut output = Vec::new();
        server().run(input.as_bytes(), &mut output).expect("the loop finishes");
        let lines: Vec<&str> = std::str::from_utf8(&output).expect("utf-8").lines().collect();
        assert_eq!(lines.len(), 3, "one response per request, none per notification: {lines:?}");
        let first: Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(first["id"], 1);
        let parse_error: Value = serde_json::from_str(lines[1]).expect("json");
        assert_eq!(parse_error["error"]["code"], -32700);
        let listed: Value = serde_json::from_str(lines[2]).expect("json");
        assert!(listed["result"]["tools"].as_array().is_some());
    }
}
