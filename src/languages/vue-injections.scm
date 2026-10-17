; A Vue single-file component, parsed as HTML: its <script> and <style>
; blocks in the language their `lang` names (TypeScript for "ts", SCSS as
; CSS), JavaScript and CSS otherwise, and directives' expressions
; (v-if="…", :prop="…", @click="…", #slot="…") as JavaScript.

(script_element
  (start_tag
    (attribute
      (attribute_name) @_attr
      (quoted_attribute_value (attribute_value) @injection.language)))
  (raw_text) @injection.content
  (#eq? @_attr "lang"))

((script_element
  (start_tag) @_start
  (raw_text) @injection.content)
 (#not-match? @_start "lang=")
 (#set! injection.language "javascript"))

(style_element
  (start_tag
    (attribute
      (attribute_name) @_attr
      (quoted_attribute_value (attribute_value) @injection.language)))
  (raw_text) @injection.content
  (#eq? @_attr "lang"))

((style_element
  (start_tag) @_start
  (raw_text) @injection.content)
 (#not-match? @_start "lang=")
 (#set! injection.language "css"))

((attribute
   (attribute_name) @_name
   (quoted_attribute_value (attribute_value) @injection.content))
 (#match? @_name "^(v-|:|@|#)")
 (#set! injection.language "javascript"))
