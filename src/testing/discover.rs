//! Finds XCTest and Swift Testing tests in source files, as Xcode's test
//! diamonds do: from the source, before anything is built.
//!
//! Each file is parsed on its own into [`Declarations`] (types, their
//! superclasses and functions with their attributes). [`resolve`] then looks
//! at all files together, because whether `class FooTests: BaseCase` is an
//! XCTest case depends on where `BaseCase` comes from, and an extension can
//! add tests to a class declared elsewhere.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use tree_sitter::{Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Framework {
    XCTest,
    SwiftTesting,
}

/// A function as declared in source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    /// Argument labels (`_` for unlabeled), for Swift Testing's `name(a:_:)`.
    pub labels: Vec<String>,
    /// `@Test`, with its display name if it has one.
    pub test: Option<Option<String>>,
    pub is_static: bool,
    pub is_private: bool,
    /// 0-based line of the `func` keyword, and the declaration's last line.
    pub line: u32,
    pub start_line: u32,
    pub end_line: u32,
}

/// A class, struct, actor, enum or extension as declared in source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypeDecl {
    /// The type's name with its enclosing types (`["Outer", "Inner"]`); for an
    /// extension, the extended type's.
    pub path: Vec<String>,
    pub is_extension: bool,
    pub is_class: bool,
    pub superclass: Option<String>,
    /// `@Suite`, with its display name if it has one.
    pub suite: Option<Option<String>>,
    pub line: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub methods: Vec<FunctionDecl>,
}

/// What one file declares that matters for finding tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declarations {
    pub types: Vec<TypeDecl>,
    /// Top-level functions (Swift Testing allows global `@Test` functions).
    pub functions: Vec<FunctionDecl>,
}

impl Declarations {
    pub fn is_empty(&self) -> bool {
        self.types.is_empty() && self.functions.is_empty()
    }
}

/// A test function, XCTest case class or Swift Testing suite.
#[derive(Clone, Debug, PartialEq)]
pub struct TestItem {
    pub framework: Framework,
    pub file: PathBuf,
    /// Enclosing class or suite types, outermost first.
    pub suite: Vec<String>,
    /// `@Suite`/`@Test` display names of `suite`'s types, where they have one.
    pub suite_display: Vec<Option<String>>,
    /// The test function (`testFoo` for XCTest, `foo(x:)` for Swift
    /// Testing), or `None` when this item is the class or suite itself.
    pub name: Option<String>,
    /// `@Test("…")` / `@Suite("…")`.
    pub display_name: Option<String>,
    /// 0-based line the diamond goes on, and the range of the declaration.
    pub line: u32,
    pub start_line: u32,
    pub end_line: u32,
}

impl TestItem {
    /// Identifier components below the target: suites, then the test.
    pub fn components(&self) -> Vec<String> {
        self.suite.iter().cloned().chain(self.name.clone()).collect()
    }

    /// What the Test navigator shows.
    pub fn title(&self) -> String {
        match (&self.name, &self.display_name) {
            (_, Some(display)) => display.clone(),
            (Some(name), None) => name.clone(),
            (None, None) => self.suite.last().cloned().unwrap_or_default(),
        }
    }

    pub fn is_function(&self) -> bool {
        self.name.is_some()
    }

    /// Whether `other` is this class/suite or one of its tests.
    pub fn contains(&self, other: &TestItem) -> bool {
        match self.name {
            Some(_) => self == other,
            None => other.framework == self.framework && other.suite.starts_with(&self.suite),
        }
    }
}

/// Whether a file could declare tests, without parsing it.
pub fn may_declare_tests(text: &str) -> bool {
    text.contains("XCTest") || text.contains("Testing") || text.contains("@Test") || text.contains("@Suite")
}

/// Parses one source file (Swift or Objective-C) for test declarations.
pub fn declarations(path: &Path, text: &str) -> Declarations {
    match path.extension().and_then(|e| e.to_str()) {
        Some("swift") => swift_declarations(text),
        Some("m" | "mm") => objc_declarations(text),
        _ => Declarations::default(),
    }
}

// ---------------------------------------------------------------------------
// Swift

thread_local! {
    static SWIFT_PARSER: std::cell::RefCell<Option<Parser>> = const { std::cell::RefCell::new(None) };
}

pub fn swift_declarations(text: &str) -> Declarations {
    if !may_declare_tests(text) {
        return Declarations::default();
    }
    let tree = SWIFT_PARSER.with(|cell| {
        let mut cell = cell.borrow_mut();
        let parser = cell.get_or_insert_with(|| {
            let mut parser = Parser::new();
            parser.set_language(&tree_sitter_swift::LANGUAGE.into()).expect("swift grammar");
            parser
        });
        parser.parse(text, None)
    });
    let Some(tree) = tree else { return Declarations::default() };
    let mut out = Declarations::default();
    collect(tree.root_node(), text, &[], &mut out);
    out
}

fn node_text<'a>(node: Node, text: &'a str) -> &'a str {
    &text[node.byte_range()]
}

fn children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// Walks declarations at one nesting level (the file, or a type's body).
fn collect(node: Node, text: &str, outer: &[String], out: &mut Declarations) {
    for child in children(node) {
        match child.kind() {
            "class_declaration" => {
                if let Some(decl) = type_decl(child, text, outer) {
                    let path = decl.path.clone();
                    out.types.push(decl);
                    if let Some(body) = child.child_by_field_name("body") {
                        let ix = out.types.len() - 1;
                        let mut nested = Declarations::default();
                        collect(body, text, &path, &mut nested);
                        // Methods belong to this type; nested types follow it.
                        out.types[ix].methods = nested.functions;
                        out.types.extend(nested.types);
                    }
                }
            }
            "function_declaration" => {
                if let Some(function) = function_decl(child, text) {
                    out.functions.push(function);
                }
            }
            // Declarations under `#if` and similar wrappers.
            "statements" | "if_statement" => collect(child, text, outer, out),
            _ => {}
        }
    }
}

/// `@Test`, `@Suite` or `@Testing.Test` and the first string argument.
fn attributes(node: Node, text: &str) -> Vec<(String, Option<String>)> {
    let Some(modifiers) = children(node).into_iter().find(|c| c.kind() == "modifiers") else {
        return vec![];
    };
    children(modifiers)
        .into_iter()
        .filter(|c| c.kind() == "attribute")
        .filter_map(|attribute| {
            let parts = children(attribute);
            let name = parts.iter().find(|c| c.kind() == "user_type")?;
            let name = node_text(*name, text);
            let name = name.rsplit('.').next().unwrap_or(name).to_string();
            let display = parts.iter().find(|c| c.kind() == "line_string_literal").map(|s| {
                children(*s)
                    .into_iter()
                    .filter(|c| c.kind() != "\"")
                    .map(|c| node_text(c, text))
                    .collect::<String>()
            });
            Some((name, display))
        })
        .collect()
}

fn modifier_words(node: Node, text: &str) -> Vec<String> {
    children(node)
        .into_iter()
        .filter(|c| c.kind() == "modifiers")
        .flat_map(|m| children(m))
        .filter(|c| c.kind() != "attribute")
        .map(|c| node_text(c, text).to_string())
        .collect()
}

fn keyword_line(node: Node, keyword: &str) -> u32 {
    children(node)
        .into_iter()
        .find(|c| c.kind() == keyword)
        .unwrap_or(node)
        .start_position()
        .row as u32
}

fn strip_generics(name: &str) -> &str {
    name.split('<').next().unwrap_or(name).trim()
}

fn type_decl(node: Node, text: &str, outer: &[String]) -> Option<TypeDecl> {
    let kind_node = node.child_by_field_name("declaration_kind")?;
    let kind = node_text(kind_node, text);
    let name = node_text(node.child_by_field_name("name")?, text);
    let is_extension = kind == "extension";
    let path: Vec<String> = if is_extension {
        strip_generics(name).split('.').map(str::to_string).collect()
    } else {
        outer.iter().cloned().chain([strip_generics(name).to_string()]).collect()
    };
    let superclass = children(node)
        .into_iter()
        .find(|c| c.kind() == "inheritance_specifier")
        .and_then(|s| s.child_by_field_name("inherits_from"))
        .map(|t| {
            let name = strip_generics(node_text(t, text));
            name.rsplit('.').next().unwrap_or(name).to_string()
        });
    let suite = attributes(node, text).into_iter().find(|(name, _)| name == "Suite").map(|(_, d)| d);
    Some(TypeDecl {
        path,
        is_extension,
        is_class: kind == "class",
        superclass,
        suite,
        line: kind_node.start_position().row as u32,
        start_line: node.start_position().row as u32,
        end_line: node.end_position().row as u32,
        methods: vec![],
    })
}

fn function_decl(node: Node, text: &str) -> Option<FunctionDecl> {
    let name = node_text(node.child_by_field_name("name")?, text).to_string();
    let labels = children(node)
        .into_iter()
        .filter(|c| c.kind() == "parameter")
        .map(|parameter| {
            let identifiers: Vec<Node> =
                children(parameter).into_iter().filter(|c| c.kind() == "simple_identifier").collect();
            identifiers.first().map(|n| node_text(*n, text).to_string()).unwrap_or_else(|| "_".into())
        })
        .collect();
    let words = modifier_words(node, text);
    Some(FunctionDecl {
        name,
        labels,
        test: attributes(node, text).into_iter().find(|(name, _)| name == "Test").map(|(_, d)| d),
        is_static: words.iter().any(|w| w == "static" || w == "class"),
        is_private: words.iter().any(|w| w.starts_with("private") || w.starts_with("fileprivate")),
        line: keyword_line(node, "func"),
        start_line: node.start_position().row as u32,
        end_line: node.end_position().row as u32,
    })
}

// ---------------------------------------------------------------------------
// Objective-C (XCTest only)

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

pub fn objc_declarations(text: &str) -> Declarations {
    static INTERFACE: OnceLock<Regex> = OnceLock::new();
    static IMPLEMENTATION: OnceLock<Regex> = OnceLock::new();
    static METHOD: OnceLock<Regex> = OnceLock::new();
    if !text.contains("XCTest") && !text.contains("@implementation") {
        return Declarations::default();
    }
    let superclasses: HashMap<&str, &str> = re(&INTERFACE, r"(?m)^\s*@interface\s+(\w+)\s*:\s*(\w+)")
        .captures_iter(text)
        .map(|c| (c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str()))
        .collect();
    let line_of = |offset: usize| text[..offset].matches('\n').count() as u32;
    let mut out = Declarations::default();
    for c in re(&IMPLEMENTATION, r"(?m)^\s*@implementation\s+(\w+)").captures_iter(text) {
        let whole = c.get(0).unwrap();
        let name = c.get(1).unwrap().as_str();
        let end = text[whole.end()..].find("@end").map(|i| whole.end() + i).unwrap_or(text.len());
        let body = &text[whole.end()..end];
        let methods = re(&METHOD, r"(?m)^[ \t]*-\s*\(\s*void\s*\)\s*(test\w*)\s*(?:\{|$)")
            .captures_iter(body)
            .map(|m| {
                let line = line_of(whole.end() + m.get(0).unwrap().start() + (m.get(0).unwrap().as_str().len() - m.get(0).unwrap().as_str().trim_start().len()));
                FunctionDecl {
                    name: m[1].to_string(),
                    line,
                    start_line: line,
                    end_line: line,
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
        // A method ends where the next one starts.
        let mut methods = methods;
        let class_end = line_of(end);
        for ix in 0..methods.len() {
            methods[ix].end_line = methods.get(ix + 1).map(|m| m.start_line.saturating_sub(1)).unwrap_or(class_end);
        }
        let line = line_of(whole.start() + (whole.as_str().len() - whole.as_str().trim_start().len()));
        out.types.push(TypeDecl {
            path: vec![name.to_string()],
            is_class: true,
            superclass: superclasses.get(name).map(|s| s.to_string()),
            line,
            start_line: line,
            end_line: class_end,
            methods,
            ..Default::default()
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Resolution across files

/// The Swift Testing name of a function: `foo()`, `check(_:value:)`.
pub fn swift_testing_name(function: &FunctionDecl) -> String {
    format!("{}({})", function.name, function.labels.iter().map(|l| format!("{l}:")).collect::<String>())
}

fn is_xctest_method(function: &FunctionDecl) -> bool {
    function.name.starts_with("test") && function.labels.is_empty() && !function.is_static && !function.is_private && function.test.is_none()
}

/// Every test, class and suite in `files`, in file order, then declaration order.
pub fn resolve(files: &[(PathBuf, Declarations)]) -> Vec<TestItem> {
    // XCTest cases: classes whose superclass chain reaches XCTestCase.
    let superclass: HashMap<&str, &str> = files
        .iter()
        .flat_map(|(_, d)| &d.types)
        .filter(|t| t.is_class)
        .filter_map(|t| Some((t.path.last()?.as_str(), t.superclass.as_deref()?)))
        .collect();
    let is_case = |name: &str| {
        let mut current = name;
        for _ in 0..32 {
            match superclass.get(current) {
                Some(&"XCTestCase") => return true,
                Some(next) => current = next,
                None => return false,
            }
        }
        false
    };
    let case_names: HashSet<&str> = superclass.keys().copied().filter(|name| is_case(name)).collect();
    // `@Suite` display names, which may sit on another file's declaration.
    let suite_display: HashMap<&[String], &str> = files
        .iter()
        .flat_map(|(_, d)| &d.types)
        .filter_map(|t| Some((t.path.as_slice(), t.suite.as_ref()?.as_deref()?)))
        .collect();
    let display_path = |path: &[String]| -> Vec<Option<String>> {
        (1..=path.len()).map(|n| suite_display.get(&path[..n]).map(|s| s.to_string())).collect()
    };

    let mut items = vec![];
    for (file, decls) in files {
        let mut file_items: Vec<TestItem> = vec![];
        for function in decls.functions.iter().filter(|f| f.test.is_some()) {
            file_items.push(TestItem {
                framework: Framework::SwiftTesting,
                file: file.clone(),
                suite: vec![],
                suite_display: vec![],
                name: Some(swift_testing_name(function)),
                display_name: function.test.clone().flatten(),
                line: function.line,
                start_line: function.start_line,
                end_line: function.end_line,
            });
        }
        for decl in &decls.types {
            let Some(type_name) = decl.path.last() else { continue };
            let xctest = case_names.contains(type_name.as_str()) && (decl.is_class || decl.is_extension);
            let tests: Vec<TestItem> = decl
                .methods
                .iter()
                .filter_map(|function| {
                    let (framework, name) = if function.test.is_some() {
                        (Framework::SwiftTesting, swift_testing_name(function))
                    } else if xctest && is_xctest_method(function) {
                        (Framework::XCTest, function.name.clone())
                    } else {
                        return None;
                    };
                    let suite = if framework == Framework::XCTest { vec![type_name.clone()] } else { decl.path.clone() };
                    Some(TestItem {
                        framework,
                        file: file.clone(),
                        suite_display: if framework == Framework::XCTest { vec![None] } else { display_path(&suite) },
                        suite,
                        name: Some(name),
                        display_name: function.test.clone().flatten(),
                        line: function.line,
                        start_line: function.start_line,
                        end_line: function.end_line,
                    })
                })
                .collect();
            if !decl.is_extension {
                let framework = if xctest {
                    Some(Framework::XCTest)
                } else if decl.suite.is_some() || has_swift_tests(files, &decl.path) {
                    Some(Framework::SwiftTesting)
                } else {
                    None
                };
                let has_tests = match framework {
                    Some(Framework::XCTest) => has_xctests(files, type_name),
                    Some(Framework::SwiftTesting) => true,
                    None => false,
                };
                if let (Some(framework), true) = (framework, has_tests) {
                    let suite = if framework == Framework::XCTest { vec![type_name.clone()] } else { decl.path.clone() };
                    file_items.push(TestItem {
                        framework,
                        file: file.clone(),
                        suite_display: if framework == Framework::XCTest { vec![None] } else { display_path(&suite) },
                        suite,
                        name: None,
                        display_name: decl.suite.clone().flatten(),
                        line: decl.line,
                        start_line: decl.start_line,
                        end_line: decl.end_line,
                    });
                }
            }
            file_items.extend(tests);
        }
        file_items.sort_by_key(|i| (i.line, i.name.is_some()));
        items.extend(file_items);
    }
    items
}

/// Whether the type at `path` (or a type nested in it) has `@Test` functions
/// in any file.
fn has_swift_tests(files: &[(PathBuf, Declarations)], path: &[String]) -> bool {
    files
        .iter()
        .flat_map(|(_, d)| &d.types)
        .any(|t| t.path.starts_with(path) && t.methods.iter().any(|m| m.test.is_some()))
}

fn has_xctests(files: &[(PathBuf, Declarations)], class: &str) -> bool {
    files
        .iter()
        .flat_map(|(_, d)| &d.types)
        .any(|t| t.path.last().map(String::as_str) == Some(class) && t.methods.iter().any(is_xctest_method))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWIFT: &str = r#"import XCTest
@testable import App

final class FooTests: XCTestCase {
    func testA() throws { XCTAssert(true) }
    private func testHidden() {}
    func helper() {}
    func testWithArgument(_ x: Int) {}
    @MainActor
    func testB() async {}
}

extension FooTests {
    func testC() {}
}

class BaseCase: XCTestCase {}
class DerivedTests: BaseCase {
    func testInherited() {}
}

@Suite("Fancy") struct Bar {
    @Test("Named") func named() {}
    @Test(arguments: [1, 2]) func param(_ n: Int, label x: String) {}
    struct Inner { @Test func deep() {} }
}

@Test func global() {}
"#;

    fn resolve_one(path: &str, text: &str) -> Vec<TestItem> {
        resolve(&[(PathBuf::from(path), declarations(Path::new(path), text))])
    }

    fn ids(items: &[TestItem]) -> Vec<String> {
        items.iter().map(|i| format!("{:?} {} @{}", i.framework, i.components().join("/"), i.line + 1)).collect()
    }

    #[test]
    fn finds_xctest_and_swift_testing() {
        let items = resolve_one("/p/FooTests.swift", SWIFT);
        assert_eq!(
            ids(&items),
            [
                "XCTest FooTests @4",
                "XCTest FooTests/testA @5",
                "XCTest FooTests/testB @10",
                "XCTest FooTests/testC @14",
                "XCTest DerivedTests @18",
                "XCTest DerivedTests/testInherited @19",
                "SwiftTesting Bar @22",
                "SwiftTesting Bar/named() @23",
                "SwiftTesting Bar/param(_:label:) @24",
                "SwiftTesting Bar/Inner @25",
                "SwiftTesting Bar/Inner/deep() @25",
                "SwiftTesting global() @28",
            ]
        );
        let named = items.iter().find(|i| i.name.as_deref() == Some("named()")).unwrap();
        assert_eq!(named.display_name.as_deref(), Some("Named"));
        assert_eq!(named.suite_display, vec![Some("Fancy".to_string())]);
        // The attribute line belongs to the test's range, the diamond to `func`.
        let b = items.iter().find(|i| i.name.as_deref() == Some("testB")).unwrap();
        assert_eq!((b.start_line, b.line), (8, 9));
        // BaseCase has no tests of its own: no diamond.
        assert!(!items.iter().any(|i| i.suite == ["BaseCase"]));
    }

    #[test]
    fn suites_and_cases_can_span_files() {
        let files = vec![
            (PathBuf::from("/p/Base.swift"), swift_declarations("import XCTest\nclass Base: XCTestCase {}\n@Suite(\"Pretty\") struct S {}\n")),
            (PathBuf::from("/p/More.swift"), swift_declarations("import XCTest\nimport Testing\nclass More: Base { func testX() {} }\nextension S { @Test func y() {} }\n")),
        ];
        let items = resolve(&files);
        assert_eq!(
            ids(&items),
            ["SwiftTesting S @3", "XCTest More @3", "XCTest More/testX @3", "SwiftTesting S/y() @4"]
        );
        let y = items.iter().find(|i| i.name.as_deref() == Some("y()")).unwrap();
        assert_eq!(y.suite_display, vec![Some("Pretty".to_string())]);
    }

    #[test]
    fn objective_c_cases() {
        let text = "#import <XCTest/XCTest.h>\n\n@interface LegacyTests : XCTestCase\n@end\n\n@implementation LegacyTests\n\n- (void)setUp {\n}\n\n- (void)testOne {\n    XCTAssert(YES);\n}\n\n- (void)testTwo\n{\n}\n\n@end\n";
        let items = resolve_one("/p/LegacyTests.m", text);
        assert_eq!(ids(&items), ["XCTest LegacyTests @6", "XCTest LegacyTests/testOne @11", "XCTest LegacyTests/testTwo @15"]);
        assert_eq!(items[1].end_line + 1, 14);
    }

    #[test]
    fn containment() {
        let items = resolve_one("/p/FooTests.swift", SWIFT);
        let bar = items.iter().find(|i| i.suite == ["Bar"] && i.name.is_none()).unwrap();
        let deep = items.iter().find(|i| i.name.as_deref() == Some("deep()")).unwrap();
        let foo = items.iter().find(|i| i.suite == ["FooTests"] && i.name.is_none()).unwrap();
        assert!(bar.contains(deep));
        assert!(!foo.contains(deep));
        assert!(deep.contains(deep));
    }

    #[test]
    fn skips_files_without_tests() {
        assert!(swift_declarations("struct Counter { func testable() {} }").is_empty());
    }
}
