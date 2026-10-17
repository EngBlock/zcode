//! The language servers zcode runs: which files each serves, which folder
//! it runs for, and what it's told when it starts and when it asks for its
//! settings.
//!
//! sourcekit-lsp serves Swift and the C family for the whole project. The
//! web servers serve the web app a file belongs to: the outermost folder
//! inside the project with a `package.json`, `tsconfig.json` or
//! `jsconfig.json` (a Vite app in `web/` of an Xcode project gets servers
//! of its own, rooted there), or the project's folder for a loose HTML or
//! JavaScript file. A document has every server that serves its language,
//! in the order of [`SERVERS`]: the first one answers hover, definitions,
//! formatting and the like, and every one's completions, code actions and
//! diagnostics are shown.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// How a server's program is found.
#[derive(Debug)]
pub enum Launch {
    /// An Xcode tool, through `xcrun --find`.
    Xcrun(&'static str),
    /// An npm package's executable: the project's `node_modules/.bin`, then
    /// `PATH`, then zcode's own copy (installed on first use unless
    /// `auto_install_language_servers` is off). `with` are installed next
    /// to it, as npm specs (TypeScript, for the servers that need its
    /// library).
    Npm { package: &'static str, bin: &'static str, with: &'static [&'static str] },
    /// Only ever the project's own (`node_modules/.bin`): never installed.
    Project { bin: &'static str },
}

/// The folders a server runs for.
#[derive(Debug)]
pub enum Scope {
    /// The whole project.
    Project,
    /// The web app a file is in, or the project's folder for one in none.
    Web,
    /// The web app a file is in; nothing for a file in none.
    WebApp,
    /// A web app using it: a `package.json` between the file and the web
    /// app's folder depends on one of `packages`, or one of `files` is in
    /// the web app's folder.
    Uses { packages: &'static [&'static str], files: &'static [&'static str] },
}

#[derive(Debug)]
pub struct ServerSpec {
    /// Its name in settings (`lsp.<id>`), the status bar and its log.
    pub id: &'static str,
    /// The LSP language ids it serves.
    pub languages: &'static [&'static str],
    /// Languages it serves only for a web app depending on a package
    /// (vtsls serves Vue files through Vue's TypeScript plugin).
    pub languages_with: &'static [(&'static str, &'static str)],
    pub launch: Launch,
    pub args: &'static [&'static str],
    pub scope: Scope,
}

/// TypeScript for the servers that load its library themselves: 5.x, the
/// last with a JavaScript API (7 is the native compiler, which has none).
const TYPESCRIPT: &[&str] = &["typescript@5"];

/// The JavaScript and TypeScript languages, as LSP names them.
const SCRIPT: &[&str] = &["javascript", "javascriptreact", "typescript", "typescriptreact"];

/// Every server, in the order a document's are asked.
pub static SERVERS: &[ServerSpec] = &[
    ServerSpec {
        id: "sourcekit-lsp",
        languages: &["swift", "c", "cpp", "objective-c", "objective-cpp"],
        languages_with: &[],
        launch: Launch::Xcrun("sourcekit-lsp"),
        args: &[],
        scope: Scope::Project,
    },
    ServerSpec {
        id: "vue-language-server",
        languages: &["vue"],
        languages_with: &[],
        launch: Launch::Npm { package: "@vue/language-server", bin: "vue-language-server", with: TYPESCRIPT },
        args: &["--stdio"],
        scope: Scope::Web,
    },
    ServerSpec {
        id: "svelte-language-server",
        languages: &["svelte"],
        languages_with: &[],
        launch: Launch::Npm { package: "svelte-language-server", bin: "svelteserver", with: TYPESCRIPT },
        args: &["--stdio"],
        scope: Scope::Web,
    },
    ServerSpec {
        id: "astro-language-server",
        languages: &["astro"],
        languages_with: &[],
        launch: Launch::Npm { package: "@astrojs/language-server", bin: "astro-ls", with: TYPESCRIPT },
        args: &["--stdio"],
        scope: Scope::Web,
    },
    ServerSpec {
        id: "vtsls",
        languages: SCRIPT,
        languages_with: &[("vue", "vue")],
        launch: Launch::Npm { package: "@vtsls/language-server", bin: "vtsls", with: &[] },
        args: &["--stdio"],
        scope: Scope::Web,
    },
    ServerSpec {
        id: "vscode-html-language-server",
        languages: &["html"],
        languages_with: &[],
        launch: Launch::Npm { package: "vscode-langservers-extracted", bin: "vscode-html-language-server", with: &[] },
        args: &["--stdio"],
        scope: Scope::Web,
    },
    ServerSpec {
        id: "vscode-css-language-server",
        languages: &["css", "scss", "less"],
        languages_with: &[],
        launch: Launch::Npm { package: "vscode-langservers-extracted", bin: "vscode-css-language-server", with: &[] },
        args: &["--stdio"],
        scope: Scope::Web,
    },
    ServerSpec {
        id: "vscode-json-language-server",
        languages: &["json", "jsonc"],
        languages_with: &[],
        launch: Launch::Npm { package: "vscode-langservers-extracted", bin: "vscode-json-language-server", with: &[] },
        args: &["--stdio"],
        // Not for an iOS app's asset catalogs: only a web app's JSON.
        scope: Scope::WebApp,
    },
    ServerSpec {
        id: "eslint",
        languages: &["javascript", "javascriptreact", "typescript", "typescriptreact", "vue", "svelte", "astro"],
        languages_with: &[],
        launch: Launch::Npm { package: "vscode-langservers-extracted", bin: "vscode-eslint-language-server", with: &[] },
        args: &["--stdio"],
        scope: Scope::Uses { packages: &["eslint"], files: ESLINT_CONFIGS },
    },
    ServerSpec {
        id: "biome",
        languages: &["javascript", "javascriptreact", "typescript", "typescriptreact", "json", "jsonc", "css"],
        languages_with: &[],
        launch: Launch::Project { bin: "biome" },
        args: &["lsp-proxy"],
        scope: Scope::Uses { packages: &["@biomejs/biome"], files: &["biome.json", "biome.jsonc"] },
    },
    ServerSpec {
        id: "tailwindcss-language-server",
        languages: &[
            "html", "css", "scss", "less", "javascript", "javascriptreact", "typescript", "typescriptreact", "vue", "svelte", "astro",
        ],
        languages_with: &[],
        launch: Launch::Npm { package: "@tailwindcss/language-server", bin: "tailwindcss-language-server", with: &[] },
        args: &["--stdio"],
        scope: Scope::Uses { packages: &["tailwindcss", "@tailwindcss/vite", "@tailwindcss/postcss"], files: TAILWIND_CONFIGS },
    },
];

const ESLINT_CONFIGS: &[&str] = &[
    "eslint.config.js",
    "eslint.config.mjs",
    "eslint.config.cjs",
    "eslint.config.ts",
    "eslint.config.mts",
    "eslint.config.cts",
    ".eslintrc",
    ".eslintrc.js",
    ".eslintrc.cjs",
    ".eslintrc.json",
    ".eslintrc.yaml",
    ".eslintrc.yml",
];

const TAILWIND_CONFIGS: &[&str] =
    &["tailwind.config.js", "tailwind.config.cjs", "tailwind.config.mjs", "tailwind.config.ts"];

/// What marks a web app's folder.
const WEB_MARKERS: &[&str] = &["package.json", "tsconfig.json", "jsconfig.json"];

pub fn spec(id: &str) -> Option<&'static ServerSpec> {
    SERVERS.iter().find(|spec| spec.id == id)
}

impl ServerSpec {
    pub fn is_sourcekit(&self) -> bool {
        matches!(self.launch, Launch::Xcrun(_))
    }

    /// The folder this server runs for to serve `path` (a file of
    /// `language`) in the project at `project_root`, or `None` when it
    /// doesn't serve it.
    pub fn root_for(&self, path: &Path, language: &str, project_root: &Path, packages: &mut PackageCache) -> Option<PathBuf> {
        let conditional = self.languages_with.iter().find(|(lang, _)| *lang == language);
        if !self.languages.contains(&language) && conditional.is_none() {
            return None;
        }
        if matches!(self.scope, Scope::Project) {
            return Some(project_root.to_path_buf());
        }
        let web = web_root(path, project_root);
        if let Some((_, package)) = conditional
            && !web.as_ref().is_some_and(|web| packages.uses(path, web, &[package], &[]))
        {
            return None;
        }
        match &self.scope {
            Scope::Project => unreachable!(),
            Scope::Web => Some(web.unwrap_or_else(|| project_root.to_path_buf())),
            Scope::WebApp => web,
            Scope::Uses { packages: names, files } => web.filter(|web| packages.uses(path, web, names, files)),
        }
    }

    /// `initializationOptions`, before the user's `lsp.<id>.initialization_options`.
    pub fn initialization_options(&self, context: &ServerContext) -> Value {
        let tsdk = context.typescript_lib.as_ref().map(|lib| lib.to_string_lossy().to_string());
        match self.id {
            "sourcekit-lsp" => json!({ "backgroundIndexing": true }),
            "vscode-html-language-server" => json!({
                "provideFormatter": true,
                "embeddedLanguages": { "css": true, "javascript": true },
                "configurationSection": ["html", "css", "javascript"],
            }),
            "vscode-css-language-server" | "vscode-json-language-server" => json!({ "provideFormatter": true }),
            "vue-language-server" => json!({ "typescript": { "tsdk": tsdk }, "vue": { "hybridMode": true } }),
            "astro-language-server" => json!({ "typescript": { "tsdk": tsdk } }),
            "tailwindcss-language-server" => json!({ "userLanguages": {} }),
            _ => Value::Null,
        }
    }

    /// Everything it's told through `workspace/didChangeConfiguration` and
    /// answered from for `workspace/configuration`, before the user's
    /// `lsp.<id>.settings`.
    pub fn settings(&self, context: &ServerContext) -> Value {
        match self.id {
            "vtsls" => {
                let language = json!({
                    "suggest": { "completeFunctionCalls": true },
                    "inlayHints": {
                        "parameterNames": { "enabled": "literals", "suppressWhenArgumentMatchesName": true },
                        "parameterTypes": { "enabled": true },
                        "variableTypes": { "enabled": false },
                        "propertyDeclarationTypes": { "enabled": true },
                        "functionLikeReturnTypes": { "enabled": true },
                        "enumMemberValues": { "enabled": true },
                    },
                    "updateImportsOnFileMove": { "enabled": "always" },
                    "format": { "tabSize": context.tab_size, "indentSize": context.tab_size, "convertTabsToSpaces": !context.hard_tabs },
                });
                let plugins: Vec<Value> = context
                    .vue_plugin
                    .iter()
                    .map(|location| {
                        json!({
                            "name": "@vue/typescript-plugin",
                            "location": location,
                            "languages": ["vue"],
                            "configNamespace": "typescript",
                            "enableForWorkspaceTypeScriptVersions": true,
                        })
                    })
                    .collect();
                json!({
                    "typescript": language,
                    "javascript": language,
                    "vtsls": {
                        "autoUseWorkspaceTsdk": true,
                        "enableMoveToFileCodeAction": true,
                        "experimental": {
                            "completion": { "enableServerSideFuzzyMatch": true, "entriesLimit": 1000 },
                            "maxInlayHintLength": 40,
                        },
                        "tsserver": { "globalPlugins": plugins },
                    },
                })
            }
            "vscode-html-language-server" => json!({
                "html": {
                    "format": { "enable": true, "wrapLineLength": 120, "indentInnerHtml": false, "tabSize": context.tab_size, "insertSpaces": !context.hard_tabs },
                    "suggest": { "html5": true },
                    "validate": { "scripts": true, "styles": true },
                    "autoClosingTags": true,
                    "hover": { "documentation": true, "references": true },
                },
                "css": { "validate": true },
                "javascript": { "validate": { "enable": true } },
            }),
            "vscode-css-language-server" => {
                let language = json!({ "validate": true, "format": { "enable": true }, "hover": { "documentation": true, "references": true } });
                json!({ "css": language, "scss": language, "less": language })
            }
            "vscode-json-language-server" => json!({
                "json": { "validate": { "enable": true }, "format": { "enable": true }, "schemas": json_schemas() },
            }),
            "eslint" => {
                let flat = context.root.as_deref().is_some_and(|root| {
                    ESLINT_CONFIGS.iter().filter(|name| name.starts_with("eslint.config")).any(|name| root.join(name).is_file())
                });
                let root = context.root.clone().unwrap_or_default();
                json!({
                    "validate": "on",
                    "run": "onType",
                    "packageManager": "npm",
                    "useESLintClass": false,
                    "useFlatConfig": flat,
                    "experimental": { "useFlatConfig": flat },
                    "codeActionOnSave": { "mode": "all" },
                    "format": false,
                    "quiet": false,
                    "onIgnoredFiles": "off",
                    "options": {},
                    "rulesCustomizations": [],
                    "problems": { "shortenToSingleLine": false },
                    "nodePath": null,
                    "workingDirectory": { "mode": "auto" },
                    "workspaceFolder": {
                        "uri": super::path_to_uri(&root).as_str(),
                        "name": root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    },
                    "codeAction": {
                        "disableRuleComment": { "enable": true, "location": "separateLine", "commentStyle": "line" },
                        "showDocumentation": { "enable": true },
                    },
                })
            }
            "tailwindcss-language-server" => json!({
                "tailwindCSS": {
                    "validate": true,
                    "hovers": true,
                    "suggestions": true,
                    "codeActions": true,
                    "colorDecorators": true,
                    "emmetCompletions": false,
                    "rootFontSize": 16,
                    "showPixelEquivalents": true,
                    "classAttributes": ["class", "className", "ngClass", "class:list"],
                    "includeLanguages": {},
                    "experimental": {},
                    "lint": {
                        "cssConflict": "warning",
                        "invalidApply": "error",
                        "invalidScreen": "error",
                        "invalidVariant": "error",
                        "invalidConfigPath": "error",
                        "invalidTailwindDirective": "error",
                        "recommendedVariantOrder": "warning",
                    },
                },
                "editor": { "tabSize": context.tab_size },
            }),
            _ => json!({}),
        }
    }

    /// The answer to one `workspace/configuration` item: the part of
    /// `settings` its section names (`html.format`), all of them for none.
    /// ESLint asks with no section, or its own name, for its settings.
    pub fn configuration_item(&self, settings: &Value, section: Option<&str>) -> Value {
        match section.filter(|s| !s.is_empty()) {
            None => settings.clone(),
            Some(_) if self.id == "eslint" => settings.clone(),
            Some(section) => {
                let pointer = format!("/{}", section.replace('.', "/"));
                settings.pointer(&pointer).cloned().unwrap_or(Value::Null)
            }
        }
    }
}

/// What a server's options and settings depend on.
#[derive(Clone, Debug, Default)]
pub struct ServerContext {
    pub root: Option<PathBuf>,
    /// TypeScript's `lib` folder (the project's, else the one zcode
    /// installed next to the server), for servers that load it themselves.
    pub typescript_lib: Option<PathBuf>,
    /// `@vue/language-server`'s folder, whose TypeScript plugin vtsls loads
    /// for a Vue app.
    pub vue_plugin: Option<PathBuf>,
    pub tab_size: u32,
    pub hard_tabs: bool,
}

/// JSON Schema Store's schemas for the configuration files of a web app.
fn json_schemas() -> Value {
    let schema = |files: &[&str], url: &str| json!({ "fileMatch": files, "url": url });
    Value::Array(vec![
        schema(&["package.json"], "https://json.schemastore.org/package.json"),
        schema(&["tsconfig.json", "tsconfig.*.json"], "https://json.schemastore.org/tsconfig.json"),
        schema(&["jsconfig.json", "jsconfig.*.json"], "https://json.schemastore.org/jsconfig.json"),
        schema(&[".eslintrc", ".eslintrc.json"], "https://json.schemastore.org/eslintrc.json"),
        schema(&[".prettierrc", ".prettierrc.json"], "https://json.schemastore.org/prettierrc.json"),
        schema(&[".babelrc", ".babelrc.json", "babel.config.json"], "https://json.schemastore.org/babelrc.json"),
        schema(&[".swcrc"], "https://json.schemastore.org/swcrc.json"),
        schema(&["turbo.json"], "https://turborepo.com/schema.json"),
        schema(&["vercel.json"], "https://openapi.vercel.sh/vercel.json"),
        schema(&["capacitor.config.json"], "https://json.schemastore.org/capacitor.config.json"),
        schema(&["components.json"], "https://ui.shadcn.com/schema.json"),
        schema(&["biome.json", "biome.jsonc"], "https://biomejs.dev/schemas/latest/schema.json"),
    ])
}

/// The outermost folder holding `path` inside `project_root` that is a web
/// app's (has a `package.json`, `tsconfig.json` or `jsconfig.json`). For
/// a file outside the project, the outermost one up to its home folder.
pub fn web_root(path: &Path, project_root: &Path) -> Option<PathBuf> {
    let inside = path.starts_with(project_root);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut found = None;
    for dir in path.ancestors().skip(1) {
        if !inside && home.as_deref() == Some(dir) {
            break;
        }
        if WEB_MARKERS.iter().any(|marker| dir.join(marker).is_file()) {
            found = Some(dir.to_path_buf());
        }
        if dir == project_root {
            break;
        }
    }
    found
}

/// The dependencies of the `package.json` files read so far, by folder:
/// asking which servers a file has reads each at most once.
#[derive(Default)]
pub struct PackageCache {
    dependencies: std::collections::HashMap<PathBuf, HashSet<String>>,
}

impl PackageCache {
    /// Whether a `package.json` between `path` and `web` (both included)
    /// depends on one of `packages`, or one of `files` is in `web`.
    pub fn uses(&mut self, path: &Path, web: &Path, packages: &[&str], files: &[&str]) -> bool {
        if files.iter().any(|name| web.join(name).is_file()) {
            return true;
        }
        for dir in path.ancestors().skip(1) {
            if !dir.starts_with(web) {
                break;
            }
            let dependencies = self.dependencies.entry(dir.to_path_buf()).or_insert_with(|| read_dependencies(dir));
            if packages.iter().any(|name| dependencies.contains(*name)) {
                return true;
            }
        }
        false
    }

    /// Forget what was read (a `package.json` changed).
    pub fn clear(&mut self) {
        self.dependencies.clear();
    }
}

fn read_dependencies(dir: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else { return HashSet::new() };
    let Ok(manifest) = serde_json::from_str::<Value>(&text) else { return HashSet::new() };
    ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"]
        .iter()
        .filter_map(|key| manifest.get(key).and_then(Value::as_object))
        .flat_map(|deps| deps.keys().cloned())
        .collect()
}

/// `base` with `overrides` merged in, objects key by key (the user's
/// `lsp.<id>.settings` over zcode's).
pub fn merge_json(base: &mut Value, overrides: &Value) {
    match (base, overrides) {
        (Value::Object(base), Value::Object(overrides)) => {
            for (key, value) in overrides {
                merge_json(base.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (base, overrides) => *base = overrides.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zcode-servers-{}-{}", std::process::id(), files.len()));
        std::fs::remove_dir_all(&dir).ok();
        for file in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, if file.ends_with("package.json") { r#"{"devDependencies":{"vue":"3","tailwindcss":"4"}}"# } else { "" }).unwrap();
        }
        dir
    }

    #[test]
    fn a_web_app_in_an_xcode_project_gets_servers_of_its_own() {
        let root = tree(&["App.xcodeproj/project.pbxproj", "web/package.json", "web/src/main.ts", "web/src/App.vue", "web/src/data.json", "Assets.xcassets/Contents.json", "docs/index.html"]);
        let web = root.join("web");
        let mut packages = PackageCache::default();
        let roots = |path: &str, language: &str, packages: &mut PackageCache| -> Vec<(&str, PathBuf)> {
            SERVERS.iter().filter_map(|spec| spec.root_for(&root.join(path), language, &root, packages).map(|r| (spec.id, r))).collect()
        };
        assert_eq!(roots("App/ContentView.swift", "swift", &mut packages), [("sourcekit-lsp", root.clone())]);
        assert_eq!(
            roots("web/src/main.ts", "typescript", &mut packages),
            [("vtsls", web.clone()), ("tailwindcss-language-server", web.clone())]
        );
        // Vue files: Vue's server, and vtsls through the plugin.
        let vue: Vec<_> = roots("web/src/App.vue", "vue", &mut packages).into_iter().map(|(id, _)| id).collect();
        assert_eq!(vue, ["vue-language-server", "vtsls", "tailwindcss-language-server"]);
        assert_eq!(roots("web/src/data.json", "json", &mut packages), [("vscode-json-language-server", web.clone())]);
        // An asset catalog's JSON isn't a web app's; a loose page is the project's.
        assert!(roots("Assets.xcassets/Contents.json", "json", &mut packages).is_empty());
        assert_eq!(roots("docs/index.html", "html", &mut packages), [("vscode-html-language-server", root.clone())]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_outermost_package_is_the_root() {
        let root = tree(&["package.json", "packages/ui/package.json", "packages/ui/src/a.ts"]);
        assert_eq!(web_root(&root.join("packages/ui/src/a.ts"), &root), Some(root.clone()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn configuration_sections() {
        let html = spec("vscode-html-language-server").unwrap();
        let settings = html.settings(&ServerContext { tab_size: 2, ..Default::default() });
        assert_eq!(html.configuration_item(&settings, Some("html.format"))["tabSize"], 2);
        assert_eq!(html.configuration_item(&settings, Some("nothing")), Value::Null);
        let eslint = spec("eslint").unwrap();
        let settings = eslint.settings(&ServerContext::default());
        assert_eq!(eslint.configuration_item(&settings, Some("eslint"))["validate"], "on");
        assert_eq!(eslint.configuration_item(&settings, None)["run"], "onType");
    }

    #[test]
    fn user_settings_merge_over_zcodes() {
        let mut base = json!({ "a": { "b": 1, "c": 2 }, "d": [1] });
        merge_json(&mut base, &json!({ "a": { "b": 3 }, "d": [2, 3], "e": true }));
        assert_eq!(base, json!({ "a": { "b": 3, "c": 2 }, "d": [2, 3], "e": true }));
    }
}
