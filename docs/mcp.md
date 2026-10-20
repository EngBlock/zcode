# zcode as an MCP server

`zcode mcp` speaks the [Model Context Protocol](https://modelcontextprotocol.io)
on stdin and stdout. It connects to the running zcode over its Unix socket
(the one `zcode <path>` hands paths to) and relays messages; the protocol is
answered inside zcode, where the project is. zcode has to be running (a
window, or `zcode --background`); without it `zcode mcp` says so and exits.

Add it to Claude Code (the `zcode` binary on your `PATH`, or its full path):

```
claude mcp add zcode -- zcode mcp                  # this project only
claude mcp add --scope user zcode -- zcode mcp     # every project
```

Other clients take the same command and argument:

* Codex: `codex mcp add zcode -- zcode mcp`, or in `~/.codex/config.toml`
  `[mcp_servers.zcode]` with `command = "zcode"` and `args = ["mcp"]`.
* Gemini CLI: `gemini mcp add zcode zcode mcp`, or `"mcpServers": { "zcode":
  { "command": "zcode", "args": ["mcp"] } }` in `~/.gemini/settings.json`.
* Cursor: the same `mcpServers` entry in `~/.cursor/mcp.json` (or a
  project's `.cursor/mcp.json`).
* Zed: `"context_servers": { "zcode": { "command": "zcode", "args": ["mcp"] } }`
  in its `settings.json`.
* Anything else: run `zcode mcp` as a stdio server.

zcode's own agent panel needs none of this: every agent it starts gets the
server in its session (`session/new`'s `mcpServers`), unless
`"agent": { "zcode_mcp_server": false }` in `settings.json`.

Which project: the one the client started `zcode mcp` in (its working
folder inside a project zcode has open), else the frontmost window; every
tool also takes `project` (a path in the project). The agent panel's agent
always works on its own window's project.

| Tool | What it does |
|---|---|
| `XcodeListSchemes`, `XcodeSwitchScheme` | The schemes and the web apps' `package.json` scripts (`web: dev`, with each run target's kind), configurations and test plans, and which are chosen; choose one as the toolbar does |
| `XcodeListRunDestinations`, `XcodeSwitchRunDestination` | My Mac, simulators (booted or not) and devices for the scheme; choose one |
| `BuildProject` | Product ▸ Build: waits, then success, the errors and warnings (file, line, column) and the log's file |
| `RunProject` | Product ▸ Run (under the debugger when breakpoints are set): returns once it's running, with its first console output. A script runs in the debug area's terminal beside a running app; it returns its first output, its dev server's URL and the errors it printed |
| `StopProject` | Product ▸ Stop: the chosen run target's run (a script's, or the build, the app and the debugger), or `scheme`'s |
| `RunAllTests`, `RunSomeTests` | Product ▸ Test, or some tests by the Test navigator's names: every test's result and duration, the failures at their lines |
| `GetTestList` | The tests found in source, with where they are and their latest results |
| `XcodeListNavigatorIssues` | The Issue navigator: live sourcekit-lsp diagnostics and the last build's issues, for a file or all |
| `GetBuildLog` | The build log, condensed or raw, filtered; the last builds' reports and log files |
| `GetConsoleOutput` | The app's and the debugger's console and the scripts running; with `scheme`, a script's output |
| `XcodeGetCurrentFile` | The focused editor's file, cursor, selection and selected text, unsaved changes, and its text if asked |
| `OpenFile` | Opens a file at a line and column, so you see it |
| `EvaluateInDebugger` | An LLDB command (`po`, `p`, `v`, `bt`, `expr`) in the paused frame, shown in the console |
| `TakeScreenshot` | A PNG of the project window (a pixel per point) or of a simulator's screen (`simctl io screenshot`); never a device |
| `ProfileProject` | Product ▸ Profile for a time limit (10 s unless told; Time Profiler unless another template is named): the trace, opened in Instruments, and the heaviest functions |
| `ArchiveProject` | Product ▸ Archive: the archive's path, app, version and team, or the errors. It never exports or uploads; Distribute App in the Organizer does that |

Xcode 27 has an MCP server too (`xcrun mcpbridge`, for a running Xcode);
where it has the tool, zcode's has its name, so instructions written for
Xcode's tools find zcode's. The arguments are zcode's own.

The protocol: zcode answers the handshake revisions (`initialize` in
2025-11-25, 2025-06-18 or 2025-03-26) and the current one, 2026-07-28 (the
revision in every request's `_meta`, `server/discover`, `resultType`;
an unknown revision is refused with the versions zcode speaks). Tool
results carry `structuredContent` and the same JSON as text; screenshots
are `image` content. A request with a `progressToken` gets
`notifications/progress` while a build or test runs (the step it's on,
tests run and failed); `notifications/cancelled` stops the build, run or
test (and the request gets no answer, as the spec says); a client that
disconnects stops what it started.

Security: the socket is in zcode's own cache folder (mode 0700), created
readable and writable by its owner alone (0600), and zcode checks every
connection's peer (`getpeereid`) and refuses other users before reading
anything; `zcode mcp` checks that the socket's other end is the same user
too. The control channel (`ZCODE_DEBUG_DIR`) is a separate test hook and
isn't reachable through MCP. Tools that start, stop or evaluate something
(`BuildProject`, `RunProject`, `StopProject`, `RunAllTests`,
`RunSomeTests`, `EvaluateInDebugger`, `ProfileProject`, `ArchiveProject`) called by the agent panel's agent
ask in the panel's thread first, with Allow, Always Allow (for the
conversation) and Reject, unless `agent.always_allow_tool_actions` is on;
the panel knows its agent by a random token in the server's environment.
Other clients ask their users themselves, as Claude Code does before
running an MCP tool.

`scripts/mcp_client.py` is a small client for trying it:
`python3 scripts/mcp_client.py tools`, `python3 scripts/mcp_client.py call
BuildProject`, `--modern` for 2026-07-28.
