# zcode patch to gpui-component 0.7.3

This is the published `gpui-component` 0.7.3 crate (upstream commit
`67265a36`, `crates/component`) with the changes below, wired in through
`[patch.crates-io]` in zcode's `Cargo.toml`.

**Why:** `gpui_component::init` resolves the theme's default UI and
monospace fonts by listing every installed font family
(`TextSystem::all_font_names`). On macOS that is a synchronous XPC round trip
to `fontd` costing ~35 ms of cold launch, and the list is only used to check
two or three names. The patch checks those names individually on macOS
(`mono_font::is_installed`, which loads just that family through GPUI) and
keeps the full listing on other platforms. On macOS both probes choose the
same fonts as before: `.SystemUIFont` resolves to itself, so no substitution
happens, and Menlo is installed.

**Editor popover padding (`src/input/popovers/`):** hover and diagnostic
popovers get `px_2`/`py_1p5` padding instead of `p_1` (diagnostics had
`px_1`/`py_0p5`). `render_markdown` passes empty `code_block_actions`, which
hides gpui-base's code-block language label: the popover style zeroes the
code block's padding, so the absolutely positioned label landed on the code.
Re-apply these when updating; they are small enough to redo by hand.

**Updating:** when gpui-kit moves to a newer gpui-component, check whether
`src/theme/mono_font.rs` still enumerates fonts. If it does, re-apply the diff
below to the new version; if it doesn't, drop this directory and the
`[patch.crates-io]` entry.

```diff
diff -ru a/src/theme/mono_font.rs b/src/theme/mono_font.rs
--- a/src/theme/mono_font.rs	2006-07-24 03:21:28
+++ b/src/theme/mono_font.rs	2026-10-16 02:31:19
@@ -50,7 +50,9 @@
     RESOLVED
         .get_or_init(|| {
             let default = default_mono_font_family();
-            let family = first_installed(&default, MONO_FONT_ALTERNATES, installed_font_names(cx));
+            let family = first_installed(&default, MONO_FONT_ALTERNATES, |name| {
+                is_installed(cx, name)
+            });
             if family != default {
                 tracing::warn!(
                     "Monospace font {default:?} is not installed, using {family:?} instead."
@@ -61,11 +63,31 @@
         .clone()
 }
 
+/// Whether `family` is installed.
+///
+/// zcode patch: on macOS this asks GPUI to load just that family. Listing
+/// every installed family is a synchronous XPC round trip to `fontd` that
+/// costs ~35 ms of startup, and only the handful of candidate names is ever
+/// looked up.
+#[cfg(target_os = "macos")]
+pub(super) fn is_installed(cx: &App, family: &str) -> bool {
+    let text_system = cx.text_system();
+    text_system
+        .get_font_for_id(text_system.resolve_font(&gpui::font(family.to_string())))
+        .is_some_and(|font| font.family.as_ref() == family)
+}
+
+#[cfg(not(target_os = "macos"))]
+pub(super) fn is_installed(cx: &App, family: &str) -> bool {
+    installed_font_names(cx).iter().any(|name| name == family)
+}
+
 /// The families installed on the machine, listed once per process.
 ///
 /// Enumerating fonts costs around a hundred milliseconds on macOS, and the
 /// answer only depends on the system fonts, which do not change while the
 /// process runs.
+#[cfg_attr(target_os = "macos", allow(dead_code))]
 pub(super) fn installed_font_names(cx: &App) -> &'static [String] {
     static NAMES: OnceLock<Vec<String>> = OnceLock::new();
     NAMES.get_or_init(|| cx.text_system().all_font_names())
@@ -73,10 +95,14 @@
 
 /// `default` when it is installed, else the first installed alternate, else
 /// `.SystemUIFont`.
-fn first_installed(default: &str, alternates: &[&str], installed: &[String]) -> SharedString {
+fn first_installed(
+    default: &str,
+    alternates: &[&str],
+    is_installed: impl Fn(&str) -> bool,
+) -> SharedString {
     std::iter::once(default)
         .chain(alternates.iter().copied())
-        .find(|candidate| installed.iter().any(|name| name == candidate))
+        .find(|candidate| is_installed(candidate))
         .unwrap_or(SYSTEM_UI_FONT)
         .to_string()
         .into()
@@ -94,7 +120,7 @@
     fn keeps_the_default_when_it_is_installed() {
         let installed = names(&["Monaco", "Menlo", ".SystemUIFont"]);
         assert_eq!(
-            first_installed("Menlo", &["Monaco"], &installed),
+            first_installed("Menlo", &["Monaco"], |name| installed.iter().any(|n| n == name)),
             SharedString::from("Menlo")
         );
     }
@@ -103,7 +129,7 @@
     fn falls_through_the_alternates_in_order() {
         let installed = names(&["Courier New", "Monaco", ".SystemUIFont"]);
         assert_eq!(
-            first_installed("Menlo", &["Monaco", "Courier New"], &installed),
+            first_installed("Menlo", &["Monaco", "Courier New"], |name| installed.iter().any(|n| n == name)),
             SharedString::from("Monaco")
         );
     }
@@ -112,7 +138,7 @@
     fn falls_back_to_the_system_font_when_nothing_is_installed() {
         let installed = names(&["Helvetica", ".SystemUIFont"]);
         assert_eq!(
-            first_installed("Menlo", &["Monaco"], &installed),
+            first_installed("Menlo", &["Monaco"], |name| installed.iter().any(|n| n == name)),
             SharedString::from(".SystemUIFont")
         );
     }
diff -ru a/src/theme/system_font.rs b/src/theme/system_font.rs
--- a/src/theme/system_font.rs	2006-07-24 03:21:28
+++ b/src/theme/system_font.rs	2026-10-16 02:33:16
@@ -22,7 +22,7 @@
 
 use gpui::{App, SharedString, font};
 
-use super::mono_font::installed_font_names;
+use super::mono_font::is_installed;
 
 /// The virtual family GPUI resolves on every platform.
 const SYSTEM_UI_FONT: &str = ".SystemUIFont";
@@ -33,15 +33,18 @@
     if cx.global::<super::Theme>().font_family != SYSTEM_UI_FONT {
         return;
     }
-    let installed = installed_font_names(cx);
-    if installed.is_empty() {
+    // zcode patch: GPUI on macOS always has `.SystemUIFont`, and checking
+    // the one resolved family replaces listing every installed font.
+    #[cfg(not(target_os = "macos"))]
+    if super::mono_font::installed_font_names(cx).is_empty() {
         return;
     }
     let text_system = cx.text_system();
     let resolved = text_system
         .get_font_for_id(text_system.resolve_font(&font(SYSTEM_UI_FONT)))
         .map(|font| font.family);
-    let Some(family) = substitute(SYSTEM_UI_FONT, resolved.as_deref(), installed) else {
+    let Some(family) = substitute(SYSTEM_UI_FONT, resolved.as_deref(), |name| is_installed(cx, name))
+    else {
         return;
     };
     tracing::info!("UI font {SYSTEM_UI_FONT:?} resolves to {family:?}, naming it on the theme.");
@@ -53,13 +56,10 @@
 fn substitute(
     requested: &str,
     resolved: Option<&str>,
-    installed: &[String],
+    is_installed: impl Fn(&str) -> bool,
 ) -> Option<SharedString> {
     let resolved = resolved.filter(|resolved| *resolved != requested)?;
-    installed
-        .iter()
-        .any(|name| name == resolved)
-        .then(|| resolved.to_string().into())
+    is_installed(resolved).then(|| resolved.to_string().into())
 }
 
 #[cfg(test)]
@@ -74,7 +74,7 @@
     fn keeps_the_system_font_when_it_resolves_to_itself() {
         let installed = names(&["Noto Sans", ".SystemUIFont"]);
         assert_eq!(
-            substitute(".SystemUIFont", Some(".SystemUIFont"), &installed),
+            substitute(".SystemUIFont", Some(".SystemUIFont"), |name| installed.iter().any(|n| n == name)),
             None
         );
     }
@@ -83,18 +83,18 @@
     fn names_the_installed_family_the_system_font_fell_back_to() {
         let installed = names(&["Noto Sans", "DejaVu Sans"]);
         assert_eq!(
-            substitute(".SystemUIFont", Some("Noto Sans"), &installed),
+            substitute(".SystemUIFont", Some("Noto Sans"), |name| installed.iter().any(|n| n == name)),
             Some(SharedString::from("Noto Sans"))
         );
     }
 
     #[test]
     fn keeps_the_system_font_when_the_resolved_family_is_not_installed() {
-        let installed = Vec::new();
+        let installed: Vec<String> = Vec::new();
         assert_eq!(
-            substitute(".SystemUIFont", Some("Noto Sans"), &installed),
+            substitute(".SystemUIFont", Some("Noto Sans"), |name| installed.iter().any(|n| n == name)),
             None
         );
-        assert_eq!(substitute(".SystemUIFont", None, &installed), None);
+        assert_eq!(substitute(".SystemUIFont", None, |name| installed.iter().any(|n| n == name)), None);
     }
 }
```
