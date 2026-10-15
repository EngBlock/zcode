//! Launches the ZKit sample's `zhello` under lldb-dap, stops at a
//! breakpoint, and reads locals. Requires Xcode; `cargo test -- --ignored`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};
use zcode::debugger::dap::{DapClient, DapEvent, initialize_args};

#[test]
#[ignore]
fn stops_at_breakpoint_and_reads_variables() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../samples/ZKit").canonicalize().unwrap();
    assert!(Command::new("/usr/bin/swift").arg("build").current_dir(&root).status().unwrap().success());
    let program = root.join(".build/debug/zhello");
    let source = root.join("Sources/zhello/main.swift");

    let binary = zcode::toolchain::find_tool("lldb-dap").unwrap();
    let (tx, rx) = smol::channel::unbounded();
    let dap = DapClient::spawn(&binary, tx).unwrap();
    smol::block_on(async {
        let wait_event = |name: &'static str| {
            let rx = rx.clone();
            async move {
                loop {
                    match rx.recv().await.unwrap() {
                        DapEvent::Event { event, body } if event == name => return body,
                        DapEvent::Exited => panic!("adapter exited waiting for {name}"),
                        _ => {}
                    }
                }
            }
        };
        dap.request("initialize", initialize_args()).await.unwrap();
        let launch = dap.request("launch", json!({ "program": program, "cwd": root, "stopOnEntry": false }));
        wait_event("initialized").await;
        let bps = dap
            .request("setBreakpoints", json!({ "source": { "path": source }, "breakpoints": [{ "line": 5 }] }))
            .await
            .unwrap();
        assert_eq!(bps["breakpoints"][0]["verified"], Value::Bool(true), "{bps}");
        dap.request("configurationDone", json!({})).await.unwrap();
        launch.await.unwrap();
        let stopped = smol::future::or(wait_event("stopped"), async {
            smol::Timer::after(Duration::from_secs(30)).await;
            panic!("did not stop")
        })
        .await;
        let thread = stopped["threadId"].as_i64().unwrap();
        let stack = dap.request("stackTrace", json!({ "threadId": thread })).await.unwrap();
        let frame = &stack["stackFrames"][0];
        assert_eq!(frame["line"], 5, "{frame}");
        let scopes = dap.request("scopes", json!({ "frameId": frame["id"] })).await.unwrap();
        let locals = scopes["scopes"][0]["variablesReference"].clone();
        let vars = dap.request("variables", json!({ "variablesReference": locals })).await.unwrap();
        let text = vars.to_string();
        assert!(text.contains("greeter") || text.contains("i"), "{text}");
        dap.request("disconnect", json!({ "terminateDebuggee": true })).await.ok();
    });
}
