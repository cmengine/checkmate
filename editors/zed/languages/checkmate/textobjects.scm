; Text objects for Vim mode (Zed v0.165+).

; Functions: whole declaration and the statements inside its block
(function_declaration
  body: (block
    (_)* @function.inside)) @function.around

; Structs
(struct_declaration
  "{"
  (_)* @class.inside
  "}") @class.around

; Enums
(enum_declaration
  "{"
  (_)* @class.inside
  "}") @class.around

; Impl blocks behave like classes for navigation purposes
(impl_declaration
  "{"
  (_)* @class.inside
  "}") @class.around

; Adjacent line comments form one comment object; block comments stand alone
(line_comment)+ @comment.around

(block_comment) @comment.around

; Match arms are convenient method-sized navigation units
(match_arm) @function.around
