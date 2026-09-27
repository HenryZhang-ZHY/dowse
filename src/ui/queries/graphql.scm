; Highlights for tree-sitter-graphql 0.1, which ships without a query.

[
  "query"
  "mutation"
  "subscription"
  "fragment"
  "on"
  "schema"
  "extend"
  "type"
  "interface"
  "implements"
  "union"
  "enum"
  "input"
  "scalar"
  "directive"
  "repeatable"
] @keyword

(comment) @comment
(description (string_value) @comment.doc)
(string_value) @string
(int_value) @number
(float_value) @number
(boolean_value) @boolean
(null_value) @constant
(enum_value) @constant
(directive_location) @constant
(operation_type) @keyword

(named_type (name) @type)
(object_type_definition (name) @type)
(interface_type_definition (name) @type)
(union_type_definition (name) @type)
(enum_type_definition (name) @type)
(input_object_type_definition (name) @type)
(scalar_type_definition (name) @type)
(object_type_extension (name) @type)
(interface_type_extension (name) @type)
(union_type_extension (name) @type)
(enum_type_extension (name) @type)
(input_object_type_extension (name) @type)
(scalar_type_extension (name) @type)

(operation_definition (name) @function)
(fragment_name (name) @function)
(directive_definition (name) @attribute)
(directive) @attribute

(variable) @variable
(alias (name) @label)
(argument (name) @variable.parameter)
(input_value_definition (name) @variable.parameter)
(object_field (name) @property)
(field_definition (name) @property)
(field (name) @property)

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ":"
  "="
  "|"
  "&"
  "!"
  "..."
] @operator
