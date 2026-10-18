//! One build server connection: sourcekit-lsp's Build Server Protocol
//! requests, answered from a project's [`BuildGraph`]. Runs on threads of
//! its own (never the main thread): requests are answered from the graph
//! as it is, and a new graph is announced to every connection with
//! `buildTarget/didChange` for the targets it changed.
//!
//! sourcekit-lsp sends the older method names (`textDocument/sourceKitOptions`,
//! `buildTarget/prepare`); newer ones prefix them with `sourcekit/`. Both
//! are answered.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use super::graph::{BuildGraph, Kind, Origin, is_header, language_of};
use crate::lsp::{path_to_uri, uri_to_path};

/// How long a request that needs the graph waits for the first one.
const FIRST_GRAPH: Duration = Duration::from_secs(5);

/// What `bsp status` and `dump` show.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    /// Connections so far, and open now.
    pub connections: usize,
    pub open: usize,
    pub requests: BTreeMap<String, usize>,
    /// The last file whose arguments were asked for, and where they came from.
    pub last_options: Option<(PathBuf, Option<Origin>)>,
    /// `buildTarget/didChange` notifications sent.
    pub changes_sent: usize,
}

/// A project's build server state, shared by its connections and the
/// workspace that keeps its graph current (`super::model`).
pub struct Shared {
    pub root: PathBuf,
    /// The index store the index build writes and sourcekit-lsp reads, and
    /// sourcekit-lsp's database of it: known before any graph, since
    /// sourcekit-lsp opens them once, when it connects.
    index: Option<(PathBuf, PathBuf)>,
    graph: RwLock<Arc<BuildGraph>>,
    /// Whether a graph has been made yet.
    ready: (Mutex<bool>, Condvar),
    connections: Mutex<Vec<smol::channel::Sender<Value>>>,
    /// Wakes the workspace's side: a connection started, or sourcekit-lsp
    /// saw files change.
    wake: smol::channel::Sender<()>,
    stats: Mutex<Stats>,
}

impl Shared {
    pub fn new(root: PathBuf, index: Option<(PathBuf, PathBuf)>) -> (Arc<Self>, smol::channel::Receiver<()>) {
        let (wake, woken) = smol::channel::unbounded();
        let shared = Arc::new(Self {
            root,
            index,
            graph: RwLock::new(Arc::new(BuildGraph::default())),
            ready: (Mutex::new(false), Condvar::new()),
            connections: Mutex::new(vec![]),
            wake,
            stats: Mutex::new(Stats::default()),
        });
        (shared, woken)
    }

    /// A server answering from `graph` alone (`zcode bsp --graph`).
    pub fn fixed(root: PathBuf, graph: BuildGraph) -> Arc<Self> {
        let index = Some((graph.index_store.clone(), graph.index_database.clone())).filter(|(store, _)| !store.as_os_str().is_empty());
        let (shared, _) = Self::new(root, index);
        shared.set_graph(graph);
        shared
    }

    pub fn graph(&self) -> Arc<BuildGraph> {
        self.graph.read().unwrap().clone()
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().unwrap().clone()
    }

    pub fn is_ready(&self) -> bool {
        *self.ready.0.lock().unwrap()
    }

    /// Replaces the graph, telling every connection which targets changed.
    /// Returns how many did.
    pub fn set_graph(&self, graph: BuildGraph) -> usize {
        let old = std::mem::replace(&mut *self.graph.write().unwrap(), Arc::new(graph));
        let new = self.graph();
        {
            let (ready, changed) = &self.ready;
            *ready.lock().unwrap() = true;
            changed.notify_all();
        }
        let changes: Vec<Value> = match new.changed_since(&old) {
            Some(ids) => ids.into_iter().map(|id| json!({ "target": { "uri": id }, "kind": 2 })).collect(),
            None => {
                let created = new.targets.iter().filter(|t| old.target(&t.id).is_none()).map(|t| (t.id.clone(), 1));
                let changed = new.targets.iter().filter(|t| old.target(&t.id).is_some()).map(|t| (t.id.clone(), 2));
                let deleted = old.targets.iter().filter(|t| new.target(&t.id).is_none()).map(|t| (t.id.clone(), 3));
                created.chain(changed).chain(deleted).map(|(id, kind)| json!({ "target": { "uri": id }, "kind": kind })).collect()
            }
        };
        if changes.is_empty() {
            return 0;
        }
        let count = changes.len();
        let notification = json!({ "jsonrpc": "2.0", "method": "buildTarget/didChange", "params": { "changes": changes } });
        let mut connections = self.connections.lock().unwrap();
        connections.retain(|c| c.send_blocking(notification.clone()).is_ok());
        if !connections.is_empty() {
            self.stats.lock().unwrap().changes_sent += 1;
        }
        count
    }

    /// Waits (a while) for the first graph.
    fn wait_ready(&self) {
        let (ready, changed) = &self.ready;
        let guard = ready.lock().unwrap();
        drop(changed.wait_timeout_while(guard, FIRST_GRAPH, |ready| !*ready));
    }

    fn count(&self, method: &str) {
        *self.stats.lock().unwrap().requests.entry(method.to_string()).or_default() += 1;
    }
}

/// The projects open in this zcode, for the connections `zcode bsp` relays.
static REGISTRY: Mutex<Vec<Weak<Shared>>> = Mutex::new(vec![]);

pub fn register(shared: &Arc<Shared>) {
    let mut registry = REGISTRY.lock().unwrap();
    registry.retain(|s| s.strong_count() > 0);
    registry.push(Arc::downgrade(shared));
}

/// The open project serving `folder`: the one whose root it is, or the
/// innermost one it's in.
pub fn find(folder: &Path) -> Option<Arc<Shared>> {
    let folder = folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf());
    REGISTRY
        .lock()
        .unwrap()
        .iter()
        .filter_map(Weak::upgrade)
        .filter(|s| folder.starts_with(&s.root) || s.root.canonicalize().is_ok_and(|root| folder.starts_with(root)))
        .max_by_key(|s| s.root.components().count())
}

/// Reads one `Content-Length`-framed message; `None` at the end.
pub fn read_message(reader: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

pub fn write_message(writer: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let body = message.to_string();
    write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    writer.flush()
}

/// Serves one connection until it closes or sourcekit-lsp sends
/// `build/exit`. `lookup` finds the project a `build/initialize` names.
pub fn serve(mut reader: impl BufRead, writer: impl Write + Send + 'static, lookup: impl Fn(&Path) -> Option<Arc<Shared>>) {
    let (out, outgoing) = smol::channel::unbounded::<Value>();
    let writing = std::thread::Builder::new().name("bsp-writer".into()).spawn(move || {
        let mut writer = writer;
        while let Ok(message) = outgoing.recv_blocking() {
            if write_message(&mut writer, &message).is_err() {
                break;
            }
        }
    });
    let mut connection = Connection { shared: None };
    while let Ok(Some(message)) = read_message(&mut reader) {
        let method = message.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if method == "build/initialize" {
            let root = path_at(&params, "/rootUri");
            connection.shared = root.as_deref().and_then(&lookup);
            match &connection.shared {
                Some(shared) => {
                    log::info!("build server: sourcekit-lsp connected for {}", shared.root.display());
                    shared.connections.lock().unwrap().push(out.clone());
                    let mut stats = shared.stats.lock().unwrap();
                    stats.connections += 1;
                    stats.open += 1;
                    drop(stats);
                    shared.wake.send_blocking(()).ok();
                }
                None => log::warn!("build server: no open project at {root:?}; answering with no targets"),
            }
        }
        if method == "build/exit" {
            break;
        }
        let Some(id) = message.get("id").cloned() else {
            connection.notification(&method, &params);
            continue;
        };
        let response = match connection.request(&method, &params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, error)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": error } }),
        };
        if out.send_blocking(response).is_err() {
            break;
        }
    }
    if let Some(shared) = &connection.shared {
        shared.stats.lock().unwrap().open -= 1;
        shared.connections.lock().unwrap().retain(|c| !c.same_channel(&out));
    }
    drop(connection);
    out.close();
    if let Ok(writing) = writing {
        writing.join().ok();
    }
}

struct Connection {
    shared: Option<Arc<Shared>>,
}

/// The file a URI in a message names.
fn path_at(value: &Value, pointer: &str) -> Option<PathBuf> {
    value.pointer(pointer).and_then(Value::as_str).and_then(|uri| uri.parse::<lsp_types::Uri>().ok()).and_then(|uri| uri_to_path(&uri))
}

/// A method's name without the `sourcekit/` prefix newer sourcekit-lsps use.
fn unprefixed(method: &str) -> &str {
    method.strip_prefix("sourcekit/").unwrap_or(method)
}

fn uri(path: &Path) -> String {
    path_to_uri(path).as_str().to_string()
}

impl Connection {
    fn graph(&self) -> Arc<BuildGraph> {
        match &self.shared {
            Some(shared) => {
                shared.wait_ready();
                shared.graph()
            }
            None => Arc::new(BuildGraph::default()),
        }
    }

    fn notification(&mut self, method: &str, _params: &Value) {
        if let Some(shared) = &self.shared {
            shared.count(method);
            // Files changed: the workspace's side looks again.
            if method == "workspace/didChangeWatchedFiles" {
                shared.wake.send_blocking(()).ok();
            }
        }
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        if let Some(shared) = &self.shared {
            shared.count(method);
        }
        match unprefixed(method) {
            "build/initialize" => Ok(self.initialize()),
            "build/shutdown" => Ok(Value::Null),
            "workspace/buildTargets" => Ok(json!({ "targets": self.graph().targets.iter().map(build_target).collect::<Vec<_>>() })),
            "buildTarget/sources" => {
                let graph = self.graph();
                let items: Vec<Value> = target_ids(params)
                    .filter_map(|id| graph.target(&id))
                    .map(|target| {
                        let files = target.files.iter().map(|f| (f, false)).chain(target.generated.iter().map(|f| (f, true)));
                        let sources: Vec<Value> = files
                            .map(|(file, generated)| {
                                let mut data = json!({ "kind": if is_header(file) { "header" } else { "source" } });
                                if let Some(language) = language_of(file) {
                                    data["language"] = json!(language);
                                }
                                json!({ "uri": uri(file), "kind": 1, "generated": generated, "dataKind": "sourceKit", "data": data })
                            })
                            .collect();
                        json!({ "target": { "uri": target.id }, "sources": sources })
                    })
                    .collect();
                Ok(json!({ "items": items }))
            }
            "buildTarget/inverseSources" => {
                let graph = self.graph();
                let file = path_at(params, "/textDocument/uri");
                let targets: Vec<Value> =
                    file.map(|file| graph.targets_of(&file).into_iter().map(|t| json!({ "uri": t.id })).collect()).unwrap_or_default();
                Ok(json!({ "targets": targets }))
            }
            "textDocument/sourceKitOptions" => {
                let graph = self.graph();
                let file = path_at(params, "/textDocument/uri");
                let target = params.pointer("/target/uri").and_then(Value::as_str);
                let language = params.get("language").and_then(Value::as_str);
                let (Some(file), Some(target)) = (file, target) else { return Ok(Value::Null) };
                let options = graph.options(target, &file, language);
                if let Some(shared) = &self.shared {
                    shared.stats.lock().unwrap().last_options = Some((file.clone(), options.as_ref().map(|o| o.origin)));
                }
                Ok(match options {
                    Some(options) => json!({
                        "compilerArguments": options.arguments,
                        "workingDirectory": options.working_directory,
                        "data": { "origin": options.origin },
                    }),
                    None => Value::Null,
                })
            }
            "workspace/waitForBuildSystemUpdates" => {
                self.graph();
                Ok(Value::Null)
            }
            // Not offered (zcode's index build prepares), but harmless.
            "buildTarget/prepare" => Ok(json!({})),
            _ => Err((-32601, format!("method not found: {method}"))),
        }
    }

    fn initialize(&self) -> Value {
        let mut data = json!({ "sourceKitOptionsProvider": true, "prepareProvider": false, "outputPathsProvider": false, "watchers": [] });
        // The index the index build writes (there before the build, so
        // sourcekit-lsp can open it); the database is sourcekit-lsp's, next
        // to it as with the compilation database.
        if let Some((store, database)) = self.shared.as_ref().and_then(|s| s.index.clone()) {
            std::fs::create_dir_all(&store).ok();
            data["indexStorePath"] = json!(store);
            data["indexDatabasePath"] = json!(database);
        }
        json!({
            "displayName": "zcode",
            "version": env!("CARGO_PKG_VERSION"),
            "bspVersion": "2.2.0",
            "capabilities": { "languageIds": ["swift", "c", "cpp", "objective-c", "objective-cpp"], "buildTargetChangedProvider": true, "inverseSourcesProvider": true },
            "dataKind": "sourceKit",
            "data": data,
        })
    }
}

fn target_ids(params: &Value) -> impl Iterator<Item = String> + '_ {
    params.get("targets").and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.get("uri").and_then(Value::as_str)).map(str::to_string)
}

fn build_target(target: &super::graph::Target) -> Value {
    let mut languages: Vec<&str> = target.files.iter().filter_map(|f| language_of(f)).collect();
    languages.sort();
    languages.dedup();
    json!({
        "id": { "uri": target.id },
        "displayName": target.name,
        "baseDirectory": uri(&target.directory),
        "tags": [target.kind.tag()],
        "capabilities": { "canCompile": true, "canTest": target.kind == Kind::Test, "canRun": target.kind == Kind::Application, "canDebug": false },
        "languageIds": languages,
        "dependencies": [],
        "dataKind": "sourceKit",
        "data": {},
    })
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;

    use super::super::graph::{ClangCommand, Target};
    use super::*;

    fn graph(files: &[&str]) -> BuildGraph {
        BuildGraph {
            targets: vec![Target {
                id: "xcode://target/0/App/App".into(),
                name: "App".into(),
                project: Some("App".into()),
                kind: Kind::Application,
                directory: "/p".into(),
                files: files.iter().map(PathBuf::from).collect(),
                generated: vec![],
                swift: Some(vec!["-module-name".into(), "App".into()]),
                swift_origin: Some(Origin::BuildLog),
                clang: BTreeMap::new(),
                clang_template: None::<ClangCommand>,
                clang_origin: None,
            }],
            index_store: "/dd/Index.noindex/DataStore".into(),
            index_database: "/dd/Index.noindex/IndexDatabase".into(),
        }
    }

    struct Client {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
        next: i64,
    }

    impl Client {
        fn request(&mut self, method: &str, params: Value) -> Value {
            self.next += 1;
            write_message(&mut self.writer, &json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params })).unwrap();
            loop {
                let message = read_message(&mut self.reader).unwrap().unwrap();
                if message["id"] == self.next {
                    return message;
                }
            }
        }

        fn notify(&mut self, method: &str, params: Value) {
            write_message(&mut self.writer, &json!({ "jsonrpc": "2.0", "method": method, "params": params })).unwrap();
        }
    }

    #[test]
    fn answers_sourcekit_lsp_and_announces_changes() {
        let store = std::env::temp_dir().join(format!("zcode-bsp-store-{}", std::process::id()));
        let index = Some((store.join("DataStore"), store.join("IndexDatabase")));
        let (shared, woken) = Shared::new("/p".into(), index);
        let (ours, theirs) = UnixStream::pair().unwrap();
        let serving = shared.clone();
        let thread = std::thread::spawn(move || serve(BufReader::new(theirs.try_clone().unwrap()), theirs, move |_| Some(serving.clone())));
        let mut client = Client { reader: BufReader::new(ours.try_clone().unwrap()), writer: ours, next: 0 };

        let init = client.request("build/initialize", json!({ "rootUri": "file:///p", "capabilities": {} }));
        let data = &init["result"]["data"];
        assert_eq!(data["sourceKitOptionsProvider"], true);
        assert_eq!(data["prepareProvider"], false);
        // Known (and made) before any graph.
        assert_eq!(data["indexStorePath"], json!(store.join("DataStore")));
        assert!(store.join("DataStore").is_dir());
        // The connection wakes the workspace's side, which makes the graph.
        woken.recv_blocking().unwrap();
        shared.set_graph(graph(&["/p/App/A.swift"]));
        client.notify("build/initialized", json!({}));

        let targets = client.request("workspace/buildTargets", json!({}));
        let target = &targets["result"]["targets"][0];
        assert_eq!(target["id"]["uri"], "xcode://target/0/App/App");
        assert_eq!(target["tags"], json!(["application"]));
        assert_eq!(target["languageIds"], json!(["swift"]));

        let sources = client.request("buildTarget/sources", json!({ "targets": [{ "uri": "xcode://target/0/App/App" }] }));
        let item = &sources["result"]["items"][0]["sources"][0];
        assert_eq!(item["uri"], "file:///p/App/A.swift");
        assert_eq!(item["data"]["language"], "swift");

        // Both names sourcekit-lsp has used.
        for method in ["textDocument/sourceKitOptions", "sourcekit/textDocument/sourceKitOptions"] {
            let options = client.request(
                method,
                json!({ "textDocument": { "uri": "file:///p/App/A.swift" }, "target": { "uri": "xcode://target/0/App/App" }, "language": "swift" }),
            );
            assert_eq!(options["result"]["compilerArguments"], json!(["-module-name", "App", "/p/App/A.swift"]), "{method}");
            assert_eq!(options["result"]["data"]["origin"], "build-log");
        }
        assert_eq!(client.request("workspace/waitForBuildSystemUpdates", json!({}))["result"], Value::Null);
        assert_eq!(client.request("build/nonsense", json!({}))["error"]["code"], -32601);

        // A file joins the target: sourcekit-lsp is told the target changed.
        assert_eq!(shared.set_graph(graph(&["/p/App/A.swift", "/p/App/B.swift"])), 1);
        let notification = read_message(&mut client.reader).unwrap().unwrap();
        assert_eq!(notification["method"], "buildTarget/didChange");
        assert_eq!(notification["params"]["changes"], json!([{ "target": { "uri": "xcode://target/0/App/App" }, "kind": 2 }]));
        let stats = shared.stats();
        // The first graph's targets were announced too.
        assert_eq!((stats.connections, stats.open, stats.changes_sent), (1, 1, 2));
        assert_eq!(stats.last_options.unwrap().0, PathBuf::from("/p/App/A.swift"));

        assert_eq!(client.request("build/shutdown", json!(null))["result"], Value::Null);
        client.notify("build/exit", json!(null));
        thread.join().unwrap();
        assert_eq!(shared.stats().open, 0);
        std::fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn finds_the_project_a_folder_is_in() {
        let (outer, _) = Shared::new("/zcode-test-find/a".into(), None);
        let (inner, _) = Shared::new("/zcode-test-find/a/b".into(), None);
        register(&outer);
        register(&inner);
        assert!(Arc::ptr_eq(&find(Path::new("/zcode-test-find/a/b/c")).unwrap(), &inner));
        assert!(Arc::ptr_eq(&find(Path::new("/zcode-test-find/a")).unwrap(), &outer));
        assert!(find(Path::new("/zcode-test-find/elsewhere")).is_none());
    }
}
