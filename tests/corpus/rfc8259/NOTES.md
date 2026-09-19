# Notes on this corpus

Every file keeps its upstream name, so the `y_` / `n_` / `i_` prefix still says what
*JSONTestSuite* expects. The directory says what *this crate* does with it. Where the two
disagree, the reason is below — and in every case it is a property of this crate's input
domain, never of RFC 8259's grammar.

## The grammar and the suite agree completely

All 95 `y_` cases are accepted and all 174 decodable `n_` cases are rejected. SCOPE.md 8
anticipated `n_` cases that pure ABNF accepts, on the grounds that some of the suite's
rejections are encoding-level rather than grammatical; against this grammar there are none, so
no file has been reclassified for that reason.

## Moved to `indeterminate/`: not valid UTF-8 (12 files)

The matching domain is Unicode scalar values (SCOPE.md 6.1), so an input that is not valid
UTF-8 cannot be decoded into one at all. Rejecting it would be answering a different question
than the one asked: the grammar never gets to see it. Invalid UTF-8 is an *error*, never a
rejection (D14), so these cannot be asserted as rejections even though a JSON parser is right
to refuse them.

  - n_array_a_invalid_utf8.json
  - n_array_invalid_utf8.json
  - n_number_invalid-utf-8-in-bigger-int.json
  - n_number_invalid-utf-8-in-exponent.json
  - n_number_invalid-utf-8-in-int.json
  - n_number_real_with_invalid_utf8_after_e.json
  - n_object_lone_continuation_byte_in_key_and_trailing_comma.json
  - n_string_invalid-utf-8-in-escape.json
  - n_string_invalid_utf8_after_escape.json
  - n_structure_incomplete_UTF8_BOM.json
  - n_structure_lone-invalid-utf-8.json
  - n_structure_single_eacute.json

## Moved to `indeterminate/`: too deeply nested (2 files)

  - n_structure_100000_opening_arrays.json
  - n_structure_open_array_object.json

Both nest 100,000 deep, which no stack accommodates: the recognizer descends recursively, so
input nesting becomes call depth.

They are *decidable* in the sense that matters, though — they return `MatchError::DepthLimit`
rather than aborting the process, which is why `MatchOptions::max_depth` exists and why it has
a finite default. They stay here because a limit is not a verdict: "could not decide" is a
different answer from "does not match" (D16), so they cannot be asserted as rejections.

`i_structure_500_nested_arrays.json` is in the same position for the same reason, at 500 levels
against a default limit of 256. Raising `max_depth` and running on a larger stack decides all
three; the corpus test does not, because the default behaviour is the thing worth testing.

## `i_` files are recorded, not asserted (35 files)

The suite leaves these to the implementation, so asserting either way would be inventing a
requirement. For the record, this crate accepts 21 and rejects 1 of the 22 that are valid
UTF-8; the other 13 are undecodable and are in the same position as the `n_` files above.
