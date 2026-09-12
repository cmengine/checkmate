; Bracket matching for Checkmate: rainbow-color pairs and cursor-pair
; highlighting. String quotes are excluded from rainbow colorization.

[
  ("[" @open "]" @close)
  ("{" @open "}" @close)
  ("(" @open ")" @close)
] ; collection and call delimiters

(("\"") @open "\"" @close (#set! rainbow.exclude))
