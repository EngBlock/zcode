//! `keymap.json`: key bindings over zcode's, in Zed's format, a list of
//! blocks each with an optional `context` and its `bindings`:
//!
//! ```json
//! [
//!   { "bindings": { "cmd-shift-b": "zcode::Build" } },
//!   { "context": "Editor && vim_mode == normal", "bindings": { "space f": "zcode::OpenQuickly" } },
//!   { "context": "Terminal", "bindings": { "cmd-k": null, "ctrl-x": ["zed::Unbind", "zcode::Cut"] } }
//! ]
//! ```
//!
//! An action is its name (`zcode::Build`, or just `Build`), or `[name,
//! arguments]`; `null` frees a key in that context and `["zed::Unbind",
//! action]` takes it off one action. Contexts are gpui's expressions over
//! the views' contexts: `Editor` (code editors, also `Input && mode ==
//! editor`), `vim_mode == normal|insert|visual|replace`, `multibuffer`,
//! `Terminal`, `Navigator` (with `navigator == project|find|…`),
//! `PlistEditor`, `Picker`, `Workspace` and the sheets'. Later blocks win
//! over earlier ones, and every block over zcode's own bindings. A binding
//! that can't be made, or a block that can't be read, is reported and the
//! rest still apply.

use std::rc::Rc;

use gpui_kit::{App, BorrowAppContext as _, DummyKeyboardMapper, Global, KeyBinding, KeyBindingContextPredicate, NoAction};
use serde::Deserialize;
use serde_json::Value;

use super::{Content, SettingsStore, jsonc};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeymapContent {
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Block {
    /// Where it is in the file, from 1 (the blocks that can't be read are
    /// left out).
    #[serde(skip)]
    pub number: usize,
    pub context: Option<String>,
    pub bindings: serde_json::Map<String, Value>,
    /// Marks a block an import from Xcode wrote (its key bindings file), so
    /// importing again replaces it.
    pub xcode_import: Option<String>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

impl Content for KeymapContent {
    fn read(text: &str, _project: bool) -> Result<(Self, Vec<String>), jsonc::ParseError> {
        // A block that isn't one is said and left out; the others apply.
        let values: Vec<Value> = jsonc::parse(text)?;
        let mut blocks = vec![];
        let mut warnings = vec![];
        for (ix, value) in values.into_iter().enumerate() {
            let mut block: Block = match serde_json::from_value(value) {
                Ok(block) => block,
                Err(err) => {
                    warnings.push(format!("block {}: {err}; its bindings aren’t used", ix + 1));
                    continue;
                }
            };
            block.number = ix + 1;
            // Zed's own flag; zcode binds keys as written.
            for key in block.other.keys().filter(|k| *k != "use_key_equivalents") {
                warnings.push(format!("block {}: unknown key “{key}”", ix + 1));
            }
            blocks.push(block);
        }
        Ok((KeymapContent { blocks }, warnings))
    }
}

/// zcode's own bindings (and the editor's and gpui-component's), as they
/// were before the user's were added.
struct DefaultBindings {
    count: usize,
    bindings: Option<Vec<KeyBinding>>,
}

impl Global for DefaultBindings {}

/// The user's bindings over the defaults just bound (startup).
pub fn bind_user(cx: &mut App) {
    let count = cx.key_bindings().borrow().bindings().len();
    cx.set_global(DefaultBindings { count, bindings: None });
    let (bindings, problems) = build(cx);
    if !bindings.is_empty() {
        cx.bind_keys(bindings);
    }
    set_problems(problems, cx);
}

/// After keymap.json changed: the defaults, then its bindings again.
pub fn rebind(cx: &mut App) {
    let defaults = defaults(cx);
    let (bindings, problems) = build(cx);
    cx.clear_key_bindings();
    cx.bind_keys(defaults);
    cx.bind_keys(bindings);
    set_problems(problems, cx);
}

/// zcode's own bindings, kept the first time they're asked for.
pub fn defaults(cx: &mut App) -> Vec<KeyBinding> {
    if !cx.has_global::<DefaultBindings>() {
        let count = cx.key_bindings().borrow().bindings().len();
        cx.set_global(DefaultBindings { count, bindings: None });
    }
    if cx.global::<DefaultBindings>().bindings.is_none() {
        let count = cx.global::<DefaultBindings>().count;
        let bindings: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().take(count).cloned().collect();
        cx.global_mut::<DefaultBindings>().bindings = Some(bindings);
    }
    cx.global::<DefaultBindings>().bindings.clone().unwrap_or_default()
}

fn set_problems(problems: Vec<String>, cx: &mut App) {
    if cx.has_global::<SettingsStore>() {
        cx.update_global::<SettingsStore, _>(|store, _| {
            if store.keymap_problems != problems {
                store.keymap_problems = problems;
                store.generation += 1;
            }
        });
    }
}

/// The bindings keymap.json makes, and what it couldn't.
fn build(cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    let Some(store) = cx.try_global::<SettingsStore>() else { return (vec![], vec![]) };
    bindings_for(&store.keymap.content, cx)
}

pub fn bindings_for(content: &KeymapContent, cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    let mut bindings = vec![];
    let mut problems = vec![];
    for block in &content.blocks {
        let predicate = match block.context.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            None => None,
            Some(context) => match KeyBindingContextPredicate::parse(context) {
                Ok(predicate) => Some(Rc::new(predicate)),
                Err(err) => {
                    problems.push(format!("block {}: context “{context}”: {err}", block.number));
                    continue;
                }
            },
        };
        for (keys, value) in &block.bindings {
            let action = match action_for(value, cx) {
                Ok(action) => action,
                Err(err) => {
                    problems.push(format!("“{keys}”: {err}"));
                    continue;
                }
            };
            match KeyBinding::load(keys, action, predicate.clone(), false, None, &DummyKeyboardMapper) {
                Ok(binding) => bindings.push(binding),
                Err(err) => problems.push(format!("“{keys}”: {err}")),
            }
        }
    }
    (bindings, problems)
}

/// What a binding's value names: `None` for `null`.
fn action_spec(value: &Value) -> Result<Option<(&str, Option<Value>)>, String> {
    const EXPECTED: &str = "expected an action's name, [name, arguments] or null";
    let (name, input) = match value {
        Value::Null => return Ok(None),
        Value::String(name) => (name.as_str(), None),
        Value::Array(parts) => match parts.as_slice() {
            [Value::String(name)] => (name.as_str(), None),
            [Value::String(name), input] => (name.as_str(), Some(input.clone())),
            _ => return Err(EXPECTED.into()),
        },
        _ => return Err(EXPECTED.into()),
    };
    let name = match name {
        "Unbind" | "zcode::Unbind" => "zed::Unbind",
        name => name,
    };
    Ok(Some((name, input)))
}

fn action_for(value: &Value, cx: &App) -> Result<Box<dyn gpui_kit::Action>, String> {
    let Some((name, input)) = action_spec(value)? else { return Ok(Box::new(NoAction)) };
    let build = |name: &str| cx.build_action(name, input.clone());
    match build(name) {
        Ok(action) => Ok(action),
        // `Build` for `zcode::Build`.
        Err(_) if !name.contains("::") && build(&format!("zcode::{name}")).is_ok() => build(&format!("zcode::{name}")).map_err(|e| e.to_string()),
        Err(err) => Err(match err {
            gpui_kit::ActionBuildError::NotFound { .. } => format!("no action named “{name}”"),
            err => err.to_string(),
        }),
    }
}

/// zcode's default bindings as a keymap file (zcode ▸ Open Default Key
/// Bindings): a block per context, in the order they're bound.
pub fn default_keymap_text(cx: &mut App) -> String {
    let defaults = defaults(cx);
    let mut blocks: Vec<(Option<String>, Vec<(String, String)>)> = vec![];
    for binding in &defaults {
        let context = binding.predicate().map(|p| p.to_string());
        let keys = binding.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" ");
        let action = if gpui_kit::is_no_action(binding.action()) { "null".to_string() } else { format!("\"{}\"", binding.action().name()) };
        match blocks.iter_mut().find(|(c, _)| *c == context) {
            Some((_, entries)) => entries.push((keys, action)),
            None => blocks.push((context, vec![(keys, action)])),
        }
    }
    let mut text = String::from(
        "// zcode's key bindings. Don't edit this file: put the bindings you want to change\n\
         // in your own (zcode ▸ Key Bindings…, ~/.config/zcode/keymap.json), in the same\n\
         // format; `null` frees a key, [\"zed::Unbind\", action] takes it off one action.\n[\n",
    );
    for (ix, (context, entries)) in blocks.iter().enumerate() {
        text.push_str("  {\n");
        if let Some(context) = context {
            text.push_str(&format!("    \"context\": {},\n", serde_json::to_string(context).unwrap_or_default()));
        }
        text.push_str("    \"bindings\": {\n");
        for (i, (keys, action)) in entries.iter().enumerate() {
            let comma = if i + 1 < entries.len() { "," } else { "" };
            text.push_str(&format!("      {}: {action}{comma}\n", serde_json::to_string(keys).unwrap_or_default()));
        }
        text.push_str(if ix + 1 < blocks.len() { "    }\n  },\n" } else { "    }\n  }\n" });
    }
    text.push_str("]\n");
    text
}

pub const KEYMAP_TEMPLATE: &str = "// Your key bindings, over zcode's (zcode ▸ Open Default Key Bindings lists them):\n// JSON with comments, in Zed's format, applied as soon as the file is saved.\n// [\n//   { \"bindings\": { \"cmd-shift-b\": \"zcode::Build\" } },\n//   { \"context\": \"Editor\", \"bindings\": { \"ctrl-k\": \"zcode::DuplicateLine\" } }\n// ]\n[\n]\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_files_are_blocks_with_contexts() {
        let text = r#"[
            // Mine.
            { "bindings": { "cmd-shift-b": "zcode::Build", "cmd-k": null } },
            { "context": "Editor && vim_mode == normal", "bindings": { "space f": ["zcode::OpenQuickly"] }, "use_key_equivalents": true },
            { "context": "Terminal", "bindings": {}, "colour": "red" },
        ]"#;
        let (content, warnings) = KeymapContent::read(text, false).unwrap();
        assert_eq!(content.blocks.len(), 3);
        assert_eq!(content.blocks[1].context.as_deref(), Some("Editor && vim_mode == normal"));
        assert_eq!(warnings, ["block 3: unknown key “colour”"]);
        assert!(KeymapContent::read("{ \"bindings\": {} }", false).is_err());
        // A malformed block is said; the others are kept, numbered as in the file.
        let text = r#"[{"bindings":{"cmd-shift-8":"OpenSettings"}},{"context":"Editor","bindings":["oops"]},"bare",{"context":5,"bindings":{}},{"context":"Terminal","bindings":{"cmd-k":null}}]"#;
        let (content, warnings) = KeymapContent::read(text, false).unwrap();
        assert_eq!(content.blocks.iter().map(|b| b.number).collect::<Vec<_>>(), [1, 5]);
        assert_eq!(content.blocks[0].bindings.len(), 1);
        assert_eq!(content.blocks[1].context.as_deref(), Some("Terminal"));
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[0].starts_with("block 2: invalid type: sequence, expected a map"), "{warnings:?}");
        assert!(warnings[1].starts_with("block 3: ") && warnings[2].starts_with("block 4: "), "{warnings:?}");
        assert!(warnings.iter().all(|w| w.ends_with("its bindings aren’t used")), "{warnings:?}");
        assert_eq!(KeymapContent::read("", false).unwrap().0, KeymapContent::default());
    }

    #[test]
    fn binding_values() {
        use serde_json::json;
        assert_eq!(action_spec(&json!("zcode::Build")).unwrap(), Some(("zcode::Build", None)));
        assert_eq!(action_spec(&json!(["Unbind", "zcode::Cut"])).unwrap(), Some(("zed::Unbind", Some(json!("zcode::Cut")))));
        assert_eq!(action_spec(&json!(null)).unwrap(), None);
        assert!(action_spec(&json!(3)).is_err());
        assert!(action_spec(&json!(["a", "b", "c"])).is_err());
    }
}
