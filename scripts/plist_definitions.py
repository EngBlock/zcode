#!/usr/bin/env python3
"""Generates src/plist/definitions.rs from Xcode's property list structure
definitions: the human-readable key names ("Bundle display name" for
CFBundleDisplayName), value types and allowed values its plist editor shows.

    python3 scripts/plist_definitions.py [/path/to/Xcode.app]

Xcode keeps them as `com.apple.xcode.plist.structure-definition` extensions
in DVTCorePlistStructDefs.xcplugindata (a binary plist whose extensions hold
the definitions as XML text). The Xcode `xcode-select -p` points at is used
when no path is given.
"""

import os
import plistlib
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "src", "plist", "definitions.rs")
PLUGIN = "Contents/PlugIns/DVTCorePlistStructDefs.dvtplugin/Contents/Resources/DVTCorePlistStructDefs.xcplugindata"
# (extension id suffix, Rust name)
SCHEMAS = [
    ("info-plist", "INFO_PLIST"),
    ("entitlements", "ENTITLEMENTS"),
    ("app-privacy.privacy", "PRIVACY"),
]
PRIMITIVES = {"String", "Boolean", "Number", "Dictionary", "Array", "Date", "Data"}


def xcode_app():
    if len(sys.argv) > 1:
        return sys.argv[1]
    developer = subprocess.run(["xcode-select", "-p"], capture_output=True, text=True, check=True).stdout.strip()
    return os.path.dirname(os.path.dirname(developer))


def extensions(path):
    found = {}

    def walk(node):
        if isinstance(node, dict):
            xml = node.get("_DVTExtensionXML")
            if isinstance(xml, str) and "plist.structure-definition" in xml:
                element = ET.fromstring(xml)
                found[element.get("id").removeprefix("com.apple.xcode.plist.structure-definition.")] = element
            for value in node.values():
                walk(value)
        elif isinstance(node, list):
            for value in node:
                walk(value)

    with open(path, "rb") as f:
        walk(plistlib.load(f))
    return found


def rust_str(s):
    return '"' + (s or "").replace("\\", "\\\\").replace('"', '\\"') + '"'


def schema_source(rust_name, element):
    patterns = [f.get("pattern") for f in element.findall("filename")]
    definitions = {}
    for d in element.findall("definition"):
        # A later definition of the same name wins, as in a dictionary.
        definitions[d.get("name")] = d
    names = set(definitions) | PRIMITIVES | {"StringArray"}
    root = definitions["_root_"]
    lines = [f"pub static {rust_name}: Schema = Schema {{"]
    lines.append(f"    name: {rust_str(element.get('name'))},")
    lines.append(f"    patterns: &[{', '.join(rust_str(p) for p in patterns)}],")
    lines.append(f"    root_label: {rust_str(root.get('localizedString'))},")
    lines.append("    definitions: &[")
    for name in sorted(definitions):
        d = definitions[name]
        keys = {}
        for k in d.findall("dictionaryKeys/key"):
            keys[k.get("name")] = k
        for k in keys.values():
            if k.get("className") not in names:
                print(f"warning: {rust_name}: {k.get('name')} has unknown class {k.get('className')}", file=sys.stderr)
        lines.append("        Definition {")
        lines.append(f"            name: {rust_str(name)},")
        lines.append(f"            class: {rust_str(d.get('className'))},")
        lines.append(f"            element: {rust_str(d.get('arrayElementClassName'))},")
        lines.append(f"            label: {rust_str(d.get('localizedString'))},")
        lines.append(f"            default: {rust_str(d.get('default'))},")
        if keys:
            lines.append("            keys: &[")
            for k in keys.values():
                lines.append(
                    f"                Key {{ name: {rust_str(k.get('name'))}, class: {rust_str(k.get('className'))}, label: {rust_str(k.get('localizedString'))} }},"
                )
            lines.append("            ],")
        else:
            lines.append("            keys: &[],")
        choices = d.findall("allowableValues/value")
        if choices:
            lines.append("            choices: &[")
            for c in choices:
                lines.append(f"                Choice {{ value: {rust_str(c.get('name'))}, label: {rust_str(c.get('localizedString'))} }},")
            lines.append("            ],")
        else:
            lines.append("            choices: &[],")
        lines.append("        },")
    lines.append("    ],")
    lines.append("};")
    return "\n".join(lines)


def main():
    app = xcode_app()
    found = extensions(os.path.join(app, PLUGIN))
    version = ""
    try:
        with open(os.path.join(app, "Contents", "version.plist"), "rb") as f:
            info = plistlib.load(f)
        version = f" {info.get('CFBundleShortVersionString', '')} ({info.get('ProductBuildVersion', '')})"
    except OSError:
        pass
    parts = [
        "//! Xcode's property list structure definitions: key names, types and",
        f"//! allowed values, from Xcode{version}. Generated by",
        "//! `scripts/plist_definitions.py`; don't edit by hand.",
        "",
        "#![allow(clippy::all)]",
        "",
        "use super::schema::{Choice, Definition, Key, Schema};",
        "",
    ]
    for suffix, rust_name in SCHEMAS:
        parts.append(schema_source(rust_name, found[suffix]))
        parts.append("")
    parts.append(f"pub static ALL: &[&Schema] = &[{', '.join('&' + name for _, name in SCHEMAS)}];")
    parts.append("")
    with open(OUT, "w") as f:
        f.write("\n".join(parts))
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
