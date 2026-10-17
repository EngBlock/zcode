//! Emmet's HTML snippets (aliases such as `bq`, `a:link`, `!`) and their
//! resolution into the tree.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::parser::{self, Mode};
use super::tree::{Node, Unroller, merge_attr};
use crate::error::Error;

/// Emmet's `snippets/html.json`: the key (aliases separated by `|`) and the
/// abbreviation it stands for. `${N}` are fields, `/` makes an element
/// self-closing, `${lang}` and `${charset}` are variables.
const HTML: &[(&str, &str)] = &[
    ("a", "a[href]"),
    (
        "a:blank",
        "a[href='http://${0}' target='_blank' rel='noopener noreferrer']",
    ),
    ("a:link", "a[href='http://${0}']"),
    ("a:mail", "a[href='mailto:${0}']"),
    ("a:tel", "a[href='tel:+${0}']"),
    ("abbr", "abbr[title]"),
    ("acr|acronym", "acronym[title]"),
    ("base", "base[href]/"),
    ("basefont", "basefont/"),
    ("br", "br/"),
    ("frame", "frame/"),
    ("hr", "hr/"),
    ("bdo", "bdo[dir]"),
    ("bdo:r", "bdo[dir=rtl]"),
    ("bdo:l", "bdo[dir=ltr]"),
    ("col", "col/"),
    ("link", "link[rel=stylesheet href]/"),
    ("link:css", "link[href='${1:style}.css']"),
    ("link:print", "link[href='${1:print}.css' media=print]"),
    (
        "link:favicon",
        "link[rel='shortcut icon' type=image/x-icon href='${1:favicon.ico}']",
    ),
    (
        "link:mf|link:manifest",
        "link[rel='manifest' href='${1:manifest.json}']",
    ),
    (
        "link:touch",
        "link[rel=apple-touch-icon href='${1:favicon.png}']",
    ),
    (
        "link:rss",
        "link[rel=alternate type=application/rss+xml title=RSS href='${1:rss.xml}']",
    ),
    (
        "link:atom",
        "link[rel=alternate type=application/atom+xml title=Atom href='${1:atom.xml}']",
    ),
    (
        "link:im|link:import",
        "link[rel=import href='${1:component}.html']",
    ),
    ("meta", "meta/"),
    (
        "meta:utf",
        "meta[http-equiv=Content-Type content='text/html;charset=UTF-8']",
    ),
    (
        "meta:vp",
        "meta[name=viewport content='width=${1:device-width}, initial-scale=${2:1.0}']",
    ),
    (
        "meta:compat",
        "meta[http-equiv=X-UA-Compatible content='${1:IE=7}']",
    ),
    ("meta:edge", "meta:compat[content='${1:ie=edge}']"),
    (
        "meta:redirect",
        "meta[http-equiv=refresh content='0; url=${1:http://example.com}']",
    ),
    ("meta:refresh", "meta[http-equiv=refresh content='${1:5}']"),
    ("meta:kw", "meta[name=keywords content]"),
    ("meta:desc", "meta[name=description content]"),
    ("style", "style"),
    ("script", "script"),
    ("script:src", "script[src]"),
    ("script:module", "script[type=module src]"),
    ("img", "img[src alt]/"),
    ("img:s|img:srcset", "img[srcset src alt]"),
    ("img:z|img:sizes", "img[sizes srcset src alt]"),
    ("picture", "picture"),
    ("src|source", "source/"),
    ("src:sc|source:src", "source[src type]"),
    ("src:s|source:srcset", "source[srcset]"),
    ("src:t|source:type", "source[srcset type='${1:image/}']"),
    ("src:z|source:sizes", "source[sizes srcset]"),
    (
        "src:m|source:media",
        "source[media='(${1:min-width: })' srcset]",
    ),
    (
        "src:mt|source:media:type",
        "source:media[type='${2:image/}']",
    ),
    ("src:mz|source:media:sizes", "source:media[sizes srcset]"),
    (
        "src:zt|source:sizes:type",
        "source[sizes srcset type='${1:image/}']",
    ),
    ("iframe", "iframe[src frameborder=0]"),
    ("embed", "embed[src type]/"),
    ("object", "object[data type]"),
    ("param", "param[name value]/"),
    ("map", "map[name]"),
    ("area", "area[shape coords href alt]/"),
    ("area:d", "area[shape=default]"),
    ("area:c", "area[shape=circle]"),
    ("area:r", "area[shape=rect]"),
    ("area:p", "area[shape=poly]"),
    ("form", "form[action]"),
    ("form:get", "form[method=get]"),
    ("form:post", "form[method=post]"),
    ("label", "label[for]"),
    ("input", "input[type=${1:text}]/"),
    ("inp", "input[name=${1} id=${1}]"),
    ("input:h|input:hidden", "input[type=hidden name]"),
    ("input:t|input:text", "inp[type=text]"),
    ("input:search", "inp[type=search]"),
    ("input:email", "inp[type=email]"),
    ("input:url", "inp[type=url]"),
    ("input:p|input:password", "inp[type=password]"),
    ("input:datetime", "inp[type=datetime]"),
    ("input:date", "inp[type=date]"),
    ("input:datetime-local", "inp[type=datetime-local]"),
    ("input:month", "inp[type=month]"),
    ("input:week", "inp[type=week]"),
    ("input:time", "inp[type=time]"),
    ("input:tel", "inp[type=tel]"),
    ("input:color", "inp[type=color]"),
    ("input:number", "inp[type=number]"),
    ("input:range", "inp[type=range]"),
    ("input:c|input:checkbox", "inp[type=checkbox]"),
    ("input:r|input:radio", "inp[type=radio]"),
    ("input:f|input:file", "inp[type=file]"),
    ("input:s|input:submit", "input[type=submit value]"),
    ("input:i|input:image", "input[type=image src alt]"),
    ("input:b|input:btn|input:button", "input[type=button value]"),
    ("input:reset", "input:button[type=reset]"),
    ("select", "select[name=${1} id=${1}]"),
    ("select:d|select:disabled", "select[disabled.]"),
    ("opt|option", "option[value]"),
    (
        "textarea",
        "textarea[name=${1} id=${1} cols=${2:30} rows=${3:10}]",
    ),
    ("marquee", "marquee[behavior direction]"),
    ("menu:c|menu:context", "menu[type=context]"),
    ("menu:t|menu:toolbar", "menu[type=toolbar]"),
    ("video", "video[src]"),
    ("audio", "audio[src]"),
    ("html:xml", "html[xmlns=http://www.w3.org/1999/xhtml]"),
    ("keygen", "keygen/"),
    ("command", "command/"),
    ("btn:s|button:s|button:submit", "button[type=submit]"),
    ("btn:r|button:r|button:reset", "button[type=reset]"),
    ("btn:b|button:b|button:button", "button[type=button]"),
    ("btn:d|button:d|button:disabled", "button[disabled.]"),
    (
        "fst:d|fset:d|fieldset:d|fieldset:disabled",
        "fieldset[disabled.]",
    ),
    ("bq", "blockquote"),
    ("fig", "figure"),
    ("figc", "figcaption"),
    ("pic", "picture"),
    ("ifr", "iframe"),
    ("emb", "embed"),
    ("obj", "object"),
    ("cap", "caption"),
    ("colg", "colgroup"),
    ("fst|fset", "fieldset"),
    ("btn", "button"),
    ("optg", "optgroup"),
    ("tarea", "textarea"),
    ("leg", "legend"),
    ("sect", "section"),
    ("art", "article"),
    ("hdr", "header"),
    ("ftr", "footer"),
    ("adr", "address"),
    ("dlg", "dialog"),
    ("str", "strong"),
    ("prog", "progress"),
    ("mn", "main"),
    ("tem", "template"),
    ("datag", "datagrid"),
    ("datal", "datalist"),
    ("kg", "keygen"),
    ("out", "output"),
    ("det", "details"),
    ("sum", "summary"),
    ("cmd", "command"),
    ("data", "data[value]"),
    ("meter", "meter[value]"),
    ("time", "time[datetime]"),
    ("ri:d|ri:dpr", "img:s"),
    ("ri:v|ri:viewport", "img:z"),
    ("ri:a|ri:art", "pic>src:m+img"),
    ("ri:t|ri:type", "pic>src:t+img"),
    ("!!!", "{<!DOCTYPE html>}"),
    (
        "doc",
        "html[lang=${lang}]>(head>meta[charset=${charset}]+meta:vp+title{${1:Document}})+body",
    ),
    ("!|html:5", "!!!+doc"),
    ("c", "{<!-- ${0} -->}"),
];

/// JSX's own versions of HTML snippets.
const JSX: &[(&str, &str)] = &[("c", "{{/* ${0} */}}")];

fn table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = HashMap::new();
        for (keys, abbreviation) in HTML {
            for key in keys.split('|') {
                table.insert(key, *abbreviation);
            }
        }
        table
    })
}

/// The abbreviation a snippet name stands for (case-sensitive, so a JSX
/// `Link` component isn't `link`).
pub(crate) fn lookup(name: &str, jsx: bool) -> Option<&'static str> {
    if jsx && let Some((_, abbreviation)) = JSX.iter().find(|(key, _)| *key == name) {
        return Some(abbreviation);
    }
    table().get(name).copied()
}

/// Replaces nodes named after snippets with the snippets' trees.
pub(crate) struct Resolver<'a> {
    jsx: bool,
    groups: u32,
    /// The snippets being expanded: their own names inside them are plain
    /// elements (`a` is `a[href]`).
    stack: Vec<String>,
    budget: &'a mut usize,
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(jsx: bool, budget: &'a mut usize) -> Self {
        Self {
            jsx,
            groups: 0,
            stack: Vec::new(),
            budget,
        }
    }

    pub(crate) fn resolve(&mut self, nodes: Vec<Node>) -> Result<Vec<Node>, Error> {
        let mut out = Vec::with_capacity(nodes.len());
        for mut node in nodes {
            node.children = self.resolve(std::mem::take(&mut node.children))?;
            self.expand(node, &mut out)?;
        }
        Ok(out)
    }

    fn expand(&mut self, node: Node, out: &mut Vec<Node>) -> Result<(), Error> {
        let definition = match node.name.as_deref() {
            Some(name) if !self.stack.iter().any(|open| open == name) => lookup(name, self.jsx),
            _ => None,
        };
        let Some(definition) = definition else {
            out.push(node);
            return Ok(());
        };
        let items = parser::parse(
            definition,
            Mode {
                jsx: self.jsx,
                snippet: true,
            },
        )?;
        self.groups += 1;
        let nodes = Unroller::new(self.groups, None, self.budget).unroll(&items)?;
        self.stack.push(node.name.clone().unwrap_or_default());
        let nodes = self.resolve(nodes);
        self.stack.pop();
        merge(node, nodes?, out);
        Ok(())
    }
}

/// Puts what the abbreviation said about a snippet's node into the
/// snippet's tree: into its element when it has one (attributes merged,
/// children appended), else its children into the innermost last element.
fn merge(node: Node, mut nodes: Vec<Node>, out: &mut Vec<Node>) {
    if nodes.len() == 1
        && nodes[0].name.is_some()
        && let Some(mut base) = nodes.pop()
    {
        for attr in node.attrs {
            merge_attr(&mut base.attrs, attr);
        }
        base.self_close |= node.self_close;
        if node.value.is_some() {
            base.value = node.value;
        }
        base.children.extend(node.children);
        base.repeat = node.repeat;
        base.repeated = node.repeated;
        base.line = node.line;
        out.push(base);
        return;
    }
    let mut children = node.children;
    if let Some(value) = node.value {
        children.insert(0, Node::text(value));
    }
    match nodes.iter_mut().rev().find(|n| n.name.is_some()) {
        Some(last) if !children.is_empty() => innermost_last(last).children.extend(children),
        _ => nodes.extend(children),
    }
    if let Some(last) = nodes.last_mut() {
        last.line = node.line;
    }
    out.extend(nodes);
}

/// The innermost last element of `node`: where children and wrapped
/// content go.
pub(crate) fn innermost_last(mut node: &mut Node) -> &mut Node {
    while node
        .children
        .last()
        .is_some_and(|child| child.name.is_some() && !child.self_close)
    {
        let last = node.children.len() - 1;
        node = &mut node.children[last];
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Options;

    #[test]
    fn every_snippet_expands() {
        for (keys, _) in HTML {
            for key in keys.split('|') {
                for jsx in [false, true] {
                    let result = super::super::expand(key, jsx, &Options::default());
                    assert!(result.is_ok(), "{key}: {result:?}");
                }
            }
        }
    }
}
