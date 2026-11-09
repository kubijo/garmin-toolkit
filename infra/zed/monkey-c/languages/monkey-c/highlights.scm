["class" "module" "function" "interface" "enum" "typedef" "extends"
 "if" "else" "switch" "case" "default" "for" "do" "while"
 "try" "catch" "finally" "throw" "return" "break" "continue"
 "var" "const" "as" "import" "using" "new" "instanceof" "has"] @keyword
(modifier) @keyword
(class_declaration name: (identifier) @type)
(module_declaration name: (identifier) @type)
(function_declaration name: (identifier) @function)
(parameter name: (identifier) @variable.parameter)
(type_name) @type
(member_expression property: (identifier) @property)
(string_literal) @string
(char_literal) @string
[(number_literal) (hex_literal) (float_literal) (long_literal) (double_literal)] @number
[(boolean_literal) (null_literal) (nan_literal)] @constant
(symbol_literal) @constant
[(comment) (doc_comment)] @comment
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["." "," ";"] @punctuation.delimiter
