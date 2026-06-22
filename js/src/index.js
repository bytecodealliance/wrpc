// The wRPC codec for JavaScript: a transport-agnostic encoder/decoder for the
// wRPC wire format.
//
// `types` (re-exported as `t`) builds the structural type tree; `value`
// encodes/decodes component-model values against it; `wit` parses a served
// interface description back into types; and `frame` is the default
// byte-stream framing used to invoke a function and read its results.

export * as t from "./types.js";
export * as types from "./types.js";

export { Reader, Writer } from "./io.js";
export { encode, decode, encodeValue, decodeValue } from "./value.js";
export { parseType, parseWit } from "./wit.js";
export { PROTOCOL, encodeInvocation, decodeResults, concatBytes } from "./frame.js";
