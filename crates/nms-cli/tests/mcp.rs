//! The `nmsedit mcp` subcommand, driven the way a client drives it.
//!
//! The protocol itself is unit-tested inside `nms-mcp`; what this checks is the
//! wiring: that the binary starts the server, that stdout carries only
//! protocol messages, and that a real request gets a real answer.

use std::io::Write;
use std::process::{Command, Stdio};

fn nmsedit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nmsedit"))
}

/// Send messages to `nmsedit mcp` and return the response lines.
fn ask(messages: &[&str]) -> Vec<String> {
    let mut child = nmsedit()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn nmsedit mcp");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for message in messages {
            writeln!(stdin, "{message}").expect("write a message");
        }
    }
    let output = child.wait_with_output().expect("the server exits at EOF");
    assert!(output.status.success(), "nmsedit mcp failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout)
        .expect("the protocol is utf-8")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_server_initializes_and_lists_its_tools() {
    let responses = ask(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    ]);
    assert_eq!(responses.len(), 2, "one response per request: {responses:?}");

    let init: serde_json::Value = serde_json::from_str(&responses[0]).expect("json");
    assert_eq!(init["result"]["serverInfo"]["name"], "nmsedit");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    let listed: serde_json::Value = serde_json::from_str(&responses[1]).expect("json");
    let tools = listed["result"]["tools"].as_array().expect("tools");
    assert!(tools.len() >= 15, "the tool list is thin: {}", tools.len());
    let names: Vec<&str> = tools.iter().filter_map(|tool| tool["name"].as_str()).collect();
    assert!(names.contains(&"list_slots"), "{names:?}");
    assert!(names.contains(&"set_currency"), "{names:?}");
}

#[test]
fn a_failing_tool_is_reported_in_band() {
    let responses = ask(&[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"save_info","arguments":{"save":"/nonexistent/save.hg"}}}"#]);
    let response: serde_json::Value = serde_json::from_str(&responses[0]).expect("json");
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("save.hg"),
        "{response}"
    );
}
