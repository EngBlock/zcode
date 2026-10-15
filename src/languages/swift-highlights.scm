; A compact Swift highlight query for zcode.
;
; Upstream tree-sitter-swift's highlights.scm has ~70 patterns and takes
; ~150 ms to compile, which the editor pays for every opened Swift file.
; This keeps what Xcode colors (keywords, types, declarations, calls,
; members, literals, comments, attributes) with far fewer parent/child
; patterns, which dominate compile time.

(comment) @comment
(multiline_comment) @comment

[
  (line_str_text)
  (multi_line_str_text)
  (raw_str_part)
  (raw_str_end_part)
  "\""
  "\"\"\""
] @string
(str_escaped_char) @string.escape

[
  (integer_literal)
  (hex_literal)
  (oct_literal)
  (bin_literal)
  (real_literal)
] @number
(boolean_literal) @boolean
(nil_literal) @constant.builtin
(special_literal) @constant.macro

(type_identifier) @type
[
  (self_expression)
  (super_expression)
] @variable.builtin

[
  "func"
  "import"
  "return"
  "if"
  "guard"
  "switch"
  "case"
  "for"
  "in"
  "while"
  "repeat"
  "break"
  "continue"
  "do"
  "try"
  "init"
  "deinit"
  "let"
  "var"
  "enum"
  "struct"
  "class"
  "protocol"
  "extension"
  "typealias"
  "some"
  "any"
  "async"
  "await"
  "static"
  "override"
  "mutating"
  "nonmutating"
  "inout"
  "weak"
  "unowned"
  "lazy"
  "final"
  "private"
  "fileprivate"
  "internal"
  "public"
  "open"
  "subscript"
  "didSet"
  "willSet"
  "get"
  "set"
  "convenience"
  "required"
  "indirect"
  "nonisolated"
  "actor"
  "macro"
  "as"
  "is"
  "fallthrough"
] @keyword

[
  (visibility_modifier)
  (property_modifier)
  (function_modifier)
  (mutation_modifier)
  (parameter_modifier)
  (inheritance_modifier)
  (throws)
  (default_keyword)
] @keyword

(attribute) @attribute
(directive) @keyword.directive

;; `(function_declaration name: ...)` costs ~70 ms to compile; anchoring on
;; the `func` keyword under any parent matches the same names in ~7 ms.
(_ "func" . (simple_identifier) @function)

(call_expression
  (simple_identifier) @function.call)

(navigation_suffix
  (simple_identifier) @variable.member)
