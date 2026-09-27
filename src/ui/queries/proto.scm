; Highlights for tree-sitter-proto 0.2, whose query the crate does not export.

[
  "syntax"
  "edition"
  "package"
  "option"
  "import"
  "service"
  "rpc"
  "returns"
  "message"
  "enum"
  "oneof"
  "repeated"
  "reserved"
  "to"
] @keyword

[
  (key_type)
  (type)
  (message_name)
  (enum_name)
  (service_name)
  (rpc_name)
] @type

(string) @string

[
  (int_lit)
  (float_lit)
] @number

[
  (true)
  (false)
] @boolean

(comment) @comment

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket
