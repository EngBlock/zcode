//! Launches the ZKit sample's `zhello` under lldb-dap with arguments and
//! environment (as Edit Scheme passes them), stops at a breakpoint, reads
//! locals and checks the program's output. Requires Xcode; `cargo test --
//! --ignored`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};
use zcode::debugger::dap::{DapClient, DapEvent, initialize_args};

#[test]
#[ignore]
fn stops_at_breakpoint_and_reads_variables() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/ZKit").canonicalize().unwrap();
    assert!(Command::new("/usr/bin/swift").arg("build").current_dir(&root).status().unwrap().success());
    let program = root.join(".build/debug/zhello");
    let source = root.join("Sources/zhello/main.swift");
    let loop_line = std::fs::read_to_string(&source).unwrap().lines().position(|l| l.contains("for i in")).unwrap() + 1;

    let binary = zcode::toolchain::find_tool("lldb-dap").unwrap();
    let (tx, rx) = smol::channel::unbounded();
    let dap = DapClient::spawn(&binary, tx).unwrap();
    // Everything the program prints, from launch on.
    let output = std::cell::RefCell::new(String::new());
    smol::block_on(async {
        let wait_event = |name: &'static str| {
            let rx = rx.clone();
            let output = &output;
            async move {
                loop {
                    match rx.recv().await.unwrap() {
                        DapEvent::Event { event, body } if event == name => return body,
                        DapEvent::Event { event, body } if event == "output" => {
                            output.borrow_mut().push_str(body["output"].as_str().unwrap_or(""));
                        }
                        DapEvent::Exited => panic!("adapter exited waiting for {name}"),
                        _ => {}
                    }
                }
            }
        };
        dap.request("initialize", initialize_args()).await.unwrap();
        let launch = dap.request(
            "launch",
            json!({
                "program": program,
                "cwd": root,
                "args": ["--flag", "two words"],
                "env": ["ZHELLO_NAME=Scheme"],
                "stopOnEntry": false,
            }),
        );
        wait_event("initialized").await;
        let bps = dap
            .request("setBreakpoints", json!({ "source": { "path": source }, "breakpoints": [{ "line": loop_line }] }))
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
        assert_eq!(frame["line"], loop_line, "{frame}");
        let scopes = dap.request("scopes", json!({ "frameId": frame["id"] })).await.unwrap();
        let locals = scopes["scopes"][0]["variablesReference"].clone();
        let vars = dap.request("variables", json!({ "variablesReference": locals })).await.unwrap();
        let text = vars.to_string();
        assert!(text.contains("greeter") || text.contains("i"), "{text}");

        // Run to the end, collecting what the program printed.
        dap.request("setBreakpoints", json!({ "source": { "path": source }, "breakpoints": [] })).await.unwrap();
        dap.request("continue", json!({ "threadId": thread })).await.unwrap();
        let collect = async {
            while !output.borrow().contains("tick 3") {
                match rx.recv().await.unwrap() {
                    DapEvent::Event { event, body } if event == "output" => {
                        output.borrow_mut().push_str(body["output"].as_str().unwrap_or(""));
                    }
                    DapEvent::Exited => break,
                    _ => {}
                }
            }
        };
        smol::future::or(collect, async {
            smol::Timer::after(Duration::from_secs(30)).await;
        })
        .await;
        let output = output.borrow().clone();
        assert!(output.contains("Hello, Scheme!"), "{output}");
        assert!(output.contains("arguments: --flag two words"), "{output}");
        dap.request("disconnect", json!({ "terminateDebuggee": true })).await.ok();
    });
}

/// What zcode's breakpoints rely on in lldb-dap: a condition and an ignore
/// count (hit condition), a symbolic breakpoint made by an lldb command
/// limited to a module, a Swift error breakpoint (exception filter) whose
/// stop names it by label, and lldb commands through `evaluate`.
#[test]
#[ignore]
fn breakpoint_options_symbolic_and_swift_error_breakpoints() {
    use zcode::debugger::breakpoints::{Breakpoint, Kind, created_id, filter_for_stop};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/ZKit").canonicalize().unwrap();
    assert!(Command::new("/usr/bin/swift").arg("build").current_dir(&root).status().unwrap().success());
    let program = root.join(".build/debug/zhello");
    let source = root.join("Sources/zhello/main.swift");
    let text = std::fs::read_to_string(&source).unwrap();
    let tick_line = text.lines().position(|l| l.contains("print(\"tick")).unwrap() as u32 + 1;

    let binary = zcode::toolchain::find_tool("lldb-dap").unwrap();
    let (tx, rx) = smol::channel::unbounded();
    let dap = DapClient::spawn(&binary, tx).unwrap();
    smol::block_on(async {
        let next_stop = || {
            let rx = rx.clone();
            let wait = async move {
                loop {
                    match rx.recv().await.unwrap() {
                        DapEvent::Event { event, body } if event == "stopped" => return body,
                        DapEvent::Exited => panic!("adapter exited"),
                        _ => {}
                    }
                }
            };
            smol::future::or(wait, async {
                smol::Timer::after(Duration::from_secs(60)).await;
                panic!("did not stop")
            })
        };
        let command = |command: String, frame: Option<i64>| {
            dap.request("evaluate", json!({ "expression": format!("`{command}"), "context": "repl", "frameId": frame }))
        };
        dap.request("initialize", initialize_args()).await.unwrap();
        let launch = dap.request("launch", json!({ "program": program, "cwd": root, "stopOnEntry": false }));
        loop {
            if let DapEvent::Event { event, .. } = rx.recv().await.unwrap()
                && event == "initialized"
            {
                break;
            }
        }
        // Stop when i > 1, after ignoring one such hit: i == 3.
        let mut line = Breakpoint::new(Kind::Line { path: source.clone(), line: tick_line });
        line.condition = "i > 1".into();
        line.ignore_count = 1;
        let set = dap
            .request("setBreakpoints", json!({ "source": { "path": source }, "breakpoints": [line.source_breakpoint().unwrap()] }))
            .await
            .unwrap();
        let line_id = set["breakpoints"][0]["id"].as_i64().unwrap();
        let mut symbolic = Breakpoint::new(Kind::Symbolic { symbol: "Greeter.greeting".into(), module: "zhello".into() });
        symbolic.id = 7;
        let created = command(symbolic.symbolic_command().unwrap(), None).await.unwrap();
        let symbolic_id = created_id(created["result"].as_str().unwrap()).expect("breakpoint set output");
        dap.request("setExceptionBreakpoints", json!({ "filters": Breakpoint::new(Kind::SwiftError).exception_filters() }))
            .await
            .unwrap();
        dap.request("configurationDone", json!({})).await.unwrap();
        launch.await.unwrap();

        let stop = next_stop().await;
        assert_eq!(stop["hitBreakpointIds"][0].as_i64(), Some(symbolic_id), "{stop}");
        let thread = stop["threadId"].as_i64().unwrap();
        dap.request("continue", json!({ "threadId": thread })).await.unwrap();

        let stop = next_stop().await;
        assert_eq!(stop["hitBreakpointIds"][0].as_i64(), Some(line_id), "{stop}");
        let stack = dap.request("stackTrace", json!({ "threadId": thread, "levels": 1 })).await.unwrap();
        let frame = stack["stackFrames"][0]["id"].as_i64();
        let value = command("po i".into(), frame).await.unwrap();
        assert_eq!(value["result"].as_str().unwrap().trim(), "3", "{value}");
        let value = command("expr i * 10".into(), frame).await.unwrap();
        assert!(value["result"].as_str().unwrap().contains("30"), "{value}");
        dap.request("continue", json!({ "threadId": thread })).await.unwrap();

        let stop = next_stop().await;
        assert_eq!(stop["reason"], "exception", "{stop}");
        assert_eq!(filter_for_stop(stop["description"].as_str().unwrap()), Some("swift_throw"), "{stop}");
        dap.request("disconnect", json!({ "terminateDebuggee": true })).await.ok();
    });
}
